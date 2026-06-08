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
    AddMarginToVault {
        vault_id: u64,
        margin_added: u64,
        block_index: u64,
        caller: Option<Principal>,
        timestamp: Option<u64>,
    },
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
    // --- Variants added by audit waves 8e/9/10/12/14 ---
    // Without these, candid decode fails for the entire get_events batch
    // whenever any post-2026-04-29 event is in range. Empty structs work
    // because candid skips extra record fields. Cycle drain root cause —
    // see investigation 2026-05-09.
    #[serde(rename = "deficit_accrued")]
    DeficitAccrued {},
    #[serde(rename = "deficit_repaid")]
    DeficitRepaid {},
    #[serde(rename = "breaker_tripped")]
    BreakerTripped {},
    #[serde(rename = "breaker_cleared")]
    BreakerCleared {},
    #[serde(rename = "set_breaker_window_ns")]
    SetBreakerWindowNs {},
    #[serde(rename = "set_breaker_window_debt_ceiling_e8s")]
    SetBreakerWindowDebtCeilingE8s {},
    #[serde(rename = "set_deficit_readonly_threshold_e8s")]
    SetDeficitReadonlyThresholdE8s {},
    #[serde(rename = "set_deficit_repayment_fraction")]
    SetDeficitRepaymentFraction {},
    #[serde(rename = "bot_claim_reconciliation_needed")]
    BotClaimReconciliationNeeded {},
    #[serde(rename = "oracle_circuit_breaker")]
    OracleCircuitBreaker {},
    #[serde(rename = "oracle_source_count_insufficient")]
    OracleSourceCountInsufficient {},
    #[serde(rename = "stability_pool_call_failed")]
    StabilityPoolCallFailed {},
    // PR #174 (deployed 2026-05-09). Without this variant, candid decode
    // fails for any get_events batch containing a SetBotCrToleranceBps row,
    // breaking analytics ingestion entirely.
    #[serde(rename = "set_bot_cr_tolerance_bps")]
    SetBotCrToleranceBps {},
    // Backend commit 18dd5f1 (2026-05-09) added the `set_amm1_canister`
    // admin endpoint. Pre-listing the event variant here so analytics
    // continues decoding cleanly the first time an admin wires the AMM1
    // canister principal into the backend (without this, the entire
    // `Vec<BackendEvent>` decode of any batch containing one of these
    // rows would fail and halt ingestion).
    #[serde(rename = "set_amm1_canister")]
    SetAmm1Canister {},
    // 2026-05-19 AMM1 pool_id mismatch fix. Admin sets the canonical AMM1
    // pool_id (e.g. `<token_a_principal>_<token_b_principal>`) so
    // donate_icusd_to_amm1 stops minting to a phantom subaccount. Mirrored
    // here so analytics ingestion doesn't break on the first set_amm1_pool_id
    // event after the backend upgrade lands.
    #[serde(rename = "set_amm1_pool_id")]
    SetAmm1PoolId {},
    // Wave-14a CDP-14 follow-up: per-collateral XRC source-count floor
    // override. Emitted when an admin tunes the per-asset floor (e.g.
    // dropping XAUT from the global 3 to 2 because XAUT only trades on
    // a handful of CEXs). Pre-listed so analytics' get_events decode
    // stays clean from the moment the backend ships this variant.
    #[serde(rename = "set_collateral_min_xrc_sources")]
    SetCollateralMinXrcSources {},
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
            | AddMarginToVault { .. }
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
            // Audit-wave variants added 2026-05-09 to fix decode failures.
            // Admin/diagnostic labels grouped under existing breakdown buckets.
            DeficitAccrued {} => None,
            DeficitRepaid {} => None,
            BreakerTripped {} => Some("BreakerTripped"),
            BreakerCleared {} => Some("BreakerCleared"),
            SetBreakerWindowNs {} => Some("SetBreakerWindowNs"),
            SetBreakerWindowDebtCeilingE8s {} => Some("SetBreakerWindowDebtCeilingE8s"),
            SetDeficitReadonlyThresholdE8s {} => Some("SetDeficitReadonlyThresholdE8s"),
            SetDeficitRepaymentFraction {} => Some("SetDeficitRepaymentFraction"),
            BotClaimReconciliationNeeded {} => Some("BotClaimReconciliationNeeded"),
            OracleCircuitBreaker {} => Some("OracleCircuitBreaker"),
            OracleSourceCountInsufficient {} => Some("OracleSourceCountInsufficient"),
            StabilityPoolCallFailed {} => Some("StabilityPoolCallFailed"),
            SetBotCrToleranceBps {} => Some("SetBotCrToleranceBps"),
            SetAmm1Canister {} => Some("SetAmm1Canister"),
            SetCollateralMinXrcSources {} => Some("SetCollateralMinXrcSources"),
            SetAmm1PoolId {} => Some("SetAmm1PoolId"),
        }
    }
}

