use crate::numeric::{Ratio, UsdIcp, ICP, ICUSD};
use crate::state::{
    CollateralConfig, CollateralStatus, CollateralType, PendingMarginTransfer, PendingPayoutKind,
    PendingPayoutNoEffectProof, RateCurveV2, State, PendingAmm1Donation,
    HeldAmm1Donation,
};
use crate::storage::{record_event, record_event_at};
use crate::vault::Vault;
use crate::{EventTimeRange, EventTypeFilter, InitArg, Mode, StableTokenType, UpgradeArg};
use candid::{CandidType, Principal};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
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

/// Wave-8e LIQ-005: identifies which fee revenue stream a deficit
/// repayment was sourced from. Persisted in the `DeficitRepaid` event so
/// the explorer can attribute repayment volume per source.
#[derive(CandidType, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeeSource {
    BorrowingFee,
    RedemptionFee,
}

/// Wave-9 RED-002: identifies which path accrued a shortfall to
/// `protocol_deficit_icusd`. Persisted on the `DeficitAccrued` event so
/// the explorer can attribute deficit growth between liquidation and
/// redemption flows. Pre-Wave-9 events serialize without this field
/// and decode with `source = None` via `serde(default)`; new events
/// always populate it.
///
/// Liquidation deficits also retain `vault_id` on the parent event for
/// back-compat with the existing event-log shape; for redemption,
/// `vault_id` on the parent is set to 0 (the cr-walk touches multiple
/// vaults, no single id applies) and the redeemer principal lives
/// inside this enum.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum DeficitSource {
    Liquidation { vault_id: u64 },
    Redemption { redeemer: Principal },
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
        operation_id: Option<u128>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        payout_kind: Option<PendingPayoutKind>,
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
        /// LIQ-0XX (review finding 2): the icUSD debt actually repaid by this
        /// liquidation, pinned pre-await by the caller (see
        /// `State::liquidate_vault`'s `repay_amount` parameter). `None` for
        /// every event recorded before this field existed — replay of those
        /// legacy events reproduces the EXACT pre-fix decision instead of
        /// recomputing through `effective_liquidation_amount`, which can
        /// return a different (smaller, capped) amount under the new unified
        /// partial-liquidation rule and would otherwise leave a replayed
        /// vault open when it was actually closed in full on-chain. Always
        /// `Some` for events recorded after this field was added, including
        /// the GA-mode full-debt case (an explicit pin, not an inferred one).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repay_amount: Option<ICUSD>,
    },

    #[serde(rename = "partial_liquidate_vault")]
    PartialLiquidateVault {
        vault_id: u64,
        #[serde(alias = "liquidated_debt")]
        liquidator_payment: ICUSD,
        /// Net collateral received by the liquidator. Bot claim events record
        /// the outbound ledger fee separately below; legacy/manual events keep
        /// their historical meaning.
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
        /// Explicit ledger transfer fee debited in addition to the net amount
        /// credited to the liquidator. None for legacy and non-bot events.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ledger_fee_collateral: Option<u64>,
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
        /// Exact native-unit payout pinned for new redemption events.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        payout_collateral_raw: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        payout_operation_id: Option<u128>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        payout_attempt_nonce: Option<u128>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        payout_ledger: Option<Principal>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        payout_transfer_amount_raw: Option<u64>,
        /// Minimum net native-unit payout consented to by the redeemer.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_net_collateral_raw: Option<u64>,
    },

    #[serde(rename = "redemption_transfered")]
    RedemptionTransfered {
        icusd_block_index: u64,
        icp_block_index: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        operation_id: Option<u128>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<u64>,
    },

    #[serde(rename = "redistribute_vault")]
    RedistributeVault {
        vault_id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<u64>,
    },

    /// Wave-8e LIQ-005 + Wave-9 RED-002: a redemption or liquidation
    /// netted seized USD < debt cleared, accruing the shortfall to
    /// `protocol_deficit_icusd`. Emitted from every liquidation path
    /// (`liquidate_vault`, `liquidate_vault_partial`,
    /// `liquidate_vault_partial_with_stable`, `partial_liquidate_vault`,
    /// `liquidate_vault_debt_already_burned`) when shortfall > 0, and
    /// from `record_redemption_on_vaults` when redeemer claim exceeds
    /// vault collateral at oracle price.
    ///
    /// `vault_id` is the originating vault for liquidation and 0 for
    /// redemption (the cr-walk touches multiple vaults). The new
    /// `source` field is the canonical attribution: pre-Wave-9 events
    /// decode with `source = None` (all pre-Wave-9 deficit rows were
    /// liquidation by definition); post-Wave-9 events always populate
    /// it.
    #[serde(rename = "deficit_accrued")]
    DeficitAccrued {
        vault_id: u64,
        amount: ICUSD,
        new_deficit: ICUSD,
        timestamp: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<DeficitSource>,
    },

    /// Wave-8e LIQ-005: a fee collection routed `amount` icUSD toward
    /// deficit repayment. For borrowing-fee source this means the protocol
    /// minted `original_fee - amount` to treasury instead of `original_fee`
    /// (foregone revenue). For redemption-fee source the redeemer's icUSD
    /// was already burned via `transfer_icusd_from`, so the deficit
    /// decremented purely as state mutation. `anchor_block_index` is the
    /// icUSD ledger block that generated the fee when available, or `None`
    /// when the deficit decrement happened before the ledger op (caller
    /// can correlate via `op_nonce` in trace logs).
    #[serde(rename = "deficit_repaid")]
    DeficitRepaid {
        amount: ICUSD,
        source: FeeSource,
        remaining_deficit: ICUSD,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        anchor_block_index: Option<u64>,
        timestamp: u64,
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
    SetCkstableRepayFee { rate: String },

    #[serde(rename = "set_min_icusd_amount")]
    SetMinIcusdAmount { amount: String },

    /// LIQ-0XX: admin set the global dust-liquidation threshold. See
    /// `State::dust_liquidation_threshold`.
    #[serde(rename = "set_dust_liquidation_threshold")]
    SetDustLiquidationThreshold { amount: String },

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
    SetTreasuryPrincipal { principal: Principal },

    #[serde(rename = "set_stability_pool_principal")]
    SetStabilityPoolPrincipal { principal: Principal },

    #[serde(rename = "set_liquidation_bot_principal")]
    SetLiquidationBotPrincipal { principal: Principal },

    #[serde(rename = "set_bot_budget")]
    SetBotBudget {
        total_e8s: u64,
        start_timestamp: u64,
    },

    #[serde(rename = "set_bot_allowed_collateral_types")]
    SetBotAllowedCollateralTypes { collateral_types: Vec<Principal> },

    #[serde(rename = "set_bot_cr_tolerance_bps")]
    SetBotCrToleranceBps { bps: u64 },

    /// Wave-14a CDP-14 follow-up: per-collateral override for the XRC
    /// source-count floor (None = inherit global). Emitted when an admin
    /// tunes the per-asset floor (typically used to lower the gate for
    /// collaterals like XAUT whose underlying asset has genuinely thin
    /// CEX coverage on XRC and can never aggregate 3 sources).
    #[serde(rename = "set_collateral_min_xrc_sources")]
    SetCollateralMinXrcSources {
        collateral_type: Principal,
        min_xrc_sources: Option<u32>,
    },

    #[serde(rename = "set_liquidation_bonus")]
    SetLiquidationBonus { rate: String },

    #[serde(rename = "set_borrowing_fee")]
    SetBorrowingFee { rate: String },

    #[serde(rename = "set_redemption_fee_floor")]
    SetRedemptionFeeFloor { rate: String },

    #[serde(rename = "set_redemption_fee_ceiling")]
    SetRedemptionFeeCeiling { rate: String },

    #[serde(rename = "set_max_partial_liquidation_ratio")]
    SetMaxPartialLiquidationRatio { rate: String },

    #[serde(rename = "set_recovery_target_cr")]
    SetRecoveryTargetCr { rate: String },

    #[serde(
        rename = "set_recovery_cr_multiplier",
        alias = "set_recovery_liquidation_buffer"
    )]
    SetRecoveryCrMultiplier {
        #[serde(alias = "buffer")]
        multiplier: String,
    },

    #[serde(rename = "set_liquidation_protocol_share")]
    SetLiquidationProtocolShare { share: String },

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
    SetReserveRedemptionsEnabled { enabled: bool },

    #[serde(rename = "set_icpswap_routing_enabled")]
    SetIcpswapRoutingEnabled { enabled: bool },

    #[serde(rename = "set_reserve_redemption_fee")]
    SetReserveRedemptionFee { fee: String },

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
        collateral_type: Option<String>, // None for global
        markers: String,                 // JSON-serialized marker pairs
    },

    /// Admin set recovery rate curve (system-wide Layer 2)
    #[serde(rename = "set_recovery_rate_curve")]
    SetRecoveryRateCurve {
        markers: String, // JSON-serialized (threshold, multiplier) pairs
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
    AccrueInterest { timestamp: u64 },

    /// Admin set the interest revenue split ratio (stability pool share).
    #[serde(rename = "set_interest_pool_share")]
    SetInterestPoolShare { share: String },

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
    SetBorrowingFeeCurve { markers: String },

    /// Admin set the N-way interest split (replaces interest_pool_share).
    #[serde(rename = "set_interest_split")]
    SetInterestSplit {
        /// JSON-encoded Vec<InterestRecipient>
        split: String,
    },

    /// Admin set the 3pool canister principal for interest donations.
    #[serde(rename = "set_three_pool_canister")]
    SetThreePoolCanister { canister: Principal },

    /// Admin set the AMM1 canister principal for interest donations.
    #[serde(rename = "set_amm1_canister")]
    SetAmm1Canister { canister: Principal },

    /// Admin set the canonical AMM1 pool_id used by `donate_icusd_to_amm1`.
    /// Must match `make_pool_id(token_a, token_b)` on rumi_amm exactly,
    /// otherwise `notify_reward_received` returns PoolNotFound and donations
    /// re-queue indefinitely.
    #[serde(rename = "set_amm1_pool_id")]
    SetAmm1PoolId { pool_id: String },

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

    /// Wave-8e LIQ-005: admin tunes the per-fee fraction routed to deficit
    /// repayment. Default 0.5; bounded [0, 1].
    #[serde(rename = "set_deficit_repayment_fraction")]
    SetDeficitRepaymentFraction { fraction: Ratio, timestamp: u64 },

    /// Wave-8e LIQ-005: admin sets the deficit-driven ReadOnly auto-latch
    /// threshold. 0 disables the latch.
    #[serde(rename = "set_deficit_readonly_threshold_e8s")]
    SetDeficitReadonlyThresholdE8s { threshold_e8s: u64, timestamp: u64 },

    /// Wave-10 LIQ-008: circuit breaker auto-tripped because the rolling-
    /// window cumulative liquidation debt crossed the configured ceiling.
    /// `total_e8s` is the windowed sum at the moment of tripping;
    /// `ceiling_e8s` is the configured trip threshold for audit purposes.
    #[serde(rename = "breaker_tripped")]
    BreakerTripped {
        total_e8s: u64,
        ceiling_e8s: u64,
        timestamp: u64,
    },

    /// Wave-10 LIQ-008: admin manually cleared the breaker latch and
    /// resumed `check_vaults` auto-publishing. `remaining_total_e8s` is the
    /// windowed sum at the moment of clearing (informational; admins inspect
    /// it before deciding to clear).
    #[serde(rename = "breaker_cleared")]
    BreakerCleared {
        remaining_total_e8s: u64,
        timestamp: u64,
    },

    /// Wave-10 LIQ-008: admin tuned the rolling-window length.
    #[serde(rename = "set_breaker_window_ns")]
    SetBreakerWindowNs { window_ns: u64, timestamp: u64 },

    /// Wave-10 LIQ-008: admin tuned the cumulative-debt ceiling. 0 disables
    /// the breaker.
    #[serde(rename = "set_breaker_window_debt_ceiling_e8s")]
    SetBreakerWindowDebtCeilingE8s { ceiling_e8s: u64, timestamp: u64 },

    /// `check_vaults` detected an expired claim without a provable full-gross
    /// return and conserving outbound fee tuple. Auto-cancel is skipped. The
    /// legacy observed/required balance fields are retained; observed_balance
    /// is zero when no exact return proof exists. Re-emitted each tick.
    #[serde(rename = "bot_claim_reconciliation_needed")]
    BotClaimReconciliationNeeded {
        vault_id: u64,
        observed_balance: u64,
        required_balance: u64,
        timestamp: u64,
    },

    /// Wave-14a CDP-10: emitted when the spawned `notify_liquidatable_vaults`
    /// call to the stability_pool returned a transport `Err` (cycle pressure,
    /// queue-full during a market crash, etc.). The dispatched vault ids are
    /// NOT marked `sp_attempted` and remain eligible for retry on the next
    /// `check_vaults` tick. External liquidators can poll `get_events` and
    /// react.
    #[serde(rename = "stability_pool_call_failed")]
    StabilityPoolCallFailed {
        vault_ids: Vec<u64>,
        reject_code: i32,
        reject_message: String,
        timestamp: u64,
    },

    /// Wave-14a CDP-01: emitted on the `check_vaults` tick where the
    /// consecutive-XRC-failure counter reached
    /// `xrc::MAX_CONSECUTIVE_XRC_FAILURES` and the protocol transitioned
    /// from `GeneralAvailability` into `ReadOnly`. Auto-clears on the
    /// next successful XRC fetch (since the trip is marked
    /// `mode_triggered_by_oracle`). Operator-set ReadOnly does not emit
    /// this event.
    #[serde(rename = "oracle_circuit_breaker")]
    OracleCircuitBreaker {
        consecutive_failures: u64,
        timestamp: u64,
    },

    /// Wave-14a CDP-14: emitted when the protocol rejects an XRC sample
    /// because `metadata.num_sources_used` was below `min_required`.
    /// The cached price stays in place. Operators monitor counts of this
    /// event over time as a signal for oracle aggregation health and can
    /// tune `MIN_XRC_SOURCES` via the developer-gated setter.
    #[serde(rename = "oracle_source_count_insufficient")]
    OracleSourceCountInsufficient {
        collateral_type: Principal,
        num_sources: u32,
        min_required: u32,
        timestamp: u64,
    },
    // Phase 1a: chain-admin audit trail.
    #[serde(rename = "chain_registered")]
    ChainRegistered {
        chain_id: crate::chains::config::ChainId,
        display_name: String,
        timestamp: u64,
    },
    #[serde(rename = "chain_disabled")]
    ChainDisabled {
        chain_id: crate::chains::config::ChainId,
        timestamp: u64,
    },
    #[serde(rename = "chain_config_updated")]
    ChainConfigUpdated {
        chain_id: crate::chains::config::ChainId,
        timestamp: u64,
    },
    #[serde(rename = "chain_bad_debt_circuit_threshold_set")]
    ChainBadDebtCircuitThresholdSet {
        chain_id: crate::chains::config::ChainId,
        threshold_e8s: Option<u128>,
        timestamp: u64,
    },
    #[serde(rename = "chain_bad_debt_circuit_tripped")]
    ChainBadDebtCircuitTripped {
        chain_id: crate::chains::config::ChainId,
        bad_debt_e8s: u128,
        total_bad_debt_e8s: u128,
        threshold_e8s: u128,
        timestamp: u64,
    },
    #[serde(rename = "chain_bad_debt_circuit_cleared")]
    ChainBadDebtCircuitCleared {
        chain_id: crate::chains::config::ChainId,
        total_bad_debt_e8s: u128,
        timestamp: u64,
    },
    // Phase 1a Task 11: Timer B supply-invariant self-check failure.
    #[serde(rename = "supply_invariant_self_check_failed")]
    SupplyInvariantSelfCheckFailed {
        sum_chain_supplies_e8s: u128,
        total_debt_e8s: u128,
        timestamp: u64,
    },

    // Phase 1b: Monad (and future foreign-chain) audit trail.
    #[serde(rename = "deposit_observed")]
    DepositObserved {
        chain_id: crate::chains::config::ChainId,
        vault_id: u64,
        custody_address: String,
        amount_e18: u128,
        tx_hash: String,
        block_number: u64,
        timestamp: u64,
    },
    #[serde(rename = "chain_mint_submitted")]
    ChainMintSubmitted {
        chain_id: crate::chains::config::ChainId,
        vault_id: u64,
        op_id: u64,
        recipient: String,
        amount_e8s: u128,
        tx_hash: String,
        timestamp: u64,
    },
    #[serde(rename = "chain_mint_confirmed")]
    ChainMintConfirmed {
        chain_id: crate::chains::config::ChainId,
        vault_id: u64,
        op_id: u64,
        amount_e8s: u128,
        tx_hash: String,
        block_number: u64,
        timestamp: u64,
    },
    #[serde(rename = "chain_burn_observed")]
    ChainBurnObserved {
        chain_id: crate::chains::config::ChainId,
        vault_id: u64,
        amount_e8s: u128,
        tx_hash: String,
        block_number: u64,
        timestamp: u64,
    },
    #[serde(rename = "withdrawal_signed")]
    WithdrawalSigned {
        chain_id: crate::chains::config::ChainId,
        vault_id: u64,
        op_id: u64,
        recipient: String,
        amount_e18: u128,
        tx_hash: String,
        timestamp: u64,
    },
    #[serde(rename = "chain_settlement_failed")]
    ChainSettlementFailed {
        chain_id: crate::chains::config::ChainId,
        op_id: u64,
        reason: String,
        timestamp: u64,
    },
    #[serde(rename = "chain_reorg_detected")]
    ChainReorgDetected {
        chain_id: crate::chains::config::ChainId,
        observed_block: u64,
        reorg_depth: u64,
        timestamp: u64,
    },
    #[serde(rename = "chain_hot_wallet_low")]
    ChainHotWalletLow {
        chain_id: crate::chains::config::ChainId,
        balance_e18: u128,
        threshold_e18: u128,
        timestamp: u64,
    },
    /// Task 12 (Option B): an interest mint confirmed on-chain. `mint_id` is the
    /// synthetic on-chain mint id; `vault_id` is the REAL vault whose `debt_e8s`
    /// grew by `amount_e8s` (matched by the chain supply growing equally).
    #[serde(rename = "chain_interest_minted")]
    ChainInterestMinted {
        chain_id: crate::chains::config::ChainId,
        vault_id: u64,
        mint_id: u64,
        amount_e8s: u128,
        tx_hash: String,
        block_number: u64,
        timestamp: u64,
    },
    // ── Chains-liquidation engine (Increments 1-4, landed and emitted) ──
    // Variants for the liquidation cascade. Additive to the append-only
    // event candid surface; defined in Increment 1 alongside the V6 bump so
    // a single deploy covered the engine, then emitted starting Increment 2
    // (bot path) and Increment 4 (SP path), both of which have since landed.
    /// Increment 2+: a bot (PSM) partial liquidation confirmed — `debt_cleared_e8s`
    /// of the vault's debt was retired into reserve (no icUSD burn) and
    /// `collateral_seized_native` was sold. Pairs with `ChainReserveCredited`.
    #[serde(rename = "chain_vault_liquidated")]
    ChainVaultLiquidated {
        chain_id: crate::chains::config::ChainId,
        vault_id: u64,
        op_id: u64,
        debt_cleared_e8s: u128,
        collateral_seized_native: u128,
        tier: crate::chains::vault::LiquidationTier,
        timestamp: u64,
    },
    /// Increment 2+: reserve backing credited for a chain after a bot swap settled
    /// (`backing_added_e8s` moved debt->reserve; `usdc_native` realized USDC
    /// recorded). The accounting side of `ChainVaultLiquidated`.
    #[serde(rename = "chain_reserve_credited")]
    ChainReserveCredited {
        chain_id: crate::chains::config::ChainId,
        vault_id: u64,
        backing_added_e8s: u128,
        usdc_native: u128,
        timestamp: u64,
    },
    /// Increment 5: the operator verified the foreign-chain burn for SP-absorbed
    /// debt and settled `pending_chain_burn_e8s -> chain_supplies`.
    #[serde(rename = "chain_pending_burn_settled")]
    ChainPendingBurnSettled {
        chain_id: crate::chains::config::ChainId,
        amount_e8s: u128,
        proof: String,
        timestamp: u64,
    },
    /// Increment 5: the operator verified the reserve-backed foreign icUSD burn
    /// after the slow bridge leg and settled `reserve_backing_e8s -> chain_supplies`.
    #[serde(rename = "chain_reserve_burn_settled")]
    ChainReserveBurnSettled {
        chain_id: crate::chains::config::ChainId,
        amount_e8s: u128,
        proof: String,
        timestamp: u64,
    },
    /// Increment 4+: an SP depositor's CFX claim was settled (paid to their EVM
    /// address) for a chain-vault liquidation. Claim-scoped, not vault-scoped.
    #[serde(rename = "chain_cfx_claim_settled")]
    ChainCfxClaimSettled {
        chain_id: crate::chains::config::ChainId,
        claim_id: u64,
        recipient: String,
        amount_native: u128,
        timestamp: u64,
    },
    /// Increment 2+: a vault was liquidatable but liquidation was DEFERRED this
    /// tick (stale price, halted chain, DEX depth too thin, etc.). Carries the
    /// reason so the operator can see why a vault is stuck at a tier.
    #[serde(rename = "chain_liquidation_deferred")]
    ChainLiquidationDeferred {
        chain_id: crate::chains::config::ChainId,
        vault_id: u64,
        reason: String,
        timestamp: u64,
    },
}

/// Durable bot proof audit transitions. These are kept in the private ordered
/// journal so old clients of the public `Event` endpoints retain their exact
/// variant set and index/length behavior.
///
/// Compatibility boundary: the paired production DID has never exposed these
/// variants, so production Event logs do not contain them. A pre-release or
/// staging log written by a build that stored these variants in the public
/// Event log needs a one-time migration before upgrading to this layout. In
/// particular, source predecessor `50245136` could write these tags, while the
/// production backend hash `1714712f...07b0ecf1` and its captured DID predate
/// them. Do not treat a staging canister on that predecessor as upgrade-safe
/// unless its Event log is known not to contain these records.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BotProofAuditEvent {
    #[serde(rename = "bot_proof_mode_enabled")]
    ProofModeEnabled,
    #[serde(rename = "bot_claim_generation_reserved")]
    ClaimGenerationReserved { generation: u64 },
    #[serde(rename = "bot_payment_proof_consumed")]
    PaymentProofConsumed {
        ledger_principal: Principal,
        block_index: u64,
        vault_id: u64,
        claim_generation: u64,
    },
}

