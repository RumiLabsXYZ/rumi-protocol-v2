use crate::numeric::{Ratio, UsdIcp, ICUSD, ICP};
use crate::state::{CollateralConfig, CollateralStatus, CollateralType, PendingMarginTransfer, RateCurveV2, State};
use crate::storage::record_event;
use crate::vault::Vault;
use crate::{InitArg, Mode, StableTokenType, UpgradeArg};
use candid::{CandidType, Principal};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde::{Deserialize, Serialize};

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Event {
    #[serde(rename = "open_vault")]
    OpenVault { vault: Vault, block_index: u64 },

    #[serde(rename = "close_vault")]
    CloseVault {
        vault_id: u64,
        block_index: Option<u64>,
    },

    #[serde(rename = "margin_transfer")]
    MarginTransfer { vault_id: u64, block_index: u64 },

    #[serde(rename = "liquidate_vault")]
    LiquidateVault {
        vault_id: u64,
        mode: Mode,
        icp_rate: UsdIcp,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        liquidator: Option<Principal>,
    },

    #[serde(rename = "partial_liquidate_vault")]
    PartialLiquidateVault {
        vault_id: u64,
        #[serde(alias = "liquidated_debt")]
        liquidator_payment: ICUSD,
        #[serde(alias = "collateral_seized")]
        icp_to_liquidator: ICP,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        liquidator: Option<Principal>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        icp_rate: Option<UsdIcp>,
        /// Collateral (e8s) taken as protocol fee from the liquidation bonus.
        /// Old events deserialize as None (protocol_cut was 0 before this field existed).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        protocol_fee_collateral: Option<u64>,
    },

    #[serde(rename = "redemption_on_vaults")]
    RedemptionOnVaults {
        owner: Principal,
        current_icp_rate: UsdIcp,
        icusd_amount: ICUSD,
        fee_amount: ICUSD,
        icusd_block_index: u64,
    },

    #[serde(rename = "redemption_transfered")]
    RedemptionTransfered {
        icusd_block_index: u64,
        icp_block_index: u64,
    },

    #[serde(rename = "redistribute_vault")]
    RedistributeVault { vault_id: u64 },

    #[serde(rename = "borrow_from_vault")]
    BorrowFromVault {
        vault_id: u64,
        borrowed_amount: ICUSD,
        fee_amount: ICUSD,
        block_index: u64,
    },

    #[serde(rename = "repay_to_vault")]
    RepayToVault {
        vault_id: u64,
        repayed_amount: ICUSD,
        block_index: u64,
    },

    #[serde(rename = "add_margin_to_vault")]
    AddMarginToVault {
        vault_id: u64,
        margin_added: ICP,
        block_index: u64,
    },

    #[serde(rename = "provide_liquidity")]
    ProvideLiquidity {
        amount: ICUSD,
        block_index: u64,
        caller: Principal,
    },

    #[serde(rename = "withdraw_liquidity")]
    WithdrawLiquidity {
        amount: ICUSD,
        block_index: u64,
        caller: Principal,
    },

    #[serde(rename = "claim_liquidity_returns")]
    ClaimLiquidityReturns {
        amount: ICP,
        block_index: u64,
        caller: Principal,
    },

    #[serde(rename = "init")]
    Init(InitArg),

    #[serde(rename = "upgrade")]
    Upgrade(UpgradeArg),

    #[serde(rename = "collateral_withdrawn")]
    CollateralWithdrawn {
        vault_id: u64,
        amount: ICP,
        block_index: u64,
    },

    // TODO(multi-collateral): amount type will need to be generic or token-tagged
    #[serde(rename = "partial_collateral_withdrawn")]
    PartialCollateralWithdrawn {
        vault_id: u64,
        amount: ICP,
        block_index: u64,
    },

    VaultWithdrawnAndClosed {
        vault_id: u64,
        caller: Principal,
        amount: ICP,
        timestamp: u64,
    },

    #[serde(rename = "withdraw_and_close_vault")]
    WithdrawAndCloseVault {
        vault_id: u64,
        amount: ICP,
        block_index: Option<u64>,
    },

    #[serde(rename = "dust_forgiven")]
    DustForgiven {
        vault_id: u64,
        amount: ICUSD,
    },

    #[serde(rename = "set_ckstable_repay_fee")]
    SetCkstableRepayFee {
        rate: String,
    },

    #[serde(rename = "set_min_icusd_amount")]
    SetMinIcusdAmount {
        amount: String,
    },

    /// Admin set global icUSD mint cap.
    /// Field `cap` is a legacy alias kept for replay compat.
    #[serde(rename = "set_global_icusd_mint_cap")]
    SetGlobalIcusdMintCap {
        #[serde(default)]
        amount: Option<String>,
        #[serde(default)]
        cap: Option<String>,
    },

    #[serde(rename = "set_stable_token_enabled")]
    SetStableTokenEnabled {
        token_type: StableTokenType,
        enabled: bool,
    },

    #[serde(rename = "set_stable_ledger_principal")]
    SetStableLedgerPrincipal {
        token_type: StableTokenType,
        principal: Principal,
    },

    #[serde(rename = "set_treasury_principal")]
    SetTreasuryPrincipal {
        principal: Principal,
    },

    #[serde(rename = "set_stability_pool_principal")]
    SetStabilityPoolPrincipal {
        principal: Principal,
    },

    #[serde(rename = "set_liquidation_bonus")]
    SetLiquidationBonus {
        rate: String,
    },

    #[serde(rename = "set_borrowing_fee")]
    SetBorrowingFee {
        rate: String,
    },

    #[serde(rename = "set_redemption_fee_floor")]
    SetRedemptionFeeFloor {
        rate: String,
    },

    #[serde(rename = "set_redemption_fee_ceiling")]
    SetRedemptionFeeCeiling {
        rate: String,
    },

    #[serde(rename = "set_max_partial_liquidation_ratio")]
    SetMaxPartialLiquidationRatio {
        rate: String,
    },

    #[serde(rename = "set_recovery_target_cr")]
    SetRecoveryTargetCr {
        rate: String,
    },

    #[serde(rename = "set_recovery_cr_multiplier", alias = "set_recovery_liquidation_buffer")]
    SetRecoveryCrMultiplier {
        #[serde(alias = "buffer")]
        multiplier: String,
    },

    #[serde(rename = "set_liquidation_protocol_share")]
    SetLiquidationProtocolShare {
        share: String,
    },

    #[serde(rename = "add_collateral_type")]
    AddCollateralType {
        collateral_type: CollateralType,
        config: CollateralConfig,
    },

    #[serde(rename = "update_collateral_status")]
    UpdateCollateralStatus {
        collateral_type: CollateralType,
        status: CollateralStatus,
    },

    #[serde(rename = "update_collateral_config")]
    UpdateCollateralConfig {
        collateral_type: CollateralType,
        config: CollateralConfig,
    },

    #[serde(rename = "set_reserve_redemptions_enabled")]
    SetReserveRedemptionsEnabled {
        enabled: bool,
    },

    #[serde(rename = "set_reserve_redemption_fee")]
    SetReserveRedemptionFee {
        fee: String,
    },

    #[serde(rename = "reserve_redemption")]
    ReserveRedemption {
        owner: Principal,
        icusd_amount: ICUSD,
        fee_amount: ICUSD,
        stable_token_ledger: Principal,
        stable_amount_sent: u64,
        fee_stable_amount: u64,
        icusd_block_index: u64,
    },
    #[serde(rename = "admin_mint")]
    AdminMint {
        amount: ICUSD,
        to: Principal,
        reason: String,
        block_index: u64,
    },
    #[serde(rename = "set_recovery_parameters")]
    SetRecoveryParameters {
        collateral_type: CollateralType,
        recovery_borrowing_fee: Option<String>,
        recovery_interest_rate_apr: Option<String>,
    },

    /// Admin correction of vault collateral amount (e.g., fixing inflation from error handler bug)
    #[serde(rename = "admin_vault_correction")]
    AdminVaultCorrection {
        vault_id: u64,
        old_amount: u64,
        new_amount: u64,
        reason: String,
    },

    /// Admin set rate curve markers (per-asset or global)
    #[serde(rename = "set_rate_curve_markers")]
    SetRateCurveMarkers {
        collateral_type: Option<String>,  // None for global
        markers: String,                  // JSON-serialized marker pairs
    },

    /// Admin set recovery rate curve (system-wide Layer 2)
    #[serde(rename = "set_recovery_rate_curve")]
    SetRecoveryRateCurve {
        markers: String,  // JSON-serialized (threshold, multiplier) pairs
    },

    /// Admin set healthy CR for a collateral type
    #[serde(rename = "set_healthy_cr")]
    SetHealthyCr {
        collateral_type: String,
        healthy_cr: Option<String>,
    },

    /// Admin set per-collateral borrowing fee.
    /// Fields `rate` and `fee` are legacy aliases kept for replay compat.
    #[serde(rename = "set_collateral_borrowing_fee")]
    SetCollateralBorrowingFee {
        collateral_type: CollateralType,
        #[serde(default)]
        borrowing_fee: Option<String>,
        #[serde(default)]
        rate: Option<String>,
        #[serde(default)]
        fee: Option<String>,
    },

    /// Admin set interest rate APR for a collateral type
    #[serde(rename = "set_interest_rate")]
    SetInterestRate {
        collateral_type: CollateralType,
        interest_rate_apr: String,
    },

    /// Per-vault interest accrual tick. One event per timer tick.
    /// On replay, calls accrue_all_vault_interest(timestamp).
    #[serde(rename = "accrue_interest")]
    AccrueInterest {
        timestamp: u64,
    },

    /// Admin set the interest revenue split ratio (stability pool share).
    #[serde(rename = "set_interest_pool_share")]
    SetInterestPoolShare {
        share: String,
    },

    /// Admin set an RMR parameter.
    #[serde(rename = "set_rmr_floor")]
    SetRmrFloor { value: String },
    #[serde(rename = "set_rmr_ceiling")]
    SetRmrCeiling { value: String },
    #[serde(rename = "set_rmr_floor_cr")]
    SetRmrFloorCr { value: String },
    #[serde(rename = "set_rmr_ceiling_cr")]
    SetRmrCeilingCr { value: String },

    /// Admin sweep of untracked collateral from backend to treasury.
    #[serde(rename = "admin_sweep_to_treasury")]
    AdminSweepToTreasury {
        amount: u64,
        treasury: Principal,
        block_index: u64,
        reason: String,
    },

    // (Legacy duplicates removed — merged into primary definitions above)

    /// Admin set the dynamic borrowing fee curve.
    #[serde(rename = "set_borrowing_fee_curve")]
    SetBorrowingFeeCurve {
        markers: String,
    },

    /// Admin set the N-way interest split (replaces interest_pool_share).
    #[serde(rename = "set_interest_split")]
    SetInterestSplit {
        /// JSON-encoded Vec<InterestRecipient>
        split: String,
    },

    /// Admin set the 3pool canister principal for interest donations.
    #[serde(rename = "set_three_pool_canister")]
    SetThreePoolCanister {
        canister: Principal,
    },
}

