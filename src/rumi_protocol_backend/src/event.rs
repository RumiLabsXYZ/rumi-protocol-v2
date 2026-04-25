use crate::numeric::{Ratio, UsdIcp, ICUSD, ICP};
use crate::state::{CollateralConfig, CollateralStatus, CollateralType, PendingMarginTransfer, RateCurveV2, State};
use crate::storage::record_event;
use crate::vault::Vault;
use crate::{EventTimeRange, EventTypeFilter, InitArg, Mode, StableTokenType, UpgradeArg};
use candid::{CandidType, Principal};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Per-vault breakdown of a redemption: how much icUSD was redeemed and how much
/// collateral was seized from each individual vault.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultRedemption {
    pub vault_id: u64,
    pub icusd_redeemed_e8s: u64,
    pub collateral_seized: u64,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Event {
    #[serde(rename = "open_vault")]
    OpenVault {
        vault: Vault,
        block_index: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<u64>,
    },

    #[serde(rename = "close_vault")]
    CloseVault {
        vault_id: u64,
        block_index: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<u64>,
    },

    #[serde(rename = "margin_transfer")]
    MarginTransfer {
        vault_id: u64,
        block_index: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<u64>,
    },

    #[serde(rename = "liquidate_vault")]
    LiquidateVault {
        vault_id: u64,
        mode: Mode,
        icp_rate: UsdIcp,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        liquidator: Option<Principal>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<u64>,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<u64>,
        /// 3USD (LP tokens) credited to protocol reserves during this liquidation.
        /// None for legacy burn-path liquidations; Some(amount_e8s) for reserves-path.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        three_usd_reserves_e8s: Option<u64>,
    },

    #[serde(rename = "redemption_on_vaults")]
    RedemptionOnVaults {
        owner: Principal,
        current_icp_rate: UsdIcp,
        icusd_amount: ICUSD,
        fee_amount: ICUSD,
        icusd_block_index: u64,
        /// Which collateral type was redeemed. None for old events (pre-tiering).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        collateral_type: Option<CollateralType>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<u64>,
        /// Per-vault breakdown: how much was redeemed from each vault.
        /// None for legacy events recorded before this field existed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vault_redemptions: Option<Vec<VaultRedemption>>,
    },

    #[serde(rename = "redemption_transfered")]
    RedemptionTransfered {
        icusd_block_index: u64,
        icp_block_index: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<u64>,
    },

    #[serde(rename = "redistribute_vault")]
    RedistributeVault {
        vault_id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<u64>,
    },

    #[serde(rename = "borrow_from_vault")]
    BorrowFromVault {
        vault_id: u64,
        borrowed_amount: ICUSD,
        fee_amount: ICUSD,
        block_index: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caller: Option<Principal>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<u64>,
    },

    #[serde(rename = "repay_to_vault")]
    RepayToVault {
        vault_id: u64,
        repayed_amount: ICUSD,
        block_index: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caller: Option<Principal>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<u64>,
    },

    #[serde(rename = "add_margin_to_vault")]
    AddMarginToVault {
        vault_id: u64,
        margin_added: ICP,
        block_index: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caller: Option<Principal>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<u64>,
    },

    #[serde(rename = "provide_liquidity")]
    ProvideLiquidity {
        amount: ICUSD,
        block_index: u64,
        caller: Principal,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<u64>,
    },

    #[serde(rename = "withdraw_liquidity")]
    WithdrawLiquidity {
        amount: ICUSD,
        block_index: u64,
        caller: Principal,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<u64>,
    },

    #[serde(rename = "claim_liquidity_returns")]
    ClaimLiquidityReturns {
        amount: ICP,
        block_index: u64,
        caller: Principal,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<u64>,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caller: Option<Principal>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<u64>,
    },

    // TODO(multi-collateral): amount type will need to be generic or token-tagged
    #[serde(rename = "partial_collateral_withdrawn")]
    PartialCollateralWithdrawn {
        vault_id: u64,
        amount: ICP,
        block_index: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caller: Option<Principal>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<u64>,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caller: Option<Principal>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<u64>,
    },

    #[serde(rename = "dust_forgiven")]
    DustForgiven {
        vault_id: u64,
        amount: ICUSD,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<u64>,
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

    #[serde(rename = "set_liquidation_bot_principal")]
    SetLiquidationBotPrincipal {
        principal: Principal,
    },

    #[serde(rename = "set_bot_budget")]
    SetBotBudget {
        total_e8s: u64,
        start_timestamp: u64,
    },

    #[serde(rename = "set_bot_allowed_collateral_types")]
    SetBotAllowedCollateralTypes {
        collateral_types: Vec<Principal>,
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

    #[serde(rename = "set_icpswap_routing_enabled")]
    SetIcpswapRoutingEnabled {
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<u64>,
    },
    #[serde(rename = "admin_mint")]
    AdminMint {
        amount: ICUSD,
        to: Principal,
        reason: String,
        block_index: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<u64>,
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

    /// Price update from XRC or other oracle. Recorded every time a collateral
    /// price is fetched so we have a complete price history.
    #[serde(rename = "price_update")]
    PriceUpdate {
        collateral_type: CollateralType,
        /// Price as a string for full Decimal precision.
        price: String,
        timestamp: u64,
    },

    /// Admin set per-collateral liquidation ratio.
    #[serde(rename = "set_collateral_liquidation_ratio")]
    SetCollateralLiquidationRatio {
        collateral_type: CollateralType,
        liquidation_ratio: String,
    },

    /// Admin set per-collateral borrow threshold ratio (recovery-mode trigger).
    #[serde(rename = "set_collateral_borrow_threshold")]
    SetCollateralBorrowThreshold {
        collateral_type: CollateralType,
        borrow_threshold_ratio: String,
    },

    /// Admin set per-collateral liquidation bonus.
    #[serde(rename = "set_collateral_liquidation_bonus")]
    SetCollateralLiquidationBonus {
        collateral_type: CollateralType,
        liquidation_bonus: String,
    },

    /// Admin set per-collateral minimum vault debt (dust threshold).
    #[serde(rename = "set_collateral_min_vault_debt")]
    SetCollateralMinVaultDebt {
        collateral_type: CollateralType,
        min_vault_debt: u64,
    },

    /// Admin set per-collateral ledger fee (native units).
    #[serde(rename = "set_collateral_ledger_fee")]
    SetCollateralLedgerFee {
        collateral_type: CollateralType,
        ledger_fee: u64,
    },

    /// Admin set per-collateral redemption fee floor.
    #[serde(rename = "set_collateral_redemption_fee_floor")]
    SetCollateralRedemptionFeeFloor {
        collateral_type: CollateralType,
        redemption_fee_floor: String,
    },

    /// Admin set per-collateral redemption fee ceiling.
    #[serde(rename = "set_collateral_redemption_fee_ceiling")]
    SetCollateralRedemptionFeeCeiling {
        collateral_type: CollateralType,
        redemption_fee_ceiling: String,
    },

    /// Admin set per-collateral minimum deposit amount (native units).
    #[serde(rename = "set_collateral_min_deposit")]
    SetCollateralMinDeposit {
        collateral_type: CollateralType,
        min_collateral_deposit: u64,
    },

    /// Admin set per-collateral display color (hex) for frontend.
    #[serde(rename = "set_collateral_display_color")]
    SetCollateralDisplayColor {
        collateral_type: CollateralType,
        display_color: Option<String>,
    },

    /// Admin correction of vault debt to fix replay interest drift.
    #[serde(rename = "admin_debt_correction")]
    AdminDebtCorrection {
        vault_id: u64,
        old_borrowed: u64,
        new_borrowed: u64,
        old_accrued: u64,
        new_accrued: u64,
        #[serde(default)]
        timestamp: Option<u64>,
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
            Event::RedemptionOnVaults { vault_redemptions, .. } => {
                match vault_redemptions {
                    Some(vrs) => vrs.iter().any(|vr| &vr.vault_id == filter_vault_id),
                    None => true, // Legacy events without per-vault data: show on all vaults
                }
            }
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
            Event::SetLiquidationBotPrincipal { .. } => false,
            Event::SetBotBudget { .. } => false,
            Event::SetBotAllowedCollateralTypes { .. } => false,
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
            Event::SetIcpswapRoutingEnabled { .. } => false,
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
            Event::PriceUpdate { .. } => false,
            Event::SetCollateralLiquidationRatio { .. } => false,
            Event::SetCollateralBorrowThreshold { .. } => false,
            Event::SetCollateralLiquidationBonus { .. } => false,
            Event::SetCollateralMinVaultDebt { .. } => false,
            Event::SetCollateralLedgerFee { .. } => false,
            Event::SetCollateralRedemptionFeeFloor { .. } => false,
            Event::SetCollateralRedemptionFeeCeiling { .. } => false,
            Event::SetCollateralMinDeposit { .. } => false,
            Event::SetCollateralDisplayColor { .. } => false,
            Event::AdminDebtCorrection { vault_id: vid, .. } => vid == filter_vault_id,
        }
    }

    /// Returns true if this is a noisy periodic event (hidden from explorer).
    pub fn is_accrue_interest(&self) -> bool {
        matches!(self, Event::AccrueInterest { .. } | Event::PriceUpdate { .. })
    }

    /// Coarse type classification for the explorer's `types` facet.
    /// All admin/setter variants collapse into `EventTypeFilter::Admin`.
    pub fn type_filter(&self) -> EventTypeFilter {
        match self {
            Event::OpenVault { .. } => EventTypeFilter::OpenVault,
            Event::CloseVault { .. }
            | Event::WithdrawAndCloseVault { .. }
            | Event::VaultWithdrawnAndClosed { .. } => EventTypeFilter::CloseVault,
            Event::AddMarginToVault { .. }
            | Event::CollateralWithdrawn { .. }
            | Event::PartialCollateralWithdrawn { .. }
            | Event::MarginTransfer { .. }
            | Event::RedistributeVault { .. }
            | Event::DustForgiven { .. }
            | Event::AdminVaultCorrection { .. }
            | Event::AdminDebtCorrection { .. } => EventTypeFilter::AdjustVault,
            Event::BorrowFromVault { .. } => EventTypeFilter::Borrow,
            Event::RepayToVault { .. } => EventTypeFilter::Repay,
            Event::LiquidateVault { .. } => EventTypeFilter::Liquidation,
            Event::PartialLiquidateVault { .. } => EventTypeFilter::PartialLiquidation,
            Event::RedemptionOnVaults { .. } | Event::RedemptionTransfered { .. } => {
                EventTypeFilter::Redemption
            }
            Event::ReserveRedemption { .. } => EventTypeFilter::ReserveRedemption,
            Event::ProvideLiquidity { .. } => EventTypeFilter::StabilityPoolDeposit,
            Event::WithdrawLiquidity { .. } | Event::ClaimLiquidityReturns { .. } => {
                EventTypeFilter::StabilityPoolWithdraw
            }
            Event::AdminMint { .. } => EventTypeFilter::AdminMint,
            Event::AdminSweepToTreasury { .. } => EventTypeFilter::AdminSweepToTreasury,
            Event::PriceUpdate { .. } => EventTypeFilter::PriceUpdate,
            Event::AccrueInterest { .. } => EventTypeFilter::AccrueInterest,
            _ => EventTypeFilter::Admin,
        }
    }

    /// Canonical label for admin/setter variants (i.e. events whose
    /// `type_filter()` returns `Admin`). Returns `None` for user-facing
    /// variants classified into any other `EventTypeFilter`. Labels are the
    /// Rust variant name in CamelCase, paralleling the `EventTypeFilter`
    /// casing so the frontend can surface per-setter facets without having to
    /// deal with both CamelCase and snake_case on the wire.
    pub fn admin_label(&self) -> Option<&'static str> {
        if self.type_filter() != EventTypeFilter::Admin {
            return None;
        }
        match self {
            Event::Init(_) => Some("Init"),
            Event::Upgrade(_) => Some("Upgrade"),
            Event::SetCkstableRepayFee { .. } => Some("SetCkstableRepayFee"),
            Event::SetMinIcusdAmount { .. } => Some("SetMinIcusdAmount"),
            Event::SetGlobalIcusdMintCap { .. } => Some("SetGlobalIcusdMintCap"),
            Event::SetStableTokenEnabled { .. } => Some("SetStableTokenEnabled"),
            Event::SetStableLedgerPrincipal { .. } => Some("SetStableLedgerPrincipal"),
            Event::SetTreasuryPrincipal { .. } => Some("SetTreasuryPrincipal"),
            Event::SetStabilityPoolPrincipal { .. } => Some("SetStabilityPoolPrincipal"),
            Event::SetLiquidationBotPrincipal { .. } => Some("SetLiquidationBotPrincipal"),
            Event::SetBotBudget { .. } => Some("SetBotBudget"),
            Event::SetBotAllowedCollateralTypes { .. } => Some("SetBotAllowedCollateralTypes"),
            Event::SetLiquidationBonus { .. } => Some("SetLiquidationBonus"),
            Event::SetBorrowingFee { .. } => Some("SetBorrowingFee"),
            Event::SetRedemptionFeeFloor { .. } => Some("SetRedemptionFeeFloor"),
            Event::SetRedemptionFeeCeiling { .. } => Some("SetRedemptionFeeCeiling"),
            Event::SetMaxPartialLiquidationRatio { .. } => Some("SetMaxPartialLiquidationRatio"),
            Event::SetRecoveryTargetCr { .. } => Some("SetRecoveryTargetCr"),
            Event::SetRecoveryCrMultiplier { .. } => Some("SetRecoveryCrMultiplier"),
            Event::SetLiquidationProtocolShare { .. } => Some("SetLiquidationProtocolShare"),
            Event::AddCollateralType { .. } => Some("AddCollateralType"),
            Event::UpdateCollateralStatus { .. } => Some("UpdateCollateralStatus"),
            Event::UpdateCollateralConfig { .. } => Some("UpdateCollateralConfig"),
            Event::SetReserveRedemptionsEnabled { .. } => Some("SetReserveRedemptionsEnabled"),
            Event::SetIcpswapRoutingEnabled { .. } => Some("SetIcpswapRoutingEnabled"),
            Event::SetReserveRedemptionFee { .. } => Some("SetReserveRedemptionFee"),
            Event::SetRecoveryParameters { .. } => Some("SetRecoveryParameters"),
            Event::SetRateCurveMarkers { .. } => Some("SetRateCurveMarkers"),
            Event::SetRecoveryRateCurve { .. } => Some("SetRecoveryRateCurve"),
            Event::SetHealthyCr { .. } => Some("SetHealthyCr"),
            Event::SetCollateralBorrowingFee { .. } => Some("SetCollateralBorrowingFee"),
            Event::SetInterestRate { .. } => Some("SetInterestRate"),
            Event::SetInterestPoolShare { .. } => Some("SetInterestPoolShare"),
            Event::SetRmrFloor { .. } => Some("SetRmrFloor"),
            Event::SetRmrCeiling { .. } => Some("SetRmrCeiling"),
            Event::SetRmrFloorCr { .. } => Some("SetRmrFloorCr"),
            Event::SetRmrCeilingCr { .. } => Some("SetRmrCeilingCr"),
            Event::SetBorrowingFeeCurve { .. } => Some("SetBorrowingFeeCurve"),
            Event::SetInterestSplit { .. } => Some("SetInterestSplit"),
            Event::SetThreePoolCanister { .. } => Some("SetThreePoolCanister"),
            Event::SetCollateralLiquidationRatio { .. } => Some("SetCollateralLiquidationRatio"),
            Event::SetCollateralBorrowThreshold { .. } => Some("SetCollateralBorrowThreshold"),
            Event::SetCollateralLiquidationBonus { .. } => Some("SetCollateralLiquidationBonus"),
            Event::SetCollateralMinVaultDebt { .. } => Some("SetCollateralMinVaultDebt"),
            Event::SetCollateralLedgerFee { .. } => Some("SetCollateralLedgerFee"),
            Event::SetCollateralRedemptionFeeFloor { .. } => Some("SetCollateralRedemptionFeeFloor"),
            Event::SetCollateralRedemptionFeeCeiling { .. } => Some("SetCollateralRedemptionFeeCeiling"),
            Event::SetCollateralMinDeposit { .. } => Some("SetCollateralMinDeposit"),
            Event::SetCollateralDisplayColor { .. } => Some("SetCollateralDisplayColor"),
            // Any variant that surfaces `Admin` via `type_filter` but isn't
            // enumerated here still matches `Admin` type filters; it just
            // carries no fine-grained label.
            _ => None,
        }
    }

    /// Recorded timestamp in nanoseconds, when the event variant carries one.
    /// Used by the time-range facet; events returning `None` are excluded
    /// from time-filtered queries.
    pub fn timestamp_ns(&self) -> Option<u64> {
        match self {
            Event::OpenVault { timestamp, .. }
            | Event::CloseVault { timestamp, .. }
            | Event::MarginTransfer { timestamp, .. }
            | Event::LiquidateVault { timestamp, .. }
            | Event::PartialLiquidateVault { timestamp, .. }
            | Event::RedemptionOnVaults { timestamp, .. }
            | Event::RedemptionTransfered { timestamp, .. }
            | Event::RedistributeVault { timestamp, .. }
            | Event::BorrowFromVault { timestamp, .. }
            | Event::RepayToVault { timestamp, .. }
            | Event::AddMarginToVault { timestamp, .. }
            | Event::ProvideLiquidity { timestamp, .. }
            | Event::WithdrawLiquidity { timestamp, .. }
            | Event::ClaimLiquidityReturns { timestamp, .. }
            | Event::CollateralWithdrawn { timestamp, .. }
            | Event::PartialCollateralWithdrawn { timestamp, .. }
            | Event::WithdrawAndCloseVault { timestamp, .. }
            | Event::DustForgiven { timestamp, .. }
            | Event::ReserveRedemption { timestamp, .. }
            | Event::AdminMint { timestamp, .. }
            | Event::AdminDebtCorrection { timestamp, .. } => *timestamp,
            Event::VaultWithdrawnAndClosed { timestamp, .. } => Some(*timestamp),
            Event::PriceUpdate { timestamp, .. } => Some(*timestamp),
            Event::AccrueInterest { timestamp } => Some(*timestamp),
            Event::SetBotBudget { start_timestamp, .. } => Some(*start_timestamp),
            _ => None,
        }
    }

    /// Collateral token referenced by this event, if any. For vault-id events
    /// the collateral type isn't carried in the event itself, so the caller
    /// passes `vault_lookup` (built once per query by walking `OpenVault`
    /// events). Returns `None` for events with no collateral context.
    pub fn collateral_token(&self, vault_lookup: &HashMap<u64, Principal>) -> Option<Principal> {
        match self {
            Event::OpenVault { vault, .. } => Some(vault.collateral_type),
            Event::AddCollateralType { collateral_type, .. }
            | Event::UpdateCollateralStatus { collateral_type, .. }
            | Event::UpdateCollateralConfig { collateral_type, .. }
            | Event::SetCollateralBorrowingFee { collateral_type, .. }
            | Event::SetInterestRate { collateral_type, .. }
            | Event::SetRecoveryParameters { collateral_type, .. }
            | Event::SetCollateralLiquidationRatio { collateral_type, .. }
            | Event::SetCollateralBorrowThreshold { collateral_type, .. }
            | Event::SetCollateralLiquidationBonus { collateral_type, .. }
            | Event::SetCollateralMinVaultDebt { collateral_type, .. }
            | Event::SetCollateralLedgerFee { collateral_type, .. }
            | Event::SetCollateralRedemptionFeeFloor { collateral_type, .. }
            | Event::SetCollateralRedemptionFeeCeiling { collateral_type, .. }
            | Event::SetCollateralMinDeposit { collateral_type, .. }
            | Event::SetCollateralDisplayColor { collateral_type, .. }
            | Event::PriceUpdate { collateral_type, .. } => Some(*collateral_type),
            Event::RedemptionOnVaults { collateral_type, .. } => *collateral_type,
            Event::ReserveRedemption { stable_token_ledger, .. } => Some(*stable_token_ledger),
            Event::CloseVault { vault_id, .. }
            | Event::MarginTransfer { vault_id, .. }
            | Event::LiquidateVault { vault_id, .. }
            | Event::PartialLiquidateVault { vault_id, .. }
            | Event::RedistributeVault { vault_id, .. }
            | Event::BorrowFromVault { vault_id, .. }
            | Event::RepayToVault { vault_id, .. }
            | Event::AddMarginToVault { vault_id, .. }
            | Event::CollateralWithdrawn { vault_id, .. }
            | Event::PartialCollateralWithdrawn { vault_id, .. }
            | Event::VaultWithdrawnAndClosed { vault_id, .. }
            | Event::WithdrawAndCloseVault { vault_id, .. }
            | Event::DustForgiven { vault_id, .. }
            | Event::AdminVaultCorrection { vault_id, .. }
            | Event::AdminDebtCorrection { vault_id, .. } => vault_lookup.get(vault_id).copied(),
            _ => None,
        }
    }

    /// Primary "size" of this event in icUSD e8s (= USD e8s) for the size facet.
    /// icUSD-denominated amounts pass through; ICP/collateral amounts are
    /// converted using `icp_price_e8s` (current spot, in 1e8 USD per ICP).
    /// Returns `None` for events with no meaningful magnitude (admin setters,
    /// init/upgrade, accrue/price ticks); the size filter treats `None` as
    /// "passes" so these events surface independent of the threshold.
    /// Multi-collateral conversions use the ICP price as a v1 approximation.
    pub fn size_e8s_usd(&self, icp_price_e8s: u64) -> Option<u64> {
        let convert = |native_amount: u64| -> u64 {
            ((native_amount as u128) * (icp_price_e8s as u128) / 100_000_000u128) as u64
        };
        match self {
            Event::BorrowFromVault { borrowed_amount, .. } => Some(borrowed_amount.0),
            Event::RepayToVault { repayed_amount, .. } => Some(repayed_amount.0),
            Event::RedemptionOnVaults { icusd_amount, .. } => Some(icusd_amount.0),
            Event::ReserveRedemption { icusd_amount, .. } => Some(icusd_amount.0),
            Event::AdminMint { amount, .. } => Some(amount.0),
            Event::DustForgiven { amount, .. } => Some(amount.0),
            Event::ProvideLiquidity { amount, .. } => Some(amount.0),
            Event::WithdrawLiquidity { amount, .. } => Some(amount.0),
            Event::PartialLiquidateVault { liquidator_payment, .. } => Some(liquidator_payment.0),
            Event::OpenVault { vault, .. } => Some(convert(vault.collateral_amount)),
            Event::AddMarginToVault { margin_added, .. } => Some(convert(margin_added.0)),
            Event::CollateralWithdrawn { amount, .. } => Some(convert(amount.0)),
            Event::PartialCollateralWithdrawn { amount, .. } => Some(convert(amount.0)),
            Event::WithdrawAndCloseVault { amount, .. } => Some(convert(amount.0)),
            Event::VaultWithdrawnAndClosed { amount, .. } => Some(convert(amount.0)),
            Event::ClaimLiquidityReturns { amount, .. } => Some(convert(amount.0)),
            Event::AdminSweepToTreasury { amount, .. } => Some(*amount),
            _ => None,
        }
    }

    /// AND-combine all `get_events_filtered` facets and return whether this
    /// event passes. Pure function — caller supplies the per-query lookup map
    /// and a price snapshot.
    #[allow(clippy::too_many_arguments)]
    pub fn passes_filters(
        &self,
        types_set: Option<&HashSet<EventTypeFilter>>,
        principal: Option<&Principal>,
        collateral_token: Option<&Principal>,
        time_range: Option<&EventTimeRange>,
        min_size_e8s: Option<u64>,
        admin_labels: Option<&HashSet<String>>,
        vault_lookup: &HashMap<u64, Principal>,
        icp_price_e8s: u64,
    ) -> bool {
        match types_set {
            Some(set) => {
                if !set.contains(&self.type_filter()) {
                    return false;
                }
            }
            None => {
                if self.is_accrue_interest() {
                    return false;
                }
            }
        }

        // `admin_labels` is an AND filter that narrows only Admin-typed events.
        // No-op when the caller didn't request a specific label set or when
        // this event isn't in the Admin bucket. Admin events with no canonical
        // label (i.e. `admin_label()` returns None) are excluded whenever the
        // caller requested specific labels.
        if let Some(labels) = admin_labels {
            if !labels.is_empty() && self.type_filter() == EventTypeFilter::Admin {
                match self.admin_label() {
                    Some(label) if labels.contains(label) => {}
                    _ => return false,
                }
            }
        }

        if let Some(p) = principal {
            if !self.involves_principal(p) {
                return false;
            }
        }

        if let Some(token) = collateral_token {
            match self.collateral_token(vault_lookup) {
                Some(t) if t == *token => {}
                _ => return false,
            }
        }

        if let Some(range) = time_range {
            match self.timestamp_ns() {
                Some(ts) if ts >= range.start_ns && ts <= range.end_ns => {}
                _ => return false,
            }
        }

        if let Some(min_size) = min_size_e8s {
            if let Some(size) = self.size_e8s_usd(icp_price_e8s) {
                if size < min_size {
                    return false;
                }
            }
        }

        true
    }

    /// Check if a given principal is involved in this event (as owner, caller, or liquidator).
    pub fn involves_principal(&self, p: &Principal) -> bool {
        match self {
            Event::OpenVault { vault, .. } => &vault.owner == p,
            Event::BorrowFromVault { caller, .. } => caller.as_ref() == Some(p),
            Event::RepayToVault { caller, .. } => caller.as_ref() == Some(p),
            Event::AddMarginToVault { caller, .. } => caller.as_ref() == Some(p),
            Event::CollateralWithdrawn { caller, .. } => caller.as_ref() == Some(p),
            Event::PartialCollateralWithdrawn { caller, .. } => caller.as_ref() == Some(p),
            Event::WithdrawAndCloseVault { caller, .. } => caller.as_ref() == Some(p),
            Event::VaultWithdrawnAndClosed { caller, .. } => caller == p,
            Event::LiquidateVault { liquidator, .. } => liquidator.as_ref() == Some(p),
            Event::PartialLiquidateVault { liquidator, .. } => liquidator.as_ref() == Some(p),
            Event::RedemptionOnVaults { owner, .. } => owner == p,
            Event::ReserveRedemption { owner, .. } => owner == p,
            Event::ProvideLiquidity { caller, .. } => caller == p,
            Event::WithdrawLiquidity { caller, .. } => caller == p,
            Event::ClaimLiquidityReturns { caller, .. } => caller == p,
            Event::AdminMint { to, .. } => to == p,
            _ => false,
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
                ..
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
                ..
            } => state.close_vault(vault_id),
            Event::LiquidateVault {
                vault_id,
                mode,
                icp_rate,
                ..
            } => { let _ = state.liquidate_vault(vault_id, mode, icp_rate); },
            Event::PartialLiquidateVault {
                vault_id,
                liquidator_payment,
                icp_to_liquidator,
                protocol_fee_collateral,
                three_usd_reserves_e8s,
                ..
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
                    // Use saturating_sub during replay: interest drift can inflate
                    // vault debts, making the payment exceed the (drifted) balance.
                    // This is safe because the replay path is only used once (first
                    // upgrade); subsequent upgrades restore from stable memory.
                    vault.borrowed_icusd_amount = vault.borrowed_icusd_amount.saturating_sub(liquidator_payment);
                    // Vault loses icp_to_liquidator + protocol_fee_collateral
                    // (old events have protocol_fee_collateral=None → 0, which is correct)
                    let total_collateral_seized = icp_to_liquidator.to_u64()
                        + protocol_fee_collateral.unwrap_or(0);
                    vault.collateral_amount = vault.collateral_amount.saturating_sub(total_collateral_seized);
                    vault.accrued_interest = vault.accrued_interest.saturating_sub(interest_share);
                }
                // Track 3USD reserves from stability pool liquidations
                if let Some(reserves_e8s) = three_usd_reserves_e8s {
                    state.protocol_3usd_reserves += reserves_e8s;
                }
            },
            Event::RedistributeVault { vault_id, .. } => state.redistribute_vault(vault_id),
            Event::BorrowFromVault {
                vault_id,
                borrowed_amount,
                ..
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
                ..
            } => {
                state.provide_liquidity(fee_amount, state.developer_principal);
                let redeem_ct = state.icp_collateral_type();
                state.redeem_on_vaults(icusd_amount, current_icp_rate, &redeem_ct);
                let margin: ICP = icusd_amount / current_icp_rate;
                let nonce = state.next_op_nonce();
                state
                    .pending_redemption_transfer
                    .insert(icusd_block_index, PendingMarginTransfer { owner, margin, collateral_type: crate::vault::default_collateral_type(), retry_count: 0, op_nonce: nonce });
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
                // Cap repayment at current debt to survive replay drift
                let capped = if let Some(vault) = state.vault_id_to_vaults.get(&vault_id) {
                    ICUSD::new(repayed_amount.0.min(vault.borrowed_icusd_amount.0))
                } else { repayed_amount };
                let _ = state.repay_to_vault(vault_id, capped);
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
                ..
            } => {
                // Cap at vault's actual collateral to survive replay drift
                if let Some(vault) = state.vault_id_to_vaults.get(&vault_id) {
                    let capped = ICP::new(amount.to_u64().min(vault.collateral_amount));
                    state.remove_margin_from_vault(vault_id, capped);
                }
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
                ..
            } => {
                // Close the vault during replay
                state.close_vault(vault_id);
            },
            Event::DustForgiven { .. } => {
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
            Event::SetLiquidationBotPrincipal { principal } => {
                state.liquidation_bot_principal = Some(principal);
            },
            Event::SetBotBudget { total_e8s, start_timestamp } => {
                state.bot_budget_total_e8s = total_e8s;
                state.bot_budget_remaining_e8s = total_e8s;
                state.bot_budget_start_timestamp = start_timestamp;
            },
            Event::SetBotAllowedCollateralTypes { collateral_types } => {
                state.bot_allowed_collateral_types = collateral_types.iter().copied().collect();
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
            Event::SetIcpswapRoutingEnabled { enabled } => {
                state.icpswap_routing_enabled = enabled;
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
            Event::PriceUpdate { .. } => {
                // Price history only; no state mutation needed during replay.
            },
            Event::SetCollateralLiquidationRatio { collateral_type, liquidation_ratio } => {
                if let Some(config) = state.collateral_configs.get_mut(&collateral_type) {
                    if let Ok(dec) = liquidation_ratio.parse::<Decimal>() {
                        config.liquidation_ratio = Ratio::from(dec);
                    }
                }
            },
            Event::SetCollateralBorrowThreshold { collateral_type, borrow_threshold_ratio } => {
                if let Ok(dec) = borrow_threshold_ratio.parse::<Decimal>() {
                    let new_ratio = Ratio::from(dec);
                    // Snapshot the global multiplier before taking a mutable borrow of configs
                    // so the replay path mirrors record_set_collateral_borrow_threshold exactly.
                    let multiplier = state.recovery_cr_multiplier;
                    if let Some(config) = state.collateral_configs.get_mut(&collateral_type) {
                        config.borrow_threshold_ratio = new_ratio;
                        config.recovery_target_cr = new_ratio * multiplier;
                    }
                }
            },
            Event::SetCollateralLiquidationBonus { collateral_type, liquidation_bonus } => {
                if let Some(config) = state.collateral_configs.get_mut(&collateral_type) {
                    if let Ok(dec) = liquidation_bonus.parse::<Decimal>() {
                        config.liquidation_bonus = Ratio::from(dec);
                    }
                }
            },
            Event::SetCollateralMinVaultDebt { collateral_type, min_vault_debt } => {
                if let Some(config) = state.collateral_configs.get_mut(&collateral_type) {
                    config.min_vault_debt = ICUSD::new(min_vault_debt);
                }
            },
            Event::SetCollateralLedgerFee { collateral_type, ledger_fee } => {
                if let Some(config) = state.collateral_configs.get_mut(&collateral_type) {
                    config.ledger_fee = ledger_fee;
                }
            },
            Event::SetCollateralRedemptionFeeFloor { collateral_type, redemption_fee_floor } => {
                if let Some(config) = state.collateral_configs.get_mut(&collateral_type) {
                    if let Ok(dec) = redemption_fee_floor.parse::<Decimal>() {
                        config.redemption_fee_floor = Ratio::from(dec);
                    }
                }
            },
            Event::SetCollateralRedemptionFeeCeiling { collateral_type, redemption_fee_ceiling } => {
                if let Some(config) = state.collateral_configs.get_mut(&collateral_type) {
                    if let Ok(dec) = redemption_fee_ceiling.parse::<Decimal>() {
                        config.redemption_fee_ceiling = Ratio::from(dec);
                    }
                }
            },
            Event::SetCollateralMinDeposit { collateral_type, min_collateral_deposit } => {
                if let Some(config) = state.collateral_configs.get_mut(&collateral_type) {
                    config.min_collateral_deposit = min_collateral_deposit;
                }
            },
            Event::SetCollateralDisplayColor { collateral_type, display_color } => {
                if let Some(config) = state.collateral_configs.get_mut(&collateral_type) {
                    config.display_color = display_color;
                }
            },
            Event::AdminDebtCorrection { vault_id: vid, new_borrowed, new_accrued, .. } => {
                if let Some(vault) = state.vault_id_to_vaults.get_mut(&vid) {
                    vault.borrowed_icusd_amount = ICUSD::new(new_borrowed);
                    vault.accrued_interest = ICUSD::new(new_accrued);
                }
            },
        }
    }
    state.next_available_vault_id = vault_id;
    Ok(state)
}

/// Helper: current canister time in nanoseconds.
fn now() -> u64 { ic_cdk::api::time() }

pub fn record_liquidate_vault(state: &mut State, vault_id: u64, mode: Mode, collateral_price: UsdIcp) {
    record_event(&Event::LiquidateVault {
        vault_id,
        mode,
        icp_rate: collateral_price,
        liquidator: None,
        timestamp: Some(now()),
    });
    let _ = state.liquidate_vault(vault_id, mode, collateral_price);
}

pub fn record_redistribute_vault(state: &mut State, vault_id: u64) {
    record_event(&Event::RedistributeVault { vault_id, timestamp: Some(now()) });
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
        timestamp: Some(now()),
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
        timestamp: Some(now()),
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
        timestamp: Some(now()),
    });
    state.claim_liquidity_returns(amount, caller);
}