/// Durable state-transition journal for payout obligations. These records are
/// kept in a private stable log rather than the public `Event` log because
/// adding public Event variants would break older clients of `get_events`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PendingPayoutEvent {
    /// Internal bot claim proof transitions share the ordered private journal
    /// so they remain replayable without expanding the public Event variant set.
    BotProofAudit { event: BotProofAuditEvent },
    Amm1DonationStarted { operation: PendingAmm1Donation },
    Amm1DonationMintAccepted { notify_nonce: u64, block_index: u64 },
    Amm1DonationReceiptReconciled {
        notify_nonce: u64,
        block_index: u64,
        reconciled_by: Principal,
    },
    Amm1DonationReconciliationRequired { notify_nonce: u64, reason: crate::state::Amm1DonationReconciliationReason },
    Amm1DonationCompleted { notify_nonce: u64 },
    Amm1DonationHeld { donation: HeldAmm1Donation },
    ThreeUsdReservePayoutPrepared {
        key: crate::state::ThreeUsdReserveIngressKey,
        payout: crate::state::ThreeUsdReserveCollateralPayout,
    },
    ThreeUsdReservePayoutCandidate {
        key: crate::state::ThreeUsdReserveIngressKey,
        operation_id: u128,
        attempt_nonce: u128,
        block_index: u64,
    },
    ThreeUsdReservePayoutFeeObserved {
        key: crate::state::ThreeUsdReserveIngressKey,
        operation_id: u128,
        attempt_nonce: u128,
        block_index: u64,
        actual_fee_e8s: u64,
    },
    ThreeUsdReservePayoutCandidateScan {
        key: crate::state::ThreeUsdReserveIngressKey,
        operation_id: u128,
        attempt_nonce: u128,
        scan: crate::state::ThreeUsdReservePayoutCandidateScan,
    },
    Queued {
        kind: PendingPayoutKind,
        operation_id: u128,
        transfer: PendingMarginTransfer,
        timestamp: Option<u64>,
    },
    DispatchBoundary {
        operation_id: u128,
        attempt_nonce: u128,
        payout_kind: Option<PendingPayoutKind>,
        ledger: Principal,
        owner: Principal,
        amount_raw: u64,
        start_index: Option<u64>,
        timestamp: Option<u64>,
    },
    TooOld {
        operation_id: u128,
        attempt_nonce: u128,
        owner: Principal,
        timestamp: Option<u64>,
    },
    Rearmed {
        operation_id: u128,
        attempt_nonce: u128,
        proof: Option<PendingPayoutNoEffectProof>,
        timestamp: Option<u64>,
        owner: Option<Principal>,
    },
    AmbiguousOutcome {
        operation_id: u128,
        attempt_nonce: u128,
        owner: Principal,
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
            Event::SetDustLiquidationThreshold { .. } => false,
            Event::SetGlobalIcusdMintCap { .. } => false,
            Event::SetStableTokenEnabled { .. } => false,
            Event::SetStableLedgerPrincipal { .. } => false,
            Event::SetTreasuryPrincipal { .. } => false,
            Event::SetStabilityPoolPrincipal { .. } => false,
            Event::SetLiquidationBotPrincipal { .. } => false,
            Event::SetBotBudget { .. } => false,
            Event::SetBotAllowedCollateralTypes { .. } => false,
            Event::SetBotCrToleranceBps { .. } => false,
            Event::SetCollateralMinXrcSources { .. } => false,
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
            Event::SetAmm1Canister { .. } => false,
            Event::SetAmm1PoolId { .. } => false,
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
            // Wave-8e LIQ-005
            Event::DeficitAccrued { vault_id, .. } => vault_id == filter_vault_id,
            Event::DeficitRepaid { .. } => false,
            Event::SetDeficitRepaymentFraction { .. } => false,
            Event::SetDeficitReadonlyThresholdE8s { .. } => false,
            // Wave-10 LIQ-008
            Event::BreakerTripped { .. } => false,
            Event::BreakerCleared { .. } => false,
            Event::SetBreakerWindowNs { .. } => false,
            Event::SetBreakerWindowDebtCeilingE8s { .. } => false,
            // Wave-11 BOT-001
            Event::BotClaimReconciliationNeeded { vault_id, .. } => vault_id == filter_vault_id,
            // Wave-14a CDP-10: vault_ids is the list of dispatched vaults; the
            // event is "related" if the filter id is among them.
            Event::StabilityPoolCallFailed { vault_ids, .. } => vault_ids.contains(filter_vault_id),
            // Wave-14a CDP-01: protocol-wide trip, no specific vault.
            Event::OracleCircuitBreaker { .. } => false,
            // Wave-14a CDP-14: per-collateral, not per-vault.
            Event::OracleSourceCountInsufficient { .. } => false,
            // Phase 1a: chain-admin events are protocol-wide, not vault-scoped.
            Event::ChainRegistered { .. }
            | Event::ChainDisabled { .. }
            | Event::ChainConfigUpdated { .. }
            | Event::ChainBadDebtCircuitThresholdSet { .. }
            | Event::ChainBadDebtCircuitTripped { .. }
            | Event::ChainBadDebtCircuitCleared { .. } => false,
            // Phase 1a Task 11: supply invariant failure is protocol-wide.
            Event::SupplyInvariantSelfCheckFailed { .. } => false,
            // Phase 1b: vault-carrying foreign-chain events surface per-vault history.
            Event::DepositObserved { vault_id, .. }
            | Event::ChainMintSubmitted { vault_id, .. }
            | Event::ChainMintConfirmed { vault_id, .. }
            | Event::ChainBurnObserved { vault_id, .. }
            | Event::ChainInterestMinted { vault_id, .. }
            // Increment 1: chains-liquidation events that name a vault.
            | Event::ChainVaultLiquidated { vault_id, .. }
            | Event::ChainReserveCredited { vault_id, .. }
            | Event::ChainLiquidationDeferred { vault_id, .. }
            | Event::WithdrawalSigned { vault_id, .. } => vault_id == filter_vault_id,
            // Phase 1b: protocol-wide or op-scoped events, not vault-specific.
            Event::ChainSettlementFailed { .. }
            | Event::ChainReorgDetected { .. }
            // Increment 1: an SP CFX claim is claim-scoped, not vault-scoped.
            | Event::ChainCfxClaimSettled { .. }
            | Event::ChainPendingBurnSettled { .. }
            | Event::ChainReserveBurnSettled { .. }
            | Event::ChainHotWalletLow { .. } => false,
        }
    }

    /// Returns true if this is a noisy periodic event (hidden from explorer).
    pub fn is_accrue_interest(&self) -> bool {
        matches!(
            self,
            Event::AccrueInterest { .. } | Event::PriceUpdate { .. }
        )
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
            Event::DeficitAccrued { .. } => EventTypeFilter::DeficitAccrued,
            Event::DeficitRepaid { .. } => EventTypeFilter::DeficitRepaid,
            // Wave-10 LIQ-008: BreakerTripped is auto-emitted; BreakerCleared
            // and the two Set* tunables collapse to Admin via the catch-all.
            Event::BreakerTripped { .. } => EventTypeFilter::BreakerTripped,
            // Wave-11 BOT-001: dedicated filter so operators can query stuck
            // claims directly without scanning the Admin bucket.
            Event::BotClaimReconciliationNeeded { .. } => {
                EventTypeFilter::BotClaimReconciliationNeeded
            }
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
            Event::SetDustLiquidationThreshold { .. } => Some("SetDustLiquidationThreshold"),
            Event::SetGlobalIcusdMintCap { .. } => Some("SetGlobalIcusdMintCap"),
            Event::SetStableTokenEnabled { .. } => Some("SetStableTokenEnabled"),
            Event::SetStableLedgerPrincipal { .. } => Some("SetStableLedgerPrincipal"),
            Event::SetTreasuryPrincipal { .. } => Some("SetTreasuryPrincipal"),
            Event::SetStabilityPoolPrincipal { .. } => Some("SetStabilityPoolPrincipal"),
            Event::SetLiquidationBotPrincipal { .. } => Some("SetLiquidationBotPrincipal"),
            Event::SetBotBudget { .. } => Some("SetBotBudget"),
            Event::SetBotAllowedCollateralTypes { .. } => Some("SetBotAllowedCollateralTypes"),
            Event::SetBotCrToleranceBps { .. } => Some("SetBotCrToleranceBps"),
            Event::SetCollateralMinXrcSources { .. } => Some("SetCollateralMinXrcSources"),
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
            Event::SetAmm1Canister { .. } => Some("SetAmm1Canister"),
            Event::SetAmm1PoolId { .. } => Some("SetAmm1PoolId"),
            Event::SetCollateralLiquidationRatio { .. } => Some("SetCollateralLiquidationRatio"),
            Event::SetCollateralBorrowThreshold { .. } => Some("SetCollateralBorrowThreshold"),
            Event::SetCollateralLiquidationBonus { .. } => Some("SetCollateralLiquidationBonus"),
            Event::SetCollateralMinVaultDebt { .. } => Some("SetCollateralMinVaultDebt"),
            Event::SetCollateralLedgerFee { .. } => Some("SetCollateralLedgerFee"),
            Event::SetCollateralRedemptionFeeFloor { .. } => {
                Some("SetCollateralRedemptionFeeFloor")
            }
            Event::SetCollateralRedemptionFeeCeiling { .. } => {
                Some("SetCollateralRedemptionFeeCeiling")
            }
            Event::SetCollateralMinDeposit { .. } => Some("SetCollateralMinDeposit"),
            Event::SetCollateralDisplayColor { .. } => Some("SetCollateralDisplayColor"),
            Event::SetDeficitRepaymentFraction { .. } => Some("SetDeficitRepaymentFraction"),
            Event::SetDeficitReadonlyThresholdE8s { .. } => Some("SetDeficitReadonlyThresholdE8s"),
            // Wave-10 LIQ-008
            Event::BreakerCleared { .. } => Some("BreakerCleared"),
            Event::SetBreakerWindowNs { .. } => Some("SetBreakerWindowNs"),
            Event::SetBreakerWindowDebtCeilingE8s { .. } => Some("SetBreakerWindowDebtCeilingE8s"),
            // Protocol-health incidents that collapse into the `Admin` type
            // filter (no dedicated `EventTypeFilter` variant). Labeled so the
            // explorer's admin-label narrowing can isolate them server-side,
            // and so the strings match the rumi_analytics labeler exactly
            // (sources/backend.rs `admin_label()`), which already emits these
            // for its breakdown rollup.
            Event::OracleCircuitBreaker { .. } => Some("OracleCircuitBreaker"),
            Event::OracleSourceCountInsufficient { .. } => Some("OracleSourceCountInsufficient"),
            Event::StabilityPoolCallFailed { .. } => Some("StabilityPoolCallFailed"),
            Event::SupplyInvariantSelfCheckFailed { .. } => Some("SupplyInvariantSelfCheckFailed"),
            // Cross-chain admin/audit events (Phase 1a/1b, dev-gated).
            Event::ChainRegistered { .. } => Some("ChainRegistered"),
            Event::ChainDisabled { .. } => Some("ChainDisabled"),
            Event::ChainConfigUpdated { .. } => Some("ChainConfigUpdated"),
            Event::ChainBadDebtCircuitThresholdSet { .. } => {
                Some("ChainBadDebtCircuitThresholdSet")
            }
            Event::ChainBadDebtCircuitTripped { .. } => Some("ChainBadDebtCircuitTripped"),
            Event::ChainBadDebtCircuitCleared { .. } => Some("ChainBadDebtCircuitCleared"),
            Event::ChainSettlementFailed { .. } => Some("ChainSettlementFailed"),
            Event::ChainReorgDetected { .. } => Some("ChainReorgDetected"),
            Event::ChainHotWalletLow { .. } => Some("ChainHotWalletLow"),
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
            Event::SetBotBudget {
                start_timestamp, ..
            } => Some(*start_timestamp),
            // Wave-10 LIQ-008: surface breaker events in time-range queries so
            // operators can audit "every trip in the last 24h" and admin sets.
            Event::BreakerTripped { timestamp, .. } => Some(*timestamp),
            Event::BreakerCleared { timestamp, .. } => Some(*timestamp),
            Event::SetBreakerWindowNs { timestamp, .. } => Some(*timestamp),
            Event::SetBreakerWindowDebtCeilingE8s { timestamp, .. } => Some(*timestamp),
            // Wave-11 BOT-001
            Event::BotClaimReconciliationNeeded { timestamp, .. } => Some(*timestamp),
            // Wave-14a CDP-10 + CDP-01 + CDP-14: surface in time-range queries
            // so operators can audit oracle and SP-call failures by window.
            Event::StabilityPoolCallFailed { timestamp, .. } => Some(*timestamp),
            Event::OracleCircuitBreaker { timestamp, .. } => Some(*timestamp),
            Event::OracleSourceCountInsufficient { timestamp, .. } => Some(*timestamp),
            Event::ChainBadDebtCircuitThresholdSet { timestamp, .. } => Some(*timestamp),
            Event::ChainBadDebtCircuitTripped { timestamp, .. } => Some(*timestamp),
            Event::ChainBadDebtCircuitCleared { timestamp, .. } => Some(*timestamp),
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
            Event::AddCollateralType {
                collateral_type, ..
            }
            | Event::UpdateCollateralStatus {
                collateral_type, ..
            }
            | Event::UpdateCollateralConfig {
                collateral_type, ..
            }
            | Event::SetCollateralBorrowingFee {
                collateral_type, ..
            }
            | Event::SetInterestRate {
                collateral_type, ..
            }
            | Event::SetRecoveryParameters {
                collateral_type, ..
            }
            | Event::SetCollateralLiquidationRatio {
                collateral_type, ..
            }
            | Event::SetCollateralBorrowThreshold {
                collateral_type, ..
            }
            | Event::SetCollateralLiquidationBonus {
                collateral_type, ..
            }
            | Event::SetCollateralMinVaultDebt {
                collateral_type, ..
            }
            | Event::SetCollateralLedgerFee {
                collateral_type, ..
            }
            | Event::SetCollateralRedemptionFeeFloor {
                collateral_type, ..
            }
            | Event::SetCollateralRedemptionFeeCeiling {
                collateral_type, ..
            }
            | Event::SetCollateralMinDeposit {
                collateral_type, ..
            }
            | Event::SetCollateralDisplayColor {
                collateral_type, ..
            }
            | Event::SetCollateralMinXrcSources {
                collateral_type, ..
            }
            | Event::PriceUpdate {
                collateral_type, ..
            } => Some(*collateral_type),
            Event::RedemptionOnVaults {
                collateral_type, ..
            } => *collateral_type,
            Event::ReserveRedemption {
                stable_token_ledger,
                ..
            } => Some(*stable_token_ledger),
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
            Event::BorrowFromVault {
                borrowed_amount, ..
            } => Some(borrowed_amount.0),
            Event::RepayToVault { repayed_amount, .. } => Some(repayed_amount.0),
            Event::RedemptionOnVaults { icusd_amount, .. } => Some(icusd_amount.0),
            Event::ReserveRedemption { icusd_amount, .. } => Some(icusd_amount.0),
            Event::AdminMint { amount, .. } => Some(amount.0),
            Event::DustForgiven { amount, .. } => Some(amount.0),
            Event::ProvideLiquidity { amount, .. } => Some(amount.0),
            Event::WithdrawLiquidity { amount, .. } => Some(amount.0),
            Event::PartialLiquidateVault {
                liquidator_payment, ..
            } => Some(liquidator_payment.0),
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

fn apply_bot_proof_audit_event(state: &mut State, event: BotProofAuditEvent) {
    match event {
        BotProofAuditEvent::ProofModeEnabled => state.bot_confirm_proof_required = true,
        BotProofAuditEvent::ClaimGenerationReserved { generation } => {
            state.bot_claim_generation_counter = state.bot_claim_generation_counter.max(generation);
        }
        BotProofAuditEvent::PaymentProofConsumed {
            ledger_principal,
            block_index,
            vault_id,
            claim_generation,
        } => {
            state.consumed_bot_payment_proofs.insert(
                format!("{}:{block_index}", ledger_principal.to_text()),
                (vault_id, claim_generation),
            );
        }
    }
}

fn apply_pending_payout_event(state: &mut State, event: PendingPayoutEvent) {
    match event {
        PendingPayoutEvent::BotProofAudit { event } => apply_bot_proof_audit_event(state, event),
        PendingPayoutEvent::Amm1DonationStarted { operation } => {
            state.amm1_donation_nonce = state.amm1_donation_nonce.max(operation.notify_nonce);
            state.op_nonce_counter = state
                .op_nonce_counter
                .max((operation.mint_op_nonce as u64).wrapping_add(1));
            state.pending_amm1_donation_operations.insert(operation.notify_nonce, operation);
        }
        PendingPayoutEvent::Amm1DonationMintAccepted { notify_nonce, block_index } => {
            if let Some(operation) = state.pending_amm1_donation_operations.get_mut(&notify_nonce) {
                operation.mint_block_index = Some(block_index);
                operation.phase = crate::state::Amm1DonationPhase::NotifyPending;
                operation.reconciliation_reason = None;
            }
        }
        PendingPayoutEvent::Amm1DonationReceiptReconciled {
            notify_nonce,
            block_index,
            reconciled_by,
        } => {
            state.reconciled_amm1_donation_receipts.insert(
                notify_nonce,
                crate::state::ReconciledAmm1DonationReceipt {
                    block_index,
                    reconciled_by,
                },
            );
            if let Some(operation) = state.pending_amm1_donation_operations.get_mut(&notify_nonce) {
                operation.mint_block_index = Some(block_index);
                operation.phase = crate::state::Amm1DonationPhase::NotifyPending;
                operation.reconciliation_reason = None;
            }
        }
        PendingPayoutEvent::Amm1DonationReconciliationRequired { notify_nonce, reason } => {
            if let Some(operation) = state.pending_amm1_donation_operations.get_mut(&notify_nonce) {
                operation.phase = crate::state::Amm1DonationPhase::ReconciliationRequired;
                operation.reconciliation_reason = Some(reason);
            }
        }
        PendingPayoutEvent::Amm1DonationCompleted { notify_nonce } => {
            state.pending_amm1_donation_operations.remove(&notify_nonce);
        }
        PendingPayoutEvent::Amm1DonationHeld { donation } => {
            if !state.held_amm1_donations.iter().any(|held| {
                held.notify_nonce == donation.notify_nonce
                    && held.amount_e8s == donation.amount_e8s
            }) {
                state.held_amm1_donations.push(donation);
            }
        }
        PendingPayoutEvent::ThreeUsdReservePayoutPrepared { key, payout } => {
            if payout.operation_id == 0
                || payout.op_nonce != payout.operation_id
                || payout.candidate_block_index.is_some()
                || payout.observed_fee_e8s.is_some()
                || payout.rearmed_attempts.len() > crate::state::MAX_THREE_USD_RESERVE_PAYOUT_ATTEMPTS
                || payout.destination.owner != key.stability_pool
                || payout.destination.subaccount.is_some()
                || payout.source.owner == Principal::anonymous()
                || payout.source.subaccount.is_some()
                || payout.gross_e8s.checked_sub(payout.net_e8s) != Some(payout.expected_fee_e8s)
                || payout.fee_arg_e8s != Some(payout.expected_fee_e8s)
                || payout.memo.as_slice()
                    != crate::management::nonce_to_memo(payout.op_nonce).0.as_slice()
                || payout.created_at_time_ns
                    != crate::management::nonce_to_created_at_time(payout.op_nonce)
                || state.three_usd_reserve_payout_operation_keys
                    .get(&payout.operation_id)
                    .is_some_and(|existing| existing != &key)
            {
                return;
            }
            match state.three_usd_reserve_collateral_payouts.get(&key) {
                None => {
                    state.three_usd_reserve_payout_operation_keys
                        .insert(payout.operation_id, key.clone());
                    state.three_usd_reserve_collateral_payouts.insert(key, payout);
                }
                Some(existing) if existing == &payout => {
                    state.three_usd_reserve_payout_operation_keys
                        .insert(payout.operation_id, key);
                }
                _ => {}
            }
        }
        PendingPayoutEvent::ThreeUsdReservePayoutCandidate { key, operation_id, attempt_nonce, block_index } => {
            if !state.three_usd_reserve_payout_operation_keys
                .get(&operation_id).is_some_and(|saved| saved == &key)
                || !state.get_pending_payout(operation_id)
                    .is_some_and(|(_, transfer)| transfer.op_nonce == attempt_nonce)
            {
                return;
            }
            let Some(payout) = state.three_usd_reserve_collateral_payouts.get_mut(&key) else {
                return;
            };
            if payout.operation_id != operation_id { return; }
            if state.three_usd_reserve_payout_candidate_scans.get(&key).is_some_and(|scan| {
                scan.operation_id != operation_id
                    || scan.attempt_nonce != attempt_nonce
                    || scan.next_index != scan.snapshot_log_length
                    || scan.candidate_block_index != Some(block_index)
                    || scan.multiple_candidates
            }) {
                return;
            }
            if payout.op_nonce == attempt_nonce && payout.candidate_block_index.is_none() {
                payout.candidate_block_index = Some(block_index);
            } else if let Some(attempt) = payout.rearmed_attempts.iter_mut()
                .find(|attempt| attempt.op_nonce == attempt_nonce && attempt.candidate_block_index.is_none())
            {
                attempt.candidate_block_index = Some(block_index);
            } else {
                return;
            }
            state.three_usd_reserve_payout_candidate_scans.remove(&key);
        }
        PendingPayoutEvent::ThreeUsdReservePayoutFeeObserved {
            key, operation_id, attempt_nonce, block_index, actual_fee_e8s,
        } => {
            if !state.three_usd_reserve_payout_operation_keys
                .get(&operation_id).is_some_and(|saved| saved == &key)
            {
                return;
            }
            if let Some(payout) = state.three_usd_reserve_collateral_payouts.get_mut(&key) {
                if payout.operation_id != operation_id { return; }
                if payout.op_nonce == attempt_nonce
                    && payout.candidate_block_index == Some(block_index)
                    && payout.observed_fee_e8s.is_none()
                {
                    payout.observed_fee_e8s = Some(actual_fee_e8s);
                } else if let Some(attempt) = payout.rearmed_attempts.iter_mut().find(|attempt| {
                    attempt.op_nonce == attempt_nonce
                        && attempt.candidate_block_index == Some(block_index)
                        && attempt.observed_fee_e8s.is_none()
                }) {
                    attempt.observed_fee_e8s = Some(actual_fee_e8s);
                }
            }
        }
        PendingPayoutEvent::ThreeUsdReservePayoutCandidateScan {
            key, operation_id, attempt_nonce, scan,
        } => {
            let Some(payout) = state.three_usd_reserve_collateral_payouts.get(&key) else {
                return;
            };
            if !state.three_usd_reserve_payout_operation_keys
                .get(&operation_id).is_some_and(|saved| saved == &key)
                || !state.get_pending_payout(operation_id).is_some_and(|(_, transfer)| {
                    transfer.operation_id == operation_id
                        && transfer.op_nonce == attempt_nonce
                        && transfer.owner == key.stability_pool
                        && transfer.vault_id == key.vault_id
                        && transfer.payout_kind == PendingPayoutKind::Margin
                        && transfer.ledger == Some(payout.ledger)
                        && transfer.collateral_type == payout.collateral_type
                        && transfer.margin.to_u64() == payout.gross_e8s
                        && transfer.transfer_amount_raw == Some(payout.net_e8s)
                        && transfer.held_for_manual_retry
                        && transfer.reconciliation_required
                        && !transfer.in_flight
                        && transfer.history_start_index == Some(scan.start_index)
                })
                || scan.operation_id != operation_id
                || scan.attempt_nonce != attempt_nonce
                || scan.next_index < scan.start_index
                || scan.next_index > scan.snapshot_log_length
                || scan.snapshot_count == 0
                || scan.snapshot_count > crate::state::MAX_THREE_USD_RESERVE_CANDIDATE_SCAN_SNAPSHOTS
                || scan.snapshot_log_length.saturating_sub(scan.start_index) > 10_000
                || scan.candidate_block_index.is_some_and(|index| {
                    index < scan.start_index || index >= scan.next_index
                })
                || (scan.multiple_candidates && scan.candidate_block_index.is_none())
            {
                return;
            }
            let attempt_matches = if payout.op_nonce == attempt_nonce {
                payout.memo.as_slice()
                    == crate::management::nonce_to_memo(attempt_nonce).0.as_slice()
                    && payout.created_at_time_ns
                        == crate::management::nonce_to_created_at_time(attempt_nonce)
                    && payout.fee_arg_e8s == Some(payout.expected_fee_e8s)
            } else {
                payout.rearmed_attempts.iter().any(|attempt| {
                    attempt.op_nonce == attempt_nonce
                        && attempt.memo.as_slice()
                            == crate::management::nonce_to_memo(attempt_nonce).0.as_slice()
                        && attempt.created_at_time_ns
                            == crate::management::nonce_to_created_at_time(attempt_nonce)
                        && attempt.fee_arg_e8s == Some(payout.expected_fee_e8s)
                })
            };
            if payout.operation_id != operation_id
                || !attempt_matches
                || payout.source.subaccount.is_some()
                || payout.destination.owner != key.stability_pool
                || payout.candidate_block_index.is_some()
                || payout.rearmed_attempts.iter().any(|a| a.candidate_block_index.is_some())
            {
                return;
            }
            match state.three_usd_reserve_payout_candidate_scans.get(&key) {
                None => {
                    if scan.snapshot_count != 1
                        || scan.next_index != scan.start_index
                        || scan.candidate_block_index.is_some()
                        || scan.multiple_candidates
                    {
                        return;
                    }
                }
                Some(previous) => {
                    let new_snapshot = previous.next_index == previous.snapshot_log_length
                        && previous.candidate_block_index.is_none()
                        && !previous.multiple_candidates
                        && scan.next_index == scan.start_index
                        && scan.snapshot_log_length >= previous.snapshot_log_length
                        && previous.snapshot_count < crate::state::MAX_THREE_USD_RESERVE_CANDIDATE_SCAN_SNAPSHOTS
                        && scan.snapshot_count == previous.snapshot_count + 1
                        && scan.candidate_block_index.is_none()
                        && !scan.multiple_candidates;
                    let advances = previous.operation_id == operation_id
                        && previous.attempt_nonce == attempt_nonce
                        && previous.start_index == scan.start_index
                        && previous.snapshot_log_length == scan.snapshot_log_length
                        && previous.snapshot_count == scan.snapshot_count
                        && scan.next_index >= previous.next_index
                        && scan.next_index.saturating_sub(previous.next_index) <= 64
                        && (previous.candidate_block_index.is_none()
                            || previous.candidate_block_index == scan.candidate_block_index)
                        && (!previous.multiple_candidates || scan.multiple_candidates);
                    if !new_snapshot && !advances {
                        return;
                    }
                }
            }
            state.three_usd_reserve_payout_candidate_scans.insert(key, scan);
        }
        PendingPayoutEvent::Queued {
            kind,
            operation_id,
            mut transfer,
            ..
        } => {
            if kind == PendingPayoutKind::Redemption
                && (transfer.operation_id != operation_id || transfer.payout_kind != kind)
            {
                return;
            }
            transfer.operation_id = operation_id;
            transfer.payout_kind = kind;
            match kind {
                PendingPayoutKind::Margin | PendingPayoutKind::Excess => {
                    state.insert_pending_payout(transfer);
                }
                PendingPayoutKind::Redemption => {
                    let Some((_, current)) = state.get_pending_payout(operation_id) else {
                        return;
                    };
                    if transfer.rearm_schema_version != 1
                        || current.payout_kind != PendingPayoutKind::Redemption
                        || current.owner != transfer.owner
                        || current.margin != transfer.margin
                        || current.collateral_type != transfer.collateral_type
                        || current.op_nonce != transfer.op_nonce
                        || current.ledger != transfer.ledger
                        || current.transfer_amount_raw != transfer.transfer_amount_raw
                        || current.operation_id != transfer.operation_id
                        || !current.held_for_manual_retry
                        || !current.reconciliation_required
                        || current.in_flight
                        || current.too_old_confirmed
                        || current.history_start_index.is_some()
                        || current.history_scan.is_some()
                        || current.no_effect_proof.is_some()
                        || transfer.held_for_manual_retry
                        || transfer.reconciliation_required
                        || transfer.in_flight
                        || transfer.too_old_confirmed
                        || transfer.history_start_index.is_some()
                        || transfer.history_scan.is_some()
                        || transfer.no_effect_proof.is_some()
                        || !redemption_receipt_supports_rearm(
                            Some(current.margin.to_u64()),
                            Some(current.operation_id),
                            Some(current.op_nonce),
                            current.ledger,
                            current.transfer_amount_raw,
                        )
                    {
                        return;
                    }
                    state.mutate_pending_payout(operation_id, |row| *row = transfer);
                }
            }
        }
        PendingPayoutEvent::DispatchBoundary {
            operation_id, attempt_nonce, payout_kind, ledger, owner, amount_raw, start_index, ..
        } => {
            state.mutate_pending_payout(operation_id, |transfer| {
                if transfer.rearm_schema_version != 1
                    || transfer.op_nonce != attempt_nonce
                    || payout_kind != Some(transfer.payout_kind)
                    || transfer.ledger != Some(ledger)
                    || transfer.owner != owner
                    || transfer.transfer_amount_raw != Some(amount_raw)
                    || transfer.history_start_index.is_some()
                {
                    return;
                }
                if let Some(index) = start_index {
                    transfer.history_start_index = Some(index);
                } else {
                    transfer.rearm_schema_version = 0;
                }
            });
        }
        PendingPayoutEvent::TooOld { operation_id, attempt_nonce, owner, .. } => {
            state.mutate_pending_payout(operation_id, |transfer| {
                if transfer.op_nonce == attempt_nonce && transfer.owner == owner {
                    transfer.in_flight = false;
                    transfer.held_for_manual_retry = true;
                    transfer.reconciliation_required = true;
                    transfer.too_old_confirmed = true;
                }
            });
        }
        PendingPayoutEvent::AmbiguousOutcome {
            operation_id,
            attempt_nonce,
            owner,
            ..
        } => {
            state.mutate_pending_payout(operation_id, |transfer| {
                if transfer.op_nonce == attempt_nonce && transfer.owner == owner {
                    transfer.in_flight = false;
                    transfer.held_for_manual_retry = true;
                    transfer.reconciliation_required = true;
                }
            });
        }
        PendingPayoutEvent::Rearmed { operation_id, attempt_nonce, proof, .. } => {
            let Some((_, mut transfer)) = state.get_pending_payout(operation_id) else {
                return;
            };
            let Some(proof) = proof else {
                // A historical rearm without its proof must never make an
                // ambiguous payout retryable.
                state.mutate_pending_payout(operation_id, |row| {
                    row.in_flight = false;
                    row.held_for_manual_retry = true;
                    row.reconciliation_required = true;
                });
                return;
            };
            if transfer.rearm_schema_version != 1
                || transfer.ledger != Some(proof.ledger)
                || transfer.owner != proof.owner
                || transfer.transfer_amount_raw != Some(proof.amount_raw)
                || transfer.op_nonce != proof.old_attempt_nonce
                || transfer.payout_kind != proof.payout_kind
                || transfer.history_start_index != Some(proof.start_index)
                || !transfer.held_for_manual_retry
                || !transfer.reconciliation_required
                || !transfer.too_old_confirmed
                || transfer.in_flight
                || transfer.no_effect_proof.is_some()
                || transfer.retry_count >= crate::MAX_PENDING_RETRIES
                || proof.operation_id != operation_id
                || proof.new_attempt_nonce != attempt_nonce
                || proof.new_attempt_nonce == proof.old_attempt_nonce
                || proof.start_index > proof.snapshot_log_length
                || !proof.complete_prefix
            {
                return;
            }
            let reserve_link_invalid = state.three_usd_reserve_payout_operation_keys
                .get(&operation_id)
                .is_some_and(|key| {
                    state.three_usd_reserve_collateral_payouts.get(key).is_none_or(|payout| {
                        payout.rearmed_attempts.len() >= crate::state::MAX_THREE_USD_RESERVE_PAYOUT_ATTEMPTS
                            || payout.op_nonce == attempt_nonce
                            || payout.candidate_block_index.is_some()
                            || payout.ledger != proof.ledger
                            || payout.destination.owner != proof.owner
                            || payout.net_e8s != proof.amount_raw
                            || payout.rearmed_attempts.iter().any(|attempt| {
                                attempt.candidate_block_index.is_some()
                                    || attempt.op_nonce == attempt_nonce
                            })
                    })
                });
            if reserve_link_invalid {
                state.mutate_pending_payout(operation_id, |row| {
                    row.in_flight = false;
                    row.held_for_manual_retry = true;
                    row.reconciliation_required = true;
                });
                return;
            }
            transfer.op_nonce = attempt_nonce;
            transfer.held_for_manual_retry = false;
            transfer.reconciliation_required = false;
            transfer.too_old_confirmed = false;
            transfer.history_log_length = None;
            transfer.history_cursor = 0;
            transfer.history_scan = None;
            transfer.history_candidate_seen = false;
            transfer.history_start_index = None;
            transfer.no_effect_proof = Some(proof);
            state.mutate_pending_payout(operation_id, |row| *row = transfer);
            if let Some(key) = state.three_usd_reserve_payout_operation_keys.get(&operation_id).cloned() {
                if let Some(payout) = state.three_usd_reserve_collateral_payouts.get_mut(&key) {
                    if payout.rearmed_attempts.len() < crate::state::MAX_THREE_USD_RESERVE_PAYOUT_ATTEMPTS
                        && !payout.rearmed_attempts.iter().any(|attempt| attempt.op_nonce == attempt_nonce)
                    {
                        payout.rearmed_attempts.push(crate::state::ThreeUsdReservePayoutAttempt {
                            op_nonce: attempt_nonce,
                            memo: crate::management::nonce_to_memo(attempt_nonce).0.as_slice()
                                .try_into().expect("nonce memo is exactly 16 bytes"),
                            created_at_time_ns: crate::management::nonce_to_created_at_time(attempt_nonce),
                            fee_arg_e8s: payout.fee_arg_e8s,
                            candidate_block_index: None,
                            observed_fee_e8s: None,
                        });
                    }
                }
                state.three_usd_reserve_payout_candidate_scans.remove(&key);
            }
            state.op_nonce_counter = state
                .op_nonce_counter
                .max((attempt_nonce as u64).wrapping_add(1));
        }
    }
}

pub fn replay(events: impl Iterator<Item = Event>) -> Result<State, ReplayLogError> {
    replay_with_pending_payout_events(events, std::iter::empty())
}

pub fn replay_with_pending_payout_events(
    events: impl Iterator<Item = Event>,
    payout_events: impl Iterator<Item = crate::storage::PendingPayoutJournalEntry>,
) -> Result<State, ReplayLogError> {
    replay_with_nonce_time_and_payout_events(events, payout_events, ic_cdk::api::time)
}

#[cfg(test)]
fn replay_with_nonce_time(
    events: impl Iterator<Item = Event>,
    nonce_time: impl FnMut() -> u64,
) -> Result<State, ReplayLogError> {
    replay_with_nonce_time_and_payout_events(events, std::iter::empty(), nonce_time)
}

fn replay_with_nonce_time_and_payout_events(
    mut events: impl Iterator<Item = Event>,
    payout_events: impl Iterator<Item = crate::storage::PendingPayoutJournalEntry>,
    mut nonce_time: impl FnMut() -> u64,
) -> Result<State, ReplayLogError> {
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
    let mut payout_events = payout_events.peekable();
    let mut public_event_count = 1u64;
    let mut vault_id = 0;
    for event in events {
        while payout_events
            .peek()
            .is_some_and(|entry| entry.after_event_count <= public_event_count)
        {
            apply_pending_payout_event(
                &mut state,
                payout_events.next().expect("peeked payout event").event,
            );
        }
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
                repay_amount,
                ..
            } => {
                // LIQ-0XX (review finding 2): `repay_amount` is `None` for
                // legacy events (pre-fix) and `Some(pinned)` for events
                // recorded after this field existed — `State::liquidate_vault`
                // reproduces the exact pre-fix decision for the `None` case,
                // so replay matches on-chain history in both cases.
                let _ = state.liquidate_vault(vault_id, mode, icp_rate, repay_amount);
            },
            Event::PartialLiquidateVault {
                vault_id,
                liquidator_payment,
                icp_to_liquidator,
                protocol_fee_collateral,
                ledger_fee_collateral,
                three_usd_reserves_e8s,
                ..
            } => {
                // Reduce vault debt and collateral, accounting for interest share
                if let Some(vault) = state.vault_id_to_vaults.get_mut(&vault_id) {
                    // Compute proportional interest share before reducing debt
                    let interest_share = if vault.accrued_interest.0 > 0 && vault.borrowed_icusd_amount.0 > 0 {
                        let share = crate::numeric::checked_proportional_amount(
                            liquidator_payment.0,
                            vault.borrowed_icusd_amount.0,
                            vault.accrued_interest.0,
                        )
                        .unwrap_or(0);
                        ICUSD::new(share.min(vault.accrued_interest.0))
                    } else { ICUSD::new(0) };
                    // Use saturating_sub during replay: interest drift can inflate
                    // vault debts, making the payment exceed the (drifted) balance.
                    // This is safe because the replay path is only used once (first
                    // upgrade); subsequent upgrades restore from stable memory.
                    vault.borrowed_icusd_amount = vault.borrowed_icusd_amount.saturating_sub(liquidator_payment);
                    // Vault loses icp_to_liquidator + protocol_fee_collateral
                    // (old events have protocol_fee_collateral=None → 0, which is correct)
                    let total_collateral_seized = icp_to_liquidator
                        .to_u64()
                        .saturating_add(protocol_fee_collateral.unwrap_or(0))
                        .saturating_add(ledger_fee_collateral.unwrap_or(0));
                    vault.collateral_amount = vault.collateral_amount.saturating_sub(total_collateral_seized);
                    vault.accrued_interest = vault.accrued_interest.saturating_sub(interest_share);
                }
                // Shared drain rule (see state::cleanup_if_drained): every
                // runtime path that records PartialLiquidateVault removes the
                // vault when the liquidation emptied it, so replay must apply
                // the identical rule — otherwise replayed state keeps shell
                // vaults and stale secondary-index ids that live state does
                // not have.
                state.cleanup_if_drained(vault_id);
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
                collateral_type,
                ref vault_redemptions,
                payout_collateral_raw,
                payout_operation_id,
                payout_attempt_nonce,
                payout_ledger,
                payout_transfer_amount_raw,
                min_net_collateral_raw,
                ..
            } => {
                state.provide_liquidity(fee_amount, state.developer_principal);
                let redeem_ct = collateral_type
                    .unwrap_or_else(|| state.icp_collateral_type());
                // AR-B-001/RED-001 (audit 2026-06-09): events that recorded
                // their per-vault outcomes replay EXACTLY by applying those
                // outcomes, because the live scan's eligibility depends on
                // transient facts (per-vault op lock, bot_processing) replay
                // cannot reconstruct. The consumed-based margin mirrors the
                // live payout clamp. Pre-Wave-9 events (no stored outcomes)
                // keep the legacy re-run + full-claim margin.
                let historical_margin: ICP = match vault_redemptions {
                    Some(vrs) => {
                        state.apply_vault_redemptions(vrs);
                        let consumed: u64 = vrs.iter().map(|v| v.icusd_redeemed_e8s).sum();
                        ICUSD::from(consumed) / current_icp_rate
                    }
                    None => {
                        state.redeem_on_vaults_legacy_full_type_for_replay(
                            icusd_amount, current_icp_rate, &redeem_ct,
                        );
                        icusd_amount / current_icp_rate
                    }
                };
                // Only newly recorded events pin native-unit payout. Older events
                // preserve their historical reconstruction exactly.
                let margin = payout_collateral_raw
                    .map(ICP::from)
                    .unwrap_or(historical_margin);
                if margin.to_u64() > 0 {
                    let operation_id = if let Some(operation_id) = payout_operation_id {
                        operation_id
                    } else {
                        let mut generated = state.next_op_nonce_at(nonce_time());
                        while generated == 0 || state.pending_payout_index.contains_key(&generated) {
                            generated = state.next_op_nonce_at(nonce_time());
                        }
                        generated
                    };
                    let attempt_nonce = payout_attempt_nonce.unwrap_or(0);
                    // The public event alone cannot prove its tuple was
                    // journaled before the first ledger await. A matching
                    // private Queued marker below is required for rearm.
                    state.insert_pending_redemption(
                        icusd_block_index,
                        PendingMarginTransfer {
                            vault_id: 0,
                            operation_id,
                            payout_kind: PendingPayoutKind::Redemption,
                            owner,
                            margin,
                            collateral_type: redeem_ct,
                            retry_count: 0,
                            op_nonce: attempt_nonce,
                            ledger: payout_ledger,
                            transfer_amount_raw: payout_transfer_amount_raw,
                            held_for_manual_retry: true,
                            reconciliation_required: true,
                            in_flight: false,
                            too_old_confirmed: false,
                            history_start_index: None,
                            rearm_schema_version: 0,
                            history_scan: None,
                            history_candidate_seen: false,
                            no_effect_proof: None,
                            history_log_length: None,
                            history_cursor: 0,
                            min_net_collateral_raw,
                        },
                    );
                }
            }
            Event::RedemptionTransfered {
                icusd_block_index, operation_id, ..
            } => {
                if let Some(id) = operation_id {
                    state.remove_pending_payout(id);
                } else if let Some(transfer) = state.pending_redemption_transfer.remove(&icusd_block_index) {
                    state.pending_payout_index.remove(&transfer.operation_id);
                }
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
            Event::MarginTransfer { vault_id, operation_id, payout_kind, .. } => {
                if let Some(operation_id) = operation_id {
                    let kind = payout_kind.unwrap_or(PendingPayoutKind::Margin);
                    let _ = state.remove_pending_payout(operation_id);
                    let _ = kind;
                } else {
                    // A legacy settlement event cannot identify which of several
                    // receipts it settled. Preserve them all and quarantine them.
                    for ((vid, _, _), payout) in state.pending_margin_transfers.iter_mut() {
                        if *vid == vault_id { payout.held_for_manual_retry = true; payout.reconciliation_required = true; }
                    }
                    for ((vid, _, _), payout) in state.pending_excess_transfers.iter_mut() {
                        if *vid == vault_id { payout.held_for_manual_retry = true; payout.reconciliation_required = true; }
                    }
                }
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
            Event::SetDustLiquidationThreshold { amount } => {
                if let Ok(val) = amount.parse::<u64>() {
                    state.dust_liquidation_threshold = ICUSD::new(val);
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
            Event::SetBotCrToleranceBps { bps } => {
                state.bot_cr_tolerance_bps = bps;
            },
            Event::SetCollateralMinXrcSources { collateral_type, min_xrc_sources } => {
                if let Some(config) = state.collateral_configs.get_mut(&collateral_type) {
                    config.min_xrc_sources = min_xrc_sources;
                }
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
            Event::SetAmm1Canister { canister } => {
                state.amm1_canister = Some(canister);
            },
            Event::SetAmm1PoolId { pool_id } => {
                state.amm1_pool_id = Some(pool_id);
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
            // Wave-8e LIQ-005: replay the deficit accounting so a state
            // rebuilt purely from the event log carries the right deficit.
            Event::DeficitAccrued { amount, .. } => {
                state.protocol_deficit_icusd = state.protocol_deficit_icusd + amount;
                // Latch on replay if the threshold was crossed at the original
                // event time. The threshold is whatever it is at this point
                // in the replay, which is deterministic given the event order.
                let _ = state.check_deficit_readonly_latch();
            },
            Event::DeficitRepaid { amount, .. } => {
                state.protocol_deficit_icusd =
                    state.protocol_deficit_icusd.saturating_sub(amount);
                state.total_deficit_repaid_icusd =
                    state.total_deficit_repaid_icusd + amount;
            },
            Event::SetDeficitRepaymentFraction { fraction, .. } => {
                state.deficit_repayment_fraction = fraction;
            },
            Event::SetDeficitReadonlyThresholdE8s { threshold_e8s, .. } => {
                state.deficit_readonly_threshold_e8s = threshold_e8s;
            },
            // Wave-10 LIQ-008: rebuild the breaker latch + admin tunables from
            // the event log. `recent_liquidations` is intentionally NOT
            // populated here — the rolling window is transient and any entries
            // older than 30 minutes (the default window) would be evicted on
            // the first record after replay anyway.
            Event::BreakerTripped { .. } => {
                state.liquidation_breaker_tripped = true;
            },
            Event::BreakerCleared { .. } => {
                state.liquidation_breaker_tripped = false;
            },
            Event::SetBreakerWindowNs { window_ns, .. } => {
                state.breaker_window_ns = window_ns;
            },
            Event::SetBreakerWindowDebtCeilingE8s { ceiling_e8s, .. } => {
                state.breaker_window_debt_ceiling_e8s = ceiling_e8s;
            },
            // Wave-11 BOT-001: informational. The audit trail records that
            // `check_vaults` skipped an auto-cancel because the bot had not
            // returned the collateral; no replay-side state mutation is needed
            // because the underlying `BotClaim` and `vault.bot_processing`
            // were intentionally left untouched.
            Event::BotClaimReconciliationNeeded { .. } => {},
            // Wave-14a CDP-10: informational. The fact that the SP call
            // failed is captured in the audit trail; the dispatched vault
            // ids are intentionally left out of `sp_attempted_vaults` so
            // they remain eligible for the next tick.
            Event::StabilityPoolCallFailed { .. } => {},
            // Wave-14a CDP-01: informational. The mode change to ReadOnly
            // (and the matching `mode_triggered_by_oracle = true` flip)
            // happens via direct state mutation in `xrc::note_xrc_failure`,
            // and the oracle-recovery path mirrors it. No replay-side
            // mutation is needed because the live mutation already happened
            // and is captured in the next snapshot.
            Event::OracleCircuitBreaker { .. } => {},
            // Wave-14a CDP-14: informational. The protocol simply skips the
            // sample; cached price stays in place. Nothing to replay.
            Event::OracleSourceCountInsufficient { .. } => {},
            // Phase 1a: chain-admin endpoints apply changes directly to state
            // before recording the event; nothing to replay.
            Event::ChainRegistered { .. }
            | Event::ChainDisabled { .. }
            | Event::ChainConfigUpdated { .. }
            | Event::ChainBadDebtCircuitThresholdSet { .. }
            | Event::ChainBadDebtCircuitTripped { .. }
            | Event::ChainBadDebtCircuitCleared { .. } => {},
            // Phase 1a Task 11: informational audit trail; state mutation
            // (invariant_halted + mode flip) happens live in the timer tick.
            Event::SupplyInvariantSelfCheckFailed { .. } => {},
            // Phase 1b: observability-only events; the actual state mutations
            // happen in their emitting tasks, not on replay.
            Event::DepositObserved { .. }
            | Event::ChainMintSubmitted { .. }
            | Event::ChainMintConfirmed { .. }
            | Event::ChainBurnObserved { .. }
            | Event::ChainInterestMinted { .. }
            | Event::WithdrawalSigned { .. }
            | Event::ChainSettlementFailed { .. }
            | Event::ChainReorgDetected { .. }
            // Increment 1: chains-liquidation events are observability-only; the
            // reserve/debt/supply mutations happen live in the bot/SP confirm
            // paths (Increments 2-4), not on replay.
            | Event::ChainVaultLiquidated { .. }
            | Event::ChainReserveCredited { .. }
            | Event::ChainCfxClaimSettled { .. }
            | Event::ChainPendingBurnSettled { .. }
            | Event::ChainReserveBurnSettled { .. }
            | Event::ChainLiquidationDeferred { .. }
            | Event::ChainHotWalletLow { .. } => {},
        }
        public_event_count = public_event_count.saturating_add(1);
    }
    while let Some(entry) = payout_events.next() {
        if entry.after_event_count > public_event_count {
            return Err(ReplayLogError::InconsistentLog(format!(
                "private payout event is anchored beyond the public event log: {} > {}",
                entry.after_event_count, public_event_count
            )));
        }
        apply_pending_payout_event(&mut state, entry.event);
    }
    state.next_available_vault_id = vault_id;
    Ok(state)
}

/// Helper: current canister time in nanoseconds.
fn now() -> u64 {
    ic_cdk::api::time()
}

pub fn record_liquidate_vault(
    state: &mut State,
    vault_id: u64,
    mode: Mode,
    collateral_price: UsdIcp,
) {
    // No pre-await snapshot exists for this synchronous caller, so decide
    // the amount now (equivalent to a `None`-style live decision) and pin
    // that SAME decided value into both the event and the applied mutation,
    // so a future replay of this event's `Some(repay_amount)` reproduces
    // exactly what happened here.
    let repay_amount = state
        .vault_id_to_vaults
        .get(&vault_id)
        .map(|v| state.effective_liquidation_amount(v, collateral_price, None));
    record_event(&Event::LiquidateVault {
        vault_id,
        mode,
        icp_rate: collateral_price,
        liquidator: None,
        timestamp: Some(now()),
        repay_amount,
    });
    let _ = state.liquidate_vault(vault_id, mode, collateral_price, repay_amount);
}

pub fn record_redistribute_vault(state: &mut State, vault_id: u64) {
    record_event(&Event::RedistributeVault {
        vault_id,
        timestamp: Some(now()),
    });
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
    record_withdraw_liquidity_at(state, amount, caller, block_index, now());
}

pub fn record_withdraw_liquidity_at(
    state: &mut State,
    amount: ICUSD,
    caller: Principal,
    block_index: u64,
    timestamp_ns: u64,
) {
    record_event_at(&Event::WithdrawLiquidity {
        amount,
        block_index,
        caller,
        timestamp: Some(timestamp_ns),
    }, timestamp_ns);
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

pub fn record_margin_transfer(
    state: &mut State,
    vault_id: u64,
    owner: Principal,
    operation_id: u128,
    payout_kind: PendingPayoutKind,
    block_index: u64,
) {
    record_margin_transfer_at(
        state,
        vault_id,
        owner,
        operation_id,
        payout_kind,
        block_index,
        now(),
    );
}

pub(crate) fn record_margin_transfer_at(
    state: &mut State,
    vault_id: u64,
    owner: Principal,
    operation_id: u128,
    payout_kind: PendingPayoutKind,
    block_index: u64,
    timestamp_ns: u64,
) {
    crate::storage::record_event_at(
        &Event::MarginTransfer {
            vault_id,
            block_index,
            operation_id: Some(operation_id),
            payout_kind: Some(payout_kind),
            timestamp: Some(timestamp_ns),
        },
        timestamp_ns,
    );
    let _ = owner;
    state.remove_pending_payout(operation_id);
}

pub fn record_pending_payout(state: &mut State, transfer: PendingMarginTransfer) {
    let mut transfer = transfer;
    transfer.rearm_schema_version = 1;
    crate::storage::record_pending_payout_event(&PendingPayoutEvent::Queued {
        kind: transfer.payout_kind,
        operation_id: transfer.operation_id,
        transfer,
        timestamp: Some(now()),
    });
    state.insert_pending_payout(transfer);
}

pub fn record_three_usd_reserve_payout_prepared(
    state: &mut State,
    key: crate::state::ThreeUsdReserveIngressKey,
    payout: crate::state::ThreeUsdReserveCollateralPayout,
) -> bool {
    if payout.rearmed_attempts.len() > crate::state::MAX_THREE_USD_RESERVE_PAYOUT_ATTEMPTS
        || state.three_usd_reserve_collateral_payouts.contains_key(&key)
        || state.three_usd_reserve_payout_operation_keys.contains_key(&payout.operation_id)
    {
        return false;
    }
    let expected = payout.clone();
    let operation_id = payout.operation_id;
    let event = PendingPayoutEvent::ThreeUsdReservePayoutPrepared { key: key.clone(), payout };
    crate::storage::record_pending_payout_event(&event);
    apply_pending_payout_event(state, event);
    state.three_usd_reserve_collateral_payouts.get(&key) == Some(&expected)
        && state.three_usd_reserve_payout_operation_keys.get(&operation_id) == Some(&key)
}

pub fn record_three_usd_reserve_payout_candidate(
    state: &mut State,
    key: crate::state::ThreeUsdReserveIngressKey,
    operation_id: u128,
    attempt_nonce: u128,
    block_index: u64,
) -> bool {
    let event = PendingPayoutEvent::ThreeUsdReservePayoutCandidate { key: key.clone(), operation_id, attempt_nonce, block_index };
    crate::storage::record_pending_payout_event(&event);
    apply_pending_payout_event(state, event);
    state.three_usd_reserve_payout_operation_keys.get(&operation_id) == Some(&key)
        && state.three_usd_reserve_collateral_payouts.get(&key).is_some_and(|payout| {
            (payout.op_nonce == attempt_nonce && payout.candidate_block_index == Some(block_index))
                || payout.rearmed_attempts.iter().any(|attempt| {
                    attempt.op_nonce == attempt_nonce && attempt.candidate_block_index == Some(block_index)
                })
        })
}

pub fn record_three_usd_reserve_payout_candidate_scan(
    state: &mut State,
    key: crate::state::ThreeUsdReserveIngressKey,
    operation_id: u128,
    attempt_nonce: u128,
    scan: crate::state::ThreeUsdReservePayoutCandidateScan,
) -> bool {
    let event = PendingPayoutEvent::ThreeUsdReservePayoutCandidateScan {
        key: key.clone(), operation_id, attempt_nonce, scan: scan.clone(),
    };
    crate::storage::record_pending_payout_event(&event);
    apply_pending_payout_event(state, event);
    state.three_usd_reserve_payout_operation_keys.get(&operation_id) == Some(&key)
        && state.three_usd_reserve_payout_candidate_scans.get(&key) == Some(&scan)
}

pub fn record_three_usd_reserve_payout_fee_observed(
    state: &mut State,
    key: crate::state::ThreeUsdReserveIngressKey,
    operation_id: u128,
    attempt_nonce: u128,
    block_index: u64,
    actual_fee_e8s: u64,
) -> bool {
    let event = PendingPayoutEvent::ThreeUsdReservePayoutFeeObserved {
        key: key.clone(), operation_id, attempt_nonce, block_index, actual_fee_e8s,
    };
    crate::storage::record_pending_payout_event(&event);
    apply_pending_payout_event(state, event);
    state.three_usd_reserve_payout_operation_keys.get(&operation_id) == Some(&key)
        && state.three_usd_reserve_collateral_payouts.get(&key).is_some_and(|payout| {
            (payout.op_nonce == attempt_nonce
                && payout.candidate_block_index == Some(block_index)
                && payout.observed_fee_e8s == Some(actual_fee_e8s))
                || payout.rearmed_attempts.iter().any(|attempt| {
                    attempt.op_nonce == attempt_nonce
                        && attempt.candidate_block_index == Some(block_index)
                        && attempt.observed_fee_e8s == Some(actual_fee_e8s)
                })
        })
}

/// Journal the immutable redemption dispatch tuple before payout processing
/// can make its first ledger await. Public event replay alone never enables
/// automatic rearm because historical events lack this ordering proof.
pub fn record_pending_redemption(
    state: &mut State,
    block_index: u64,
    mut transfer: PendingMarginTransfer,
) {
    assert_eq!(transfer.payout_kind, PendingPayoutKind::Redemption);
    assert!(redemption_receipt_supports_rearm(
        Some(transfer.margin.to_u64()),
        Some(transfer.operation_id),
        Some(transfer.op_nonce),
        transfer.ledger,
        transfer.transfer_amount_raw,
    ));
    transfer.rearm_schema_version = 1;
    crate::storage::record_pending_payout_event(&PendingPayoutEvent::Queued {
        kind: transfer.payout_kind,
        operation_id: transfer.operation_id,
        transfer,
        // The stable journal's event count provides the ordering guarantee;
        // wall-clock time is not needed for the internal marker.
        timestamp: None,
    });
    state.insert_pending_redemption(block_index, transfer);
}

pub fn record_pending_payout_dispatch_boundary(
    state: &mut State,
    operation_id: u128,
    attempt_nonce: u128,
    ledger: Principal,
    owner: Principal,
    amount_raw: u64,
    start_index: Option<u64>,
) -> bool {
    let Some((_, transfer)) = state.get_pending_payout(operation_id) else {
        return false;
    };
    if transfer.rearm_schema_version != 1
        || transfer.op_nonce != attempt_nonce
        || transfer.ledger != Some(ledger)
        || transfer.owner != owner
        || transfer.transfer_amount_raw != Some(amount_raw)
        || !transfer.in_flight
        || transfer.history_start_index.is_some()
    {
        return false;
    }
    crate::storage::record_pending_payout_event(&PendingPayoutEvent::DispatchBoundary {
        operation_id,
        attempt_nonce,
        payout_kind: Some(transfer.payout_kind),
        ledger,
        owner,
        amount_raw,
        start_index,
        timestamp: Some(now()),
    });
    state.mutate_pending_payout(operation_id, |row| {
        if let Some(index) = start_index {
            row.history_start_index = Some(index);
        } else {
            row.rearm_schema_version = 0;
        }
    });
    true
}

pub fn record_pending_payout_too_old(
    state: &mut State,
    operation_id: u128,
    attempt_nonce: u128,
) -> bool {
    let Some((_, transfer)) = state.get_pending_payout(operation_id) else {
        return false;
    };
    if transfer.op_nonce != attempt_nonce || !transfer.in_flight {
        return false;
    }
    crate::storage::record_pending_payout_event(&PendingPayoutEvent::TooOld {
        operation_id,
        attempt_nonce,
        owner: transfer.owner,
        timestamp: Some(now()),
    });
    state.mutate_pending_payout(operation_id, |row| {
        row.in_flight = false;
        row.held_for_manual_retry = true;
        row.reconciliation_required = true;
        row.too_old_confirmed = true;
    });
    true
}

pub fn record_pending_payout_ambiguous_outcome(
    state: &mut State,
    operation_id: u128,
    attempt_nonce: u128,
) -> bool {
    let Some((_, transfer)) = state.get_pending_payout(operation_id) else {
        return false;
    };
    if transfer.op_nonce != attempt_nonce || !transfer.in_flight {
        return false;
    }
    crate::storage::record_pending_payout_event(&PendingPayoutEvent::AmbiguousOutcome {
        operation_id,
        attempt_nonce,
        owner: transfer.owner,
        timestamp: Some(now()),
    });
    state.mutate_pending_payout(operation_id, |row| {
        if row.op_nonce == attempt_nonce && row.owner == transfer.owner {
            row.in_flight = false;
            row.held_for_manual_retry = true;
            row.reconciliation_required = true;
        }
    });
    true
}

pub fn record_pending_payout_rearmed(
    state: &mut State,
    proof: PendingPayoutNoEffectProof,
) -> bool {
    let Some((_, transfer)) = state.get_pending_payout(proof.operation_id) else {
        return false;
    };
    if transfer.rearm_schema_version != 1
        || !transfer.too_old_confirmed
        || !transfer.held_for_manual_retry
        || !transfer.reconciliation_required
        || transfer.in_flight
        || transfer.ledger != Some(proof.ledger)
        || transfer.owner != proof.owner
        || transfer.transfer_amount_raw != Some(proof.amount_raw)
        || transfer.op_nonce != proof.old_attempt_nonce
        || transfer.payout_kind != proof.payout_kind
        || transfer.history_start_index != Some(proof.start_index)
        || transfer.history_scan.as_ref().is_none_or(|scan| {
            scan.operation_id != proof.operation_id
                || scan.payout_kind != proof.payout_kind
                || scan.ledger != proof.ledger
                || scan.owner != proof.owner
                || scan.amount_raw != proof.amount_raw
                || scan.attempt_nonce != proof.old_attempt_nonce
                || scan.start_index != proof.start_index
                || scan.snapshot_log_length != proof.snapshot_log_length
                || scan.next_index != proof.snapshot_log_length
        })
        || transfer.retry_count >= crate::MAX_PENDING_RETRIES
        || proof.new_attempt_nonce == 0
        || proof.new_attempt_nonce == proof.old_attempt_nonce
        || proof.operation_id != transfer.operation_id
        || proof.start_index > proof.snapshot_log_length
        || !proof.complete_prefix
        || transfer.no_effect_proof.is_some()
    {
        return false;
    }
    if let Some(key) = state.three_usd_reserve_payout_operation_keys.get(&proof.operation_id) {
        let Some(payout) = state.three_usd_reserve_collateral_payouts.get(key) else {
            return false;
        };
        if payout.rearmed_attempts.len() >= crate::state::MAX_THREE_USD_RESERVE_PAYOUT_ATTEMPTS
            || payout.op_nonce == proof.new_attempt_nonce
            || payout.candidate_block_index.is_some()
            || payout.rearmed_attempts.iter().any(|attempt| {
                attempt.op_nonce == proof.new_attempt_nonce || attempt.candidate_block_index.is_some()
            })
            || payout.ledger != proof.ledger
            || payout.destination.owner != proof.owner
            || payout.net_e8s != proof.amount_raw
        {
            return false;
        }
    }
    let event = PendingPayoutEvent::Rearmed {
        operation_id: proof.operation_id,
        attempt_nonce: proof.new_attempt_nonce,
        proof: Some(proof.clone()),
        timestamp: Some(proof.verified_at_ns),
        owner: Some(proof.owner),
    };
    crate::storage::record_pending_payout_event(&event);
    apply_pending_payout_event(state, event);
    let Some((_, rearmed)) = state.get_pending_payout(proof.operation_id) else {
        return false;
    };
    rearmed.op_nonce == proof.new_attempt_nonce
        && rearmed.no_effect_proof == Some(proof)
        && state.three_usd_reserve_payout_operation_keys.get(&proof.operation_id)
            .and_then(|key| state.three_usd_reserve_collateral_payouts.get(key))
            .is_none_or(|payout| payout.rearmed_attempts.iter().any(|attempt| {
                attempt.op_nonce == proof.new_attempt_nonce
                    && attempt.candidate_block_index.is_none()
                    && attempt.observed_fee_e8s.is_none()
            }))
}

// ─── Wave-8e LIQ-005: deficit-account event recorders ───

/// Record a `DeficitAccrued` event and increment `protocol_deficit_icusd`.
/// Caller is responsible for invoking `state.check_deficit_readonly_latch()`
/// afterwards if the latch threshold is configured.
///
/// Wave-9 RED-002: takes a `DeficitSource` so liquidation and redemption
/// deficits are distinguishable in the event log. The legacy `vault_id`
/// field on the event is preserved for back-compat and continues to be
/// populated for liquidation; redemption deficits set `vault_id = 0` on
/// the parent event because the cr-walk touches multiple vaults.
pub fn record_deficit_accrued(
    state: &mut State,
    source: DeficitSource,
    amount: ICUSD,
    timestamp: u64,
) {
    record_deficit_accrued_with(state, source, amount, timestamp, &mut record_event);
}

fn record_deficit_accrued_with(
    state: &mut State,
    source: DeficitSource,
    amount: ICUSD,
    timestamp: u64,
    persist_event: &mut impl FnMut(&Event),
) {
    state.accrue_deficit_shortfall(amount);
    let vault_id = match source {
        DeficitSource::Liquidation { vault_id } => vault_id,
        DeficitSource::Redemption { .. } => 0,
    };
    persist_event(&Event::DeficitAccrued {
        vault_id,
        amount,
        new_deficit: state.protocol_deficit_icusd,
        timestamp,
        source: Some(source),
    });
}

/// Record a `DeficitRepaid` event and apply the repayment to state.
pub fn record_deficit_repaid(
    state: &mut State,
    amount: ICUSD,
    source: FeeSource,
    anchor_block_index: Option<u64>,
    timestamp: u64,
) {
    state.apply_deficit_repayment(amount);
    record_event(&Event::DeficitRepaid {
        amount,
        source,
        remaining_deficit: state.protocol_deficit_icusd,
        anchor_block_index,
        timestamp,
    });
}

/// Admin: tune the per-fee fraction routed to deficit repayment.
pub fn record_set_deficit_repayment_fraction(state: &mut State, fraction: Ratio) {
    state.deficit_repayment_fraction = fraction;
    record_event(&Event::SetDeficitRepaymentFraction {
        fraction,
        timestamp: now(),
    });
}

/// Admin: set the deficit-driven ReadOnly auto-latch threshold (0 disables).
pub fn record_set_deficit_readonly_threshold_e8s(state: &mut State, threshold_e8s: u64) {
    state.deficit_readonly_threshold_e8s = threshold_e8s;
    record_event(&Event::SetDeficitReadonlyThresholdE8s {
        threshold_e8s,
        timestamp: now(),
    });
}

/// Wave-10 LIQ-008: production wrapper called from each vault.rs liquidation
/// site. Delegates the rolling-window state mutation to
/// `state::record_recent_liquidation`; if that returns `true` (latch just
/// flipped), logs the trip and emits a `BreakerTripped` event so the
/// explorer audit trail captures it. No-op when the breaker is disabled or
/// already tripped — vault.rs sites can call this unconditionally.
pub fn record_liquidation_for_breaker(state: &mut State, debt_e8s: u64) {
    let now_ns = now();
    let just_tripped = crate::state::record_recent_liquidation(state, debt_e8s, now_ns);
    if just_tripped {
        let total = state.windowed_liquidation_total(now_ns);
        let ceiling = state.breaker_window_debt_ceiling_e8s;
        ic_canister_log::log!(
            crate::INFO,
            "[LIQ-008] circuit breaker tripped: windowed total {} e8s >= ceiling {} e8s (window {} ns, log size {})",
            total,
            ceiling,
            state.breaker_window_ns,
            state.recent_liquidations.len()
        );
        record_event(&Event::BreakerTripped {
            total_e8s: total,
            ceiling_e8s: ceiling,
            timestamp: now_ns,
        });
    }
}

/// Wave-10 LIQ-008: admin clears the breaker latch and resumes auto-publishing.
/// Records `BreakerCleared` with the windowed total at the moment of clearing
/// so the audit trail captures what state the operator was looking at when
/// they decided to resume.
pub fn record_breaker_cleared(state: &mut State, remaining_total_e8s: u64) {
    state.liquidation_breaker_tripped = false;
    record_event(&Event::BreakerCleared {
        remaining_total_e8s,
        timestamp: now(),
    });
}

/// Wave-10 LIQ-008: admin tunes the rolling-window length. 0 disables the breaker.
pub fn record_set_breaker_window_ns(state: &mut State, window_ns: u64) {
    state.breaker_window_ns = window_ns;
    record_event(&Event::SetBreakerWindowNs {
        window_ns,
        timestamp: now(),
    });
}

/// Wave-10 LIQ-008: admin tunes the cumulative-debt ceiling. 0 disables tripping.
pub fn record_set_breaker_window_debt_ceiling_e8s(state: &mut State, ceiling_e8s: u64) {
    state.breaker_window_debt_ceiling_e8s = ceiling_e8s;
    record_event(&Event::SetBreakerWindowDebtCeilingE8s {
        ceiling_e8s,
        timestamp: now(),
    });
}

/// Wave-11 BOT-001: records that `check_vaults` skipped an auto-cancel of an
/// expired `bot_claims` entry because the bot had not returned the collateral.
/// The `BotClaim` is intentionally left in place so admin can reconcile via
/// `bot_cancel_liquidation` once the collateral is back; this recorder makes
/// no state mutation. `_state` is taken for consistency with the rest of the
/// recorder API.
pub fn record_bot_claim_reconciliation_needed(
    _state: &mut State,
    vault_id: u64,
    observed_balance: u64,
    required_balance: u64,
) {
    record_event(&Event::BotClaimReconciliationNeeded {
        vault_id,
        observed_balance,
        required_balance,
        timestamp: now(),
    });
}

pub fn record_borrow_from_vault(
    state: &mut State,
    vault_id: u64,
    borrowed_amount: ICUSD,
    fee_amount: ICUSD,
    block_index: u64,
) {
    record_borrow_from_vault_for_caller(
        state,
        vault_id,
        borrowed_amount,
        fee_amount,
        block_index,
        ic_cdk::caller(),
    );
}

pub fn record_borrow_from_vault_for_caller(
    state: &mut State,
    vault_id: u64,
    borrowed_amount: ICUSD,
    fee_amount: ICUSD,
    block_index: u64,
    caller: Principal,
) {
    record_borrow_from_vault_at(
        state,
        vault_id,
        borrowed_amount,
        fee_amount,
        block_index,
        caller,
        ic_cdk::api::time(),
    );
}

pub fn record_borrow_from_vault_at(
    state: &mut State,
    vault_id: u64,
    borrowed_amount: ICUSD,
    fee_amount: ICUSD,
    block_index: u64,
    caller: Principal,
    timestamp_ns: u64,
) {
    record_event_at(
        &Event::BorrowFromVault {
            vault_id,
            block_index,
            fee_amount,
            borrowed_amount,
            caller: Some(caller),
            timestamp: Some(timestamp_ns),
        },
        timestamp_ns,
    );
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
) -> Result<(), crate::ProtocolError> {
    // Validate and apply first. If the callback observes an unexpected state
    // change, return cleanly without publishing an event that replay would
    // apply differently. The caller holds the per-vault guard across the pull.
    state.try_add_margin_to_vault(vault_id, margin_added)?;
    record_event(&Event::AddMarginToVault {
        vault_id,
        margin_added,
        block_index,
        caller: Some(ic_cdk::caller()),
        timestamp: Some(now()),
    });
    Ok(())
}

/// Outcome of a redemption's vault water-fill, returned to the caller so the
/// payout/refund accounting stays consistent with what the fill actually did.
pub struct RedemptionOutcome {
    /// icUSD actually retired against vault debt (sum of per-vault shares).
    pub consumed: ICUSD,
    /// Collateral payout queued for the redeemer.
    pub margin: ICP,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RedemptionRecordError {
    PayoutUnrepresentable,
    MinimumNotMet {
        minimum_net_raw: u64,
        actual_net_raw: u64,
    },
}

pub fn record_redemption_on_vault_run(
    state: &mut State,
    owner: Principal,
    icusd_amount: ICUSD,
    fee_amount: ICUSD,
    collateral_price: UsdIcp,
    icusd_block_index: u64,
    redeem_ct: Principal,
    allowed_vault_ids: &[u64],
    min_net_collateral_raw: Option<u64>,
) -> Result<RedemptionOutcome, RedemptionRecordError> {
    record_redemption_on_vault_run_with(
        state,
        owner,
        icusd_amount,
        fee_amount,
        collateral_price,
        icusd_block_index,
        redeem_ct,
        allowed_vault_ids,
        min_net_collateral_raw,
        || now(),
        record_event,
    )
}

/// Deterministic test seam that exercises the production recorder core,
/// including the same state mutation and persisted event path.
#[cfg(test)]
pub(crate) fn record_redemption_on_vault_run_at(
    state: &mut State,
    owner: Principal,
    icusd_amount: ICUSD,
    fee_amount: ICUSD,
    collateral_price: UsdIcp,
    icusd_block_index: u64,
    redeem_ct: Principal,
    allowed_vault_ids: &[u64],
    min_net_collateral_raw: Option<u64>,
    timestamp: u64,
    persisted_events: &mut Vec<Event>,
) -> Result<RedemptionOutcome, RedemptionRecordError> {
    record_redemption_on_vault_run_with(
        state,
        owner,
        icusd_amount,
        fee_amount,
        collateral_price,
        icusd_block_index,
        redeem_ct,
        allowed_vault_ids,
        min_net_collateral_raw,
        || timestamp,
        |event| persisted_events.push(event.clone()),
    )
}

fn record_redemption_on_vault_run_with(
    state: &mut State,
    owner: Principal,
    icusd_amount: ICUSD,
    fee_amount: ICUSD,
    collateral_price: UsdIcp,
    icusd_block_index: u64,
    redeem_ct: Principal,
    allowed_vault_ids: &[u64],
    min_net_collateral_raw: Option<u64>,
    timestamp: impl FnOnce() -> u64,
    mut persist_event: impl FnMut(&Event),
) -> Result<RedemptionOutcome, RedemptionRecordError> {
    // Fee is already deducted from icusd_amount before calling redeem_on_vaults,
    // so vault owners effectively keep the fee (less collateral seized for their debt).
    // The fee portion of icUSD stays in the protocol canister (burned).
    //
    // RED-002 (audit 2026-06-09): `redeem_ct` (the redemption-priority winner)
    // is now resolved by the caller BEFORE pulling icUSD, so freshness, fee
    // pricing, and the base-rate bump all key on the collateral actually
    // seized rather than the caller-supplied type.

    // Use the selected collateral type's price for both water-filling and
    // pending transfer amount calculation. The caller's collateral_price
    // parameter may be for a different collateral type.
    let ct_price = state
        .get_collateral_config(&redeem_ct)
        .and_then(|c| c.last_price)
        .and_then(rust_decimal::Decimal::from_f64_retain)
        .map(UsdIcp::from)
        .unwrap_or(collateral_price); // fallback to parameter if no config price

    // Wave-9 RED-002: snapshot price + decimals before mutation so the
    // shortfall calc below uses the same oracle figures the water-fill
    // walked. `redeem_on_vaults` mutates state but doesn't touch
    // `collateral_configs`, so reading after is also safe — we read
    // here to keep the data flow explicit.
    let (price_decimal, decimals) = state
        .get_collateral_config(&redeem_ct)
        .map(|c| {
            let p = c
                .last_price
                .and_then(rust_decimal::Decimal::from_f64_retain)
                .unwrap_or(ct_price.0);
            (p, c.decimals)
        })
        .unwrap_or((ct_price.0, 8));

    // Preflight the exact selected-ID execution without mutating state. This
    // checked total is required before any debt, collateral, fee, event, or
    // pending-transfer mutation so an unrepresentable payout can be refunded.
    let simulated = state.simulate_redemption_for_vault_ids(
        icusd_amount,
        ct_price,
        &redeem_ct,
        allowed_vault_ids,
    ).ok_or(RedemptionRecordError::PayoutUnrepresentable)?;
    let payout_collateral_raw = total_actual_collateral_seized(&simulated)
        .ok_or(RedemptionRecordError::PayoutUnrepresentable)?;
    if let Some(minimum_net_raw) = min_net_collateral_raw {
        let ledger_fee = state
            .get_collateral_config(&redeem_ct)
            .map(|config| config.ledger_fee)
            .unwrap_or(0);
        let actual_net_raw = payout_collateral_raw.saturating_sub(ledger_fee);
        if actual_net_raw < minimum_net_raw {
            return Err(RedemptionRecordError::MinimumNotMet {
                minimum_net_raw,
                actual_net_raw,
            });
        }
    }

    let event_timestamp = timestamp();
    let vault_redemptions =
        state.redeem_on_vaults_for_vault_ids(icusd_amount, ct_price, &redeem_ct, allowed_vault_ids);
    let actual_payout_raw = total_actual_collateral_seized(&vault_redemptions).expect(
        "simulated redemption payout must remain representable during synchronous execution",
    );
    debug_assert_eq!(actual_payout_raw, payout_collateral_raw);
    let (payout_ledger, payout_fee) = state
        .get_collateral_config(&redeem_ct)
        .map(|config| (Some(config.ledger_canister_id), config.ledger_fee))
        .unwrap_or((
            Some(state.icp_ledger_principal),
            state.icp_ledger_fee.to_u64(),
        ));
    let payout_transfer_amount_raw = actual_payout_raw.checked_sub(payout_fee);
    let payout_attempt_nonce = if actual_payout_raw > 0 {
        let mut attempt = state.next_op_nonce_at(event_timestamp);
        while state.pending_payout_index.contains_key(&attempt) {
            attempt = state.next_op_nonce_at(event_timestamp);
        }
        Some(attempt)
    } else {
        None
    };
    let payout_operation_id = payout_attempt_nonce;
    persist_event(&Event::RedemptionOnVaults {
        owner,
        current_icp_rate: ct_price,
        icusd_amount,
        fee_amount,
        icusd_block_index,
        collateral_type: Some(redeem_ct),
        timestamp: Some(event_timestamp),
        // Some(empty) distinguishes a new no-op from legacy events whose
        // missing outcome vector requires historical full-type replay.
        vault_redemptions: Some(vault_redemptions.clone()),
        payout_collateral_raw: Some(actual_payout_raw),
        payout_operation_id,
        payout_attempt_nonce,
        payout_ledger,
        payout_transfer_amount_raw,
        min_net_collateral_raw,
    });

    // RED-001 (audit 2026-06-09): the payout is derived from the icUSD the
    // water-fill ACTUALLY retired, never from the requested claim. When the
    // fill exhausts eligible vaults early, the unconsumed remainder is
    // refunded by the caller (`redeem_collateral`) instead of being paid out
    // in collateral that no vault was debited for (which drained co-collateral
    // vaults' shared backing). The deficit accrual target shrinks accordingly:
    // only the consumed-but-undercollateralized gap (underwater vaults) is
    // genuine bad debt; the unconsumed remainder is not a deficit once it is
    // refunded.
    let consumed = ICUSD::from(
        vault_redemptions
            .iter()
            .map(|v| v.icusd_redeemed_e8s)
            .sum::<u64>(),
    );

    // Wave-9 RED-002: route any redemption-side shortfall into the
    // Wave-8e deficit account. The pure helper takes an explicit
    // timestamp so unit tests can exercise the predicate without an
    // `ic_cdk::api::time()` panic — production callers pass `now()`.
    let _shortfall = accrue_redemption_shortfall_with(
        state,
        owner,
        consumed,
        &vault_redemptions,
        price_decimal,
        decimals,
        event_timestamp,
        &mut persist_event,
    );

    let margin = ICP::from(actual_payout_raw);
    if margin.to_u64() > 0 {
        let op_nonce = payout_attempt_nonce.expect("nonzero payout has a stable attempt nonce");
        let complete_receipt = redemption_receipt_supports_rearm(
            Some(actual_payout_raw),
            payout_operation_id,
            payout_attempt_nonce,
            payout_ledger,
            payout_transfer_amount_raw,
        );
        let transfer = PendingMarginTransfer {
            vault_id: 0,
            operation_id: payout_operation_id.expect("nonzero payout has an operation identity"),
            payout_kind: PendingPayoutKind::Redemption,
            owner,
            margin,
            collateral_type: redeem_ct,
            retry_count: 0,
            op_nonce,
            ledger: payout_ledger,
            transfer_amount_raw: payout_transfer_amount_raw,
            held_for_manual_retry: !complete_receipt,
            reconciliation_required: !complete_receipt,
            in_flight: false,
            too_old_confirmed: false,
            history_start_index: None,
            rearm_schema_version: 0,
            history_scan: None,
            history_candidate_seen: false,
            no_effect_proof: None,
            history_log_length: None,
            history_cursor: 0,
            min_net_collateral_raw,
        };
        if complete_receipt {
            record_pending_redemption(state, icusd_block_index, transfer);
        } else {
            state.insert_pending_redemption(icusd_block_index, transfer);
        }
    }
    Ok(RedemptionOutcome { consumed, margin })
}

fn total_actual_collateral_seized(vault_redemptions: &[VaultRedemption]) -> Option<u64> {
    let total = vault_redemptions
        .iter()
        .try_fold(0u128, |total, redemption| {
            total.checked_add(redemption.collateral_seized as u128)
        })?;
    u64::try_from(total).ok()
}

/// A redemption receipt is eligible for proof-gated rearm only when the event
/// pins the exact gross/net payout, ledger, and stable operation/attempt
/// identity before any ledger await. Legacy or partial receipts remain held.
fn redemption_receipt_supports_rearm(
    gross_amount_raw: Option<u64>,
    operation_id: Option<u128>,
    attempt_nonce: Option<u128>,
    ledger: Option<Principal>,
    transfer_amount_raw: Option<u64>,
) -> bool {
    matches!(
        (gross_amount_raw, operation_id, attempt_nonce, ledger, transfer_amount_raw),
        (Some(gross), Some(operation), Some(attempt), Some(_), Some(net))
            if gross > 0 && operation > 0 && attempt > 0 && operation == attempt && net > 0
    )
}

/// Wave-9 RED-002: pure-math predicate for the redemption shortfall.
/// Returns `target - sum(actual_collateral_seized) * price` clamped at
/// zero, the metric the audit asked for. Pure (no state mutation, no
/// event recording, no canister-clock read), so unit tests can drive
/// it directly off `state.redeem_on_vaults` output.
///
/// Two silent-shortfall modes are unified by this metric:
///
///   * **Mode 1** — vault debt cap fires (water-fill exhausts available
///     vaults before consuming the full redemption claim);
///     `collateral_seized` totals to less than `icusd_amount / price`.
///   * **Mode 2** — underwater vault (saturating-sub on
///     `vault.collateral_amount` clips an attempted deduction). The
///     `VaultRedemption.collateral_seized` field is now authoritative
///     for the post-saturation actual amount (pre-Wave-9 it was the
///     *requested* amount and silently over-reported).
pub fn compute_redemption_shortfall(
    target_icusd: ICUSD,
    vault_redemptions: &[VaultRedemption],
    price_decimal: rust_decimal::Decimal,
    decimals: u8,
) -> ICUSD {
    let total_collateral_seized: u64 = vault_redemptions.iter().map(|v| v.collateral_seized).sum();
    let value_seized_at_oracle =
        crate::numeric::collateral_usd_value(total_collateral_seized, price_decimal, decimals);
    target_icusd.saturating_sub(value_seized_at_oracle)
}

/// Wave-9 RED-002: predicate + accrual + auto-latch check for redemption
/// shortfalls. Calls `compute_redemption_shortfall` for the math, then
/// (if non-zero) routes the shortfall through the same accrual helper
/// that liquidation paths use (LIQ-005). Mirrors the LIQ-005
/// liquidation accrual predicate inside `vault.rs::liquidate_vault`.
///
/// Pure with respect to the canister clock — callers pass the timestamp
/// explicitly. Returns the shortfall accrued (zero when the redemption
/// was solvent at oracle price).
pub fn accrue_redemption_shortfall_at(
    state: &mut State,
    redeemer: Principal,
    target_icusd: ICUSD,
    vault_redemptions: &[VaultRedemption],
    price_decimal: rust_decimal::Decimal,
    decimals: u8,
    timestamp: u64,
) -> ICUSD {
    accrue_redemption_shortfall_with(
        state,
        redeemer,
        target_icusd,
        vault_redemptions,
        price_decimal,
        decimals,
        timestamp,
        &mut record_event,
    )
}

fn accrue_redemption_shortfall_with(
    state: &mut State,
    redeemer: Principal,
    target_icusd: ICUSD,
    vault_redemptions: &[VaultRedemption],
    price_decimal: rust_decimal::Decimal,
    decimals: u8,
    timestamp: u64,
    persist_event: &mut impl FnMut(&Event),
) -> ICUSD {
    let shortfall =
        compute_redemption_shortfall(target_icusd, vault_redemptions, price_decimal, decimals);
    if shortfall.0 > 0 {
        record_deficit_accrued_with(
            state,
            DeficitSource::Redemption { redeemer },
            shortfall,
            timestamp,
            persist_event,
        );
        if state.check_deficit_readonly_latch() {
            ic_canister_log::log!(
                crate::logs::INFO,
                "[RED-002] deficit threshold {} crossed by redemption (redeemer {}) shortfall {}; auto-latched ReadOnly",
                state.deficit_readonly_threshold_e8s,
                redeemer,
                shortfall.to_u64()
            );
        }
    }
    shortfall
}

pub fn record_redemption_transfered(
    state: &mut State,
    icusd_block_index: u64,
    operation_id: u128,
    icp_block_index: u64,
) {
    record_event(&Event::RedemptionTransfered {
        icusd_block_index,
        icp_block_index,
        operation_id: Some(operation_id),
        timestamp: Some(now()),
    });
    state.remove_pending_payout(operation_id);
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
    block_index: Option<u64>,
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

/// LIQ-0XX: admin set the global dust-liquidation threshold. See
/// `State::dust_liquidation_threshold`.
pub fn record_set_dust_liquidation_threshold(state: &mut State, amount: ICUSD) {
    record_event(&Event::SetDustLiquidationThreshold {
        amount: amount.to_u64().to_string(),
    });
    state.dust_liquidation_threshold = amount;
}

pub fn record_set_global_icusd_mint_cap(state: &mut State, amount: u64) {
    record_event(&Event::SetGlobalIcusdMintCap {
        amount: Some(amount.to_string()),
        cap: None,
    });
    state.global_icusd_mint_cap = amount;
}

pub fn record_set_stable_token_enabled(
    state: &mut State,
    token_type: StableTokenType,
    enabled: bool,
) {
    record_event(&Event::SetStableTokenEnabled {
        token_type: token_type.clone(),
        enabled,
    });
    match token_type {
        StableTokenType::CKUSDT => state.ckusdt_enabled = enabled,
        StableTokenType::CKUSDC => state.ckusdc_enabled = enabled,
    }
}

pub fn record_set_stable_ledger_principal(
    state: &mut State,
    token_type: StableTokenType,
    principal: Principal,
) {
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
    record_event(&Event::SetBotBudget {
        total_e8s,
        start_timestamp,
    });
    state.bot_budget_total_e8s = total_e8s;
    state.bot_budget_remaining_e8s = total_e8s;
    state.bot_budget_start_timestamp = start_timestamp;
}

pub fn record_set_bot_allowed_collateral_types(
    state: &mut State,
    collateral_types: Vec<Principal>,
) {
    record_event(&Event::SetBotAllowedCollateralTypes {
        collateral_types: collateral_types.clone(),
    });
    state.bot_allowed_collateral_types = collateral_types.into_iter().collect();
}

pub fn record_set_bot_cr_tolerance_bps(state: &mut State, bps: u64) {
    record_event(&Event::SetBotCrToleranceBps { bps });
    state.bot_cr_tolerance_bps = bps;
}

pub fn record_bot_proof_mode_enabled(state: &mut State) {
    if !state.bot_confirm_proof_required {
        crate::storage::record_bot_proof_audit_event(BotProofAuditEvent::ProofModeEnabled);
        state.bot_confirm_proof_required = true;
    }
}

pub fn record_bot_claim_generation_reserved(state: &mut State, generation: u64) {
    debug_assert!(generation > state.bot_claim_generation_counter);
    crate::storage::record_bot_proof_audit_event(BotProofAuditEvent::ClaimGenerationReserved {
        generation,
    });
    state.bot_claim_generation_counter = state.bot_claim_generation_counter.max(generation);
}

pub fn record_bot_payment_proof_consumed(
    state: &mut State,
    ledger_principal: Principal,
    block_index: u64,
    vault_id: u64,
    claim_generation: u64,
) {
    crate::storage::record_bot_proof_audit_event(BotProofAuditEvent::PaymentProofConsumed {
        ledger_principal,
        block_index,
        vault_id,
        claim_generation,
    });
    state.consumed_bot_payment_proofs.insert(
        format!("{}:{block_index}", ledger_principal.to_text()),
        (vault_id, claim_generation),
    );
}

/// Wave-14a CDP-14 follow-up: record + apply a per-collateral override
/// for the XRC source-count floor. Used for collaterals whose underlying
/// asset has genuinely thin CEX coverage on XRC. Pass `None` to clear
/// the override and inherit the global floor again.
pub fn record_set_collateral_min_xrc_sources(
    state: &mut State,
    collateral_type: CollateralType,
    min_xrc_sources: Option<u32>,
) {
    record_event(&Event::SetCollateralMinXrcSources {
        collateral_type,
        min_xrc_sources,
    });
    if let Some(config) = state.collateral_configs.get_mut(&collateral_type) {
        config.min_xrc_sources = min_xrc_sources;
    }
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
    record_event(&Event::SetRmrFloor {
        value: value.0.to_string(),
    });
    state.rmr_floor = value;
}

pub fn record_set_rmr_ceiling(state: &mut State, value: Ratio) {
    record_event(&Event::SetRmrCeiling {
        value: value.0.to_string(),
    });
    state.rmr_ceiling = value;
}

pub fn record_set_rmr_floor_cr(state: &mut State, value: Ratio) {
    record_event(&Event::SetRmrFloorCr {
        value: value.0.to_string(),
    });
    state.rmr_floor_cr = value;
}

pub fn record_set_rmr_ceiling_cr(state: &mut State, value: Ratio) {
    record_event(&Event::SetRmrCeilingCr {
        value: value.0.to_string(),
    });
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

pub fn record_admin_mint(amount: ICUSD, to: Principal, reason: String, block_index: u64) {
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
    use crate::state::{InterpolationMethod, RateCurve, RateMarker};
    let serialized: Vec<(String, String)> = markers
        .iter()
        .map(|(cr, mult)| (cr.to_string(), mult.to_string()))
        .collect();
    let markers_json = serde_json::to_string(&serialized).unwrap_or_default();
    record_event(&Event::SetRateCurveMarkers {
        collateral_type: collateral_type.map(|ct| ct.to_text()),
        markers: markers_json,
    });
    let parsed: Vec<RateMarker> = markers
        .iter()
        .map(|(cr, mult)| RateMarker {
            cr_level: Ratio::from_f64(*cr),
            multiplier: Ratio::from_f64(*mult),
        })
        .collect();
    let curve = RateCurve {
        markers: parsed,
        method: InterpolationMethod::Linear,
    };
    match collateral_type {
        None => {
            state.global_rate_curve = curve;
        }
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
    let serialized: Vec<(String, String)> = markers
        .iter()
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
    state.recovery_rate_curve = markers
        .iter()
        .map(|(thresh, mult)| RecoveryRateMarker {
            threshold: thresh.clone(),
            multiplier: Ratio::from_f64(*mult),
        })
        .collect();
}

pub fn record_set_borrowing_fee_curve(state: &mut State, curve: Option<RateCurveV2>) {
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

pub fn record_set_amm1_canister(state: &mut State, canister: Principal) {
    record_event(&Event::SetAmm1Canister { canister });
    state.amm1_canister = Some(canister);
}

pub fn record_set_amm1_pool_id(state: &mut State, pool_id: String) {
    record_event(&Event::SetAmm1PoolId {
        pool_id: pool_id.clone(),
    });
    state.amm1_pool_id = Some(pool_id);
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
    record_event(&Event::AccrueInterest {
        timestamp: now_nanos,
    });
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

    #[test]
    fn legacy_partial_liquidation_event_defaults_missing_ledger_fee_to_none() {
        let event = Event::PartialLiquidateVault {
            vault_id: 1,
            liquidator_payment: ICUSD::new(100),
            icp_to_liquidator: ICP::new(90),
            liquidator: None,
            icp_rate: None,
            protocol_fee_collateral: None,
            ledger_fee_collateral: None,
            timestamp: None,
            three_usd_reserves_e8s: None,
        };
        let mut old_value =
            serde_json::to_value(event).expect("serialize partial liquidation event");
        let event_record = old_value
            .as_object_mut()
            .expect("event serializes as a tagged record")
            .values_mut()
            .next()
            .expect("event variant payload");
        event_record
            .as_object_mut()
            .expect("event payload serializes as a record")
            .remove("ledger_fee_collateral");

        let decoded: Event = serde_json::from_value(old_value).expect("decode legacy event");
        assert!(matches!(
            decoded,
            Event::PartialLiquidateVault {
                ledger_fee_collateral: None,
                ..
            }
        ));
    }

    fn p(seed: u8) -> Principal {
        Principal::self_authenticating([seed; 32])
    }

    fn caller_a() -> Principal {
        p(1)
    }
    fn caller_b() -> Principal {
        p(2)
    }
    fn icp_token() -> Principal {
        p(10)
    }
    fn ckbtc_token() -> Principal {
        p(11)
    }

    fn payout_init_args(icp: Principal) -> InitArg {
        InitArg {
            xrc_principal: p(20),
            icusd_ledger_principal: p(21),
            icp_ledger_principal: icp,
            fee_e8s: 0,
            developer_principal: p(22),
            treasury_principal: None,
            stability_pool_principal: None,
            ckusdt_ledger_principal: None,
            ckusdc_ledger_principal: None,
        }
    }

    fn replay_payout_events(
        events: Vec<Event>,
        payout_events: Vec<PendingPayoutEvent>,
    ) -> State {
        let journal = payout_events.into_iter().map(|event| {
            crate::storage::PendingPayoutJournalEntry {
                after_event_count: 1,
                event,
            }
        });
        replay_with_nonce_time_and_payout_events(events.into_iter(), journal, || 2)
            .expect("payout replay fixture should be consistent")
    }

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
            repay_amount: None,
        }
    }

    fn accrue_event(ts: u64) -> Event {
        Event::AccrueInterest { timestamp: ts }
    }

    fn price_event(ct: Principal, ts: u64) -> Event {
        Event::PriceUpdate {
            collateral_type: ct,
            price: "5.0".into(),
            timestamp: ts,
        }
    }

    /// Build a vault_id → collateral_type lookup for tests.
    fn lookup(entries: &[(u64, Principal)]) -> HashMap<u64, Principal> {
        entries.iter().copied().collect()
    }

    // ── type_filter classification ────────────────────────────────────────

    #[test]
    fn type_filter_classifies_each_user_facing_variant() {
        assert_eq!(
            open_vault_event(1, caller_a(), icp_token()).type_filter(),
            EventTypeFilter::OpenVault
        );
        assert_eq!(
            borrow_event(1, caller_a(), 100, 0).type_filter(),
            EventTypeFilter::Borrow
        );
        assert_eq!(
            repay_event(1, caller_a(), 100, 0).type_filter(),
            EventTypeFilter::Repay
        );
        assert_eq!(
            liquidate_event(1, caller_a(), 0).type_filter(),
            EventTypeFilter::Liquidation
        );
        assert_eq!(
            accrue_event(0).type_filter(),
            EventTypeFilter::AccrueInterest
        );
        assert_eq!(
            price_event(icp_token(), 0).type_filter(),
            EventTypeFilter::PriceUpdate
        );

        // Setter falls into Admin
        let setter = Event::SetBorrowingFee {
            rate: "0.005".into(),
        };
        assert_eq!(setter.type_filter(), EventTypeFilter::Admin);
    }

    // ── timestamp_ns ──────────────────────────────────────────────────────

    #[test]
    fn timestamp_ns_extracts_when_present_and_returns_none_otherwise() {
        assert_eq!(
            borrow_event(1, caller_a(), 100, 12_345).timestamp_ns(),
            Some(12_345)
        );
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

    #[test]
    fn replayed_rearm_requires_proof_and_preserves_retry_budget_and_operation_id() {
        let owner = p(30);
        let ledger = p(31);
        let operation_id = 91;
        let old_nonce = 92;
        let new_nonce = 93;
        let transfer = PendingMarginTransfer {
            vault_id: 7,
            operation_id,
            payout_kind: PendingPayoutKind::Margin,
            owner,
            margin: ICP::new(100),
            collateral_type: ledger,
            retry_count: 9,
            op_nonce: old_nonce,
            ledger: Some(ledger),
            transfer_amount_raw: Some(90),
            held_for_manual_retry: false,
            reconciliation_required: false,
            in_flight: false,
            too_old_confirmed: false,
            history_start_index: None,
            rearm_schema_version: 1,
            history_scan: None,
            history_candidate_seen: false,
            no_effect_proof: None,
            history_log_length: None,
            history_cursor: 0,
            min_net_collateral_raw: None,
        };
        let queued = PendingPayoutEvent::Queued {
            kind: PendingPayoutKind::Margin,
            operation_id,
            transfer,
            timestamp: Some(1),
        };
        let boundary = PendingPayoutEvent::DispatchBoundary {
            operation_id,
            attempt_nonce: old_nonce,
            payout_kind: Some(PendingPayoutKind::Margin),
            ledger,
            owner,
            amount_raw: 90,
            start_index: Some(10),
            timestamp: Some(2),
        };
        let too_old = PendingPayoutEvent::TooOld {
            operation_id,
            attempt_nonce: old_nonce,
            owner,
            timestamp: Some(3),
        };
        let proof = PendingPayoutNoEffectProof {
            operation_id,
            payout_kind: PendingPayoutKind::Margin,
            ledger,
            owner,
            amount_raw: 90,
            old_attempt_nonce: old_nonce,
            new_attempt_nonce: new_nonce,
            start_index: 10,
            snapshot_log_length: 25,
            complete_prefix: true,
            verified_at_ns: 4,
        };
        let rearmed = PendingPayoutEvent::Rearmed {
            operation_id,
            attempt_nonce: new_nonce,
            proof: Some(proof),
            timestamp: Some(4),
            owner: Some(owner),
        };
        let state = replay_payout_events(
            vec![Event::Init(payout_init_args(p(32)))],
            vec![queued.clone(), boundary.clone(), too_old.clone(), rearmed],
        );
        let (_, recovered) = state.get_pending_payout(operation_id).unwrap();
        assert_eq!(recovered.operation_id, operation_id);
        assert_eq!(recovered.op_nonce, new_nonce);
        assert_eq!(recovered.retry_count, 9);
        assert!(!recovered.held_for_manual_retry);
        assert!(recovered.no_effect_proof.is_some());
        assert_eq!(recovered.history_start_index, None);

        let unproved = PendingPayoutEvent::Rearmed {
            operation_id,
            attempt_nonce: new_nonce,
            proof: None,
            timestamp: Some(4),
            owner: Some(owner),
        };
        let held = replay_payout_events(
            vec![Event::Init(payout_init_args(p(32)))],
            vec![queued, boundary, too_old, unproved],
        );
        let (_, held_row) = held.get_pending_payout(operation_id).unwrap();
        assert_eq!(held_row.op_nonce, old_nonce);
        assert!(held_row.held_for_manual_retry);
        assert!(held_row.too_old_confirmed);

        let mut legacy_transfer = transfer;
        legacy_transfer.rearm_schema_version = 0;
        let legacy_queued = PendingPayoutEvent::Queued {
            kind: PendingPayoutKind::Margin,
            operation_id,
            transfer: legacy_transfer,
            timestamp: Some(1),
        };
        let legacy_rearm = PendingPayoutEvent::Rearmed {
            operation_id,
            attempt_nonce: new_nonce,
            proof: None,
            timestamp: Some(4),
            owner: Some(owner),
        };
        let legacy = replay_payout_events(
            vec![Event::Init(payout_init_args(p(32)))],
            vec![legacy_queued, legacy_rearm],
        );
        let (_, legacy_row) = legacy.get_pending_payout(operation_id).unwrap();
        assert_eq!(legacy_row.op_nonce, old_nonce);
        assert!(legacy_row.held_for_manual_retry);
        assert!(legacy_row.reconciliation_required);
    }

    #[test]
    fn ambiguous_payout_hold_survives_private_journal_replay() {
        let owner = p(40);
        let ledger = p(41);
        let operation_id = 101;
        let attempt_nonce = 102;
        let transfer = PendingMarginTransfer {
            vault_id: 7,
            operation_id,
            payout_kind: PendingPayoutKind::Margin,
            owner,
            margin: ICP::new(100),
            collateral_type: ledger,
            retry_count: 0,
            op_nonce: attempt_nonce,
            ledger: Some(ledger),
            transfer_amount_raw: Some(90),
            held_for_manual_retry: false,
            reconciliation_required: false,
            in_flight: false,
            too_old_confirmed: false,
            history_start_index: None,
            rearm_schema_version: 1,
            history_scan: None,
            history_candidate_seen: false,
            no_effect_proof: None,
            history_log_length: None,
            history_cursor: 0,
            min_net_collateral_raw: None,
        };
        let journal_entry = crate::storage::PendingPayoutJournalEntry {
            after_event_count: 1,
            event: PendingPayoutEvent::AmbiguousOutcome {
                operation_id,
                attempt_nonce,
                owner,
                timestamp: Some(3),
            },
        };
        let mut encoded = Vec::new();
        ciborium::ser::into_writer(&journal_entry, &mut encoded)
            .expect("encode ambiguous outcome before upgrade");
        let restored_entry: crate::storage::PendingPayoutJournalEntry =
            ciborium::de::from_reader(encoded.as_slice())
                .expect("decode ambiguous outcome after upgrade");
        assert_eq!(restored_entry, journal_entry);

        let recovered = replay_payout_events(
            vec![Event::Init(payout_init_args(p(42)))],
            vec![
                PendingPayoutEvent::Queued {
                    kind: PendingPayoutKind::Margin,
                    operation_id,
                    transfer,
                    timestamp: Some(1),
                },
                PendingPayoutEvent::DispatchBoundary {
                    operation_id,
                    attempt_nonce,
                    payout_kind: Some(PendingPayoutKind::Margin),
                    ledger,
                    owner,
                    amount_raw: 90,
                    start_index: Some(10),
                    timestamp: Some(2),
                },
                PendingPayoutEvent::AmbiguousOutcome {
                    operation_id,
                    attempt_nonce,
                    owner,
                    timestamp: Some(3),
                },
            ],
        );

        let (_, recovered) = recovered.get_pending_payout(operation_id).unwrap();
        assert!(!recovered.in_flight);
        assert!(recovered.held_for_manual_retry);
        assert!(recovered.reconciliation_required);
        assert_eq!(recovered.op_nonce, attempt_nonce);
        assert_eq!(recovered.transfer_amount_raw, Some(90));
        assert_eq!(recovered.history_start_index, Some(10));
    }

    #[test]
    fn private_payout_journal_is_interleaved_before_public_settlement() {
        let owner = p(33);
        let operation_id = 97;
        let transfer = PendingMarginTransfer {
            vault_id: 7,
            operation_id,
            payout_kind: PendingPayoutKind::Margin,
            owner,
            margin: ICP::new(100),
            collateral_type: icp_token(),
            retry_count: 0,
            op_nonce: 92,
            ledger: None,
            transfer_amount_raw: None,
            held_for_manual_retry: true,
            reconciliation_required: true,
            in_flight: false,
            too_old_confirmed: false,
            history_start_index: None,
            rearm_schema_version: 0,
            history_scan: None,
            history_candidate_seen: false,
            no_effect_proof: None,
            history_log_length: None,
            history_cursor: 0,
            min_net_collateral_raw: None,
        };
        let state = replay_with_nonce_time_and_payout_events(
            vec![
                Event::Init(payout_init_args(p(32))),
                Event::MarginTransfer {
                    vault_id: 7,
                    block_index: 12,
                    operation_id: Some(operation_id),
                    payout_kind: Some(PendingPayoutKind::Margin),
                    timestamp: Some(3),
                },
            ]
            .into_iter(),
            vec![crate::storage::PendingPayoutJournalEntry {
                after_event_count: 1,
                event: PendingPayoutEvent::Queued {
                    kind: PendingPayoutKind::Margin,
                    operation_id,
                    transfer,
                    timestamp: Some(2),
                },
            }]
            .into_iter(),
            || 4,
        )
        .expect("interleaved payout journal should replay");
        assert!(state.get_pending_payout(operation_id).is_none());
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
        let ev = if let Event::OpenVault {
            mut vault,
            block_index,
            timestamp,
        } = ev
        {
            vault.collateral_amount = icp_e8s;
            Event::OpenVault {
                vault,
                block_index,
                timestamp,
            }
        } else {
            unreachable!()
        };
        assert_eq!(ev.size_e8s_usd(price_e8s), Some(500_000_000));
    }

    #[test]
    fn size_returns_none_for_admin_setters() {
        let ev = Event::SetBorrowingFee {
            rate: "0.005".into(),
        };
        assert_eq!(ev.size_e8s_usd(500_000_000), None);
    }

    // ── passes_filters: each dimension in isolation ───────────────────────

    #[test]
    fn empty_filter_excludes_accrue_interest_and_price_update() {
        let lookup = HashMap::new();
        assert!(!accrue_event(0).passes_filters(None, None, None, None, None, None, &lookup, 0));
        assert!(!price_event(icp_token(), 0)
            .passes_filters(None, None, None, None, None, None, &lookup, 0));
        assert!(borrow_event(1, caller_a(), 100, 0)
            .passes_filters(None, None, None, None, None, None, &lookup, 0));
    }

    #[test]
    fn explicit_type_set_includes_accrue_interest_when_requested() {
        let lookup = HashMap::new();
        let set: HashSet<_> = [EventTypeFilter::AccrueInterest].into_iter().collect();
        assert!(accrue_event(0).passes_filters(
            Some(&set),
            None,
            None,
            None,
            None,
            None,
            &lookup,
            0
        ));
        assert!(!borrow_event(1, caller_a(), 100, 0).passes_filters(
            Some(&set),
            None,
            None,
            None,
            None,
            None,
            &lookup,
            0
        ));
    }

    #[test]
    fn type_filter_or_combines_within_the_set() {
        let lookup = HashMap::new();
        let set: HashSet<_> = [EventTypeFilter::Borrow, EventTypeFilter::Repay]
            .into_iter()
            .collect();
        assert!(borrow_event(1, caller_a(), 100, 0).passes_filters(
            Some(&set),
            None,
            None,
            None,
            None,
            None,
            &lookup,
            0
        ));
        assert!(repay_event(1, caller_a(), 100, 0).passes_filters(
            Some(&set),
            None,
            None,
            None,
            None,
            None,
            &lookup,
            0
        ));
        assert!(!liquidate_event(1, caller_a(), 0).passes_filters(
            Some(&set),
            None,
            None,
            None,
            None,
            None,
            &lookup,
            0
        ));
    }

    #[test]
    fn principal_filter_matches_caller_or_owner() {
        let lookup = HashMap::new();
        assert!(borrow_event(1, caller_a(), 100, 0).passes_filters(
            None,
            Some(&caller_a()),
            None,
            None,
            None,
            None,
            &lookup,
            0
        ));
        assert!(!borrow_event(1, caller_a(), 100, 0).passes_filters(
            None,
            Some(&caller_b()),
            None,
            None,
            None,
            None,
            &lookup,
            0
        ));
    }

    #[test]
    fn collateral_token_filter_matches_via_vault_lookup() {
        let lookup = lookup(&[(1, ckbtc_token())]);
        let ev = borrow_event(1, caller_a(), 100, 0);
        assert!(ev.passes_filters(
            None,
            None,
            Some(&ckbtc_token()),
            None,
            None,
            None,
            &lookup,
            0
        ));
        assert!(!ev.passes_filters(None, None, Some(&icp_token()), None, None, None, &lookup, 0));
    }

    #[test]
    fn time_range_excludes_outside_window_and_no_timestamp_events() {
        let lookup = HashMap::new();
        let range = EventTimeRange {
            start_ns: 1_000,
            end_ns: 2_000,
        };

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
        let setter = Event::SetBorrowingFee {
            rate: "0.005".into(),
        };
        assert!(setter.passes_filters(None, None, None, None, Some(u64::MAX), None, &lookup, 0));
    }

    // ── two-filter AND combinations ───────────────────────────────────────

    #[test]
    fn type_and_principal_combine_with_and_semantics() {
        let lookup = HashMap::new();
        let types: HashSet<_> = [EventTypeFilter::Borrow].into_iter().collect();

        // Right type AND right principal → match
        assert!(borrow_event(1, caller_a(), 100, 0).passes_filters(
            Some(&types),
            Some(&caller_a()),
            None,
            None,
            None,
            None,
            &lookup,
            0,
        ));
        // Right type, wrong principal → reject
        assert!(!borrow_event(1, caller_a(), 100, 0).passes_filters(
            Some(&types),
            Some(&caller_b()),
            None,
            None,
            None,
            None,
            &lookup,
            0,
        ));
        // Wrong type, right principal → reject
        assert!(!repay_event(1, caller_a(), 100, 0).passes_filters(
            Some(&types),
            Some(&caller_a()),
            None,
            None,
            None,
            None,
            &lookup,
            0,
        ));
    }

    #[test]
    fn time_and_token_combine_with_and_semantics() {
        let lookup = lookup(&[(1, ckbtc_token())]);
        let range = EventTimeRange {
            start_ns: 1_000,
            end_ns: 2_000,
        };

        // In-window, right token → match
        assert!(borrow_event(1, caller_a(), 100, 1_500).passes_filters(
            None,
            None,
            Some(&ckbtc_token()),
            Some(&range),
            None,
            None,
            &lookup,
            0,
        ));
        // Out-of-window, right token → reject
        assert!(!borrow_event(1, caller_a(), 100, 9_999).passes_filters(
            None,
            None,
            Some(&ckbtc_token()),
            Some(&range),
            None,
            None,
            &lookup,
            0,
        ));
        // In-window, wrong token → reject
        assert!(!borrow_event(1, caller_a(), 100, 1_500).passes_filters(
            None,
            None,
            Some(&icp_token()),
            Some(&range),
            None,
            None,
            &lookup,
            0,
        ));
    }

    // ── admin_label + admin_labels filter ─────────────────────────────────

    #[test]
    fn admin_label_returns_variant_name_for_admin_variants() {
        let borrow_fee = Event::SetBorrowingFee {
            rate: "0.005".into(),
        };
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

        let borrow_fee = Event::SetBorrowingFee {
            rate: "0.005".into(),
        };
        let healthy = Event::SetHealthyCr {
            collateral_type: "ICP".to_string(),
            healthy_cr: Some("1.2".to_string()),
        };

        assert!(borrow_fee.passes_filters(
            Some(&types),
            None,
            None,
            None,
            None,
            Some(&labels),
            &lookup,
            0,
        ));
        assert!(!healthy.passes_filters(
            Some(&types),
            None,
            None,
            None,
            None,
            Some(&labels),
            &lookup,
            0,
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
            Some(&types),
            None,
            None,
            None,
            None,
            Some(&labels),
            &lookup,
            0,
        ));

        // An admin event is excluded because the types filter doesn't include
        // Admin. admin_labels doesn't re-enable it.
        let setter = Event::SetBorrowingFee {
            rate: "0.005".into(),
        };
        assert!(!setter.passes_filters(
            Some(&types),
            None,
            None,
            None,
            None,
            Some(&labels),
            &lookup,
            0,
        ));
    }

    #[test]
    fn admin_labels_with_no_types_narrows_admin_events_only() {
        // When types is None, non-admin events pass via the default filter
        // (which hides only accrue/price). admin_labels narrows admin events
        // to those whose label is in the set; non-admin events are unaffected.
        let lookup = HashMap::new();
        let labels: HashSet<String> = ["SetBorrowingFee".to_string()].into_iter().collect();

        let matching_admin = Event::SetBorrowingFee {
            rate: "0.005".into(),
        };
        let non_matching_admin = Event::SetHealthyCr {
            collateral_type: "ICP".to_string(),
            healthy_cr: Some("1.2".to_string()),
        };
        let non_admin = borrow_event(1, caller_a(), 100, 0);

        assert!(matching_admin.passes_filters(
            None,
            None,
            None,
            None,
            None,
            Some(&labels),
            &lookup,
            0,
        ));
        assert!(!non_matching_admin.passes_filters(
            None,
            None,
            None,
            None,
            None,
            Some(&labels),
            &lookup,
            0,
        ));
        assert!(non_admin.passes_filters(None, None, None, None, None, Some(&labels), &lookup, 0,));
    }

    #[test]
    fn admin_labels_empty_set_behaves_like_none() {
        // An empty admin_labels set should be ignored (same semantics as None).
        let lookup = HashMap::new();
        let types: HashSet<_> = [EventTypeFilter::Admin].into_iter().collect();
        let empty: HashSet<String> = HashSet::new();

        let setter = Event::SetBorrowingFee {
            rate: "0.005".into(),
        };
        assert!(setter.passes_filters(
            Some(&types),
            None,
            None,
            None,
            None,
            Some(&empty),
            &lookup,
            0,
        ));
    }
    #[test]
    fn amm1_private_journal_replays_mint_phase_and_completion_without_public_events() {
        let icp = p(29);
        let operation = PendingAmm1Donation {
            ledger: p(31),
            amm_canister: p(32),
            pool_id: "pool".to_string(),
            reward_subaccount: [7; 32],
            amount_e8s: 100,
            mint_op_nonce: 44,
            notify_nonce: 45,
            mint_block_index: None,
            phase: crate::state::Amm1DonationPhase::MintPending,
            reconciliation_reason: None,
        };
        let after_mint = replay_payout_events(
            vec![Event::Init(payout_init_args(icp))],
            vec![
                PendingPayoutEvent::Amm1DonationStarted { operation: operation.clone() },
                PendingPayoutEvent::Amm1DonationMintAccepted {
                    notify_nonce: 45,
                    block_index: 46,
                },
            ],
        );
        let pending = after_mint.pending_amm1_donation_operations.get(&45).unwrap();
        assert_eq!(pending.phase, crate::state::Amm1DonationPhase::NotifyPending);
        assert_eq!(pending.mint_block_index, Some(46));
        assert_eq!(pending.mint_op_nonce, 44);
        assert_eq!(after_mint.amm1_donation_nonce, 45);
        assert_eq!(after_mint.op_nonce_counter, 45);

        let held = replay_payout_events(
            vec![Event::Init(payout_init_args(icp))],
            vec![
                PendingPayoutEvent::Amm1DonationStarted { operation: operation.clone() },
                PendingPayoutEvent::Amm1DonationReconciliationRequired {
                    notify_nonce: 45,
                    reason: crate::state::Amm1DonationReconciliationReason::LedgerTooOld,
                },
            ],
        );
        let held_operation = held.pending_amm1_donation_operations.get(&45).unwrap();
        assert_eq!(held_operation.phase, crate::state::Amm1DonationPhase::ReconciliationRequired);
        assert_eq!(
            held_operation.reconciliation_reason,
            Some(crate::state::Amm1DonationReconciliationReason::LedgerTooOld),
        );

        let reconciled_by = p(33);
        let reconciled = replay_payout_events(
            vec![Event::Init(payout_init_args(icp))],
            vec![
                PendingPayoutEvent::Amm1DonationStarted { operation: operation.clone() },
                PendingPayoutEvent::Amm1DonationReconciliationRequired {
                    notify_nonce: 45,
                    reason: crate::state::Amm1DonationReconciliationReason::LedgerTooOld,
                },
                PendingPayoutEvent::Amm1DonationReceiptReconciled {
                    notify_nonce: 45,
                    block_index: 47,
                    reconciled_by,
                },
            ],
        );
        let reconciled_operation = reconciled.pending_amm1_donation_operations.get(&45).unwrap();
        assert_eq!(reconciled_operation.phase, crate::state::Amm1DonationPhase::NotifyPending);
        assert_eq!(reconciled_operation.mint_block_index, Some(47));
        assert_eq!(reconciled_operation.reconciliation_reason, None);
        assert_eq!(reconciled.reconciled_amm1_donation_receipts.get(&45).unwrap().block_index, 47);
        assert_eq!(reconciled.reconciled_amm1_donation_receipts.get(&45).unwrap().reconciled_by, reconciled_by);

        let completed_reconciled = replay_payout_events(
            vec![Event::Init(payout_init_args(icp))],
            vec![
                PendingPayoutEvent::Amm1DonationStarted { operation: operation.clone() },
                PendingPayoutEvent::Amm1DonationReconciliationRequired {
                    notify_nonce: 45,
                    reason: crate::state::Amm1DonationReconciliationReason::LedgerTooOld,
                },
                PendingPayoutEvent::Amm1DonationReceiptReconciled {
                    notify_nonce: 45,
                    block_index: 47,
                    reconciled_by,
                },
                PendingPayoutEvent::Amm1DonationCompleted { notify_nonce: 45 },
            ],
        );
        assert!(completed_reconciled.pending_amm1_donation_operations.is_empty());
        assert_eq!(completed_reconciled.reconciled_amm1_donation_receipts.get(&45).unwrap().block_index, 47);

        let completed = replay_payout_events(
            vec![Event::Init(payout_init_args(icp))],
            vec![
                PendingPayoutEvent::Amm1DonationStarted { operation },
                PendingPayoutEvent::Amm1DonationMintAccepted {
                    notify_nonce: 45,
                    block_index: 46,
                },
                PendingPayoutEvent::Amm1DonationCompleted { notify_nonce: 45 },
            ],
        );
        assert!(completed.pending_amm1_donation_operations.is_empty());
    }
}

#[cfg(test)]
mod three_usd_reserve_payout_replay_tests {
    use super::*;

    fn principal(seed: u8) -> Principal {
        Principal::self_authenticating([seed; 32])
    }

    fn init_args(icp: Principal) -> InitArg {
        InitArg {
            xrc_principal: principal(20),
            icusd_ledger_principal: principal(21),
            icp_ledger_principal: icp,
            fee_e8s: 0,
            developer_principal: principal(22),
            treasury_principal: None,
            stability_pool_principal: None,
            ckusdt_ledger_principal: None,
            ckusdc_ledger_principal: None,
        }
    }

    #[test]
    fn private_replay_retains_exact_reserve_payout_candidate_and_fee_after_generic_row_clears() {
        use crate::state::{
            PendingMarginTransfer, PendingPayoutKind, ThreeUsdReserveCollateralPayout,
            ThreeUsdReserveIngressKey,
        };
        let backend = principal(31);
        let pool = principal(32);
        let ledger = principal(33);
        let key = ThreeUsdReserveIngressKey { stability_pool: pool, vault_id: 44, absorb_id: 55 };
        let op_nonce = 66;
        let memo: [u8; 16] = crate::management::nonce_to_memo(op_nonce).0.as_slice()
            .try_into().unwrap();
        let payout = ThreeUsdReserveCollateralPayout {
            operation_id: op_nonce,
            op_nonce,
            collateral_type: ledger,
            ledger,
            source: icrc_ledger_types::icrc1::account::Account { owner: backend, subaccount: None },
            destination: icrc_ledger_types::icrc1::account::Account { owner: pool, subaccount: None },
            gross_e8s: 1_010,
            net_e8s: 1_000,
            expected_fee_e8s: 10,
            memo,
            created_at_time_ns: crate::management::nonce_to_created_at_time(op_nonce),
            fee_arg_e8s: Some(10),
            candidate_block_index: None,
            observed_fee_e8s: None,
            rearmed_attempts: Vec::new(),
        };
        let transfer = PendingMarginTransfer {
            vault_id: 44,
            operation_id: op_nonce,
            payout_kind: PendingPayoutKind::Margin,
            owner: pool,
            margin: crate::numeric::ICP::from(1_010),
            collateral_type: ledger,
            retry_count: 0,
            op_nonce,
            ledger: Some(ledger),
            transfer_amount_raw: Some(1_000),
            held_for_manual_retry: false,
            reconciliation_required: false,
            in_flight: false,
            too_old_confirmed: false,
            history_start_index: None,
            rearm_schema_version: 1,
            history_scan: None,
            history_candidate_seen: false,
            no_effect_proof: None,
            history_log_length: None,
            history_cursor: 0,
            min_net_collateral_raw: None,
        };
        let payout_events = vec![
            PendingPayoutEvent::ThreeUsdReservePayoutPrepared { key: key.clone(), payout },
            PendingPayoutEvent::Queued {
                kind: PendingPayoutKind::Margin,
                operation_id: op_nonce,
                transfer,
                timestamp: Some(1),
            },
            PendingPayoutEvent::ThreeUsdReservePayoutCandidate {
                key: key.clone(), operation_id: op_nonce, attempt_nonce: op_nonce, block_index: 77,
            },
            PendingPayoutEvent::ThreeUsdReservePayoutFeeObserved {
                key: key.clone(), operation_id: op_nonce, attempt_nonce: op_nonce,
                block_index: 77, actual_fee_e8s: 10,
            },
            // The public completion event removed the generic pending row;
            // this private journal projection must remain independently readable.
        ];
        let journal = payout_events.into_iter().map(|event| crate::storage::PendingPayoutJournalEntry {
            after_event_count: 1,
            event,
        });
        let state = replay_with_nonce_time_and_payout_events(
            vec![
                Event::Init(init_args(principal(34))),
                Event::MarginTransfer {
                    vault_id: 44,
                    block_index: 77,
                    operation_id: Some(op_nonce),
                    payout_kind: Some(PendingPayoutKind::Margin),
                    timestamp: Some(3),
                },
            ].into_iter(), journal, || 2,
        ).expect("private payout replay should succeed");
        let recovered = state.three_usd_reserve_collateral_payouts.get(&key).unwrap();
        assert_eq!(state.get_pending_payout(op_nonce), None);
        assert_eq!(state.three_usd_reserve_payout_operation_keys.get(&op_nonce), Some(&key));
        assert_eq!(recovered.candidate_block_index, Some(77));
        assert_eq!(recovered.expected_fee_e8s, 10);
        assert_eq!(recovered.observed_fee_e8s, Some(10));
        assert_eq!(recovered.expected_fee_e8s, recovered.observed_fee_e8s.unwrap());
    }

    #[test]
    fn candidate_scan_replay_preserves_partial_and_multiple_match_holds() {
        use crate::state::{
            PendingMarginTransfer, PendingPayoutKind, ThreeUsdReserveCollateralPayout,
            ThreeUsdReserveIngressKey, ThreeUsdReservePayoutCandidateScan,
        };
        let backend = principal(61);
        let pool = principal(62);
        let ledger = principal(63);
        let operation_id = 64;
        let key = ThreeUsdReserveIngressKey { stability_pool: pool, vault_id: 7, absorb_id: 8 };
        let memo: [u8; 16] = crate::management::nonce_to_memo(operation_id).0.as_slice()
            .try_into().unwrap();
        let payout = ThreeUsdReserveCollateralPayout {
            operation_id,
            op_nonce: operation_id,
            collateral_type: ledger,
            ledger,
            source: icrc_ledger_types::icrc1::account::Account { owner: backend, subaccount: None },
            destination: icrc_ledger_types::icrc1::account::Account { owner: pool, subaccount: None },
            gross_e8s: 110,
            net_e8s: 100,
            expected_fee_e8s: 10,
            memo,
            created_at_time_ns: crate::management::nonce_to_created_at_time(operation_id),
            fee_arg_e8s: Some(10),
            candidate_block_index: None,
            observed_fee_e8s: None,
            rearmed_attempts: Vec::new(),
        };
        let transfer = PendingMarginTransfer {
            vault_id: 7, operation_id, payout_kind: PendingPayoutKind::Margin,
            owner: pool, margin: crate::numeric::ICP::new(110), collateral_type: ledger,
            retry_count: 0, op_nonce: operation_id, ledger: Some(ledger),
            transfer_amount_raw: Some(100), held_for_manual_retry: false,
            reconciliation_required: false, in_flight: false, too_old_confirmed: false,
            history_start_index: None, rearm_schema_version: 1, history_scan: None,
            history_candidate_seen: false, no_effect_proof: None, history_log_length: None,
            history_cursor: 0, min_net_collateral_raw: None,
        };
        let scan_event = |next_index, candidate_block_index, multiple_candidates, snapshot_count| {
            PendingPayoutEvent::ThreeUsdReservePayoutCandidateScan {
                key: key.clone(), operation_id, attempt_nonce: operation_id,
                scan: ThreeUsdReservePayoutCandidateScan {
                    operation_id, attempt_nonce: operation_id, start_index: 10,
                    snapshot_log_length: 20, snapshot_count, next_index, candidate_block_index,
                    multiple_candidates,
                },
            }
        };
        let prefix = vec![
            PendingPayoutEvent::ThreeUsdReservePayoutPrepared { key: key.clone(), payout },
            PendingPayoutEvent::Queued { kind: PendingPayoutKind::Margin, operation_id, transfer, timestamp: None },
            PendingPayoutEvent::DispatchBoundary {
                operation_id, attempt_nonce: operation_id, payout_kind: Some(PendingPayoutKind::Margin),
                ledger, owner: pool, amount_raw: 100, start_index: Some(10), timestamp: None,
            },
            PendingPayoutEvent::AmbiguousOutcome { operation_id, attempt_nonce: operation_id, owner: pool, timestamp: None },
            scan_event(10, None, false, 1),
            scan_event(15, Some(12), false, 1),
        ];
        let partial = replay_with_nonce_time_and_payout_events(
            vec![Event::Init(init_args(ledger))].into_iter(),
            prefix.clone().into_iter().map(|event| crate::storage::PendingPayoutJournalEntry {
                after_event_count: 1, event,
            }),
            || 1,
        ).expect("partial candidate scan replays");
        let partial_payout = partial.three_usd_reserve_collateral_payouts.get(&key).unwrap();
        assert_eq!(partial_payout.candidate_block_index, None);
        assert_eq!(partial.three_usd_reserve_payout_candidate_scans.get(&key).unwrap().candidate_block_index, Some(12));
        assert_eq!(partial.three_usd_reserve_payout_candidate_scans.get(&key).unwrap().next_index, 15);

        let mut found_twice = prefix;
        found_twice.push(scan_event(20, Some(12), true, 1));
        // Even a buggy promotion record after a second exact match is rejected
        // during replay; multiplicity remains durably held.
        found_twice.push(PendingPayoutEvent::ThreeUsdReservePayoutCandidate {
            key: key.clone(), operation_id, attempt_nonce: operation_id, block_index: 12,
        });
        let multiple = replay_with_nonce_time_and_payout_events(
            vec![Event::Init(init_args(ledger))].into_iter(),
            found_twice.into_iter().map(|event| crate::storage::PendingPayoutJournalEntry {
                after_event_count: 1, event,
            }),
            || 1,
        ).expect("ambiguous candidate scan replays");
        let multiple_payout = multiple.three_usd_reserve_collateral_payouts.get(&key).unwrap();
        assert_eq!(multiple_payout.candidate_block_index, None);
        let scan = multiple.three_usd_reserve_payout_candidate_scans.get(&key).unwrap();
        assert_eq!(scan.candidate_block_index, Some(12));
        assert!(scan.multiple_candidates);
    }

    #[test]
    fn repeated_no_match_candidate_scans_have_a_replay_persisted_cap() {
        use crate::state::{
            PendingMarginTransfer, PendingPayoutKind, ThreeUsdReserveCollateralPayout,
            ThreeUsdReserveIngressKey, ThreeUsdReservePayoutCandidateScan,
        };
        let backend = principal(71);
        let pool = principal(72);
        let ledger = principal(73);
        let operation_id = 74;
        let key = ThreeUsdReserveIngressKey { stability_pool: pool, vault_id: 9, absorb_id: 10 };
        let memo: [u8; 16] = crate::management::nonce_to_memo(operation_id).0.as_slice()
            .try_into().unwrap();
        let payout = ThreeUsdReserveCollateralPayout {
            operation_id, op_nonce: operation_id, collateral_type: ledger, ledger,
            source: icrc_ledger_types::icrc1::account::Account { owner: backend, subaccount: None },
            destination: icrc_ledger_types::icrc1::account::Account { owner: pool, subaccount: None },
            gross_e8s: 110, net_e8s: 100, expected_fee_e8s: 10,
            memo, created_at_time_ns: crate::management::nonce_to_created_at_time(operation_id),
            fee_arg_e8s: Some(10), candidate_block_index: None, observed_fee_e8s: None,
            rearmed_attempts: Vec::new(),
        };
        let transfer = PendingMarginTransfer {
            vault_id: 9, operation_id, payout_kind: PendingPayoutKind::Margin,
            owner: pool, margin: crate::numeric::ICP::new(110), collateral_type: ledger,
            retry_count: 0, op_nonce: operation_id, ledger: Some(ledger), transfer_amount_raw: Some(100),
            held_for_manual_retry: false, reconciliation_required: false, in_flight: false,
            too_old_confirmed: false, history_start_index: None, rearm_schema_version: 1,
            history_scan: None, history_candidate_seen: false, no_effect_proof: None,
            history_log_length: None, history_cursor: 0, min_net_collateral_raw: None,
        };
        let scan = |snapshot_count, next_index| PendingPayoutEvent::ThreeUsdReservePayoutCandidateScan {
            key: key.clone(), operation_id, attempt_nonce: operation_id,
            scan: ThreeUsdReservePayoutCandidateScan {
                operation_id, attempt_nonce: operation_id, start_index: 10,
                snapshot_log_length: 20, snapshot_count, next_index,
                candidate_block_index: None, multiple_candidates: false,
            },
        };
        let mut events = vec![
            PendingPayoutEvent::ThreeUsdReservePayoutPrepared { key: key.clone(), payout },
            PendingPayoutEvent::Queued { kind: PendingPayoutKind::Margin, operation_id, transfer, timestamp: None },
            PendingPayoutEvent::DispatchBoundary {
                operation_id, attempt_nonce: operation_id, payout_kind: Some(PendingPayoutKind::Margin),
                ledger, owner: pool, amount_raw: 100, start_index: Some(10), timestamp: None,
            },
            PendingPayoutEvent::AmbiguousOutcome { operation_id, attempt_nonce: operation_id, owner: pool, timestamp: None },
            scan(1, 10), scan(1, 20),
            scan(2, 10), scan(2, 20),
            scan(3, 10), scan(3, 20),
        ];
        let capped = replay_with_nonce_time_and_payout_events(
            vec![Event::Init(init_args(ledger))].into_iter(),
            events.clone().into_iter().map(|event| crate::storage::PendingPayoutJournalEntry {
                after_event_count: 1, event,
            }),
            || 1,
        ).expect("bounded no-match rescans replay");
        let saved = capped.three_usd_reserve_payout_candidate_scans.get(&key).unwrap();
        assert_eq!(saved.snapshot_count, crate::state::MAX_THREE_USD_RESERVE_CANDIDATE_SCAN_SNAPSHOTS);
        assert_eq!(saved.next_index, saved.snapshot_log_length);
        assert_eq!(saved.candidate_block_index, None);

        // A fourth full snapshot is rejected even if a caller manufactures its
        // private event; the persisted lifetime cap remains at three.
        events.push(scan(4, 10));
        let exhausted = replay_with_nonce_time_and_payout_events(
            vec![Event::Init(init_args(ledger))].into_iter(),
            events.into_iter().map(|event| crate::storage::PendingPayoutJournalEntry {
                after_event_count: 1, event,
            }),
            || 1,
        ).expect("over-cap event is ignored during replay");
        let saved = exhausted.three_usd_reserve_payout_candidate_scans.get(&key).unwrap();
        assert_eq!(saved.snapshot_count, crate::state::MAX_THREE_USD_RESERVE_CANDIDATE_SCAN_SNAPSHOTS);
        assert_eq!(saved.next_index, saved.snapshot_log_length);
    }

    #[test]
    fn live_and_replayed_reserve_rearm_apply_the_same_sidecar_attempt() {
        use crate::state::{
            PendingMarginTransfer, PendingPayoutHistoryScan, PendingPayoutKind,
            PendingPayoutNoEffectProof, ThreeUsdReserveCollateralPayout,
            ThreeUsdReserveIngressKey,
        };
        let backend = principal(41);
        let pool = principal(42);
        let ledger = principal(43);
        let operation_id = 44;
        let old_nonce = 44;
        let new_nonce = 45;
        let key = ThreeUsdReserveIngressKey { stability_pool: pool, vault_id: 7, absorb_id: 8 };
        let make_state = || {
            let mut state = State::from(init_args(ledger));
            let memo: [u8; 16] = crate::management::nonce_to_memo(old_nonce).0.as_slice()
                .try_into().unwrap();
            state.three_usd_reserve_collateral_payouts.insert(key.clone(), ThreeUsdReserveCollateralPayout {
                operation_id,
                op_nonce: old_nonce,
                collateral_type: ledger,
                ledger,
                source: icrc_ledger_types::icrc1::account::Account { owner: backend, subaccount: None },
                destination: icrc_ledger_types::icrc1::account::Account { owner: pool, subaccount: None },
                gross_e8s: 1_010,
                net_e8s: 1_000,
                expected_fee_e8s: 10,
                memo,
                created_at_time_ns: crate::management::nonce_to_created_at_time(old_nonce),
                fee_arg_e8s: Some(10),
                candidate_block_index: None,
                observed_fee_e8s: None,
                rearmed_attempts: Vec::new(),
            });
            state.three_usd_reserve_payout_operation_keys.insert(operation_id, key.clone());
            state.insert_pending_payout(PendingMarginTransfer {
                vault_id: 7,
                operation_id,
                payout_kind: PendingPayoutKind::Margin,
                owner: pool,
                margin: crate::numeric::ICP::new(1_010),
                collateral_type: ledger,
                retry_count: 1,
                op_nonce: old_nonce,
                ledger: Some(ledger),
                transfer_amount_raw: Some(1_000),
                held_for_manual_retry: true,
                reconciliation_required: true,
                in_flight: false,
                too_old_confirmed: true,
                history_start_index: Some(10),
                rearm_schema_version: 1,
                history_scan: Some(PendingPayoutHistoryScan {
                    operation_id,
                    payout_kind: PendingPayoutKind::Margin,
                    ledger,
                    owner: pool,
                    amount_raw: 1_000,
                    attempt_nonce: old_nonce,
                    start_index: 10,
                    snapshot_log_length: 25,
                    next_index: 25,
                }),
                history_candidate_seen: false,
                no_effect_proof: None,
                history_log_length: Some(25),
                history_cursor: 25,
                min_net_collateral_raw: None,
            });
            state
        };
        let mut state = make_state();
        let mut replayed = make_state();
        let proof = PendingPayoutNoEffectProof {
            operation_id,
            payout_kind: PendingPayoutKind::Margin,
            ledger,
            owner: pool,
            amount_raw: 1_000,
            old_attempt_nonce: old_nonce,
            new_attempt_nonce: new_nonce,
            start_index: 10,
            snapshot_log_length: 25,
            complete_prefix: true,
            verified_at_ns: 99,
        };
        assert!(record_pending_payout_rearmed(&mut state, proof));
        apply_pending_payout_event(&mut replayed, PendingPayoutEvent::Rearmed {
            operation_id,
            attempt_nonce: new_nonce,
            proof: Some(proof),
            timestamp: Some(99),
            owner: Some(pool),
        });
        assert_eq!(state.get_pending_payout(operation_id), replayed.get_pending_payout(operation_id));
        assert_eq!(state.three_usd_reserve_collateral_payouts, replayed.three_usd_reserve_collateral_payouts);
        let payout = state.three_usd_reserve_collateral_payouts.get(&key).unwrap();
        assert_eq!(payout.rearmed_attempts.len(), 1);
        assert_eq!(payout.rearmed_attempts[0].op_nonce, new_nonce);
        assert_eq!(payout.rearmed_attempts[0].fee_arg_e8s, Some(10));
    }
}

#[cfg(test)]
mod redemption_replay_tests {
    use super::*;
    use crate::state::State;
    use crate::vault::Vault;
    use rust_decimal::prelude::FromPrimitive;
    use rust_decimal_macros::dec;

    fn principal(seed: u8) -> Principal {
        Principal::self_authenticating([seed; 32])
    }

    fn init_args(icp: Principal) -> InitArg {
        InitArg {
            xrc_principal: principal(20),
            icusd_ledger_principal: principal(21),
            icp_ledger_principal: icp,
            fee_e8s: 0,
            developer_principal: principal(22),
            treasury_principal: None,
            stability_pool_principal: None,
            ckusdt_ledger_principal: None,
            ckusdc_ledger_principal: None,
        }
    }

    fn config_for(args: &InitArg, ct: Principal, decimals: u8, price: f64) -> CollateralConfig {
        let mut config =
            State::from(args.clone()).collateral_configs[&args.icp_ledger_principal].clone();
        config.ledger_canister_id = ct;
        config.decimals = decimals;
        config.last_price = Some(price);
        config
    }

    fn open_vault(
        id: u64,
        owner: Principal,
        ct: Principal,
        collateral_raw: u64,
        debt_e8s: u64,
    ) -> Event {
        Event::OpenVault {
            vault: Vault {
                owner,
                borrowed_icusd_amount: ICUSD::from(debt_e8s),
                collateral_amount: collateral_raw,
                vault_id: id,
                collateral_type: ct,
                last_accrual_time: 0,
                accrued_interest: ICUSD::new(0),
                bot_processing: false,
            },
            block_index: id,
            timestamp: Some(1),
        }
    }

    fn redemption_event(
        owner: Principal,
        block: u64,
        ct: Option<Principal>,
        rate: UsdIcp,
        amount_e8s: u64,
        redemptions: Option<Vec<VaultRedemption>>,
        payout_raw: Option<u64>,
        minimum_net_raw: Option<u64>,
    ) -> Event {
        Event::RedemptionOnVaults {
            owner,
            current_icp_rate: rate,
            icusd_amount: ICUSD::from(amount_e8s),
            fee_amount: ICUSD::new(0),
            icusd_block_index: block,
            collateral_type: ct,
            timestamp: Some(2),
            vault_redemptions: redemptions,
            payout_collateral_raw: payout_raw,
            payout_operation_id: None,
            payout_attempt_nonce: None,
            payout_ledger: None,
            payout_transfer_amount_raw: None,
            min_net_collateral_raw: minimum_net_raw,
        }
    }

    fn replay(events: Vec<Event>) -> State {
        super::replay_with_nonce_time(events.into_iter(), || 2)
            .expect("replay fixture should be consistent")
    }

    #[test]
    fn bot_proof_security_state_survives_event_only_replay() {
        let icp = principal(29);
        let payment_ledger = principal(31);
        let private_events = [
            BotProofAuditEvent::ProofModeEnabled,
            BotProofAuditEvent::ClaimGenerationReserved { generation: 41 },
            BotProofAuditEvent::PaymentProofConsumed {
                ledger_principal: payment_ledger,
                block_index: 9001,
                vault_id: 77,
                claim_generation: 41,
            },
            BotProofAuditEvent::ClaimGenerationReserved { generation: 42 },
        ]
        .into_iter()
        .map(|event| crate::storage::PendingPayoutJournalEntry {
            after_event_count: 1,
            event: PendingPayoutEvent::BotProofAudit { event },
        });
        let state = super::replay_with_pending_payout_events(
            vec![Event::Init(init_args(icp))].into_iter(),
            private_events,
        )
        .expect("bot proof private journal should replay");

        assert!(state.bot_confirm_proof_required);
        assert_eq!(state.bot_claim_generation_counter, 42);
        assert_eq!(
            state
                .consumed_bot_payment_proofs
                .get(&format!("{}:9001", payment_ledger.to_text())),
            Some(&(77, 41)),
        );
    }

    #[test]
    fn legacy_redemption_replay_retains_incomplete_payout_as_held() {
        let icp = principal(29);
        let owner = principal(30);
        let state = replay(vec![
            Event::Init(init_args(icp)),
            redemption_event(
                owner,
                77,
                Some(icp),
                UsdIcp::new(Decimal::ONE),
                100,
                Some(Vec::new()),
                Some(25),
                None,
            ),
        ]);
        let payout = state
            .pending_redemption_transfer
            .get(&77)
            .expect("legacy redemption payout must remain visible");
        assert!(payout.held_for_manual_retry);
        assert!(payout.reconciliation_required);
        assert!(payout.ledger.is_none());
        assert!(payout.transfer_amount_raw.is_none());
        assert_eq!(payout.rearm_schema_version, 0);
    }

    #[test]
    fn new_redemption_receipt_replays_with_rearm_schema_only_for_exact_pinned_tuple() {
        let icp = principal(29);
        let owner = principal(30);
        let operation_id = 77;
        let mut event = redemption_event(
            owner,
            77,
            Some(icp),
            UsdIcp::new(Decimal::ONE),
            100,
            Some(Vec::new()),
            Some(25),
            None,
        );
        let Event::RedemptionOnVaults {
            payout_collateral_raw,
            payout_operation_id,
            payout_attempt_nonce,
            payout_ledger,
            payout_transfer_amount_raw,
            ..
        } = &mut event
        else {
            panic!("expected redemption event");
        };
        *payout_collateral_raw = Some(30);
        *payout_operation_id = Some(operation_id);
        *payout_attempt_nonce = Some(operation_id);
        *payout_ledger = Some(icp);
        *payout_transfer_amount_raw = Some(25);

        let old_public_only = replay(vec![Event::Init(init_args(icp)), event.clone()]);
        let old_row = old_public_only
            .pending_redemption_transfer
            .get(&77)
            .expect("legacy public redemption row must replay");
        assert!(old_row.held_for_manual_retry);
        assert!(old_row.reconciliation_required);
        assert_eq!(old_row.rearm_schema_version, 0);

        let queued_transfer = PendingMarginTransfer {
            vault_id: 0,
            operation_id,
            payout_kind: PendingPayoutKind::Redemption,
            owner,
            margin: ICP::from(30),
            collateral_type: icp,
            retry_count: 0,
            op_nonce: operation_id,
            ledger: Some(icp),
            transfer_amount_raw: Some(25),
            held_for_manual_retry: false,
            reconciliation_required: false,
            in_flight: false,
            too_old_confirmed: false,
            history_start_index: None,
            rearm_schema_version: 1,
            history_scan: None,
            history_candidate_seen: false,
            no_effect_proof: None,
            history_log_length: None,
            history_cursor: 0,
            min_net_collateral_raw: None,
        };
        let private_marker = crate::storage::PendingPayoutJournalEntry {
            after_event_count: 2,
            event: PendingPayoutEvent::Queued {
                kind: PendingPayoutKind::Redemption,
                operation_id,
                transfer: queued_transfer,
                timestamp: Some(3),
            },
        };
        let state = replay_with_nonce_time_and_payout_events(
            vec![Event::Init(init_args(icp)), event.clone()].into_iter(),
            std::iter::once(private_marker),
            || 2,
        )
        .expect("new redemption private marker should replay");
        let payout = state
            .pending_redemption_transfer
            .get(&77)
            .expect("new redemption payout must replay");
        assert!(!payout.held_for_manual_retry);
        assert!(!payout.reconciliation_required);
        assert_eq!(payout.operation_id, operation_id);
        assert_eq!(payout.op_nonce, operation_id);
        assert_eq!(payout.ledger, Some(icp));
        assert_eq!(payout.transfer_amount_raw, Some(25));
        assert_eq!(payout.rearm_schema_version, 1);

        let forged_owner_marker = crate::storage::PendingPayoutJournalEntry {
            after_event_count: 2,
            event: PendingPayoutEvent::Queued {
                kind: PendingPayoutKind::Redemption,
                operation_id,
                transfer: PendingMarginTransfer {
                    owner: principal(31),
                    ..queued_transfer
                },
                timestamp: None,
            },
        };
        let mismatch_state = replay_with_nonce_time_and_payout_events(
            vec![Event::Init(init_args(icp)), event.clone()].into_iter(),
            std::iter::once(forged_owner_marker),
            || 2,
        )
        .expect("mismatched private marker should be ignored");
        let mismatched = mismatch_state
            .pending_redemption_transfer
            .get(&77)
            .expect("public row remains visible");
        assert!(mismatched.held_for_manual_retry);
        assert_eq!(mismatched.rearm_schema_version, 0);

        // A mismatch between operation identity and dispatch nonce is not an
        // exact receipt and must not enable the rearm scanner.
        assert!(!super::redemption_receipt_supports_rearm(
            Some(30),
            Some(operation_id),
            Some(operation_id + 1),
            Some(icp),
            Some(25),
        ));
    }

    fn remove_v2_fields(event: Event) -> Event {
        let mut value = serde_json::to_value(event).expect("serialize event");
        let payload = value
            .get_mut("redemption_on_vaults")
            .expect("redemption event variant")
            .as_object_mut()
            .expect("event payload");
        payload.remove("payout_collateral_raw");
        payload.remove("min_net_collateral_raw");
        serde_json::from_value(value).expect("legacy redemption event defaults new fields")
    }

    #[test]
    fn legacy_none_replay_keeps_icp_alias_full_type_and_fractional_18_decimal_payout() {
        let icp = principal(1);
        let xaut = principal(2);
        let owner = principal(3);
        let args = init_args(icp);
        let icp_config = config_for(&args, icp, 18, 0.1);
        let xaut_config = config_for(&args, xaut, 18, 0.1);
        let prefix = vec![
            Event::Init(args),
            Event::UpdateCollateralConfig {
                collateral_type: icp,
                config: icp_config,
            },
            Event::AddCollateralType {
                collateral_type: xaut,
                config: xaut_config,
            },
            open_vault(1, owner, icp, 2_000_000_000_000_000_000, 1_000_000_000),
            open_vault(2, owner, xaut, 1_000_000_000_000_000_000, 1_000_000_000),
        ];
        let before = replay(prefix.clone());
        assert_eq!(
            before.redemption_runs()[0].collateral_type,
            xaut,
            "fixture must put another collateral type ahead of ICP globally"
        );

        // A pre-tiering event has no CT field (ICP alias), no stored outcomes,
        // and no pinned payout/minimum. It must replay the historical full-ICP
        // scan, even though today's queue ranks XAUT first.
        let rate = UsdIcp::new(Decimal::from_f64_retain(0.1).unwrap());
        let old_event = remove_v2_fields(redemption_event(
            owner, 77, None, rate, 10_000_000, None, None, None,
        ));
        let after = replay(prefix.into_iter().chain([old_event]).collect());

        let icp_vault = &after.vault_id_to_vaults[&1];
        assert_eq!(icp_vault.collateral_amount, 1_000_000_000_000_000_000);
        assert_eq!(icp_vault.borrowed_icusd_amount.to_u64(), 990_000_000);
        let xaut_vault = &after.vault_id_to_vaults[&2];
        assert_eq!(xaut_vault.collateral_amount, 1_000_000_000_000_000_000);
        assert_eq!(xaut_vault.borrowed_icusd_amount.to_u64(), 1_000_000_000);
        assert_eq!(
            after.pending_redemption_transfer[&77].margin.to_u64(),
            (ICUSD::new(10_000_000) / rate).to_u64(),
            "legacy pending payout retains its historical event-price conversion"
        );
        assert_eq!(
            after.pending_redemption_transfer[&77].min_net_collateral_raw,
            None
        );
    }

    #[test]
    fn old_some_outcomes_without_new_payout_fields_keep_the_committed_claim() {
        let icp = principal(4);
        let owner = principal(5);
        let args = init_args(icp);
        let old_event = remove_v2_fields(redemption_event(
            owner,
            88,
            Some(icp),
            UsdIcp::new(dec!(2)),
            30_000_000,
            Some(vec![VaultRedemption {
                vault_id: 9,
                icusd_redeemed_e8s: 30_000_000,
                collateral_seized: 12_345,
            }]),
            None,
            None,
        ));
        let state = replay(vec![Event::Init(args), old_event]);
        let pending = state.pending_redemption_transfer.get(&88).unwrap();
        assert_eq!(pending.margin.to_u64(), 15_000_000);
        assert_eq!(pending.min_net_collateral_raw, None);
    }

    #[test]
    fn new_event_pins_actual_native_payout_and_minimum_for_supported_decimals() {
        let icp = principal(6);
        let token = principal(7);
        let owner = principal(8);
        let cases = [
            (6, 123_456u64),
            (8, 12_345_678u64),
            (18, 1_234_567_890_123_456_789u64),
        ];

        for (decimals, payout_raw) in cases {
            let args = init_args(icp);
            let config = config_for(&args, token, decimals, 1.0);
            let event = redemption_event(
                owner,
                100 + decimals as u64,
                Some(token),
                UsdIcp::new(Decimal::ONE),
                10_000_000,
                Some(vec![VaultRedemption {
                    vault_id: 1,
                    icusd_redeemed_e8s: 10_000_000,
                    collateral_seized: payout_raw,
                }]),
                Some(payout_raw),
                Some(payout_raw.saturating_sub(1)),
            );
            let state = replay(vec![
                Event::Init(args),
                Event::AddCollateralType {
                    collateral_type: token,
                    config,
                },
                open_vault(1, owner, token, u64::MAX, 100_000_000),
                event,
            ]);
            let pending = state
                .pending_redemption_transfer
                .get(&(100 + decimals as u64))
                .unwrap();
            assert_eq!(pending.margin.to_u64(), payout_raw);
            assert_eq!(
                pending.min_net_collateral_raw,
                Some(payout_raw.saturating_sub(1))
            );
            assert_eq!(
                state.vault_id_to_vaults[&1].collateral_amount,
                u64::MAX - payout_raw
            );
        }
    }

    #[test]
    fn new_some_empty_event_is_distinct_from_legacy_none_and_has_no_claim() {
        let icp = principal(9);
        let owner = principal(10);
        let args = init_args(icp);
        let vault = open_vault(1, owner, icp, 500_000_000, 100_000_000);
        let event = redemption_event(
            owner,
            111,
            Some(icp),
            UsdIcp::new(Decimal::ONE),
            1,
            Some(Vec::new()),
            Some(0),
            Some(1),
        );
        let json = serde_json::to_value(&event).unwrap();
        let round_tripped: Event = serde_json::from_value(json).unwrap();
        match &round_tripped {
            Event::RedemptionOnVaults {
                vault_redemptions,
                payout_collateral_raw,
                min_net_collateral_raw,
                ..
            } => {
                assert_eq!(vault_redemptions.as_deref(), Some(&[][..]));
                assert_eq!(*payout_collateral_raw, Some(0));
                assert_eq!(*min_net_collateral_raw, Some(1));
            }
            _ => unreachable!(),
        }

        let state = replay(vec![Event::Init(args), vault.clone(), round_tripped]);
        assert_eq!(
            state.vault_id_to_vaults[&1].borrowed_icusd_amount.to_u64(),
            100_000_000
        );
        assert_eq!(state.vault_id_to_vaults[&1].collateral_amount, 500_000_000);
        assert!(!state.pending_redemption_transfer.contains_key(&111));
    }

    #[test]
    fn recorder_rejects_unrepresentable_aggregate_before_mutating_any_vault() {
        let icp = principal(11);
        let owner = principal(12);
        let args = init_args(icp);
        let mut state = State::from(args);
        let mut config = state.collateral_configs[&icp].clone();
        config.decimals = 18;
        config.last_price = Some(0.1);
        state.collateral_configs.insert(icp, config);
        for id in [1, 2] {
            state.open_vault(Vault {
                owner,
                borrowed_icusd_amount: ICUSD::new(1_000_000_000),
                collateral_amount: 10_000_000_000_000_000_000,
                vault_id: id,
                collateral_type: icp,
                last_accrual_time: 0,
                accrued_interest: ICUSD::new(0),
                bot_processing: false,
            });
        }
        let before = state.vault_id_to_vaults.clone();
        let configs_before = state.collateral_configs.clone();
        let events_before = crate::storage::count_events();
        let result = record_redemption_on_vault_run(
            &mut state,
            owner,
            ICUSD::new(200_000_000),
            ICUSD::new(0),
            UsdIcp::new(Decimal::from_f64_retain(0.1).unwrap()),
            222,
            icp,
            &[1, 2],
            None,
        );
        assert!(matches!(
            result,
            Err(RedemptionRecordError::PayoutUnrepresentable)
        ));
        assert_eq!(state.vault_id_to_vaults, before);
        assert_eq!(state.collateral_configs, configs_before);
        assert!(state.pending_redemption_transfer.is_empty());
        assert_eq!(state.protocol_deficit_icusd, ICUSD::new(0));
        assert_eq!(crate::storage::count_events(), events_before);
    }

    #[test]
    fn recorder_rejects_minimum_before_mutating_debt_or_collateral() {
        let icp = principal(13);
        let owner = principal(14);
        let args = init_args(icp);
        let mut state = State::from(args);
        let mut config = state.collateral_configs[&icp].clone();
        config.decimals = 8;
        config.last_price = Some(1.0);
        state.collateral_configs.insert(icp, config);
        state.open_vault(Vault {
            owner,
            borrowed_icusd_amount: ICUSD::new(100_000_000),
            collateral_amount: 500_000_000,
            vault_id: 1,
            collateral_type: icp,
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        });
        let before = state.vault_id_to_vaults.clone();
        let configs_before = state.collateral_configs.clone();
        let events_before = crate::storage::count_events();
        let result = record_redemption_on_vault_run(
            &mut state,
            owner,
            ICUSD::new(10_000_000),
            ICUSD::new(0),
            UsdIcp::new(Decimal::ONE),
            223,
            icp,
            &[1],
            Some(20_000_000),
        );
        assert!(matches!(
            result,
            Err(RedemptionRecordError::MinimumNotMet {
                minimum_net_raw: 20_000_000,
                actual_net_raw: 9_990_000,
            })
        ));
        assert_eq!(state.vault_id_to_vaults, before);
        assert_eq!(state.collateral_configs, configs_before);
        assert!(state.pending_redemption_transfer.is_empty());
        assert_eq!(state.protocol_deficit_icusd, ICUSD::new(0));
        assert_eq!(crate::storage::count_events(), events_before);
    }

    #[test]
    fn recorder_event_and_pending_payout_match_actual_native_seizure_across_precision() {
        let icp = principal(15);
        let owner = principal(16);
        let vectors = [(6, 0.1), (6, 0.3), (8, 0.1), (8, 0.3), (18, 0.1), (18, 0.3)];

        for (index, (decimals, price_f64)) in vectors.into_iter().enumerate() {
            let args = init_args(icp);
            let mut state = State::from(args.clone());
            let mut config = state.collateral_configs[&icp].clone();
            config.decimals = decimals;
            config.last_price = Some(price_f64);
            config.ledger_fee = 10_000;
            state.collateral_configs.insert(icp, config.clone());
            state.open_vault(Vault {
                owner,
                borrowed_icusd_amount: ICUSD::new(10_000_000_000),
                collateral_amount: u64::MAX,
                vault_id: 1,
                collateral_type: icp,
                last_accrual_time: 0,
                accrued_interest: ICUSD::new(0),
                bot_processing: false,
            });

            let rate = UsdIcp::new(Decimal::from_f64_retain(price_f64).unwrap());
            let simulated = state
                .simulate_redemption_for_vault_ids(ICUSD::new(10_000_000), rate, &icp, &[1])
                .unwrap();
            let gross = total_actual_collateral_seized(&simulated).unwrap();
            let expected_net = gross.saturating_sub(config.ledger_fee);
            let block = 300 + index as u64;
            let mut persisted_events = Vec::new();
            let outcome = record_redemption_on_vault_run_with(
                &mut state,
                owner,
                ICUSD::new(10_000_000),
                ICUSD::new(0),
                rate,
                block,
                icp,
                &[1],
                Some(expected_net),
                || 123,
                |event| persisted_events.push(event.clone()),
            )
            .expect("representable exact-minimum payout should be recorded");

            assert_eq!(outcome.margin.to_u64(), gross);
            assert_eq!(
                outcome.margin.to_u64().saturating_sub(config.ledger_fee),
                expected_net
            );
            let persisted = persisted_events
                .into_iter()
                .find(|event| matches!(event, Event::RedemptionOnVaults { .. }))
                .expect("recorder must persist a redemption event");
            match persisted {
                Event::RedemptionOnVaults {
                    vault_redemptions,
                    payout_collateral_raw,
                    min_net_collateral_raw,
                    timestamp,
                    ..
                } => {
                    let records = vault_redemptions.expect("new event pins outcomes");
                    assert_eq!(total_actual_collateral_seized(&records), Some(gross));
                    assert_eq!(payout_collateral_raw, Some(gross));
                    assert_eq!(min_net_collateral_raw, Some(expected_net));
                    assert_eq!(timestamp, Some(123));
                }
                _ => panic!("recorder persisted the wrong event variant"),
            }
            let pending = state.pending_redemption_transfer.get(&block).unwrap();
            assert_eq!(pending.margin.to_u64(), gross);
            assert_eq!(pending.min_net_collateral_raw, Some(expected_net));
        }
    }

    #[test]
    fn recorder_pins_underwater_saturated_gross_and_minimum_net_payout() {
        let icp = principal(17);
        let owner = principal(18);
        let args = init_args(icp);
        let mut state = State::from(args);
        let mut config = state.collateral_configs[&icp].clone();
        config.decimals = 8;
        config.last_price = Some(1.0);
        config.ledger_fee = 10_000;
        state.collateral_configs.insert(icp, config);
        state.open_vault(Vault {
            owner,
            borrowed_icusd_amount: ICUSD::new(100_000_000),
            collateral_amount: 50_000_000,
            vault_id: 1,
            collateral_type: icp,
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        });
        let min_net_raw = 49_990_000;
        let mut persisted_events = Vec::new();
        let outcome = record_redemption_on_vault_run_with(
            &mut state,
            owner,
            ICUSD::new(100_000_000),
            ICUSD::new(0),
            UsdIcp::new(Decimal::ONE),
            400,
            icp,
            &[1],
            Some(min_net_raw),
            || 456,
            |event| persisted_events.push(event.clone()),
        )
        .expect("underwater payout meets the exact net minimum after saturation");

        assert_eq!(outcome.consumed.to_u64(), 100_000_000);
        assert_eq!(outcome.margin.to_u64(), 50_000_000);
        let pending = state.pending_redemption_transfer.get(&400).unwrap();
        assert_eq!(pending.margin.to_u64(), 50_000_000);
        assert_eq!(pending.min_net_collateral_raw, Some(min_net_raw));
        let persisted = persisted_events
            .into_iter()
            .find(|event| matches!(event, Event::RedemptionOnVaults { .. }))
            .expect("recorder must persist the underwater event");
        let Event::RedemptionOnVaults {
            vault_redemptions,
            payout_collateral_raw,
            min_net_collateral_raw,
            ..
        } = persisted
        else {
            panic!("recorder persisted the wrong event variant");
        };
        let records = vault_redemptions.unwrap();
        assert_eq!(records[0].collateral_seized, 50_000_000);
        assert_eq!(payout_collateral_raw, Some(50_000_000));
        assert_eq!(min_net_collateral_raw, Some(min_net_raw));
        assert_eq!(50_000_000u64 - 10_000, min_net_raw);
    }
}

#[cfg(test)]
mod add_margin_recording_tests {
    use super::*;
    use crate::state::State;
    use crate::vault::Vault;

    #[test]
    fn overflow_is_rejected_before_event_is_appended() {
        let mut state = State::default();
        state.vault_id_to_vaults.insert(
            73,
            Vault {
                owner: Principal::anonymous(),
                borrowed_icusd_amount: ICUSD::new(0),
                collateral_amount: u64::MAX,
                vault_id: 73,
                collateral_type: Principal::anonymous(),
                last_accrual_time: 0,
                accrued_interest: ICUSD::new(0),
                bot_processing: false,
            },
        );
        let events_before = crate::storage::count_events();

        assert!(matches!(
            record_add_margin_to_vault(&mut state, 73, ICP::new(1), 9),
            Err(crate::ProtocolError::GenericError(_))
        ));
        assert_eq!(crate::storage::count_events(), events_before);
        assert_eq!(state.vault_id_to_vaults.get(&73).unwrap().collateral_amount, u64::MAX);
    }
}