impl Event {
    // Define a method to check if the event contains vault_id
    pub fn is_vault_related(&self, filter_vault_id: &u64) -> bool {
        match self {
            Event::OpenVault { vault, .. } => &vault.vault_id == filter_vault_id,
            Event::CloseVault { vault_id, .. } => vault_id == filter_vault_id,
            Event::MarginTransfer { vault_id, .. } => vault_id == filter_vault_id,
            Event::LiquidateVault { vault_id, .. } => vault_id == filter_vault_id,
            Event::PartialLiquidateVault { vault_id, .. } => vault_id == filter_vault_id,
            Event::RedemptionOnVaults { .. } => true,
            Event::RedemptionTransfered { .. } => false,
            Event::RedistributeVault { vault_id, .. } => vault_id == filter_vault_id,
            Event::BorrowFromVault { vault_id, .. } => vault_id == filter_vault_id,
            Event::RepayToVault { vault_id, .. } => vault_id == filter_vault_id,
            Event::AddMarginToVault { vault_id, .. } => vault_id == filter_vault_id,
            Event::ProvideLiquidity { .. } => false,
            Event::WithdrawLiquidity { .. } => false,
            Event::ClaimLiquidityReturns { .. } => false,
            Event::Init(_) => false,
            Event::Upgrade(_) => false,
            Event::CollateralWithdrawn { vault_id, .. } => vault_id == filter_vault_id,
            Event::PartialCollateralWithdrawn { vault_id, .. } => vault_id == filter_vault_id,
            Event::VaultWithdrawnAndClosed { vault_id, .. } => vault_id == filter_vault_id,
            Event::WithdrawAndCloseVault { vault_id, .. } => vault_id == filter_vault_id,
            Event::DustForgiven { vault_id, .. } => vault_id == filter_vault_id,
            Event::SetCkstableRepayFee { .. } => false,
            Event::SetMinIcusdAmount { .. } => false,
            Event::SetGlobalIcusdMintCap { .. } => false,
            Event::SetStableTokenEnabled { .. } => false,
            Event::SetStableLedgerPrincipal { .. } => false,
            Event::SetTreasuryPrincipal { .. } => false,
            Event::SetStabilityPoolPrincipal { .. } => false,
            Event::SetLiquidationBonus { .. } => false,
            Event::SetBorrowingFee { .. } => false,
            Event::SetRedemptionFeeFloor { .. } => false,
            Event::SetRedemptionFeeCeiling { .. } => false,
            Event::SetMaxPartialLiquidationRatio { .. } => false,
            Event::SetRecoveryTargetCr { .. } => false,
            Event::SetRecoveryCrMultiplier { .. } => false,
            Event::SetLiquidationProtocolShare { .. } => false,
            Event::AddCollateralType { .. } => false,
            Event::UpdateCollateralStatus { .. } => false,
            Event::UpdateCollateralConfig { .. } => false,
            Event::SetReserveRedemptionsEnabled { .. } => false,
            Event::SetReserveRedemptionFee { .. } => false,
            Event::ReserveRedemption { .. } => false,
            Event::AdminMint { .. } => false,
            Event::SetRecoveryParameters { .. } => false,
            Event::AdminVaultCorrection { vault_id, .. } => vault_id == filter_vault_id,
            Event::SetRateCurveMarkers { .. } => false,
            Event::SetRecoveryRateCurve { .. } => false,
            Event::SetHealthyCr { .. } => false,
            Event::SetCollateralBorrowingFee { .. } => false,
            Event::SetInterestRate { .. } => false,
            Event::AccrueInterest { .. } => false,
            Event::SetInterestPoolShare { .. } => false,
            Event::SetRmrFloor { .. } => false,
            Event::SetRmrCeiling { .. } => false,
            Event::SetRmrFloorCr { .. } => false,
            Event::SetRmrCeilingCr { .. } => false,
            Event::AdminSweepToTreasury { .. } => false,
            Event::SetBorrowingFeeCurve { .. } => false,
            Event::SetInterestSplit { .. } => false,
            Event::SetThreePoolCanister { .. } => false,
        }
    }
}