pub fn record_open_vault(state: &mut State, vault: Vault, block_index: u64) {
    record_event(&Event::OpenVault {
        vault: vault.clone(),
        block_index,
        timestamp: Some(now()),
    });
    state.open_vault(vault);
}

pub fn record_close_vault(state: &mut State, vault_id: u64, block_index: Option<u64>) {
    record_event(&Event::CloseVault {
        vault_id,
        block_index,
        timestamp: Some(now()),
    });
    state.close_vault(vault_id);
}

pub fn record_margin_transfer(state: &mut State, vault_id: u64, block_index: u64) {
    record_event(&Event::MarginTransfer {
        vault_id,
        block_index,
        timestamp: Some(now()),
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
        caller: Some(ic_cdk::caller()),
        timestamp: Some(now()),
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
        caller: Some(ic_cdk::caller()),
        timestamp: Some(now()),
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
        caller: Some(ic_cdk::caller()),
        timestamp: Some(now()),
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
    // Fee is already deducted from icusd_amount before calling redeem_on_vaults,
    // so vault owners effectively keep the fee (less collateral seized for their debt).
    // The fee portion of icUSD stays in the protocol canister (burned).

    // Pick the best collateral type based on redemption tier priority.
    // Tier 1 (most exposed) is redeemed first; within a tier, the collateral
    // type whose worst vault has the lowest health score goes first.
    let priority_types = state.get_collateral_types_by_redemption_priority();
    let redeem_ct = priority_types.first()
        .copied()
        .unwrap_or_else(|| state.icp_collateral_type()); // fallback to ICP

    // Use the selected collateral type's price for both water-filling and
    // pending transfer amount calculation. The caller's collateral_price
    // parameter may be for a different collateral type.
    let ct_price = state.get_collateral_config(&redeem_ct)
        .and_then(|c| c.last_price)
        .and_then(rust_decimal::Decimal::from_f64_retain)
        .map(UsdIcp::from)
        .unwrap_or(collateral_price); // fallback to parameter if no config price

    let vault_redemptions = state.redeem_on_vaults(icusd_amount, ct_price, &redeem_ct);
    record_event(&Event::RedemptionOnVaults {
        owner,
        current_icp_rate: ct_price,
        icusd_amount,
        fee_amount,
        icusd_block_index,
        collateral_type: Some(redeem_ct),
        timestamp: Some(now()),
        vault_redemptions: if vault_redemptions.is_empty() { None } else { Some(vault_redemptions) },
    });
    let margin: ICP = icusd_amount / ct_price;
    let op_nonce = state.next_op_nonce();
    state
        .pending_redemption_transfer
        .insert(icusd_block_index, PendingMarginTransfer { owner, margin, collateral_type: redeem_ct, retry_count: 0, op_nonce });
}

pub fn record_redemption_transfered(
    state: &mut State,
    icusd_block_index: u64,
    icp_block_index: u64,
) {
    record_event(&Event::RedemptionTransfered {
        icusd_block_index,
        icp_block_index,
        timestamp: Some(now()),
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
        caller: Some(ic_cdk::caller()),
        timestamp: Some(now()),
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
        caller: Some(ic_cdk::caller()),
        timestamp: Some(now()),
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
        caller: Some(ic_cdk::caller()),
        timestamp: Some(now()),
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

pub fn record_set_liquidation_bot_principal(state: &mut State, principal: Principal) {
    record_event(&Event::SetLiquidationBotPrincipal { principal });
    state.liquidation_bot_principal = Some(principal);
}

pub fn record_set_bot_budget(state: &mut State, total_e8s: u64, start_timestamp: u64) {
    record_event(&Event::SetBotBudget { total_e8s, start_timestamp });
    state.bot_budget_total_e8s = total_e8s;
    state.bot_budget_remaining_e8s = total_e8s;
    state.bot_budget_start_timestamp = start_timestamp;
}

pub fn record_set_bot_allowed_collateral_types(state: &mut State, collateral_types: Vec<Principal>) {
    record_event(&Event::SetBotAllowedCollateralTypes { collateral_types: collateral_types.clone() });
    state.bot_allowed_collateral_types = collateral_types.into_iter().collect();
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

pub fn record_set_icpswap_routing_enabled(state: &mut State, enabled: bool) {
    record_event(&Event::SetIcpswapRoutingEnabled { enabled });
    state.icpswap_routing_enabled = enabled;
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
        timestamp: Some(now()),
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
        timestamp: Some(now()),
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

pub fn record_set_collateral_liquidation_ratio(
    state: &mut State,
    collateral_type: CollateralType,
    liquidation_ratio: Ratio,
) {
    record_event(&Event::SetCollateralLiquidationRatio {
        collateral_type,
        liquidation_ratio: liquidation_ratio.0.to_string(),
    });
    if let Some(config) = state.collateral_configs.get_mut(&collateral_type) {
        config.liquidation_ratio = liquidation_ratio;
    }
}

pub fn record_set_collateral_borrow_threshold(
    state: &mut State,
    collateral_type: CollateralType,
    borrow_threshold_ratio: Ratio,
) {
    record_event(&Event::SetCollateralBorrowThreshold {
        collateral_type,
        borrow_threshold_ratio: borrow_threshold_ratio.0.to_string(),
    });
    if let Some(config) = state.collateral_configs.get_mut(&collateral_type) {
        config.borrow_threshold_ratio = borrow_threshold_ratio;
        // Keep the stored (but largely derived) recovery_target_cr in sync.
        config.recovery_target_cr = borrow_threshold_ratio * state.recovery_cr_multiplier;
    }
}

pub fn record_set_collateral_liquidation_bonus(
    state: &mut State,
    collateral_type: CollateralType,
    liquidation_bonus: Ratio,
) {
    record_event(&Event::SetCollateralLiquidationBonus {
        collateral_type,
        liquidation_bonus: liquidation_bonus.0.to_string(),
    });
    if let Some(config) = state.collateral_configs.get_mut(&collateral_type) {
        config.liquidation_bonus = liquidation_bonus;
    }
}

pub fn record_set_collateral_min_vault_debt(
    state: &mut State,
    collateral_type: CollateralType,
    min_vault_debt: u64,
) {
    record_event(&Event::SetCollateralMinVaultDebt {
        collateral_type,
        min_vault_debt,
    });
    if let Some(config) = state.collateral_configs.get_mut(&collateral_type) {
        config.min_vault_debt = ICUSD::new(min_vault_debt);
    }
}

pub fn record_set_collateral_ledger_fee(
    state: &mut State,
    collateral_type: CollateralType,
    ledger_fee: u64,
) {
    record_event(&Event::SetCollateralLedgerFee {
        collateral_type,
        ledger_fee,
    });
    if let Some(config) = state.collateral_configs.get_mut(&collateral_type) {
        config.ledger_fee = ledger_fee;
    }
}

pub fn record_set_collateral_redemption_fee_floor(
    state: &mut State,
    collateral_type: CollateralType,
    redemption_fee_floor: Ratio,
) {
    record_event(&Event::SetCollateralRedemptionFeeFloor {
        collateral_type,
        redemption_fee_floor: redemption_fee_floor.0.to_string(),
    });
    if let Some(config) = state.collateral_configs.get_mut(&collateral_type) {
        config.redemption_fee_floor = redemption_fee_floor;
    }
}

pub fn record_set_collateral_redemption_fee_ceiling(
    state: &mut State,
    collateral_type: CollateralType,
    redemption_fee_ceiling: Ratio,
) {
    record_event(&Event::SetCollateralRedemptionFeeCeiling {
        collateral_type,
        redemption_fee_ceiling: redemption_fee_ceiling.0.to_string(),
    });
    if let Some(config) = state.collateral_configs.get_mut(&collateral_type) {
        config.redemption_fee_ceiling = redemption_fee_ceiling;
    }
}

pub fn record_set_collateral_min_deposit(
    state: &mut State,
    collateral_type: CollateralType,
    min_collateral_deposit: u64,
) {
    record_event(&Event::SetCollateralMinDeposit {
        collateral_type,
        min_collateral_deposit,
    });
    if let Some(config) = state.collateral_configs.get_mut(&collateral_type) {
        config.min_collateral_deposit = min_collateral_deposit;
    }
}

pub fn record_set_collateral_display_color(
    state: &mut State,
    collateral_type: CollateralType,
    display_color: Option<String>,
) {
    record_event(&Event::SetCollateralDisplayColor {
        collateral_type,
        display_color: display_color.clone(),
    });
    if let Some(config) = state.collateral_configs.get_mut(&collateral_type) {
        config.display_color = display_color;
    }
}

pub fn record_accrue_interest(state: &mut State, now_nanos: u64) {
    record_event(&Event::AccrueInterest { timestamp: now_nanos });
    state.accrue_all_vault_interest(now_nanos);
}

pub fn record_price_update(collateral_type: CollateralType, price: Decimal, timestamp: u64) {
    record_event(&Event::PriceUpdate {
        collateral_type,
        price: price.to_string(),
        timestamp,
    });
}

#[cfg(test)]
mod filter_tests {
    use super::*;
    use crate::vault::Vault;

    fn p(seed: u8) -> Principal {
        Principal::self_authenticating([seed; 32])
    }

    fn caller_a() -> Principal { p(1) }
    fn caller_b() -> Principal { p(2) }
    fn icp_token() -> Principal { p(10) }
    fn ckbtc_token() -> Principal { p(11) }

    fn vault_with(id: u64, owner: Principal, ct: Principal, collateral_e8s: u64) -> Vault {
        Vault {
            owner,
            vault_id: id,
            collateral_type: ct,
            collateral_amount: collateral_e8s,
            borrowed_icusd_amount: ICUSD::new(0),
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        }
    }

    fn open_vault_event(id: u64, owner: Principal, ct: Principal) -> Event {
        Event::OpenVault {
            vault: vault_with(id, owner, ct, 1_000_000_000),
            block_index: 0,
            timestamp: Some(1_000),
        }
    }

    fn borrow_event(vault_id: u64, caller: Principal, amount_e8s: u64, ts: u64) -> Event {
        Event::BorrowFromVault {
            vault_id,
            borrowed_amount: ICUSD::new(amount_e8s),
            fee_amount: ICUSD::new(0),
            block_index: 0,
            caller: Some(caller),
            timestamp: Some(ts),
        }
    }

    fn repay_event(vault_id: u64, caller: Principal, amount_e8s: u64, ts: u64) -> Event {
        Event::RepayToVault {
            vault_id,
            repayed_amount: ICUSD::new(amount_e8s),
            block_index: 0,
            caller: Some(caller),
            timestamp: Some(ts),
        }
    }

    fn liquidate_event(vault_id: u64, liquidator: Principal, ts: u64) -> Event {
        Event::LiquidateVault {
            vault_id,
            mode: Mode::GeneralAvailability,
            icp_rate: UsdIcp::new(Decimal::from(5u32)),
            liquidator: Some(liquidator),
            timestamp: Some(ts),
        }
    }

    fn accrue_event(ts: u64) -> Event {
        Event::AccrueInterest { timestamp: ts }
    }

    fn price_event(ct: Principal, ts: u64) -> Event {
        Event::PriceUpdate { collateral_type: ct, price: "5.0".into(), timestamp: ts }
    }

    /// Build a vault_id → collateral_type lookup for tests.
    fn lookup(entries: &[(u64, Principal)]) -> HashMap<u64, Principal> {
        entries.iter().copied().collect()
    }

    // ── type_filter classification ────────────────────────────────────────

    #[test]
    fn type_filter_classifies_each_user_facing_variant() {
        assert_eq!(open_vault_event(1, caller_a(), icp_token()).type_filter(), EventTypeFilter::OpenVault);
        assert_eq!(borrow_event(1, caller_a(), 100, 0).type_filter(), EventTypeFilter::Borrow);
        assert_eq!(repay_event(1, caller_a(), 100, 0).type_filter(), EventTypeFilter::Repay);
        assert_eq!(liquidate_event(1, caller_a(), 0).type_filter(), EventTypeFilter::Liquidation);
        assert_eq!(accrue_event(0).type_filter(), EventTypeFilter::AccrueInterest);
        assert_eq!(price_event(icp_token(), 0).type_filter(), EventTypeFilter::PriceUpdate);

        // Setter falls into Admin
        let setter = Event::SetBorrowingFee { rate: "0.005".into() };
        assert_eq!(setter.type_filter(), EventTypeFilter::Admin);
    }

    // ── timestamp_ns ──────────────────────────────────────────────────────

    #[test]
    fn timestamp_ns_extracts_when_present_and_returns_none_otherwise() {
        assert_eq!(borrow_event(1, caller_a(), 100, 12_345).timestamp_ns(), Some(12_345));
        assert_eq!(accrue_event(7).timestamp_ns(), Some(7));

        // Init has no timestamp.
        let init = Event::Init(InitArg {
            xrc_principal: p(0),
            icusd_ledger_principal: p(0),
            icp_ledger_principal: p(0),
            fee_e8s: 0,
            developer_principal: p(0),
            treasury_principal: None,
            stability_pool_principal: None,
            ckusdt_ledger_principal: None,
            ckusdc_ledger_principal: None,
        });
        assert_eq!(init.timestamp_ns(), None);
    }

    // ── collateral_token + vault lookup ───────────────────────────────────

    #[test]
    fn collateral_token_falls_back_to_vault_lookup_for_id_only_events() {
        let lookup = lookup(&[(42, ckbtc_token())]);
        let ev = borrow_event(42, caller_a(), 100, 0);
        assert_eq!(ev.collateral_token(&lookup), Some(ckbtc_token()));

        // Unknown vault id → None
        let ev2 = borrow_event(99, caller_a(), 100, 0);
        assert_eq!(ev2.collateral_token(&HashMap::new()), None);
    }

    #[test]
    fn collateral_token_uses_event_field_for_open_vault() {
        let ev = open_vault_event(1, caller_a(), ckbtc_token());
        assert_eq!(ev.collateral_token(&HashMap::new()), Some(ckbtc_token()));
    }

    // ── size_e8s_usd conversions ──────────────────────────────────────────

    #[test]
    fn size_in_usd_passes_through_icusd_amounts() {
        let ev = borrow_event(1, caller_a(), 250_000_000, 0); // 2.50 icUSD
        assert_eq!(ev.size_e8s_usd(0), Some(250_000_000));
    }

    #[test]
    fn size_in_usd_converts_icp_amounts_at_spot_price() {
        // 1 ICP @ $5 = $5 = 500_000_000 e8s
        let icp_e8s = 100_000_000u64;
        let price_e8s = 500_000_000u64;
        let ev = open_vault_event(1, caller_a(), icp_token());
        let ev = if let Event::OpenVault { mut vault, block_index, timestamp } = ev {
            vault.collateral_amount = icp_e8s;
            Event::OpenVault { vault, block_index, timestamp }
        } else { unreachable!() };
        assert_eq!(ev.size_e8s_usd(price_e8s), Some(500_000_000));
    }

    #[test]
    fn size_returns_none_for_admin_setters() {
        let ev = Event::SetBorrowingFee { rate: "0.005".into() };
        assert_eq!(ev.size_e8s_usd(500_000_000), None);
    }

    // ── passes_filters: each dimension in isolation ───────────────────────

    #[test]
    fn empty_filter_excludes_accrue_interest_and_price_update() {
        let lookup = HashMap::new();
        assert!(!accrue_event(0).passes_filters(None, None, None, None, None, None, &lookup, 0));
        assert!(!price_event(icp_token(), 0).passes_filters(None, None, None, None, None, None, &lookup, 0));
        assert!(borrow_event(1, caller_a(), 100, 0).passes_filters(None, None, None, None, None, None, &lookup, 0));
    }

    #[test]
    fn explicit_type_set_includes_accrue_interest_when_requested() {
        let lookup = HashMap::new();
        let set: HashSet<_> = [EventTypeFilter::AccrueInterest].into_iter().collect();
        assert!(accrue_event(0).passes_filters(Some(&set), None, None, None, None, None, &lookup, 0));
        assert!(!borrow_event(1, caller_a(), 100, 0).passes_filters(Some(&set), None, None, None, None, None, &lookup, 0));
    }

    #[test]
    fn type_filter_or_combines_within_the_set() {
        let lookup = HashMap::new();
        let set: HashSet<_> = [EventTypeFilter::Borrow, EventTypeFilter::Repay].into_iter().collect();
        assert!(borrow_event(1, caller_a(), 100, 0).passes_filters(Some(&set), None, None, None, None, None, &lookup, 0));
        assert!(repay_event(1, caller_a(), 100, 0).passes_filters(Some(&set), None, None, None, None, None, &lookup, 0));
        assert!(!liquidate_event(1, caller_a(), 0).passes_filters(Some(&set), None, None, None, None, None, &lookup, 0));
    }

    #[test]
    fn principal_filter_matches_caller_or_owner() {
        let lookup = HashMap::new();
        assert!(borrow_event(1, caller_a(), 100, 0).passes_filters(None, Some(&caller_a()), None, None, None, None, &lookup, 0));
        assert!(!borrow_event(1, caller_a(), 100, 0).passes_filters(None, Some(&caller_b()), None, None, None, None, &lookup, 0));
    }

    #[test]
    fn collateral_token_filter_matches_via_vault_lookup() {
        let lookup = lookup(&[(1, ckbtc_token())]);
        let ev = borrow_event(1, caller_a(), 100, 0);
        assert!(ev.passes_filters(None, None, Some(&ckbtc_token()), None, None, None, &lookup, 0));
        assert!(!ev.passes_filters(None, None, Some(&icp_token()), None, None, None, &lookup, 0));
    }

    #[test]
    fn time_range_excludes_outside_window_and_no_timestamp_events() {
        let lookup = HashMap::new();
        let range = EventTimeRange { start_ns: 1_000, end_ns: 2_000 };

        let inside = borrow_event(1, caller_a(), 100, 1_500);
        let outside = borrow_event(1, caller_a(), 100, 5_000);
        assert!(inside.passes_filters(None, None, None, Some(&range), None, None, &lookup, 0));
        assert!(!outside.passes_filters(None, None, None, Some(&range), None, None, &lookup, 0));

        let init = Event::Init(InitArg {
            xrc_principal: p(0),
            icusd_ledger_principal: p(0),
            icp_ledger_principal: p(0),
            fee_e8s: 0,
            developer_principal: p(0),
            treasury_principal: None,
            stability_pool_principal: None,
            ckusdt_ledger_principal: None,
            ckusdc_ledger_principal: None,
        });
        // Init has no timestamp_ns → excluded by an active time_range.
        assert!(!init.passes_filters(None, None, None, Some(&range), None, None, &lookup, 0));
    }

    #[test]
    fn min_size_excludes_below_threshold_and_passes_unsized_events() {
        let lookup = HashMap::new();
        // Borrow $0.50 — under $1.00 threshold.
        let small = borrow_event(1, caller_a(), 50_000_000, 0);
        let big = borrow_event(1, caller_a(), 500_000_000, 0);
        let threshold = 100_000_000u64; // $1.00 in e8s

        assert!(!small.passes_filters(None, None, None, None, Some(threshold), None, &lookup, 0));
        assert!(big.passes_filters(None, None, None, None, Some(threshold), None, &lookup, 0));

        // Admin setter has no size — passes through any threshold.
        let setter = Event::SetBorrowingFee { rate: "0.005".into() };
        assert!(setter.passes_filters(None, None, None, None, Some(u64::MAX), None, &lookup, 0));
    }

    // ── two-filter AND combinations ───────────────────────────────────────

    #[test]
    fn type_and_principal_combine_with_and_semantics() {
        let lookup = HashMap::new();
        let types: HashSet<_> = [EventTypeFilter::Borrow].into_iter().collect();

        // Right type AND right principal → match
        assert!(borrow_event(1, caller_a(), 100, 0).passes_filters(
            Some(&types), Some(&caller_a()), None, None, None, None, &lookup, 0,
        ));
        // Right type, wrong principal → reject
        assert!(!borrow_event(1, caller_a(), 100, 0).passes_filters(
            Some(&types), Some(&caller_b()), None, None, None, None, &lookup, 0,
        ));
        // Wrong type, right principal → reject
        assert!(!repay_event(1, caller_a(), 100, 0).passes_filters(
            Some(&types), Some(&caller_a()), None, None, None, None, &lookup, 0,
        ));
    }

    #[test]
    fn time_and_token_combine_with_and_semantics() {
        let lookup = lookup(&[(1, ckbtc_token())]);
        let range = EventTimeRange { start_ns: 1_000, end_ns: 2_000 };

        // In-window, right token → match
        assert!(borrow_event(1, caller_a(), 100, 1_500).passes_filters(
            None, None, Some(&ckbtc_token()), Some(&range), None, None, &lookup, 0,
        ));
        // Out-of-window, right token → reject
        assert!(!borrow_event(1, caller_a(), 100, 9_999).passes_filters(
            None, None, Some(&ckbtc_token()), Some(&range), None, None, &lookup, 0,
        ));
        // In-window, wrong token → reject
        assert!(!borrow_event(1, caller_a(), 100, 1_500).passes_filters(
            None, None, Some(&icp_token()), Some(&range), None, None, &lookup, 0,
        ));
    }

    // ── admin_label + admin_labels filter ─────────────────────────────────

    #[test]
    fn admin_label_returns_variant_name_for_admin_variants() {
        let borrow_fee = Event::SetBorrowingFee { rate: "0.005".into() };
        assert_eq!(borrow_fee.admin_label(), Some("SetBorrowingFee"));
        let healthy = Event::SetHealthyCr {
            collateral_type: "ICP".to_string(),
            healthy_cr: Some("1.2".to_string()),
        };
        assert_eq!(healthy.admin_label(), Some("SetHealthyCr"));
    }

    #[test]
    fn admin_label_returns_none_for_non_admin_variants() {
        assert_eq!(borrow_event(1, caller_a(), 100, 0).admin_label(), None);
        assert_eq!(liquidate_event(1, caller_a(), 0).admin_label(), None);
        assert_eq!(accrue_event(0).admin_label(), None);
        assert_eq!(price_event(icp_token(), 0).admin_label(), None);
    }

    #[test]
    fn admin_labels_narrows_admin_type_matches() {
        let lookup = HashMap::new();
        let types: HashSet<_> = [EventTypeFilter::Admin].into_iter().collect();
        let labels: HashSet<String> = ["SetBorrowingFee".to_string()].into_iter().collect();

        let borrow_fee = Event::SetBorrowingFee { rate: "0.005".into() };
        let healthy = Event::SetHealthyCr {
            collateral_type: "ICP".to_string(),
            healthy_cr: Some("1.2".to_string()),
        };

        assert!(borrow_fee.passes_filters(
            Some(&types), None, None, None, None, Some(&labels), &lookup, 0,
        ));
        assert!(!healthy.passes_filters(
            Some(&types), None, None, None, None, Some(&labels), &lookup, 0,
        ));
    }

    #[test]
    fn admin_labels_is_noop_without_admin_in_types() {
        let lookup = HashMap::new();
        // types filter requests Borrow, not Admin — so admin_labels should
        // have no effect and the borrow event should still pass.
        let types: HashSet<_> = [EventTypeFilter::Borrow].into_iter().collect();
        let labels: HashSet<String> = ["SetBorrowingFee".to_string()].into_iter().collect();

        assert!(borrow_event(1, caller_a(), 100, 0).passes_filters(
            Some(&types), None, None, None, None, Some(&labels), &lookup, 0,
        ));

        // An admin event is excluded because the types filter doesn't include
        // Admin. admin_labels doesn't re-enable it.
        let setter = Event::SetBorrowingFee { rate: "0.005".into() };
        assert!(!setter.passes_filters(
            Some(&types), None, None, None, None, Some(&labels), &lookup, 0,
        ));
    }

    #[test]
    fn admin_labels_with_no_types_narrows_admin_events_only() {
        // When types is None, non-admin events pass via the default filter
        // (which hides only accrue/price). admin_labels narrows admin events
        // to those whose label is in the set; non-admin events are unaffected.
        let lookup = HashMap::new();
        let labels: HashSet<String> = ["SetBorrowingFee".to_string()].into_iter().collect();

        let matching_admin = Event::SetBorrowingFee { rate: "0.005".into() };
        let non_matching_admin = Event::SetHealthyCr {
            collateral_type: "ICP".to_string(),
            healthy_cr: Some("1.2".to_string()),
        };
        let non_admin = borrow_event(1, caller_a(), 100, 0);

        assert!(matching_admin.passes_filters(
            None, None, None, None, None, Some(&labels), &lookup, 0,
        ));
        assert!(!non_matching_admin.passes_filters(
            None, None, None, None, None, Some(&labels), &lookup, 0,
        ));
        assert!(non_admin.passes_filters(
            None, None, None, None, None, Some(&labels), &lookup, 0,
        ));
    }

    #[test]
    fn admin_labels_empty_set_behaves_like_none() {
        // An empty admin_labels set should be ignored (same semantics as None).
        let lookup = HashMap::new();
        let types: HashSet<_> = [EventTypeFilter::Admin].into_iter().collect();
        let empty: HashSet<String> = HashSet::new();

        let setter = Event::SetBorrowingFee { rate: "0.005".into() };
        assert!(setter.passes_filters(
            Some(&types), None, None, None, None, Some(&empty), &lookup, 0,
        ));
    }
}
