//! Typed wrapper around rumi_protocol_backend queries.
//!
//! Every function here returns Result<T, String> and never panics. Errors
//! propagate up to the caller (collectors), which increment the per-source
//! error counter and skip the snapshot for this tick.

use candid::{CandidType, Deserialize, Principal};

/// Subset of `ProtocolStatus` that Phase 1 needs. We deliberately decode only
/// the fields we use; this insulates us from upstream additions to the type.
///
/// IMPORTANT: candid decodes record types structurally by field name, so this
/// minimal struct works as long as the source canister's ProtocolStatus
/// includes these fields. Additional fields on the source side are ignored.
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ProtocolStatusSubset {
    pub total_icp_margin: u64,
    pub total_icusd_borrowed: u64,
    pub total_collateral_ratio: f64,
}

pub async fn get_protocol_status(backend: Principal) -> Result<ProtocolStatusSubset, String> {
    let res: Result<(ProtocolStatusSubset,), _> =
        ic_cdk::api::call::call(backend, "get_protocol_status", ()).await;
    match res {
        Ok((status,)) => Ok(status),
        Err((code, msg)) => Err(format!("get_protocol_status: {:?} {}", code, msg)),
    }
}

/// Subset of `CandidVault` that analytics needs. We deliberately decode only
/// the fields we use; this insulates us from upstream additions to the type.
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct CandidVaultSubset {
    pub owner: Principal,
    pub borrowed_icusd_amount: u64,
    pub icp_margin_amount: u64,
    pub vault_id: u64,
    pub collateral_amount: u64,
    pub collateral_type: Principal,
    pub accrued_interest: u64,
}

pub async fn get_all_vaults(backend: Principal) -> Result<Vec<CandidVaultSubset>, String> {
    let res: Result<(Vec<CandidVaultSubset>,), _> =
        ic_cdk::api::call::call(backend, "get_all_vaults", ()).await;
    match res {
        Ok((vaults,)) => Ok(vaults),
        Err((code, msg)) => Err(format!("get_all_vaults: {:?} {}", code, msg)),
    }
}

/// Subset of `CollateralTotals` that analytics needs.
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct CollateralTotalsSubset {
    pub decimals: u8,
    pub total_collateral: u64,
    pub total_debt: u64,
    pub collateral_type: Principal,
    pub price: f64,
    pub vault_count: u64,
    pub symbol: String,
}

pub async fn get_collateral_totals(
    backend: Principal,
) -> Result<Vec<CollateralTotalsSubset>, String> {
    let res: Result<(Vec<CollateralTotalsSubset>,), _> =
        ic_cdk::api::call::call(backend, "get_collateral_totals", ()).await;
    match res {
        Ok((totals,)) => Ok(totals),
        Err((code, msg)) => Err(format!("get_collateral_totals: {:?} {}", code, msg)),
    }
}

// --- Event tailing (Phase 4) ---

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct GetEventsArg {
    pub start: u64,
    pub length: u64,
}