#[derive(Debug)]
pub enum ReplayLogError {
    /// There are no events in the event log.
    EmptyLog,
    /// The event log is inconsistent.
    InconsistentLog(String),
}

pub fn replay(mut events: impl Iterator<Item = Event>) -> Result<State, ReplayLogError> {
    let mut state = match events.next() {
        Some(Event::Init(args)) => State::from(args),
        Some(evt) => {
            return Err(ReplayLogError::InconsistentLog(format!(
                "The first event is not Init: {:?}",
                evt
            )))
        }
        None => return Err(ReplayLogError::EmptyLog),
    };
    let mut vault_id = 0;
    for event in events {
        match event {
            Event::OpenVault {
                mut vault,
                block_index: _,
            } => {
                vault_id += 1;
                // Fix up legacy events that lack collateral_type (serde default = anonymous)
                if vault.collateral_type == Principal::anonymous() {
                    vault.collateral_type = state.icp_ledger_principal;
                }
                state.open_vault(vault);
            }
            Event::CloseVault {
                vault_id,
                block_index: _,
            } => state.close_vault(vault_id),
            Event::LiquidateVault {
                vault_id,
                mode,
                icp_rate,
                liquidator: _,
            } => { let _ = state.liquidate_vault(vault_id, mode, icp_rate); },
            Event::PartialLiquidateVault {
                vault_id,
                liquidator_payment,
                icp_to_liquidator,
                liquidator: _,
                icp_rate: _,
                protocol_fee_collateral,
            } => {
                // Reduce vault debt and collateral, accounting for interest share
                if let Some(vault) = state.vault_id_to_vaults.get_mut(&vault_id) {
                    // Compute proportional interest share before reducing debt
                    let interest_share = if vault.accrued_interest.0 > 0 && vault.borrowed_icusd_amount.0 > 0 {
                        let share = (rust_decimal::Decimal::from(liquidator_payment.0)
                            * rust_decimal::Decimal::from(vault.accrued_interest.0)
                            / rust_decimal::Decimal::from(vault.borrowed_icusd_amount.0))
                            .to_u64().unwrap_or(0);
                        ICUSD::new(share.min(vault.accrued_interest.0))
                    } else { ICUSD::new(0) };
                    vault.borrowed_icusd_amount -= liquidator_payment;
                    // Vault loses icp_to_liquidator + protocol_fee_collateral
                    // (old events have protocol_fee_collateral=None → 0, which is correct)
                    let total_collateral_seized = icp_to_liquidator.to_u64()
                        + protocol_fee_collateral.unwrap_or(0);
                    vault.collateral_amount -= total_collateral_seized;
                    vault.accrued_interest -= interest_share;
                }
            },
            Event::RedistributeVault { vault_id } => state.redistribute_vault(vault_id),
            Event::BorrowFromVault {
                vault_id,
                borrowed_amount,
                fee_amount: _,
                block_index: _,
            } => {
                // Fee was phantom (never minted) in old events; now routed to treasury in async caller.
                state.borrow_from_vault(vault_id, borrowed_amount)
            }
            Event::RedemptionOnVaults {
                owner,
                current_icp_rate,
                icusd_amount,
                fee_amount,
                icusd_block_index,
            } => {
                state.provide_liquidity(fee_amount, state.developer_principal);
                let redeem_ct = state.icp_collateral_type();
                state.redeem_on_vaults(icusd_amount, current_icp_rate, &redeem_ct);
                let margin: ICP = icusd_amount / current_icp_rate;
                state
                    .pending_redemption_transfer
                    .insert(icusd_block_index, PendingMarginTransfer { owner, margin, collateral_type: crate::vault::default_collateral_type() });
            }
            Event::RedemptionTransfered {
                icusd_block_index, ..
            } => {
                state.pending_redemption_transfer.remove(&icusd_block_index);
            }
            Event::AddMarginToVault {
                vault_id,
                margin_added,
                ..
            } => state.add_margin_to_vault(vault_id, margin_added),
            Event::RepayToVault {
                vault_id,
                repayed_amount,
                ..
            } => {
                let _ = state.repay_to_vault(vault_id, repayed_amount);
            }
            Event::ProvideLiquidity { amount, caller, .. } => {
                state.provide_liquidity(amount, caller);
            }
            Event::WithdrawLiquidity { amount, caller, .. } => {
                state.withdraw_liquidity(amount, caller);
            }
            Event::ClaimLiquidityReturns { amount, caller, .. } => {
                state.claim_liquidity_returns(amount, caller);
            }
            Event::Init(_) => panic!("should have only one init event"),
            Event::Upgrade(upgrade_args) => {
                state.upgrade(upgrade_args);
            }
            Event::MarginTransfer { vault_id, .. } => {
                state.pending_margin_transfers.remove(&vault_id);
            }
            Event::CollateralWithdrawn { vault_id, amount, .. } => {
                // Zero the vault's collateral during replay so that if a
                // subsequent close_vault() reads the vault, the balance is
                // accurate. (During live operation this is done in vault.rs
                // before the transfer; during replay we must mirror it here.)
                if let Some(vault) = state.vault_id_to_vaults.get_mut(&vault_id) {
                    let withdraw = amount.to_u64().min(vault.collateral_amount);
                    vault.collateral_amount -= withdraw;
                }
            }
            Event::PartialCollateralWithdrawn {
                vault_id,
                amount,
                block_index: _,
            } => {
                state.remove_margin_from_vault(vault_id, amount);
            }
            // In the match statement inside replay function
            Event::VaultWithdrawnAndClosed {
                vault_id,
                caller: _,   // Ignore caller
                amount: _,   // Ignore amount
                timestamp: _, // Ignore timestamp
            } => {
                // Simply close the vault - previous implementation was incorrect
                state.close_vault(vault_id);
            },
            // Add this case:
            Event::WithdrawAndCloseVault {
                vault_id,
                amount: _,
                block_index: _,
            } => {
                // Close the vault during replay
                state.close_vault(vault_id);
            },
            Event::DustForgiven {
                vault_id: _,
                amount: _,
            } => {
                // Dust forgiveness doesn't need state changes during replay
            },
            Event::SetCkstableRepayFee { rate } => {
                if let Ok(dec) = rate.parse::<Decimal>() {
                    state.ckstable_repay_fee = Ratio::from(dec);
                }
            },
            Event::SetMinIcusdAmount { amount } => {
                if let Ok(val) = amount.parse::<u64>() {
                    state.min_icusd_amount = ICUSD::new(val);
                }
            },
            Event::SetGlobalIcusdMintCap { amount, cap } => {
                let value = amount.as_deref().or(cap.as_deref());
                if let Some(Ok(val)) = value.map(|s| s.parse::<u64>()) {
                    state.global_icusd_mint_cap = val;
                }
            },
            Event::SetStableTokenEnabled { token_type, enabled } => {
                match token_type {
                    StableTokenType::CKUSDT => state.ckusdt_enabled = enabled,
                    StableTokenType::CKUSDC => state.ckusdc_enabled = enabled,
                }
            },
            Event::SetStableLedgerPrincipal { token_type, principal } => {
                match token_type {
                    StableTokenType::CKUSDT => state.ckusdt_ledger_principal = Some(principal),
                    StableTokenType::CKUSDC => state.ckusdc_ledger_principal = Some(principal),
                }
            },
            Event::SetTreasuryPrincipal { principal } => {
                state.treasury_principal = Some(principal);
            },
            Event::SetStabilityPoolPrincipal { principal } => {
                state.stability_pool_canister = Some(principal);
            },
            Event::SetLiquidationBonus { rate } => {
                if let Ok(dec) = rate.parse::<Decimal>() {
                    state.liquidation_bonus = Ratio::from(dec);
                    state.sync_icp_collateral_config();
                }
            },
            Event::SetBorrowingFee { rate } => {
                if let Ok(dec) = rate.parse::<Decimal>() {
                    state.fee = Ratio::from(dec);
                    state.sync_icp_collateral_config();
                }
            },
            Event::SetRedemptionFeeFloor { rate } => {
                if let Ok(dec) = rate.parse::<Decimal>() {
                    state.redemption_fee_floor = Ratio::from(dec);
                    state.sync_icp_collateral_config();
                }
            },
            Event::SetRedemptionFeeCeiling { rate } => {
                if let Ok(dec) = rate.parse::<Decimal>() {
                    state.redemption_fee_ceiling = Ratio::from(dec);
                    state.sync_icp_collateral_config();
                }
            },
            Event::SetMaxPartialLiquidationRatio { rate } => {
                if let Ok(dec) = rate.parse::<Decimal>() {
                    state.max_partial_liquidation_ratio = Ratio::from(dec);
                }
            },
            Event::SetRecoveryTargetCr { rate } => {
                // Legacy: old events stored an absolute target (e.g. 1.55).
                // We keep replaying into recovery_target_cr for historical fidelity,
                // but the protocol now uses recovery_cr_multiplier for computation.
                if let Ok(dec) = rate.parse::<Decimal>() {
                    state.recovery_target_cr = Ratio::from(dec);
                    state.sync_icp_collateral_config();
                }
            },
            Event::SetRecoveryCrMultiplier { multiplier } => {
                if let Ok(dec) = multiplier.parse::<Decimal>() {
                    // If value < 1.0, it's a legacy additive buffer (e.g., 0.05).
                    // Convert: multiplier ≈ 1 + buffer (conservative approximation)
                    let effective = if dec < Decimal::ONE {
                        Decimal::ONE + dec  // 0.05 -> 1.05
                    } else {
                        dec
                    };
                    state.recovery_cr_multiplier = Ratio::from(effective);
                    state.sync_icp_collateral_config();
                }
            },
            Event::SetLiquidationProtocolShare { share } => {
                if let Ok(dec) = share.parse::<Decimal>() {
                    state.liquidation_protocol_share = Ratio::from(dec);
                }
            },
            Event::AddCollateralType { collateral_type, config } => {
                state.collateral_configs.insert(collateral_type, config);
            },
            Event::UpdateCollateralStatus { collateral_type, status } => {
                if let Some(config) = state.collateral_configs.get_mut(&collateral_type) {
                    config.status = status;
                }
            },
            Event::UpdateCollateralConfig { collateral_type, config } => {
                state.collateral_configs.insert(collateral_type, config);
            },
            Event::SetReserveRedemptionsEnabled { enabled } => {
                state.reserve_redemptions_enabled = enabled;
            },
            Event::SetReserveRedemptionFee { fee } => {
                if let Ok(dec) = fee.parse::<Decimal>() {
                    state.reserve_redemption_fee = Ratio::from(dec);
                }
            },
            Event::ReserveRedemption { .. } => {
                // Reserve redemptions don't change in-memory state during replay;
                // the actual token transfers are async and not replayed.
            },
            Event::AdminMint { .. } => {
                // Admin mints are ledger-only operations; no in-memory state changes.
            },
            Event::SetRecoveryParameters {
                collateral_type,
                recovery_borrowing_fee,
                recovery_interest_rate_apr,
            } => {
                if let Some(config) = state.collateral_configs.get_mut(&collateral_type) {
                    config.recovery_borrowing_fee = recovery_borrowing_fee
                        .as_ref()
                        .and_then(|s| s.parse::<Decimal>().ok())
                        .map(Ratio::from);
                    config.recovery_interest_rate_apr = recovery_interest_rate_apr
                        .as_ref()
                        .and_then(|s| s.parse::<Decimal>().ok())
                        .map(Ratio::from);
                }
            },
            Event::AdminVaultCorrection {
                vault_id,
                old_amount: _,
                new_amount,
                reason: _,
            } => {
                if let Some(vault) = state.vault_id_to_vaults.get_mut(&vault_id) {
                    vault.collateral_amount = new_amount;
                }
            },
            Event::SetRateCurveMarkers { collateral_type, markers } => {
                use crate::state::{RateMarker, RateCurve, InterpolationMethod};
                if let Ok(pairs) = serde_json::from_str::<Vec<(String, String)>>(&markers) {
                    let parsed: Vec<RateMarker> = pairs.iter()
                        .filter_map(|(cr, mult)| {
                            let cr_dec = cr.parse::<Decimal>().ok()?;
                            let mult_dec = mult.parse::<Decimal>().ok()?;
                            Some(RateMarker { cr_level: Ratio::from(cr_dec), multiplier: Ratio::from(mult_dec) })
                        })
                        .collect();
                    let curve = RateCurve { markers: parsed, method: InterpolationMethod::Linear };
                    match collateral_type {
                        None => { state.global_rate_curve = curve; },
                        Some(ct_str) => {
                            if let Ok(ct) = Principal::from_text(&ct_str) {
                                if let Some(config) = state.collateral_configs.get_mut(&ct) {
                                    config.rate_curve = Some(curve);
                                }
                            }
                        }
                    }
                }
            },
            Event::SetRecoveryRateCurve { markers } => {
                use crate::state::{RecoveryRateMarker, SystemThreshold};
                if let Ok(pairs) = serde_json::from_str::<Vec<(String, String)>>(&markers) {
                    let parsed: Vec<RecoveryRateMarker> = pairs.iter()
                        .filter_map(|(thresh_str, mult_str)| {
                            let threshold = match thresh_str.as_str() {
                                "LiquidationRatio" => SystemThreshold::LiquidationRatio,
                                "BorrowThreshold" => SystemThreshold::BorrowThreshold,
                                "WarningCr" => SystemThreshold::WarningCr,
                                "HealthyCr" => SystemThreshold::HealthyCr,
                                "TotalCollateralRatio" => SystemThreshold::TotalCollateralRatio,
                                _ => return None,
                            };
                            let mult_dec = mult_str.parse::<Decimal>().ok()?;
                            Some(RecoveryRateMarker { threshold, multiplier: Ratio::from(mult_dec) })
                        })
                        .collect();
                    state.recovery_rate_curve = parsed;
                }
            },
            Event::SetHealthyCr { collateral_type, healthy_cr } => {
                if let Ok(ct) = Principal::from_text(&collateral_type) {
                    if let Some(config) = state.collateral_configs.get_mut(&ct) {
                        config.healthy_cr = healthy_cr
                            .as_ref()
                            .and_then(|s| s.parse::<Decimal>().ok())
                            .map(Ratio::from);
                    }
                }
            },
            Event::SetCollateralBorrowingFee { collateral_type, borrowing_fee, rate, fee } => {
                // Try borrowing_fee first, then legacy rate/fee fields
                let value = borrowing_fee.as_deref()
                    .or(rate.as_deref())
                    .or(fee.as_deref());
                if let Some(Ok(dec)) = value.map(|s| s.parse::<Decimal>()) {
                    if let Some(config) = state.collateral_configs.get_mut(&collateral_type) {
                        config.borrowing_fee = Ratio::from(dec);
                    }
                }
            },
            Event::SetInterestRate { collateral_type, interest_rate_apr } => {
                if let Some(config) = state.collateral_configs.get_mut(&collateral_type) {
                    if let Ok(rate) = interest_rate_apr.parse::<Decimal>() {
                        config.interest_rate_apr = Ratio::from(rate);
                    }
                }
            },
            Event::AccrueInterest { timestamp } => {
                state.accrue_all_vault_interest(timestamp);
            },
            Event::SetInterestPoolShare { share } => {
                if let Ok(dec) = share.parse::<Decimal>() {
                    state.interest_pool_share = Ratio::from(dec);
                }
            },
            Event::SetRmrFloor { value } => {
                if let Ok(dec) = value.parse::<Decimal>() {
                    state.rmr_floor = Ratio::from(dec);
                }
            },
            Event::SetRmrCeiling { value } => {
                if let Ok(dec) = value.parse::<Decimal>() {
                    state.rmr_ceiling = Ratio::from(dec);
                }
            },
            Event::SetRmrFloorCr { value } => {
                if let Ok(dec) = value.parse::<Decimal>() {
                    state.rmr_floor_cr = Ratio::from(dec);
                }
            },
            Event::SetRmrCeilingCr { value } => {
                if let Ok(dec) = value.parse::<Decimal>() {
                    state.rmr_ceiling_cr = Ratio::from(dec);
                }
            },
            Event::AdminSweepToTreasury { .. } => {
                // Ledger-only operation; no in-memory state changes during replay.
            },
            Event::SetBorrowingFeeCurve { markers } => {
                if markers == "null" {
                    state.borrowing_fee_curve = None;
                } else {
                    state.borrowing_fee_curve = serde_json::from_str(&markers).ok();
                }
            },
            Event::SetInterestSplit { split } => {
                if let Ok(recipients) = serde_json::from_str::<Vec<crate::state::InterestRecipient>>(&split) {
                    state.interest_split = recipients;
                }
            },
            Event::SetThreePoolCanister { canister } => {
                state.three_pool_canister = Some(canister);
            },
        }
    }
    state.next_available_vault_id = vault_id;
    Ok(state)
}