/// Decode a raw Candid response blob from `get_events` into a `(decoded,
/// total_fetched)` pair using per-element decoding.
///
/// `cursor_start` is the absolute event index of the first element in this
/// batch; it is used only for log messages.
///
/// Exposed as `pub(crate)` so unit tests can drive it without a live IC
/// runtime.  The async `get_events_resilient` wrapper is the only production
/// call-site.
pub(crate) fn decode_events_resilient(
    raw_bytes: &[u8],
    cursor_start: u64,
) -> Result<(Vec<BackendEvent>, u64), String> {
    // Decode the response as generic IDLArgs so that unknown variant hashes
    // do not abort the parse.  The response is a 1-tuple whose single arg is
    // a vec of variants.
    let idl_args = candid::IDLArgs::from_bytes(raw_bytes)
        .map_err(|e| format!("get_events IDLArgs parse: {}", e))?;

    let elements: Vec<candid::IDLValue> = match idl_args.args.into_iter().next() {
        Some(candid::IDLValue::Vec(v)) => v,
        Some(other) => {
            return Err(format!(
                "get_events: expected Vec, got {:?}",
                std::mem::discriminant(&other)
            ))
        }
        None => return Err("get_events: empty response tuple".to_string()),
    };

    let total_fetched = elements.len() as u64;
    let mut decoded = Vec::with_capacity(elements.len());

    for (idx, elem) in elements.into_iter().enumerate() {
        let abs_idx = cursor_start + idx as u64;
        // Re-encode the individual element as a 1-arg IDL blob so that the
        // standard typed decoder can consume it.
        let elem_bytes = match candid::IDLArgs::new(&[elem]).to_bytes() {
            Ok(b) => b,
            Err(e) => {
                ic_cdk::println!(
                    "[tail_backend] event idx={} re-encode failed (skipping): {}",
                    abs_idx,
                    e
                );
                continue;
            }
        };
        match candid::decode_one::<BackendEvent>(&elem_bytes) {
            Ok(event) => decoded.push(event),
            Err(e) => {
                ic_cdk::println!(
                    "[tail_backend] event idx={} unknown/undecodable variant (skipping): {}",
                    abs_idx,
                    e
                );
            }
        }
    }

    Ok((decoded, total_fetched))
}

/// Fetch a batch of backend events and decode each element individually.
///
/// Returns `(decoded, total_fetched)` where:
/// - `decoded` is the list of events that could be decoded as a known
///   `BackendEvent` variant; elements that fail to decode are skipped.
/// - `total_fetched` is the number of elements the backend actually returned
///   in the batch, including any that were skipped due to unknown/undecodable
///   variants.
///
/// The caller MUST advance the cursor by `total_fetched` (not just
/// `decoded.len()`) so that undecodable events are not re-fetched on the
/// next tick. A single unknown variant in the middle of a batch no longer
/// poisons the entire batch.
pub async fn get_events_resilient(
    backend: Principal,
    start: u64,
    length: u64,
) -> Result<(Vec<BackendEvent>, u64), String> {
    let arg = GetEventsArg { start, length };
    let arg_bytes = candid::encode_one(&arg)
        .map_err(|e| format!("get_events_resilient encode: {}", e))?;

    let raw_bytes = ic_cdk::api::call::call_raw(backend, "get_events", arg_bytes, 0)
        .await
        .map_err(|(code, msg)| format!("get_events: {:?} {}", code, msg))?;

    decode_events_resilient(&raw_bytes, start)
}