/// Backend Event variant. We enumerate ALL variants from the backend .did so
/// that Candid deserialization succeeds for every event type. Variants we
/// actually process have their fields; all others use empty structs (Candid
/// skips extra record fields). This avoids reliance on `#[serde(other)]` which
/// does not work reliably with the candid crate's deserializer.
#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum BackendEvent {
    // --- Variants we process ---
    #[serde(rename = "open_vault")]
    OpenVault {
        block_index: u64,
        vault: BackendVaultRecord,
        timestamp: Option<u64>,
    },
    #[serde(rename = "borrow_from_vault")]
    BorrowFromVault {
        block_index: u64,
        vault_id: u64,
        fee_amount: u64,
        borrowed_amount: u64,
        caller: Option<Principal>,
        timestamp: Option<u64>,
    },
    #[serde(rename = "repay_to_vault")]
    RepayToVault {
        block_index: u64,
        vault_id: u64,
        repayed_amount: u64,
        caller: Option<Principal>,
        timestamp: Option<u64>,
    },
    #[serde(rename = "collateral_withdrawn")]
    CollateralWithdrawn {
        block_index: u64,
        vault_id: u64,
        amount: u64,
        caller: Option<Principal>,
        timestamp: Option<u64>,
    },
    #[serde(rename = "partial_collateral_withdrawn")]
    PartialCollateralWithdrawn {
        block_index: u64,
        vault_id: u64,
        amount: u64,
        caller: Option<Principal>,
        timestamp: Option<u64>,
    },
    #[serde(rename = "withdraw_and_close_vault")]
    WithdrawAndCloseVault {
        block_index: Option<u64>,
        vault_id: u64,
        amount: u64,
        caller: Option<Principal>,
        timestamp: Option<u64>,
    },
    VaultWithdrawnAndClosed {
        vault_id: u64,
        timestamp: u64,
        caller: Principal,
        amount: u64,
    },
    #[serde(rename = "dust_forgiven")]
    DustForgiven {
        vault_id: u64,
        amount: u64,
        timestamp: Option<u64>,
    },
    #[serde(rename = "redemption_on_vaults")]
    RedemptionOnVaults {
        icusd_amount: u64,
        icusd_block_index: u64,
        owner: Principal,
        fee_amount: u64,
        collateral_type: Option<Principal>,
        timestamp: Option<u64>,
    },
    #[serde(rename = "liquidate_vault")]
    LiquidateVault {
        vault_id: u64,
        liquidator: Option<Principal>,
        timestamp: Option<u64>,
    },
    #[serde(rename = "partial_liquidate_vault")]
    PartialLiquidateVault {
        protocol_fee_collateral: Option<u64>,
        liquidator_payment: u64,
        vault_id: u64,
        liquidator: Option<Principal>,
        icp_to_liquidator: u64,
        timestamp: Option<u64>,
    },
    #[serde(rename = "redistribute_vault")]
    RedistributeVault {
        vault_id: u64,
        timestamp: Option<u64>,
    },
    // --- Variants we don't process (empty structs skip all fields) ---
    #[serde(rename = "init")]
    Init {},
    #[serde(rename = "upgrade")]
    Upgrade {},
    #[serde(rename = "set_borrowing_fee")]
    SetBorrowingFee {},
    #[serde(rename = "claim_liquidity_returns")]
    ClaimLiquidityReturns {
        amount: u64,
        caller: Principal,
        timestamp: Option<u64>,
    },
    #[serde(rename = "provide_liquidity")]
    ProvideLiquidity {
        amount: u64,
        caller: Principal,
        timestamp: Option<u64>,
    },
    #[serde(rename = "set_rmr_ceiling_cr")]
    SetRmrCeilingCr {},
    #[serde(rename = "set_recovery_rate_curve")]
    SetRecoveryRateCurve {},
    #[serde(rename = "set_ckstable_repay_fee")]
    SetCkstableRepayFee {},
    #[serde(rename = "set_treasury_principal")]
    SetTreasuryPrincipal {},
    #[serde(rename = "accrue_interest")]
    AccrueInterest {},
    #[serde(rename = "set_max_partial_liquidation_ratio")]
    SetMaxPartialLiquidationRatio {},
    #[serde(rename = "admin_vault_correction")]
    AdminVaultCorrection {},
    #[serde(rename = "set_recovery_target_cr")]
    SetRecoveryTargetCr {},
    #[serde(rename = "set_stable_ledger_principal")]
    SetStableLedgerPrincipal {},
    #[serde(rename = "set_recovery_parameters")]
    SetRecoveryParameters {},
    #[serde(rename = "set_collateral_borrowing_fee")]
    SetCollateralBorrowingFee {},
    #[serde(rename = "set_collateral_liquidation_ratio")]
    SetCollateralLiquidationRatio {},
    #[serde(rename = "set_collateral_borrow_threshold")]
    SetCollateralBorrowThreshold {},
    #[serde(rename = "set_collateral_liquidation_bonus")]
    SetCollateralLiquidationBonus {},
    #[serde(rename = "set_collateral_min_vault_debt")]
    SetCollateralMinVaultDebt {},
    #[serde(rename = "set_collateral_ledger_fee")]
    SetCollateralLedgerFee {},
    #[serde(rename = "set_collateral_redemption_fee_floor")]
    SetCollateralRedemptionFeeFloor {},
    #[serde(rename = "set_collateral_redemption_fee_ceiling")]
    SetCollateralRedemptionFeeCeiling {},
    #[serde(rename = "set_collateral_min_deposit")]
    SetCollateralMinDeposit {},
    #[serde(rename = "set_collateral_display_color")]
    SetCollateralDisplayColor {},
    #[serde(rename = "set_bot_allowed_collateral_types")]
    SetBotAllowedCollateralTypes {},
    #[serde(rename = "margin_transfer")]
    MarginTransfer {},
    #[serde(rename = "admin_sweep_to_treasury")]
    AdminSweepToTreasury {},
    #[serde(rename = "set_rmr_floor_cr")]
    SetRmrFloorCr {},
    #[serde(rename = "set_rmr_ceiling")]
    SetRmrCeiling {},
    #[serde(rename = "set_global_icusd_mint_cap")]
    SetGlobalIcusdMintCap {},
    #[serde(rename = "set_reserve_redemptions_enabled")]
    SetReserveRedemptionsEnabled {},
    #[serde(rename = "set_icpswap_routing_enabled")]
    SetIcpswapRoutingEnabled {},
    #[serde(rename = "set_min_icusd_amount")]
    SetMinIcusdAmount {},
    #[serde(rename = "set_borrowing_fee_curve")]
    SetBorrowingFeeCurve {},
    #[serde(rename = "set_interest_pool_share")]
    SetInterestPoolShare {},
    #[serde(rename = "set_liquidation_protocol_share")]
    SetLiquidationProtocolShare {},
    #[serde(rename = "update_collateral_config")]
    UpdateCollateralConfig {},
    #[serde(rename = "set_rate_curve_markers")]
    SetRateCurveMarkers {},
    #[serde(rename = "withdraw_liquidity")]
    WithdrawLiquidity {
        amount: u64,
        caller: Principal,
        timestamp: Option<u64>,
    },
    #[serde(rename = "admin_mint")]
    AdminMint {},
    #[serde(rename = "set_three_pool_canister")]
    SetThreePoolCanister {},
    #[serde(rename = "set_liquidation_bonus")]
    SetLiquidationBonus {},
    #[serde(rename = "reserve_redemption")]
    ReserveRedemption {},
    #[serde(rename = "close_vault")]
    CloseVault {},
    #[serde(rename = "update_collateral_status")]
    UpdateCollateralStatus {},
    #[serde(rename = "set_healthy_cr")]
    SetHealthyCr {},
    #[serde(rename = "set_redemption_fee_ceiling")]
    SetRedemptionFeeCeiling {},
    #[serde(rename = "add_margin_to_vault")]
    AddMarginToVault {},
    #[serde(rename = "set_stability_pool_principal")]
    SetStabilityPoolPrincipal {},
    #[serde(rename = "set_interest_split")]
    SetInterestSplit {},
    #[serde(rename = "set_bot_budget")]
    SetBotBudget {},
    #[serde(rename = "set_rmr_floor")]
    SetRmrFloor {},
    #[serde(rename = "set_redemption_fee_floor")]
    SetRedemptionFeeFloor {},
    #[serde(rename = "set_interest_rate")]
    SetInterestRate {},
    #[serde(rename = "set_reserve_redemption_fee")]
    SetReserveRedemptionFee {},
    #[serde(rename = "redemption_transfered")]
    RedemptionTransfered {},
    #[serde(rename = "set_liquidation_bot_principal")]
    SetLiquidationBotPrincipal {},
    #[serde(rename = "add_collateral_type")]
    AddCollateralType {},
    #[serde(rename = "set_stable_token_enabled")]
    SetStableTokenEnabled {},
    #[serde(rename = "set_recovery_cr_multiplier")]
    SetRecoveryCrMultiplier {},
    #[serde(rename = "price_update")]
    PriceUpdate {},
    #[serde(rename = "admin_debt_correction")]
    AdminDebtCorrection {},
    #[serde(rename = "set_collateral_debt_ceiling")]
    SetCollateralDebtCeiling {},
    #[serde(rename = "set_collateral_interest_rate")]
    SetCollateralInterestRate {},
    #[serde(rename = "set_collateral_redemption_tier")]
    SetCollateralRedemptionTier {},
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct BackendVaultRecord {
    pub vault_id: u64,
    pub owner: Principal,
    pub collateral_type: Principal,
    pub collateral_amount: u64,
    pub borrowed_icusd_amount: u64,
}