pub fn record_liquidate_vault(state: &mut State, vault_id: u64, mode: Mode, collateral_price: UsdIcp) {
    record_event(&Event::LiquidateVault {
        vault_id,
        mode,
        icp_rate: collateral_price,
        liquidator: None,
    });
    let _ = state.liquidate_vault(vault_id, mode, collateral_price);
}

pub fn record_redistribute_vault(state: &mut State, vault_id: u64) {
    record_event(&Event::RedistributeVault { vault_id });
    state.redistribute_vault(vault_id);
}

pub fn record_provide_liquidity(
    state: &mut State,
    amount: ICUSD,
    caller: Principal,
    block_index: u64,
) {
    record_event(&Event::ProvideLiquidity {
        amount,
        block_index,
        caller,
    });
    state.provide_liquidity(amount, caller);
}

pub fn record_withdraw_liquidity(
    state: &mut State,
    amount: ICUSD,
    caller: Principal,
    block_index: u64,
) {
    record_event(&Event::WithdrawLiquidity {
        amount,
        block_index,
        caller,
    });
    state.withdraw_liquidity(amount, caller);
}

pub fn record_claim_liquidity_returns(
    state: &mut State,
    amount: ICP,
    caller: Principal,
    block_index: u64,
) {
    record_event(&Event::ClaimLiquidityReturns {
        amount,
        block_index,
        caller,
    });
    state.claim_liquidity_returns(amount, caller);
}