/// Legacy typed fetch — kept for reference; prefer `get_events_resilient`.
#[allow(dead_code)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use candid::CandidType;

    /// A superset of `BackendEvent` used on the "sender side" in tests.
    ///
    /// This mirrors how the real backend works: the backend may add new variants
    /// that the analytics mirror does not yet know about.  We encode a batch
    /// using this extended enum and then verify that `decode_events_resilient`
    /// (which uses `BackendEvent` as the target type) handles the unknown
    /// variants gracefully.
    ///
    /// IMPORTANT: All variants that should decode successfully as `BackendEvent`
    /// must carry the same `#[serde(rename = "...")]` tag as they do in
    /// `BackendEvent`. Unknown variants use any name not in `BackendEvent`.
    #[derive(CandidType, Deserialize, Clone, Debug)]
    enum BackendEventExtended {
        // Known variants (same serde rename as BackendEvent)
        #[serde(rename = "price_update")]
        PriceUpdate {},
        #[serde(rename = "init")]
        Init {},
        // Unknown variant — not present in the analytics BackendEvent mirror.
        // This simulates a future backend variant that analytics hasn't added yet.
        #[serde(rename = "new_future_backend_event_v999")]
        NewFutureBackendEventV999 {},
    }

    /// Encode a batch as a raw Candid response blob `(Vec<BackendEventExtended>,)`.
    ///
    /// This produces bytes in exactly the shape the backend would send: a
    /// 1-tuple whose first (and only) element is a vec of variant values,
    /// each encoded with the full candid type table including all variant
    /// hashes.  The type table is derived from `BackendEventExtended`, so the
    /// unknown variant's hash IS present in the wire format — analytics'
    /// `decode_events_resilient` must skip it without aborting the batch.
    fn encode_batch(events: Vec<BackendEventExtended>) -> Vec<u8> {
        candid::encode_one(events).expect("encode_batch: failed to encode")
    }

    /// (a) A batch containing ONLY known variants decodes fully without error.
    #[test]
    fn all_known_variants_decode_fully() {
        let raw = encode_batch(vec![
            BackendEventExtended::PriceUpdate {},
            BackendEventExtended::Init {},
        ]);
        let (decoded, total) = decode_events_resilient(&raw, 0).expect("should not error");
        assert_eq!(total, 2, "total_fetched must equal elements in batch");
        assert_eq!(decoded.len(), 2, "all known events must be decoded");
    }

    /// (b) A batch with one unknown variant in the middle does not fail the
    ///     whole batch: known events on either side are still decoded.
    /// (c) The cursor-advance count (total_fetched) equals the full batch size,
    ///     not just the number of successfully decoded events, so the tailer
    ///     always makes forward progress even when new backend variants appear.
    #[test]
    fn unknown_variant_skipped_known_events_preserved_cursor_advances() {
        // Three-element batch: known | unknown | known
        let raw = encode_batch(vec![
            BackendEventExtended::PriceUpdate {},
            BackendEventExtended::NewFutureBackendEventV999 {},
            BackendEventExtended::Init {},
        ]);
        let (decoded, total) =
            decode_events_resilient(&raw, 100).expect("should not error on mixed batch");

        // (c) cursor advance count = full batch size (3), not decoded count (2)
        assert_eq!(
            total, 3,
            "total_fetched must include the skipped unknown event so cursor always advances"
        );

        // (b) the two known events on either side of the unknown are preserved
        assert_eq!(
            decoded.len(),
            2,
            "known events on either side of unknown variant must be decoded"
        );

        // Verify the known events decode to the expected variants
        assert!(
            matches!(decoded[0], BackendEvent::PriceUpdate {}),
            "first event must decode as PriceUpdate"
        );
        assert!(
            matches!(decoded[1], BackendEvent::Init {}),
            "third event must decode as Init"
        );
    }

    /// A batch consisting entirely of unknown variants: decoded list is empty
    /// but total_fetched still equals the batch size so the cursor advances.
    #[test]
    fn all_unknown_variants_cursor_still_advances() {
        let raw = encode_batch(vec![
            BackendEventExtended::NewFutureBackendEventV999 {},
            BackendEventExtended::NewFutureBackendEventV999 {},
        ]);
        let (decoded, total) =
            decode_events_resilient(&raw, 0).expect("should not error on all-unknown batch");
        assert_eq!(total, 2, "total_fetched must equal batch size even when all are unknown");
        assert_eq!(decoded.len(), 0, "no events should be decoded when all are unknown");
    }

    /// An empty batch is valid: both counts are 0 and no error is returned.
    #[test]
    fn empty_batch_is_valid() {
        let raw = encode_batch(vec![]);
        let (decoded, total) =
            decode_events_resilient(&raw, 0).expect("should not error on empty batch");
        assert_eq!(total, 0);
        assert_eq!(decoded.len(), 0);
    }
}