impl BackendEvent {
    /// Canonical admin label (variant name) for admin/setter/init/upgrade
    /// variants. Returns None for user-facing variants (vault, swap, stability,
    /// redemption, etc.). Stable across Rust renames as long as this match
    /// stays in sync with the backend enum.
    pub fn admin_label(&self) -> Option<&'static str> {
        use BackendEvent::*;
        match self {
            // User-facing / non-admin variants (they map to their own
            // EventTypeFilter categories in the backend, not Admin):
            OpenVault { .. }
            | BorrowFromVault { .. }
            | RepayToVault { .. }
            | CollateralWithdrawn { .. }
            | PartialCollateralWithdrawn { .. }
            | WithdrawAndCloseVault { .. }
            | VaultWithdrawnAndClosed { .. }
            | DustForgiven { .. }
            | RedemptionOnVaults { .. }
            | RedemptionTransfered {}
            | LiquidateVault { .. }
            | PartialLiquidateVault { .. }
            | RedistributeVault { .. }
            | ProvideLiquidity { .. }
            | WithdrawLiquidity { .. }
            | ClaimLiquidityReturns { .. }
            | AccrueInterest {}
            | PriceUpdate {}
            | AdminMint {}
            | AdminSweepToTreasury {}
            | CloseVault {}
            | MarginTransfer {}
            | AddMarginToVault {}
            | AdminVaultCorrection {}
            | AdminDebtCorrection {}
            | ReserveRedemption {} => None,

            // Lifecycle variants.
            Init {} => Some("Init"),
            Upgrade {} => Some("Upgrade"),

            // Admin setters and config events (hit `_ => Admin` in backend
            // type_filter).
            SetBorrowingFee {} => Some("SetBorrowingFee"),
            SetRmrCeilingCr {} => Some("SetRmrCeilingCr"),
            SetRecoveryRateCurve {} => Some("SetRecoveryRateCurve"),
            SetCkstableRepayFee {} => Some("SetCkstableRepayFee"),
            SetTreasuryPrincipal {} => Some("SetTreasuryPrincipal"),
            SetMaxPartialLiquidationRatio {} => Some("SetMaxPartialLiquidationRatio"),
            SetRecoveryTargetCr {} => Some("SetRecoveryTargetCr"),
            SetStableLedgerPrincipal {} => Some("SetStableLedgerPrincipal"),
            SetRecoveryParameters {} => Some("SetRecoveryParameters"),
            SetCollateralBorrowingFee {} => Some("SetCollateralBorrowingFee"),
            SetCollateralLiquidationRatio {} => Some("SetCollateralLiquidationRatio"),
            SetCollateralBorrowThreshold {} => Some("SetCollateralBorrowThreshold"),
            SetCollateralLiquidationBonus {} => Some("SetCollateralLiquidationBonus"),
            SetCollateralMinVaultDebt {} => Some("SetCollateralMinVaultDebt"),
            SetCollateralLedgerFee {} => Some("SetCollateralLedgerFee"),
            SetCollateralRedemptionFeeFloor {} => Some("SetCollateralRedemptionFeeFloor"),
            SetCollateralRedemptionFeeCeiling {} => Some("SetCollateralRedemptionFeeCeiling"),
            SetCollateralMinDeposit {} => Some("SetCollateralMinDeposit"),
            SetCollateralDisplayColor {} => Some("SetCollateralDisplayColor"),
            SetBotAllowedCollateralTypes {} => Some("SetBotAllowedCollateralTypes"),
            SetRmrFloorCr {} => Some("SetRmrFloorCr"),
            SetRmrCeiling {} => Some("SetRmrCeiling"),
            SetGlobalIcusdMintCap {} => Some("SetGlobalIcusdMintCap"),
            SetReserveRedemptionsEnabled {} => Some("SetReserveRedemptionsEnabled"),
            SetIcpswapRoutingEnabled {} => Some("SetIcpswapRoutingEnabled"),
            SetMinIcusdAmount {} => Some("SetMinIcusdAmount"),
            SetBorrowingFeeCurve {} => Some("SetBorrowingFeeCurve"),
            SetInterestPoolShare {} => Some("SetInterestPoolShare"),
            SetLiquidationProtocolShare {} => Some("SetLiquidationProtocolShare"),
            UpdateCollateralConfig {} => Some("UpdateCollateralConfig"),
            SetRateCurveMarkers {} => Some("SetRateCurveMarkers"),
            SetThreePoolCanister {} => Some("SetThreePoolCanister"),
            SetLiquidationBonus {} => Some("SetLiquidationBonus"),
            UpdateCollateralStatus {} => Some("UpdateCollateralStatus"),
            SetHealthyCr {} => Some("SetHealthyCr"),
            SetRedemptionFeeCeiling {} => Some("SetRedemptionFeeCeiling"),
            SetStabilityPoolPrincipal {} => Some("SetStabilityPoolPrincipal"),
            SetInterestSplit {} => Some("SetInterestSplit"),
            SetBotBudget {} => Some("SetBotBudget"),
            SetRmrFloor {} => Some("SetRmrFloor"),
            SetRedemptionFeeFloor {} => Some("SetRedemptionFeeFloor"),
            SetInterestRate {} => Some("SetInterestRate"),
            SetReserveRedemptionFee {} => Some("SetReserveRedemptionFee"),
            SetLiquidationBotPrincipal {} => Some("SetLiquidationBotPrincipal"),
            AddCollateralType {} => Some("AddCollateralType"),
            SetStableTokenEnabled {} => Some("SetStableTokenEnabled"),
            SetRecoveryCrMultiplier {} => Some("SetRecoveryCrMultiplier"),
            SetCollateralDebtCeiling {} => Some("SetCollateralDebtCeiling"),
            SetCollateralInterestRate {} => Some("SetCollateralInterestRate"),
            SetCollateralRedemptionTier {} => Some("SetCollateralRedemptionTier"),
        }
    }
}

pub async fn get_events(
    backend: Principal,
    start: u64,
    length: u64,
) -> Result<Vec<BackendEvent>, String> {
    let arg = GetEventsArg { start, length };
    let (events,): (Vec<BackendEvent>,) = ic_cdk::call(backend, "get_events", (arg,))
        .await
        .map_err(|(code, msg)| format!("get_events: {:?} {}", code, msg))?;
    Ok(events)
}

pub async fn get_event_count(backend: Principal) -> Result<u64, String> {
    let (count,): (u64,) = ic_cdk::call(backend, "get_event_count", ())
        .await
        .map_err(|(code, msg)| format!("get_event_count: {:?} {}", code, msg))?;
    Ok(count)
}