pub fn record_open_vault(state: &mut State, vault: Vault, block_index: u64) {
    record_event(&Event::OpenVault {
        vault: vault.clone(),
        block_index,
    });
    state.open_vault(vault);
}

pub fn record_close_vault(state: &mut State, vault_id: u64, block_index: Option<u64>) {
    record_event(&Event::CloseVault {
        vault_id,
        block_index,
    });
    state.close_vault(vault_id);
}

pub fn record_margin_transfer(state: &mut State, vault_id: u64, block_index: u64) {
    record_event(&Event::MarginTransfer {
        vault_id,
        block_index,
    });
    state.pending_margin_transfers.remove(&vault_id);
}

pub fn record_borrow_from_vault(
    state: &mut State,
    vault_id: u64,
    borrowed_amount: ICUSD,
    fee_amount: ICUSD,
    block_index: u64,
) {
    record_event(&Event::BorrowFromVault {
        vault_id,
        block_index,
        fee_amount,
        borrowed_amount,
    });
    state.borrow_from_vault(vault_id, borrowed_amount);
    // Fee is now minted to treasury in the async caller — no longer credited to liquidity pool.
}

/// Record a repayment event and update vault state.
/// Returns the interest share of the repayment (for treasury routing).
pub fn record_repayed_to_vault(
    state: &mut State,
    vault_id: u64,
    repayed_amount: ICUSD,
    block_index: u64,
) -> ICUSD {
    record_event(&Event::RepayToVault {
        vault_id,
        block_index,
        repayed_amount,
    });
    let (interest_share, _) = state.repay_to_vault(vault_id, repayed_amount);
    interest_share
}

pub fn record_add_margin_to_vault(
    state: &mut State,
    vault_id: u64,
    margin_added: ICP,
    block_index: u64,
) {
    record_event(&Event::AddMarginToVault {
        vault_id,
        margin_added,
        block_index,
    });
    state.add_margin_to_vault(vault_id, margin_added);
}

pub fn record_redemption_on_vaults(
    state: &mut State,
    owner: Principal,
    icusd_amount: ICUSD,
    fee_amount: ICUSD,
    collateral_price: UsdIcp,
    icusd_block_index: u64,
) {
    record_event(&Event::RedemptionOnVaults {
        owner,
        current_icp_rate: collateral_price,
        icusd_amount,
        fee_amount,
        icusd_block_index,
    });
    // Fee is already deducted from icusd_amount before calling redeem_on_vaults,
    // so vault owners effectively keep the fee (less collateral seized for their debt).
    // The fee portion of icUSD stays in the protocol canister (burned).
    let redeem_ct = state.icp_collateral_type();
    state.redeem_on_vaults(icusd_amount, collateral_price, &redeem_ct);
    let margin: ICP = icusd_amount / collateral_price;
    state
        .pending_redemption_transfer
        .insert(icusd_block_index, PendingMarginTransfer { owner, margin, collateral_type: crate::vault::default_collateral_type() });
}

pub fn record_redemption_transfered(
    state: &mut State,
    icusd_block_index: u64,
    icp_block_index: u64,
) {
    record_event(&Event::RedemptionTransfered {
        icusd_block_index,
        icp_block_index,
    });
    state.pending_redemption_transfer.remove(&icusd_block_index);
}

pub fn record_collateral_withdrawn(
    _state: &mut State,
    vault_id: u64,
    amount: ICP,
    block_index: u64,
) {
    record_event(&Event::CollateralWithdrawn {
        vault_id,
        amount,
        block_index,
    });

}

pub fn record_partial_collateral_withdrawn(
    state: &mut State,
    vault_id: u64,
    amount: ICP,
    block_index: u64,
) {
    record_event(&Event::PartialCollateralWithdrawn {
        vault_id,
        amount,
        block_index,
    });
    state.remove_margin_from_vault(vault_id, amount);
}

pub fn record_withdraw_and_close_vault(
    state: &mut State,
    vault_id: u64,
    amount: ICP,
    block_index: Option<u64>
) {
    record_event(&Event::WithdrawAndCloseVault {
        vault_id,
        amount,
        block_index,
    });
    
    // Close the vault (withdrawal is already handled in vault.rs)
    state.close_vault(vault_id);
}

pub fn record_set_ckstable_repay_fee(state: &mut State, rate: Ratio) {
    record_event(&Event::SetCkstableRepayFee {
        rate: rate.0.to_string(),
    });
    state.ckstable_repay_fee = rate;
}

pub fn record_set_min_icusd_amount(state: &mut State, amount: ICUSD) {
    record_event(&Event::SetMinIcusdAmount {
        amount: amount.to_u64().to_string(),
    });
    state.min_icusd_amount = amount;
}

pub fn record_set_global_icusd_mint_cap(state: &mut State, amount: u64) {
    record_event(&Event::SetGlobalIcusdMintCap {
        amount: Some(amount.to_string()),
        cap: None,
    });
    state.global_icusd_mint_cap = amount;
}

pub fn record_set_stable_token_enabled(state: &mut State, token_type: StableTokenType, enabled: bool) {
    record_event(&Event::SetStableTokenEnabled {
        token_type: token_type.clone(),
        enabled,
    });
    match token_type {
        StableTokenType::CKUSDT => state.ckusdt_enabled = enabled,
        StableTokenType::CKUSDC => state.ckusdc_enabled = enabled,
    }
}

pub fn record_set_stable_ledger_principal(state: &mut State, token_type: StableTokenType, principal: Principal) {
    record_event(&Event::SetStableLedgerPrincipal {
        token_type: token_type.clone(),
        principal,
    });
    match token_type {
        StableTokenType::CKUSDT => state.ckusdt_ledger_principal = Some(principal),
        StableTokenType::CKUSDC => state.ckusdc_ledger_principal = Some(principal),
    }
}

pub fn record_set_treasury_principal(state: &mut State, principal: Principal) {
    record_event(&Event::SetTreasuryPrincipal { principal });
    state.treasury_principal = Some(principal);
}

pub fn record_set_stability_pool_principal(state: &mut State, principal: Principal) {
    record_event(&Event::SetStabilityPoolPrincipal { principal });
    state.stability_pool_canister = Some(principal);
}

pub fn record_set_liquidation_bonus(state: &mut State, rate: Ratio) {
    record_event(&Event::SetLiquidationBonus {
        rate: rate.0.to_string(),
    });
    state.liquidation_bonus = rate;
    state.sync_icp_collateral_config();
}

pub fn record_set_borrowing_fee(state: &mut State, rate: Ratio) {
    record_event(&Event::SetBorrowingFee {
        rate: rate.0.to_string(),
    });
    state.fee = rate;
    state.sync_icp_collateral_config();
}

pub fn record_set_redemption_fee_floor(state: &mut State, rate: Ratio) {
    record_event(&Event::SetRedemptionFeeFloor {
        rate: rate.0.to_string(),
    });
    state.redemption_fee_floor = rate;
    state.sync_icp_collateral_config();
}

pub fn record_set_redemption_fee_ceiling(state: &mut State, rate: Ratio) {
    record_event(&Event::SetRedemptionFeeCeiling {
        rate: rate.0.to_string(),
    });
    state.redemption_fee_ceiling = rate;
    state.sync_icp_collateral_config();
}

pub fn record_set_max_partial_liquidation_ratio(state: &mut State, rate: Ratio) {
    record_event(&Event::SetMaxPartialLiquidationRatio {
        rate: rate.0.to_string(),
    });
    state.max_partial_liquidation_ratio = rate;
}

pub fn record_set_recovery_target_cr(state: &mut State, rate: Ratio) {
    record_event(&Event::SetRecoveryTargetCr {
        rate: rate.0.to_string(),
    });
    state.recovery_target_cr = rate;
    state.sync_icp_collateral_config();
}

pub fn record_set_recovery_cr_multiplier(state: &mut State, multiplier: Ratio) {
    record_event(&Event::SetRecoveryCrMultiplier {
        multiplier: multiplier.0.to_string(),
    });
    state.recovery_cr_multiplier = multiplier;
    state.sync_icp_collateral_config();
}

pub fn record_set_liquidation_protocol_share(state: &mut State, share: Ratio) {
    record_event(&Event::SetLiquidationProtocolShare {
        share: share.0.to_string(),
    });
    state.liquidation_protocol_share = share;
}

pub fn record_set_interest_pool_share(state: &mut State, share: Ratio) {
    record_event(&Event::SetInterestPoolShare {
        share: share.0.to_string(),
    });
    state.interest_pool_share = share;
}

pub fn record_set_rmr_floor(state: &mut State, value: Ratio) {
    record_event(&Event::SetRmrFloor { value: value.0.to_string() });
    state.rmr_floor = value;
}

pub fn record_set_rmr_ceiling(state: &mut State, value: Ratio) {
    record_event(&Event::SetRmrCeiling { value: value.0.to_string() });
    state.rmr_ceiling = value;
}

pub fn record_set_rmr_floor_cr(state: &mut State, value: Ratio) {
    record_event(&Event::SetRmrFloorCr { value: value.0.to_string() });
    state.rmr_floor_cr = value;
}

pub fn record_set_rmr_ceiling_cr(state: &mut State, value: Ratio) {
    record_event(&Event::SetRmrCeilingCr { value: value.0.to_string() });
    state.rmr_ceiling_cr = value;
}

pub fn record_add_collateral_type(
    state: &mut State,
    collateral_type: CollateralType,
    config: CollateralConfig,
) {
    record_event(&Event::AddCollateralType {
        collateral_type,
        config: config.clone(),
    });
    state.collateral_configs.insert(collateral_type, config);
}

pub fn record_update_collateral_status(
    state: &mut State,
    collateral_type: CollateralType,
    status: CollateralStatus,
) {
    record_event(&Event::UpdateCollateralStatus {
        collateral_type,
        status,
    });
    if let Some(config) = state.collateral_configs.get_mut(&collateral_type) {
        config.status = status;
    }
}

pub fn record_update_collateral_config(
    state: &mut State,
    collateral_type: CollateralType,
    config: CollateralConfig,
) {
    record_event(&Event::UpdateCollateralConfig {
        collateral_type,
        config: config.clone(),
    });
    state.collateral_configs.insert(collateral_type, config);
}

pub fn record_set_reserve_redemptions_enabled(state: &mut State, enabled: bool) {
    record_event(&Event::SetReserveRedemptionsEnabled { enabled });
    state.reserve_redemptions_enabled = enabled;
}

pub fn record_set_reserve_redemption_fee(state: &mut State, fee: Ratio) {
    record_event(&Event::SetReserveRedemptionFee {
        fee: fee.0.to_string(),
    });
    state.reserve_redemption_fee = fee;
}

pub fn record_reserve_redemption(
    owner: Principal,
    icusd_amount: ICUSD,
    fee_amount: ICUSD,
    stable_token_ledger: Principal,
    stable_amount_sent: u64,
    fee_stable_amount: u64,
    icusd_block_index: u64,
) {
    record_event(&Event::ReserveRedemption {
        owner,
        icusd_amount,
        fee_amount,
        stable_token_ledger,
        stable_amount_sent,
        fee_stable_amount,
        icusd_block_index,
    });
}

pub fn record_admin_mint(
    amount: ICUSD,
    to: Principal,
    reason: String,
    block_index: u64,
) {
    record_event(&Event::AdminMint {
        amount,
        to,
        reason,
        block_index,
    });
}

pub fn record_set_recovery_parameters(
    state: &mut State,
    collateral_type: CollateralType,
    recovery_borrowing_fee: Option<Ratio>,
    recovery_interest_rate_apr: Option<Ratio>,
) {
    record_event(&Event::SetRecoveryParameters {
        collateral_type,
        recovery_borrowing_fee: recovery_borrowing_fee.map(|r| r.0.to_string()),
        recovery_interest_rate_apr: recovery_interest_rate_apr.map(|r| r.0.to_string()),
    });
    if let Some(config) = state.collateral_configs.get_mut(&collateral_type) {
        config.recovery_borrowing_fee = recovery_borrowing_fee;
        config.recovery_interest_rate_apr = recovery_interest_rate_apr;
    }
}

pub fn record_admin_vault_correction(
    state: &mut State,
    vault_id: u64,
    old_amount: u64,
    new_amount: u64,
    reason: String,
) {
    record_event(&Event::AdminVaultCorrection {
        vault_id,
        old_amount,
        new_amount,
        reason,
    });
    if let Some(vault) = state.vault_id_to_vaults.get_mut(&vault_id) {
        vault.collateral_amount = new_amount;
    }
}

pub fn record_admin_sweep_to_treasury(
    amount: u64,
    treasury: Principal,
    block_index: u64,
    reason: String,
) {
    record_event(&Event::AdminSweepToTreasury {
        amount,
        treasury,
        block_index,
        reason,
    });
}

pub fn record_set_rate_curve_markers(
    state: &mut State,
    collateral_type: Option<CollateralType>,
    markers: Vec<(f64, f64)>,
) {
    use crate::state::{RateMarker, RateCurve, InterpolationMethod};
    let serialized: Vec<(String, String)> = markers.iter()
        .map(|(cr, mult)| (cr.to_string(), mult.to_string()))
        .collect();
    let markers_json = serde_json::to_string(&serialized).unwrap_or_default();
    record_event(&Event::SetRateCurveMarkers {
        collateral_type: collateral_type.map(|ct| ct.to_text()),
        markers: markers_json,
    });
    let parsed: Vec<RateMarker> = markers.iter()
        .map(|(cr, mult)| RateMarker {
            cr_level: Ratio::from_f64(*cr),
            multiplier: Ratio::from_f64(*mult),
        })
        .collect();
    let curve = RateCurve { markers: parsed, method: InterpolationMethod::Linear };
    match collateral_type {
        None => { state.global_rate_curve = curve; },
        Some(ct) => {
            if let Some(config) = state.collateral_configs.get_mut(&ct) {
                config.rate_curve = Some(curve);
            }
        }
    }
}

pub fn record_set_recovery_rate_curve(
    state: &mut State,
    markers: Vec<(crate::state::SystemThreshold, f64)>,
) {
    use crate::state::{RecoveryRateMarker, SystemThreshold};
    let serialized: Vec<(String, String)> = markers.iter()
        .map(|(thresh, mult)| {
            let thresh_str = match thresh {
                SystemThreshold::LiquidationRatio => "LiquidationRatio",
                SystemThreshold::BorrowThreshold => "BorrowThreshold",
                SystemThreshold::WarningCr => "WarningCr",
                SystemThreshold::HealthyCr => "HealthyCr",
                SystemThreshold::TotalCollateralRatio => "TotalCollateralRatio",
            };
            (thresh_str.to_string(), mult.to_string())
        })
        .collect();
    let markers_json = serde_json::to_string(&serialized).unwrap_or_default();
    record_event(&Event::SetRecoveryRateCurve {
        markers: markers_json,
    });
    state.recovery_rate_curve = markers.iter()
        .map(|(thresh, mult)| RecoveryRateMarker {
            threshold: thresh.clone(),
            multiplier: Ratio::from_f64(*mult),
        })
        .collect();
}

pub fn record_set_borrowing_fee_curve(
    state: &mut State,
    curve: Option<RateCurveV2>,
) {
    let markers_json = match &curve {
        Some(c) => serde_json::to_string(&c).unwrap_or_default(),
        None => "null".to_string(),
    };
    record_event(&Event::SetBorrowingFeeCurve {
        markers: markers_json,
    });
    state.borrowing_fee_curve = curve;
}

pub fn record_set_healthy_cr(
    state: &mut State,
    collateral_type: CollateralType,
    healthy_cr: Option<Ratio>,
) {
    record_event(&Event::SetHealthyCr {
        collateral_type: collateral_type.to_text(),
        healthy_cr: healthy_cr.map(|r| r.0.to_string()),
    });
    if let Some(config) = state.collateral_configs.get_mut(&collateral_type) {
        config.healthy_cr = healthy_cr;
    }
}

pub fn record_set_interest_split(state: &mut State, split: Vec<crate::state::InterestRecipient>) {
    let split_json = serde_json::to_string(&split).unwrap_or_default();
    record_event(&Event::SetInterestSplit { split: split_json });
    state.interest_split = split;
}

pub fn record_set_three_pool_canister(state: &mut State, canister: Principal) {
    record_event(&Event::SetThreePoolCanister { canister });
    state.three_pool_canister = Some(canister);
}

pub fn record_set_collateral_borrowing_fee(
    state: &mut State,
    collateral_type: CollateralType,
    borrowing_fee: Ratio,
) {
    record_event(&Event::SetCollateralBorrowingFee {
        collateral_type,
        borrowing_fee: Some(borrowing_fee.0.to_string()),
        rate: None,
        fee: None,
    });
    if let Some(config) = state.collateral_configs.get_mut(&collateral_type) {
        config.borrowing_fee = borrowing_fee;
    }
}

/// Record an interest accrual event and apply to all vaults.
pub fn record_set_interest_rate(
    state: &mut State,
    collateral_type: CollateralType,
    interest_rate_apr: Ratio,
) {
    record_event(&Event::SetInterestRate {
        collateral_type,
        interest_rate_apr: interest_rate_apr.0.to_string(),
    });
    if let Some(config) = state.collateral_configs.get_mut(&collateral_type) {
        config.interest_rate_apr = interest_rate_apr;
    }
}

pub fn record_accrue_interest(state: &mut State, now_nanos: u64) {
    record_event(&Event::AccrueInterest { timestamp: now_nanos });
    state.accrue_all_vault_interest(now_nanos);
}
