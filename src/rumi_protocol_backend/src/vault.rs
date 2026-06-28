use crate::event::{
    record_add_margin_to_vault, record_borrow_from_vault, record_open_vault,
    record_redemption_on_vaults, record_repayed_to_vault,
};
use crate::guard::{GuardPrincipal, VaultLiquidationGuard};
use crate::logs::INFO;
use crate::management;
use crate::management::{
    mint_icusd, transfer_collateral, transfer_collateral_from, transfer_icusd_from,
    transfer_stable_from,
};
use crate::numeric::{Ratio, UsdIcp, ICP, ICUSD};
use crate::state::Mode;
use crate::GuardError;
use crate::PendingMarginTransfer;
use crate::DEBUG;
use crate::{
    mutate_state, read_state, ProtocolError, StabilityPoolLiquidationResult, StableTokenType,
    SuccessWithFee, VaultArgWithToken, DUST_THRESHOLD,
};
use candid::{CandidType, Deserialize, Principal};
use ic_canister_log::log;
use icrc_ledger_types::icrc2::transfer_from::TransferFromError;
use rust_decimal::prelude::{FromPrimitive, ToPrimitive};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::compute_collateral_ratio;

/// INT-003 defense in depth: clamp a raw borrow fee so `amount - fee >= 1 e8s`.
/// The validation cap on borrowing-fee curve multipliers (see
/// `state::MAX_BORROWING_FEE_MULTIPLIER`) is the primary fence; this clamp
/// protects against any path that bypasses validation (legacy state, future
/// drift) by ensuring `borrow_from_vault_internal::mint_icusd(amount - fee)`
/// never underflows. `min_icusd_amount` (the protocol's borrow minimum) is
/// orders of magnitude above 1 e8s, so the clamp never reduces a legitimate
/// fee.
pub fn clamp_borrow_fee(amount: ICUSD, raw_fee: ICUSD) -> ICUSD {
    raw_fee.min(amount.saturating_sub(ICUSD::new(1)))
}

/// Checks that a partial repayment won't leave the vault with dust debt below `min_vault_debt`.
/// Returns Ok(()) if remaining debt is zero or >= min_vault_debt, Err otherwise.
fn check_min_vault_debt_after_repay(
    vault: &Vault,
    repay_amount: ICUSD,
) -> Result<(), ProtocolError> {
    let remaining_debt = vault.borrowed_icusd_amount - repay_amount;
    if remaining_debt > ICUSD::new(0) {
        let min_vault_debt = read_state(|s| {
            s.get_collateral_config(&vault.collateral_type)
                .map(|c| c.min_vault_debt)
                .unwrap_or(ICUSD::new(0))
        });
        if remaining_debt < min_vault_debt {
            return Err(ProtocolError::GenericError(format!(
                "Partial repayment would leave {} icUSD debt, below the minimum of {}. \
                 Repay the full amount or leave at least {} icUSD.",
                remaining_debt, min_vault_debt, min_vault_debt
            )));
        }
    }
    Ok(())
}

/// LIQ-003: round a partial-liquidation amount up to the vault's full debt if
/// the residual would land in the open interval `(0, min_vault_debt)`. Mirrors
/// the dust-forgiveness pattern in `repay_to_vault`. The repay path enforces
/// `residual == 0 || residual >= min_vault_debt` via
/// `check_min_vault_debt_after_repay`; partial-liquidation endpoints must
/// enforce the same invariant on the residual after their cap math, otherwise
/// a liquidator could leave a vault with debt below `min_vault_debt` and bypass
/// the repay-side guarantee.
///
/// Returns the (possibly-rounded-up) amount the liquidator will actually
/// consume. The caller is responsible for pulling the corresponding icUSD
/// (or stable) from the liquidator and reducing vault debt by the returned
/// amount.
pub fn round_up_partial_liq_dust(
    vault: &Vault,
    proposed_amount: ICUSD,
    min_vault_debt: ICUSD,
) -> ICUSD {
    let residual = vault.borrowed_icusd_amount.saturating_sub(proposed_amount);
    if residual > ICUSD::new(0) && residual < min_vault_debt {
        vault.borrowed_icusd_amount
    } else {
        proposed_amount
    }
}

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

/// Returns zero ICUSD for serde default of `accrued_interest` field.
fn default_zero_icusd() -> ICUSD {
    ICUSD::new(0)
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
    /// Nanosecond timestamp of last interest accrual for this vault.
    /// Defaults to 0 for existing vaults (migration sets it in post_upgrade).
    #[serde(default)]
    pub last_accrual_time: u64,
    /// Accumulated interest on this vault's debt.
    /// Sub-component of `borrowed_icusd_amount` — tracks how much is interest vs principal.
    /// Defaults to 0 for existing vaults (backward compat).
    #[serde(default = "default_zero_icusd")]
    pub accrued_interest: ICUSD,
    /// True while the bot has claimed this vault for liquidation but hasn't
    /// confirmed or cancelled yet. Blocks ALL user operations on the vault.
    #[serde(default)]
    pub bot_processing: bool,
}

impl Vault {
    /// Compute the vault's health score: CR / liquidation_ratio.
    /// A score of 1.0 means the vault is at its liquidation threshold.
    /// Higher is healthier. Normalizes across collateral types so that
    /// vaults with different liquidation thresholds can be compared.
    ///
    /// `cr` — the vault's current collateral ratio (from compute_collateral_ratio)
    /// `liquidation_ratio` — the collateral type's liquidation threshold (e.g. 1.33)
    pub fn health_score(&self, cr: f64, liquidation_ratio: f64) -> f64 {
        if self.borrowed_icusd_amount == 0 {
            return f64::MAX;
        }
        if liquidation_ratio <= 0.0 {
            return f64::MAX; // defensive: avoid division by zero
        }
        cr / liquidation_ratio
    }
}

/// Returns an error if the vault is locked for bot processing.
pub fn require_vault_not_processing(vault: &Vault) -> Result<(), ProtocolError> {
    if vault.bot_processing {
        Err(ProtocolError::GenericError(format!(
            "Vault #{} is locked — bot liquidation in progress",
            vault.vault_id
        )))
    } else {
        Ok(())
    }
}

/// LIQ-101: vault-id wrapper around `require_vault_not_processing` for the
/// liquidation entry points (manual + SP), which only fetch the vault later in
/// their amount-computing read_state. If the liquidation bot has already
/// claimed this vault (`bot_processing` set, with the write-down deferred until
/// the bot's swap settles), a manual / stability-pool liquidation here would
/// seize the same collateral a second time. Mirrors the lock every user op
/// already honors. Absent vault => Ok (a later check surfaces "not found").
pub fn reject_if_bot_processing(vault_id: u64) -> Result<(), ProtocolError> {
    match read_state(|s| s.vault_id_to_vaults.get(&vault_id).cloned()) {
        Some(vault) => require_vault_not_processing(&vault),
        None => Ok(()),
    }
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
    /// Accumulated interest portion of the vault's debt (in e8s)
    pub accrued_interest: u64,
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
            accrued_interest: vault.accrued_interest.to_u64(),
        }
    }
}

/// Redeem icUSD for ckStable tokens from the protocol's reserves.
/// Two-tier system: reserves first (flat fee), then vault spillover (dynamic fee).
pub async fn redeem_reserves(
    icusd_amount_raw: u64,
    preferred_token: Option<Principal>,
) -> Result<crate::ReserveRedemptionResult, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let _guard_principal = GuardPrincipal::new(caller, "redeem_reserves")?;

    // RED-101 / RED-003: gate on protocol mode here too. The endpoint keeps its
    // own validate_mode() (defense in depth), but the spillover branch below
    // calls record_redemption_on_vaults DIRECTLY (it does not route through
    // redeem_collateral), so the redeem_collateral gate does not cover it. Gate
    // at this entry point so the reserve path is covered by construction as well.
    if read_state(|s| s.mode) == Mode::ReadOnly {
        return Err(ProtocolError::read_only_mode());
    }

    let icusd_amount: ICUSD = icusd_amount_raw.into();

    if icusd_amount < read_state(|s| s.min_icusd_amount) {
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: read_state(|s| s.min_icusd_amount).to_u64(),
        });
    }

    // Check reserve redemptions are enabled
    let (enabled, reserve_fee_ratio, ckusdt_ledger, ckusdc_ledger, treasury) = read_state(|s| {
        (
            s.reserve_redemptions_enabled,
            s.reserve_redemption_fee,
            s.ckusdt_ledger_principal,
            s.ckusdc_ledger_principal,
            s.treasury_principal,
        )
    });

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
        ckusdt_ledger.or(ckusdc_ledger).ok_or_else(|| {
            ProtocolError::GenericError("No reserve token ledgers configured.".to_string())
        })?
    };

    // Calculate fee (flat rate)
    let fee_icusd = icusd_amount * reserve_fee_ratio;
    let net_icusd = icusd_amount - fee_icusd;

    // Apply dynamic Redemption Margin Ratio: redeemers get RMR × face value
    let rmr = read_state(|s| s.get_redemption_margin_ratio());
    let effective_icusd = net_icusd * rmr;

    // Convert e8s (icUSD) to e6s (ckStable): divide by 100
    let net_e6s = effective_icusd.to_u64() / 100;
    let fee_e6s = fee_icusd.to_u64() / 100;

    if net_e6s == 0 {
        return Err(ProtocolError::GenericError(
            "Redemption amount too small after fee.".to_string(),
        ));
    }

    // Check reserve balance before pulling icUSD
    let reserve_balance = management::get_token_balance(stable_ledger)
        .await
        .map_err(|e| {
            ProtocolError::TemporarilyUnavailable(format!("Cannot query reserve balance: {}", e))
        })?;

    // Determine how much can come from reserves vs vault spillover.
    // Each ICRC-1 transfer also costs a ledger fee (deducted from sender balance).
    // Query the actual fee from the ledger rather than hardcoding.
    let ledger_fee = management::get_ledger_fee(stable_ledger)
        .await
        .unwrap_or(10_000); // fallback to 10_000 e6s (0.01 USD) if query fails
    let fee_budget = if fee_e6s > 0 {
        ledger_fee * 2
    } else {
        ledger_fee
    };
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
    let icusd_block_index = transfer_icusd_from(icusd_amount, caller)
        .await
        .map_err(|e| ProtocolError::TransferFromError(e, icusd_amount.to_u64()))?;

    // Transfer ckStable to user from reserves.
    // CRITICAL: If this fails we MUST refund the icUSD, otherwise the user
    // loses funds with nothing received. ICP inter-canister calls are NOT
    // atomic, so we implement the saga/compensation pattern manually.
    //
    // Wave-4 ICC-007: if the inline refund itself fails, persist a
    // `PendingRefund` keyed by `icusd_block_index`; `process_pending_transfer`
    // retries it until success or MAX_PENDING_RETRIES. The op_nonce is minted
    // once and reused across retries (idempotent at the icUSD ledger).
    if available_for_user > 0 {
        if let Err(transfer_err) =
            management::transfer_collateral(available_for_user, caller, stable_ledger).await
        {
            log!(
                crate::INFO,
                "[redeem_reserves] ckStable transfer failed for {}: {:?}. Refunding {} icUSD.",
                caller,
                transfer_err,
                icusd_amount.to_u64()
            );
            let refund_nonce = mutate_state(|s| s.next_op_nonce());
            match management::transfer_icusd_with_nonce(icusd_amount, caller, refund_nonce).await {
                Ok(refund_block) => {
                    log!(
                        crate::INFO,
                        "[redeem_reserves] Refunded {} icUSD to {} (block {})",
                        icusd_amount.to_u64(),
                        caller,
                        refund_block
                    );
                }
                Err(refund_err) => {
                    log!(crate::INFO,
                        "[redeem_reserves] ckStable transfer failed AND inline icUSD refund failed for {}! \
                         Amount: {} icUSD, ckStable error: {:?}, refund error: {:?}. \
                         Enqueueing durable refund (block {}).",
                        caller, icusd_amount.to_u64(), transfer_err, refund_err, icusd_block_index
                    );
                    mutate_state(|s| {
                        s.pending_refunds.insert(
                            icusd_block_index,
                            crate::state::PendingRefund {
                                user: caller,
                                amount_e8s: icusd_amount.to_u64(),
                                retry_count: 0,
                                op_nonce: refund_nonce,
                            },
                        );
                    });
                    ic_cdk_timers::set_timer(std::time::Duration::from_secs(2), || {
                        ic_cdk::spawn(crate::process_pending_transfer())
                    });
                }
            }
            return Err(ProtocolError::GenericError(format!(
                "Reserve transfer failed; your icUSD refund is in flight. Error: {:?}",
                transfer_err
            )));
        }
    }

    // Transfer fee to treasury (if configured), otherwise fee stays in reserves
    if fee_e6s > 0 {
        if let Some(treasury_principal) = treasury {
            if let Err(e) =
                management::transfer_collateral(fee_e6s, treasury_principal, stable_ledger).await
            {
                log!(crate::INFO,
                    "[redeem_reserves] WARNING: treasury fee transfer failed ({} e6s to {}): {:?}. Fee stays in reserves.",
                    fee_e6s, treasury_principal, e
                );
            }
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

    // Wave-8e LIQ-005: route the reserves-portion fee (in icUSD e8s)
    // through deficit repayment. The redeemer's icUSD was burned via
    // `transfer_icusd_from` above, so this is a pure state mutation.
    // The stablecoin fee transfer to treasury (line ~349) is unaffected
    // — that ckUSDT/ckUSDC payment is the actual revenue, the deficit
    // bookkeeping is the foregone-equity offset.
    if fee_icusd.0 > 0 {
        mutate_state(|s| {
            let _routing = crate::treasury::plan_fee_routing(
                s,
                fee_icusd,
                crate::event::FeeSource::RedemptionFee,
            );
        });
    }

    // Handle vault spillover if reserves didn't cover everything
    if spillover_e8s > 0 {
        // Pick the best collateral type for vault redemption based on tier priority
        let best_ct = read_state(|s| {
            s.get_collateral_types_by_redemption_priority()
                .first()
                .copied()
                .unwrap_or_else(|| s.icp_collateral_type())
        });
        // Wave-5 RED-001: spillover redeems against the best-priority collateral,
        // which may be non-ICP. validate_call only refreshes ICP. Refresh the
        // spillover collateral's price on-demand so the redeemer can't capture a
        // stale price during the 300s background-timer window.
        crate::xrc::ensure_fresh_price_for(&best_ct).await?;
        let collateral_price = read_state(|s| s.get_collateral_price_decimal(&best_ct)).ok_or(
            ProtocolError::TemporarilyUnavailable(
                "No price for vault spillover collateral".to_string(),
            ),
        )?;
        let current_price = UsdIcp::from(collateral_price);

        let refund_e8s = mutate_state(|s| {
            let spillover_icusd = ICUSD::from(spillover_e8s);
            // Wave-14b CDP-03: per-collateral fee path (see redeem_collateral
            // for full rationale). The spillover redeems against `best_ct`,
            // so the base rate is read from and written back to that
            // collateral's config alone.
            let base_fee = s.get_redemption_fee_for(&best_ct, spillover_icusd);
            crate::record_per_collateral_redemption_fee(s, &best_ct, base_fee, ic_cdk::api::time());
            let vault_fee = spillover_icusd * base_fee;

            // Note: RMR was already applied when computing spillover_e8s (line 160).
            // Do NOT apply it again here — that would double-discount.
            let effective_spillover = spillover_icusd - vault_fee;

            let outcome = record_redemption_on_vaults(
                s,
                caller,
                effective_spillover,
                vault_fee,
                current_price,
                icusd_block_index,
                best_ct,
            );

            // Wave-8e LIQ-005: route the spillover-portion fee through
            // deficit repayment. icUSD already burned via `transfer_icusd_from`.
            let _routing = crate::treasury::plan_fee_routing(
                s,
                vault_fee,
                crate::event::FeeSource::RedemptionFee,
            );

            // RED-001: unconsumed spillover is refunded (RMR already applied
            // upstream, so the unconsumed effective amount IS the raw refund;
            // the fee stays with the protocol as priced).
            effective_spillover
                .saturating_sub(outcome.consumed)
                .to_u64()
        });
        if refund_e8s > 0 {
            let refund_nonce = mutate_state(|s| s.next_op_nonce());
            match management::transfer_icusd_with_nonce(
                ICUSD::from(refund_e8s),
                caller,
                refund_nonce,
            )
            .await
            {
                Ok(refund_block) => {
                    log!(
                        crate::INFO,
                        "[redeem_reserves] Refunded {} unconsumed spillover icUSD to {} (block {})",
                        refund_e8s,
                        caller,
                        refund_block
                    );
                }
                Err(refund_err) => {
                    log!(crate::INFO,
                        "[redeem_reserves] Unconsumed-spillover refund of {} icUSD to {} failed: {:?}. Enqueueing durable refund (block {}).",
                        refund_e8s, caller, refund_err, icusd_block_index
                    );
                    mutate_state(|s| {
                        s.pending_refunds.insert(
                            icusd_block_index,
                            crate::state::PendingRefund {
                                user: caller,
                                amount_e8s: refund_e8s,
                                retry_count: 0,
                                op_nonce: refund_nonce,
                            },
                        );
                    });
                }
            }
        }
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
pub async fn redeem_collateral(
    collateral_type: Principal,
    _icusd_amount: u64,
) -> Result<SuccessWithFee, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let _guard_principal = GuardPrincipal::new(caller, "redeem_collateral")?;

    // RED-101 / RED-003: gate redemption on protocol mode at the shared internal
    // entry point, not just at the Candid endpoints. ReadOnly auto-latches on
    // insolvency (total collateral ratio < 100%, or the deficit account over its
    // threshold); redeeming then extracts collateral at oracle face value from an
    // already-insolvent protocol, deepening the bad-debt position. Placed after
    // the guard and before any icUSD is pulled so every redemption surface
    // (redeem_icp, redeem_collateral) is covered by construction. The Wave-9
    // fix lived only in main.rs::validate_mode and the redeem_icp endpoint
    // bypassed it. Same error as that gate via the shared constructor.
    if read_state(|s| s.mode) == Mode::ReadOnly {
        return Err(ProtocolError::read_only_mode());
    }

    let icusd_amount: ICUSD = _icusd_amount.into();

    if icusd_amount < read_state(|s| s.min_icusd_amount) {
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: read_state(|s| s.min_icusd_amount).to_u64(),
        });
    }

    // Input sanity: the caller-supplied collateral type must exist. The type
    // actually seized is the redemption-priority winner resolved below; the
    // caller's argument cannot target a specific collateral (priority order
    // is a protocol-level peg defense).
    if read_state(|s| s.get_collateral_status(&collateral_type)).is_none() {
        return Err(ProtocolError::GenericError(format!(
            "Collateral type {} not found.",
            collateral_type
        )));
    }

    // RED-002 (audit 2026-06-09): resolve the redemption-priority winner
    // BEFORE pulling icUSD, and key every check on it. Previously freshness,
    // fee pricing, and the base-rate bump used the caller-supplied type while
    // the water-fill seized the priority winner, so a redeemer could pass a
    // deep-debt type to floor the dynamic fee while draining a thin type
    // against a price with no staleness gate.
    let redeem_ct = read_state(|s| {
        s.get_collateral_types_by_redemption_priority()
            .first()
            .copied()
            .unwrap_or_else(|| s.icp_collateral_type())
    });

    let redeem_status = read_state(|s| s.get_collateral_status(&redeem_ct));
    if let Some(status) = redeem_status {
        if !status.allows_redemption() {
            return Err(ProtocolError::GenericError(format!(
                "Redemption is not allowed for collateral type {}.",
                redeem_ct
            )));
        }
    } else {
        return Err(ProtocolError::GenericError(format!(
            "Collateral type {} not found.",
            redeem_ct
        )));
    }

    // Fail closed on a stale price for the collateral actually being seized
    // (VER-001 ceiling applies inside ensure_fresh_price_for).
    crate::xrc::ensure_fresh_price_for(&redeem_ct).await?;

    let collateral_price = read_state(|s| s.get_collateral_price_decimal(&redeem_ct)).ok_or(
        ProtocolError::TemporarilyUnavailable("No price available for collateral".to_string()),
    )?;
    let current_collateral_price = UsdIcp::from(collateral_price);

    // RED-001 (audit 2026-06-09): reject claims that exceed what the
    // water-fill can consume. Without this, the full claim was burned and the
    // payout was computed from the claim rather than the consumed amount,
    // draining co-collateral vaults' shared backing for debt that was never
    // redeemed. The estimate uses the pre-pull fee/RMR; any residual gap
    // (state moving during the icUSD pull) is covered by the unconsumed
    // refund below.
    let (estimated_effective, total_redeemable) = read_state(|s| {
        let base_fee = s.get_redemption_fee_for(&redeem_ct, icusd_amount);
        let fee_est = icusd_amount * base_fee;
        let rmr = s.get_redemption_margin_ratio();
        (
            (icusd_amount - fee_est) * rmr,
            s.total_redeemable_debt_for(&redeem_ct),
        )
    });
    if estimated_effective > total_redeemable {
        return Err(ProtocolError::GenericError(format!(
            "Redemption exceeds redeemable debt for {}: effective claim {} > redeemable {}. Reduce the amount.",
            redeem_ct,
            estimated_effective.to_u64(),
            total_redeemable.to_u64()
        )));
    }

    match transfer_icusd_from(icusd_amount, caller).await {
        Ok(block_index) => {
            let (fee_amount, outcome, refund_e8s) = mutate_state(|s| {
                // Wave-14b CDP-03: price the fee against the per-collateral
                // base rate, and write the post-redemption rate back to the
                // per-collateral config (NOT the legacy global fields). A
                // redemption against one collateral no longer corrupts the
                // base rate used to price redemptions against any other.
                // RED-002: keyed on the seized collateral, not the caller's.
                let base_fee = s.get_redemption_fee_for(&redeem_ct, icusd_amount);
                crate::record_per_collateral_redemption_fee(
                    s,
                    &redeem_ct,
                    base_fee,
                    ic_cdk::api::time(),
                );
                let fee_amount = icusd_amount * base_fee;

                // Apply dynamic Redemption Margin Ratio: redeemers get RMR × face value
                let rmr = s.get_redemption_margin_ratio();
                let effective_icusd = (icusd_amount - fee_amount) * rmr;

                let outcome = record_redemption_on_vaults(
                    s,
                    caller,
                    effective_icusd,
                    fee_amount,
                    current_collateral_price,
                    block_index,
                    redeem_ct,
                );

                // RED-001: refund the unconsumed remainder of the claim in raw
                // icUSD (un-scale by RMR; the fee stays with the protocol as
                // priced). Floor division favors the protocol; capped at the
                // post-fee pull so a refund can never exceed what was taken.
                let unconsumed = effective_icusd.saturating_sub(outcome.consumed);
                let refund_e8s: u64 =
                    if unconsumed.to_u64() == 0 || rmr.0 <= rust_decimal::Decimal::ZERO {
                        0
                    } else {
                        let raw = (rust_decimal::Decimal::from(unconsumed.to_u64()) / rmr.0)
                            .to_u64()
                            .unwrap_or(0);
                        raw.min((icusd_amount - fee_amount).to_u64())
                    };

                // Wave-8e LIQ-005: route a configurable fraction of the
                // redemption fee toward deficit repayment. The redeemer's
                // icUSD has already been burned via `transfer_icusd_from`
                // (the protocol's main account is the icUSD minting
                // account), so the supply side is already correct — this
                // is a pure state mutation that decrements the deficit.
                let _routing = crate::treasury::plan_fee_routing(
                    s,
                    fee_amount,
                    crate::event::FeeSource::RedemptionFee,
                );

                (fee_amount, outcome, refund_e8s)
            });

            // RED-001: pay back the unconsumed icUSD. Same saga as
            // redeem_reserves (Wave-4 ICC-007): inline refund first, durable
            // `pending_refunds` entry (keyed by the unique burn block index,
            // nonce reused across retries) if the inline transfer fails.
            if refund_e8s > 0 {
                let refund_nonce = mutate_state(|s| s.next_op_nonce());
                match management::transfer_icusd_with_nonce(
                    ICUSD::from(refund_e8s),
                    caller,
                    refund_nonce,
                )
                .await
                {
                    Ok(refund_block) => {
                        log!(
                            INFO,
                            "[redeem_collateral] Refunded {} unconsumed icUSD to {} (block {})",
                            refund_e8s,
                            caller,
                            refund_block
                        );
                    }
                    Err(refund_err) => {
                        log!(INFO,
                            "[redeem_collateral] Unconsumed-claim refund of {} icUSD to {} failed: {:?}. Enqueueing durable refund (block {}).",
                            refund_e8s, caller, refund_err, block_index
                        );
                        mutate_state(|s| {
                            s.pending_refunds.insert(
                                block_index,
                                crate::state::PendingRefund {
                                    user: caller,
                                    amount_e8s: refund_e8s,
                                    retry_count: 0,
                                    op_nonce: refund_nonce,
                                },
                            );
                        });
                    }
                }
            }

            ic_cdk_timers::set_timer(std::time::Duration::from_secs(0), || {
                ic_cdk::spawn(crate::process_pending_transfer())
            });
            Ok(SuccessWithFee {
                block_index,
                fee_amount_paid: fee_amount.to_u64(),
                collateral_amount_received: Some(outcome.margin.to_u64()),
                debt_liquidated_e8s: None, // SP-101
                stable_pulled_e6s: None,   // SP-110
                xrp_claim_id: None,
            })
        }
        Err(transfer_from_error) => Err(ProtocolError::TransferFromError(
            transfer_from_error,
            icusd_amount.to_u64(),
        )),
    }
}

pub async fn open_vault(
    collateral_amount_raw: u64,
    collateral_type_opt: Option<Principal>,
) -> Result<OpenVaultSuccess, ProtocolError> {
    let caller = ic_cdk::api::caller();
    // Pass operation name to guard for better tracking
    let guard_principal = match GuardPrincipal::new(caller, "open_vault") {
        Ok(guard) => guard,
        Err(GuardError::AlreadyProcessing) => {
            log!(
                INFO,
                "[open_vault] Principal {:?} already has an ongoing operation",
                caller
            );
            return Err(ProtocolError::AlreadyProcessing);
        }
        Err(GuardError::StaleOperation) => {
            log!(
                INFO,
                "[open_vault] Principal {:?} has a stale operation that's being cleaned up",
                caller
            );
            return Err(ProtocolError::TemporarilyUnavailable(
                "Previous operation is being cleaned up. Please try again in a few seconds."
                    .to_string(),
            ));
        }
        Err(err) => return Err(err.into()),
    };

    // Resolve collateral type: default to ICP if not specified
    let collateral_type =
        collateral_type_opt.unwrap_or_else(|| read_state(|s| s.icp_collateral_type()));

    // Look up CollateralConfig; check status is Active
    let (config_ledger, config_status, min_deposit, is_native_xrp) =
        read_state(|s| match s.get_collateral_config(&collateral_type) {
            Some(config) => Ok((
                config.ledger_canister_id,
                config.status,
                config.min_collateral_deposit,
                config.is_native_xrp(),
            )),
            None => Err(ProtocolError::GenericError(
                "Collateral type not supported.".to_string(),
            )),
        })?;

    // P2: native-XRP collateral is custodied on the XRP Ledger (chains::xrp), not
    // pulled via an ICRC `transfer_from`. Its deposit flow (open-then-verify) is
    // wired in P3; until then reject opens through this ICRC path so XRP collateral
    // can never be silently mishandled as an ICRC token.
    if is_native_xrp {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "Native-XRP collateral uses the XRP deposit flow (not yet enabled).".to_string(),
        ));
    }

    if !config_status.allows_open() {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "Collateral type is not accepting new vaults.".to_string(),
        ));
    }

    let icp_margin_amount: ICP = collateral_amount_raw.into();

    if min_deposit > 0 && icp_margin_amount < ICP::new(min_deposit) {
        guard_principal.fail();
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: min_deposit,
        });
    }

    match transfer_collateral_from(collateral_amount_raw, caller, config_ledger).await {
        Ok(block_index) => {
            // Wrap state mutation in catch_unwind so that if vault record
            // creation panics (e.g. OOM), we can refund the collateral
            // instead of silently losing it.
            let vault_result = catch_unwind(AssertUnwindSafe(|| {
                mutate_state(|s| {
                    let vault_id = s.increment_vault_id();
                    record_open_vault(
                        s,
                        Vault {
                            owner: caller,
                            borrowed_icusd_amount: 0.into(),
                            collateral_amount: collateral_amount_raw,
                            vault_id,
                            collateral_type,
                            last_accrual_time: ic_cdk::api::time(),
                            accrued_interest: ICUSD::new(0),
                            bot_processing: false,
                        },
                        block_index,
                    );
                    vault_id
                })
            }));

            match vault_result {
                Ok(vault_id) => {
                    log!(INFO, "[open_vault] opened vault with id: {vault_id}");
                    guard_principal.complete();
                    Ok(OpenVaultSuccess {
                        vault_id,
                        block_index,
                    })
                }
                Err(panic_info) => {
                    // State mutation failed -- refund collateral to caller
                    log!(INFO,
                        "[open_vault] CRITICAL: vault record creation panicked after collateral transfer \
                         (block {}). Attempting refund of {} to {}. Panic: {:?}",
                        block_index, collateral_amount_raw, caller, panic_info
                    );

                    // Best-effort refund: transfer collateral back minus ledger fee
                    let ledger_fee = read_state(|s| {
                        s.get_collateral_config(&collateral_type)
                            .map(|c| c.ledger_fee)
                            .unwrap_or(10_000)
                    });
                    if collateral_amount_raw > ledger_fee {
                        match transfer_collateral(
                            collateral_amount_raw - ledger_fee,
                            caller,
                            config_ledger,
                        )
                        .await
                        {
                            Ok(refund_block) => {
                                log!(
                                    INFO,
                                    "[open_vault] Refunded {} collateral to {} (block {})",
                                    collateral_amount_raw - ledger_fee,
                                    caller,
                                    refund_block
                                );
                            }
                            Err(refund_err) => {
                                log!(INFO,
                                    "[open_vault] CRITICAL: collateral refund ALSO failed for {}! \
                                     Amount: {}, ledger: {}. Error: {:?}. Manual intervention required.",
                                    caller, collateral_amount_raw, config_ledger, refund_err
                                );
                            }
                        }
                    }
                    guard_principal.fail();
                    Err(ProtocolError::GenericError(
                        "Vault creation failed after collateral transfer. Your collateral has been refunded.".to_string()
                    ))
                }
            }
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
/// P3 return value for `open_xrp_vault`: the reserved vault id and the XRPL custody
/// address the user funds.
#[derive(candid::CandidType, Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct XrpVaultOpenInfo {
    pub vault_id: u64,
    pub custody_address: String,
    pub reserve_base_drops: u64,
}

fn require_xrp_production_key() -> Result<(), ProtocolError> {
    let configured_key = crate::chains::xrp::config::xrp_schnorr_key_name();
    if crate::chains::xrp::config::is_xrp_production_key_name(&configured_key) {
        return Ok(());
    }
    Err(ProtocolError::GenericError(format!(
        "native-XRP operations require production Schnorr key key_1 (configured: {configured_key})"
    )))
}

/// P3 (native-XRP collateral): open a vault in the open-then-verify staging area.
/// Derives the per-vault XRPL custody address (threshold Ed25519), records an
/// `XrpPendingDeposit` under a freshly reserved vault_id, and returns the address
/// for the user to fund. NO collateral is credited and NO icUSD is minted until
/// `confirm_xrp_deposit` verifies the deposit. Errors if native-XRP collateral is
/// not registered (P5) or is not accepting new vaults.
pub async fn open_xrp_vault() -> Result<XrpVaultOpenInfo, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal = GuardPrincipal::new(caller, "open_xrp_vault")?;
    if let Err(e) = require_xrp_production_key() {
        guard_principal.fail();
        return Err(e);
    }

    let xrp_ct = crate::state::xrp_collateral_principal();
    let cfg = read_state(|s| {
        s.get_collateral_config(&xrp_ct)
            .map(|c| (c.status, c.is_native_xrp()))
    });
    match cfg {
        Some((status, true)) => {
            if !status.allows_open() {
                guard_principal.fail();
                return Err(ProtocolError::GenericError(
                    "XRP collateral is not accepting new vaults.".to_string(),
                ));
            }
        }
        Some((_, false)) => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(
                "XRP collateral is misconfigured (custody is not native-XRP).".to_string(),
            ));
        }
        None => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(
                "XRP collateral is not registered.".to_string(),
            ));
        }
    }

    // Hardening (P3/P4 review): bound per-caller pending deposits so a caller can't
    // spam unfunded opens (each would consume a vault_id + a threshold derivation +
    // a persisted state entry).
    const MAX_XRP_PENDING_PER_CALLER: usize = 10;
    // Global cap bounds total persisted pending-deposit state (and the O(N) per-caller
    // scan below) across all callers — safe (refuses NEW opens when full; never
    // orphans an existing entry, unlike a TTL prune of a maybe-funded deposit).
    const MAX_XRP_PENDING_GLOBAL: usize = 10_000;
    let (global_pending, caller_pending) = read_state(|s| {
        (
            s.xrp_pending_deposits.len(),
            s.xrp_pending_deposits
                .values()
                .filter(|d| d.owner == caller)
                .count(),
        )
    });
    if global_pending >= MAX_XRP_PENDING_GLOBAL {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "XRP deposit staging is full; please retry after pending deposits clear.".to_string(),
        ));
    }
    if caller_pending >= MAX_XRP_PENDING_PER_CALLER {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "Too many open XRP deposits; confirm or settle existing ones first.".to_string(),
        ));
    }

    let reserve_base_drops = match crate::chains::xrp::xrp_rpc::fetch_reserve_base().await {
        Ok(r) => match u64::try_from(r) {
            Ok(drops) => drops,
            Err(_) => {
                guard_principal.fail();
                return Err(ProtocolError::GenericError(
                    "xrp reserve base exceeds u64 drops".to_string(),
                ));
            }
        },
        Err(e) => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(format!(
                "xrp server_state failed: {e}"
            )));
        }
    };

    // Reserve a vault_id (also the threshold-derivation nonce). A derive failure
    // below just leaves a gap in the id sequence, which is harmless.
    let vault_id = mutate_state(|s| s.increment_vault_id());

    let path = crate::chains::xrp::ted25519::custody_derivation_path(
        crate::chains::xrp::XRP_CHAIN_ID,
        caller,
        vault_id,
    );
    let custody_address = match crate::chains::xrp::ted25519::derive_xrp_address(path).await {
        Ok((_pubkey, addr)) => addr,
        Err(e) => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(format!(
                "xrp custody derive failed: {e}"
            )));
        }
    };

    let opened_at_ns = ic_cdk::api::time();
    mutate_state(|s| {
        s.xrp_pending_deposits.insert(
            vault_id,
            crate::state::XrpPendingDeposit {
                owner: caller,
                custody_address: custody_address.clone(),
                derivation_nonce: vault_id,
                opened_at_ns,
                reserve_base_drops,
            },
        );
    });

    guard_principal.complete();
    Ok(XrpVaultOpenInfo {
        vault_id,
        custody_address,
        reserve_base_drops,
    })
}

/// Pure: collateral drops to credit from a verified XRP custody balance, net of the
/// base reserve the user funds. Errors if nothing is creditable (balance ≤ reserve),
/// the net exceeds u64 drops, or the net is below the per-collateral minimum.
pub(crate) fn xrp_credit_amount(
    balance_drops: u128,
    reserve_base: u128,
    min_deposit: u64,
) -> Result<u64, ProtocolError> {
    let net = balance_drops.saturating_sub(reserve_base);
    let credited = u64::try_from(net)
        .map_err(|_| ProtocolError::GenericError("XRP balance exceeds u64 drops".to_string()))?;
    if credited == 0 || credited < min_deposit {
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: min_deposit.max(1),
        });
    }
    Ok(credited)
}

/// P3 (native-XRP collateral): verify the user's XRP deposit to the vault's custody
/// address and credit it as collateral, creating a real `Vault` with zero debt. The
/// user then borrows icUSD via the normal `borrow_from_vault` (the borrow→mint path
/// is collateral-generic and mints on the IC). Owner-only and idempotent: the
/// pending entry is removed on success, so a second call errors. Credits
/// `balance - reserve_base` drops — the user funds the XRPL base reserve returned
/// by server_state, which stays locked at the custody account. Returns the credited
/// drops.
pub async fn confirm_xrp_deposit(vault_id: u64) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal =
        GuardPrincipal::new(caller, &format!("confirm_xrp_deposit_{}", vault_id))?;
    if let Err(e) = require_xrp_production_key() {
        guard_principal.fail();
        return Err(e);
    }

    let pending = match read_state(|s| s.xrp_pending_deposits.get(&vault_id).cloned()) {
        Some(p) => p,
        None => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(
                "No pending XRP deposit for this vault (already confirmed or unknown).".to_string(),
            ));
        }
    };
    if pending.owner != caller {
        guard_principal.fail();
        return Err(ProtocolError::CallerNotOwner);
    }

    let xrp_ct = crate::state::xrp_collateral_principal();
    let min_deposit = read_state(|s| {
        s.get_collateral_config(&xrp_ct)
            .map(|c| c.min_collateral_deposit)
            .unwrap_or(0)
    });

    // Verify on the XRP Ledger (consensus-retry-wrapped reads).
    let acct = match crate::chains::xrp::xrp_rpc::fetch_account_info(&pending.custody_address).await
    {
        Ok(a) => a,
        Err(e) => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(format!(
                "xrp account_info failed: {e}"
            )));
        }
    };
    if !acct.exists {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "XRP custody account is unfunded; deposit not yet received.".to_string(),
        ));
    }
    let reserve = if pending.reserve_base_drops > 0 {
        u128::from(pending.reserve_base_drops)
    } else {
        match crate::chains::xrp::xrp_rpc::fetch_reserve_base().await {
            Ok(r) => r,
            Err(e) => {
                guard_principal.fail();
                return Err(ProtocolError::GenericError(format!(
                    "xrp server_state failed: {e}"
                )));
            }
        }
    };

    // Credit balance net of the reserve quoted when the deposit address was
    // prepared. Legacy pending deposits did not store it, so they fall back to a
    // live reserve read above.
    let credited = match xrp_credit_amount(acct.balance_drops, reserve, min_deposit) {
        Ok(c) => c,
        Err(e) => {
            guard_principal.fail();
            return Err(e);
        }
    };

    // Atomically: re-check the pending entry still exists (no concurrent confirm
    // slipped in during the awaits), create the vault, and clear the pending entry.
    let created = mutate_state(|s| {
        if !s.xrp_pending_deposits.contains_key(&vault_id) {
            return false;
        }
        record_open_vault(
            s,
            Vault {
                owner: caller,
                borrowed_icusd_amount: 0.into(),
                collateral_amount: credited,
                vault_id,
                collateral_type: xrp_ct,
                last_accrual_time: ic_cdk::api::time(),
                accrued_interest: ICUSD::new(0),
                bot_processing: false,
            },
            acct.ledger_index as u64,
        );
        s.xrp_pending_deposits.remove(&vault_id);
        true
    });

    if !created {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "XRP deposit was already confirmed concurrently.".to_string(),
        ));
    }

    guard_principal.complete();
    Ok(credited)
}

/// P4: record an unsettled XRP collateral claim and return its id. The OUT-paths
/// (withdraw / liquidation / redemption) call this instead of an ICRC transfer when
/// the collateral is native-XRP. `custody_owner`+`custody_nonce` (the source vault's
/// owner + id) locate the XRPL custody address the protocol later pays the claimant
/// from via `settle_xrp_claim`.
pub(crate) fn record_xrp_claim(
    s: &mut crate::state::State,
    claimant: Principal,
    custody_owner: Principal,
    custody_nonce: u64,
    drops: u64,
    now_ns: u64,
) -> u64 {
    let claim_id = s.next_xrp_claim_id;
    s.next_xrp_claim_id = s.next_xrp_claim_id.wrapping_add(1);
    s.xrp_claims.insert(
        claim_id,
        crate::state::XrpClaim {
            claimant,
            drops,
            custody_owner,
            custody_nonce,
            created_at_ns: now_ns,
            settlement: None,
            quarantine_reason: None,
        },
    );
    claim_id
}

/// P4: queue a collateral payout to `recipient`. ICRC collateral -> a
/// PendingMarginTransfer (the ICRC transfer machinery pays it). Native-XRP -> an
/// XrpClaim instead (settled later via settle_xrp_claim from the vault's custody
/// address); native-XRP therefore never enters the ICRC pending-transfer flow.
/// `custody_owner` is the SOURCE vault's owner (its threshold key controls the
/// custody address), captured while the vault is in hand — safe even when
/// cleanup_if_drained removes the vault immediately after.
fn queue_collateral_payout(
    s: &mut crate::state::State,
    vault_id: u64,
    custody_owner: Principal,
    recipient: Principal,
    margin: ICP,
    collateral_type: Principal,
    op_nonce: u128,
    now_ns: u64,
) -> Option<u64> {
    let is_xrp = s
        .get_collateral_config(&collateral_type)
        .map(|c| c.is_native_xrp())
        .unwrap_or(false);
    if is_xrp {
        Some(record_xrp_claim(
            s,
            recipient,
            custody_owner,
            vault_id,
            margin.to_u64(),
            now_ns,
        ))
    } else {
        s.pending_margin_transfers.insert(
            (vault_id, recipient),
            PendingMarginTransfer {
                owner: recipient,
                margin,
                collateral_type,
                retry_count: 0,
                op_nonce,
            },
        );
        None
    }
}

pub const XRP_SP_ABSORB_PREFLIGHT_TTL_NS: u64 = 15 * 60 * 1_000_000_000;

#[derive(Clone, Debug, PartialEq, Eq)]
struct XrpSpAbsorbSizing {
    preflight: crate::XrpSpAbsorbPreflight,
    total_to_seize_drops: u64,
}

fn ensure_registered_sp(
    state: &crate::state::State,
    caller: Principal,
) -> Result<(), ProtocolError> {
    if state.stability_pool_canister != Some(caller) {
        return Err(ProtocolError::GenericError(
            "Caller is not the registered stability pool canister".to_string(),
        ));
    }
    Ok(())
}

pub fn stability_pool_xrp_claim_outstanding_in_state(
    state: &crate::state::State,
    caller: Principal,
    claim_id: u64,
    claimant: Principal,
) -> Result<bool, ProtocolError> {
    ensure_registered_sp(state, caller)?;
    match state.xrp_claims.get(&claim_id) {
        Some(claim) if claim.claimant == claimant => Ok(true),
        Some(_) => Err(ProtocolError::GenericError(format!(
            "XRP claim #{claim_id} belongs to a different claimant"
        ))),
        None => Ok(false),
    }
}

fn xrp_sp_absorb_sizing(
    state: &crate::state::State,
    vault_id: u64,
    expected_icusd_burn_e8s: u64,
) -> Result<XrpSpAbsorbSizing, ProtocolError> {
    if expected_icusd_burn_e8s == 0 {
        return Err(ProtocolError::GenericError(
            "XRP SP absorb burn amount must be non-zero".to_string(),
        ));
    }
    let vault = state
        .vault_id_to_vaults
        .get(&vault_id)
        .ok_or_else(|| ProtocolError::GenericError(format!("Vault #{vault_id} not found")))?;
    let cfg = state
        .get_collateral_config(&vault.collateral_type)
        .ok_or_else(|| {
            ProtocolError::GenericError(format!("No collateral config for vault #{vault_id}"))
        })?;
    if !cfg.is_native_xrp() {
        return Err(ProtocolError::GenericError(
            "XRP SP absorb requires a native-XRP vault".to_string(),
        ));
    }
    if !cfg.status.allows_liquidation() {
        return Err(ProtocolError::GenericError(
            "Liquidation is not allowed for this collateral type.".to_string(),
        ));
    }
    if vault.borrowed_icusd_amount.to_u64() != expected_icusd_burn_e8s {
        return Err(ProtocolError::GenericError(format!(
            "XRP SP absorb burn {} does not match live debt {} for vault {}",
            expected_icusd_burn_e8s,
            vault.borrowed_icusd_amount.to_u64(),
            vault_id
        )));
    }
    let price = state
        .get_collateral_price_decimal(&vault.collateral_type)
        .ok_or_else(|| {
            ProtocolError::GenericError(
                "No price available for collateral. Price feed may be down.".to_string(),
            )
        })?;
    let price_usd = UsdIcp::from(price);
    let cr = compute_collateral_ratio(vault, price_usd, state);
    let min_liq = state.get_min_liquidation_ratio_for(&vault.collateral_type);
    if cr >= min_liq {
        return Err(ProtocolError::GenericError(format!(
            "native-XRP vault {vault_id} is no longer liquidatable"
        )));
    }

    let liquidation_amount = ICUSD::new(expected_icusd_burn_e8s);
    let collateral_raw =
        crate::numeric::icusd_to_collateral_amount(liquidation_amount, price, cfg.decimals);
    let collateral_with_bonus =
        ICP::from(collateral_raw) * state.get_liquidation_bonus_for(&vault.collateral_type);
    let total_to_seize = collateral_with_bonus.min(ICP::from(vault.collateral_amount));
    let total_to_seize_drops = total_to_seize.to_u64();
    let bonus_portion = total_to_seize_drops.saturating_sub(collateral_raw);
    let protocol_cut = (Decimal::from(bonus_portion) * state.get_liquidation_protocol_share().0)
        .to_u64()
        .unwrap_or(0)
        .min(total_to_seize_drops);
    let collateral_received_drops = total_to_seize_drops.saturating_sub(protocol_cut);
    if collateral_received_drops == 0 {
        return Err(ProtocolError::GenericError(
            "XRP SP absorb would receive zero collateral".to_string(),
        ));
    }

    Ok(XrpSpAbsorbSizing {
        preflight: crate::XrpSpAbsorbPreflight {
            vault_id,
            icusd_burn_e8s: expected_icusd_burn_e8s,
            collateral_received_drops,
            collateral_price_e8s: price_usd.to_e8s(),
            expires_at_ns: 0,
        },
        total_to_seize_drops,
    })
}

pub fn stability_pool_preflight_xrp_absorb_in_state(
    state: &mut crate::state::State,
    caller: Principal,
    vault_id: u64,
    expected_icusd_burn_e8s: u64,
    now_ns: u64,
) -> Result<crate::XrpSpAbsorbPreflight, ProtocolError> {
    ensure_registered_sp(state, caller)?;
    if state.frozen {
        return Err(ProtocolError::TemporarilyUnavailable(
            "Protocol is frozen. All operations are suspended pending admin review.".to_string(),
        ));
    }
    if state.liquidation_frozen {
        return Err(ProtocolError::TemporarilyUnavailable(
            "Liquidations are currently frozen by admin.".to_string(),
        ));
    }
    if state.sp_writedown_disabled {
        return Err(ProtocolError::TemporarilyUnavailable(
            "SP writedown path is disabled by admin".to_string(),
        ));
    }
    if crate::guard::is_vault_liquidating(vault_id) {
        return Err(ProtocolError::TemporarilyUnavailable(format!(
            "Vault #{vault_id} has another operation in flight; retry shortly"
        )));
    }

    let sizing = xrp_sp_absorb_sizing(state, vault_id, expected_icusd_burn_e8s)?;
    let mut preflight = sizing.preflight;
    preflight.expires_at_ns = now_ns.saturating_add(XRP_SP_ABSORB_PREFLIGHT_TTL_NS);
    state.sp_xrp_absorb_preflights.insert(
        vault_id,
        crate::state::StoredXrpSpAbsorbPreflight {
            caller,
            vault_id,
            icusd_burn_e8s: expected_icusd_burn_e8s,
            total_to_seize_drops: sizing.total_to_seize_drops,
            collateral_received_drops: preflight.collateral_received_drops,
            collateral_price_e8s: preflight.collateral_price_e8s,
            expires_at_ns: preflight.expires_at_ns,
        },
    );
    Ok(preflight)
}

fn matching_xrp_absorb_preflight(
    state: &crate::state::State,
    vault_id: u64,
    icusd_burned_e8s: u64,
    caller: Principal,
    now_ns: u64,
) -> Option<crate::state::StoredXrpSpAbsorbPreflight> {
    let preflight = state.sp_xrp_absorb_preflights.get(&vault_id)?;
    if preflight.caller == caller
        && preflight.icusd_burn_e8s == icusd_burned_e8s
        && preflight.expires_at_ns >= now_ns
    {
        Some(preflight.clone())
    } else {
        None
    }
}

fn ensure_no_active_xrp_sp_absorb_preflight(
    state: &crate::state::State,
    vault_id: u64,
    now_ns: u64,
) -> Result<(), ProtocolError> {
    if let Some(preflight) = state.sp_xrp_absorb_preflights.get(&vault_id) {
        if preflight.expires_at_ns >= now_ns {
            return Err(ProtocolError::TemporarilyUnavailable(format!(
                "Vault #{vault_id} has a pending native-XRP stability-pool liquidation reservation; retry after it expires or completes"
            )));
        }
    }
    Ok(())
}

fn reject_active_xrp_sp_absorb_preflight(
    vault_id: u64,
    now_ns: u64,
) -> Result<(), ProtocolError> {
    read_state(|s| ensure_no_active_xrp_sp_absorb_preflight(s, vault_id, now_ns))
}

fn ensure_xrp_sp_absorb_preflight_vault(
    state: &crate::state::State,
    vault_id: u64,
) -> Result<&Vault, ProtocolError> {
    let vault = state
        .vault_id_to_vaults
        .get(&vault_id)
        .ok_or_else(|| ProtocolError::GenericError(format!("Vault #{vault_id} not found")))?;
    let cfg = state
        .get_collateral_config(&vault.collateral_type)
        .ok_or_else(|| {
            ProtocolError::GenericError(format!("No collateral config for vault #{vault_id}"))
        })?;
    if !cfg.is_native_xrp() {
        return Err(ProtocolError::GenericError(
            "XRP SP absorb requires a native-XRP vault".to_string(),
        ));
    }
    Ok(vault)
}

fn canonical_xrp_allocations(
    allocations: &[crate::XrpSpPayoutAllocation],
) -> Vec<crate::XrpSpPayoutAllocation> {
    let mut sorted = allocations.to_vec();
    sorted.sort_by(|a, b| {
        a.claimant
            .as_slice()
            .cmp(b.claimant.as_slice())
            .then_with(|| a.payout_address.cmp(&b.payout_address))
            .then_with(|| a.destination_tag.cmp(&b.destination_tag))
            .then_with(|| a.drops.cmp(&b.drops))
    });
    sorted
}

fn validate_xrp_sp_allocations(
    allocations: &[crate::XrpSpPayoutAllocation],
    expected_drops: u64,
) -> Result<Vec<crate::XrpSpPayoutAllocation>, ProtocolError> {
    if allocations.is_empty() {
        return Err(ProtocolError::GenericError(
            "XRP SP absorb requires at least one payout allocation".to_string(),
        ));
    }
    if allocations.len() > crate::MAX_XRP_SP_PAYOUT_ALLOCATIONS {
        return Err(ProtocolError::GenericError(format!(
            "XRP SP absorb supports at most {} payout allocations",
            crate::MAX_XRP_SP_PAYOUT_ALLOCATIONS
        )));
    }
    let mut sum: u128 = 0;
    for allocation in allocations {
        if allocation.payout_address.trim().is_empty() {
            return Err(ProtocolError::GenericError(
                "XRP SP absorb payout address is required".to_string(),
            ));
        }
        if allocation.drops == 0 {
            return Err(ProtocolError::GenericError(
                "XRP SP absorb payout allocation drops must be non-zero".to_string(),
            ));
        }
        sum = sum.saturating_add(u128::from(allocation.drops));
    }
    if sum != u128::from(expected_drops) {
        return Err(ProtocolError::GenericError(format!(
            "XRP SP absorb allocation sum {} does not match collateral received {}",
            sum, expected_drops
        )));
    }
    Ok(canonical_xrp_allocations(allocations))
}

fn hash_len_prefixed(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

fn xrp_sp_allocation_fingerprint(
    caller: Principal,
    request: &crate::XrpSpAbsorbRequest,
    allocations: &[crate::XrpSpPayoutAllocation],
) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hash_len_prefixed(&mut hasher, caller.as_slice());
    hasher.update(request.vault_id.to_be_bytes());
    hasher.update(request.icusd_burned_e8s.to_be_bytes());
    let proof_kind = match request.proof.ledger_kind {
        crate::icrc3_proof::SpProofLedger::IcusdBurn => 0u8,
        crate::icrc3_proof::SpProofLedger::ThreePoolTransfer => 1u8,
    };
    hasher.update([proof_kind]);
    hasher.update(request.proof.block_index.to_be_bytes());
    hasher.update(request.proof.vault_id_memo.to_be_bytes());
    hasher.update((allocations.len() as u64).to_be_bytes());
    for allocation in allocations {
        hash_len_prefixed(&mut hasher, allocation.claimant.as_slice());
        hash_len_prefixed(&mut hasher, allocation.payout_address.as_bytes());
        match allocation.destination_tag {
            Some(tag) => {
                hasher.update([1u8]);
                hasher.update(tag.to_be_bytes());
            }
            None => hasher.update([0u8]),
        }
        hasher.update(allocation.drops.to_be_bytes());
    }
    hasher.finalize().to_vec()
}

fn stored_xrp_sp_absorb_matches_retry(
    stored: &crate::state::StoredXrpSpAbsorbResult,
    caller: Principal,
    request: &crate::XrpSpAbsorbRequest,
    allocation_fingerprint: &[u8],
) -> bool {
    stored.caller == caller
        && stored.vault_id == request.vault_id
        && stored.icusd_burned_e8s == request.icusd_burned_e8s
        && stored.proof_ledger == request.proof.ledger_kind
        && stored.proof_block_index == request.proof.block_index
        && stored.allocation_fingerprint == allocation_fingerprint
}

pub fn record_sp_xrp_absorb_result_bounded(
    state: &mut crate::state::State,
    proof_key: (crate::icrc3_proof::SpProofLedger, u64),
    stored: crate::state::StoredXrpSpAbsorbResult,
) {
    state
        .sp_xrp_absorb_results_by_proof
        .insert(proof_key, stored);
    while state.sp_xrp_absorb_results_by_proof.len()
        > crate::state::MAX_SP_XRP_ABSORB_RESULTS_BY_PROOF
    {
        let Some(oldest_key) = state
            .sp_xrp_absorb_results_by_proof
            .keys()
            .copied()
            .find(|key| *key != proof_key)
        else {
            break;
        };
        state.sp_xrp_absorb_results_by_proof.remove(&oldest_key);
    }
}

pub fn xrp_sp_absorb_cached_replay_result(
    state: &crate::state::State,
    caller: Principal,
    request: &crate::XrpSpAbsorbRequest,
) -> Option<Result<crate::XrpSpAbsorbResult, ProtocolError>> {
    let proof_key = (request.proof.ledger_kind, request.proof.block_index);
    let stored = state.sp_xrp_absorb_results_by_proof.get(&proof_key)?;
    Some(
        validate_xrp_sp_allocations(&request.allocations, stored.result.collateral_received_drops)
            .and_then(|allocations| {
                let fingerprint = xrp_sp_allocation_fingerprint(caller, request, &allocations);
                if stored_xrp_sp_absorb_matches_retry(stored, caller, request, &fingerprint) {
                    Ok(stored.result.clone())
                } else {
                    Err(ProtocolError::GenericError(format!(
                        "SP XRP absorb proof replay rejected: ({:?}, block {}) already consumed for a different request",
                        request.proof.ledger_kind, request.proof.block_index
                    )))
                }
            }),
    )
}

pub fn stability_pool_liquidate_xrp_vault_in_state(
    state: &mut crate::state::State,
    caller: Principal,
    request: crate::XrpSpAbsorbRequest,
    now_ns: u64,
) -> Result<crate::XrpSpAbsorbResult, ProtocolError> {
    ensure_registered_sp(state, caller)?;
    let proof_key = (request.proof.ledger_kind, request.proof.block_index);

    if let Some(stored) = state.sp_xrp_absorb_results_by_proof.get(&proof_key) {
        let allocations = validate_xrp_sp_allocations(
            &request.allocations,
            stored.result.collateral_received_drops,
        )?;
        let fingerprint = xrp_sp_allocation_fingerprint(caller, &request, &allocations);
        if stored_xrp_sp_absorb_matches_retry(stored, caller, &request, &fingerprint) {
            return Ok(stored.result.clone());
        }
        return Err(ProtocolError::GenericError(format!(
            "SP XRP absorb proof replay rejected: ({:?}, block {}) already consumed for a different request",
            request.proof.ledger_kind, request.proof.block_index
        )));
    }

    if request.proof.ledger_kind != crate::icrc3_proof::SpProofLedger::IcusdBurn {
        return Err(ProtocolError::GenericError(
            "XRP SP absorb requires an icUSD burn proof".to_string(),
        ));
    }
    if request.proof.vault_id_memo != request.vault_id {
        return Err(ProtocolError::GenericError(format!(
            "SP writedown proof vault_id_memo {} does not match call vault_id {}",
            request.proof.vault_id_memo, request.vault_id
        )));
    }
    if state.consumed_writedown_proofs.contains(&proof_key) {
        return Err(ProtocolError::GenericError(format!(
            "SP writedown proof replay rejected: ({:?}, block {}) already consumed",
            request.proof.ledger_kind, request.proof.block_index
        )));
    }

    let preflight = matching_xrp_absorb_preflight(
        state,
        request.vault_id,
        request.icusd_burned_e8s,
        caller,
        now_ns,
    )
    .ok_or_else(|| {
        ProtocolError::GenericError(
            "XRP SP absorb requires a matching unexpired preflight".to_string(),
        )
    })?;

    let allocations =
        validate_xrp_sp_allocations(&request.allocations, preflight.collateral_received_drops)?;
    let fingerprint = xrp_sp_allocation_fingerprint(caller, &request, &allocations);
    let custody_owner = ensure_xrp_sp_absorb_preflight_vault(state, request.vault_id)?.owner;

    let mut payout_claims = Vec::with_capacity(allocations.len());
    for allocation in &allocations {
        let claim_id = record_xrp_claim(
            state,
            allocation.claimant,
            custody_owner,
            request.vault_id,
            allocation.drops,
            now_ns,
        );
        payout_claims.push(crate::XrpSpPayoutClaim {
            claimant: allocation.claimant,
            claim_id,
            payout_address: allocation.payout_address.clone(),
            destination_tag: allocation.destination_tag,
            drops: allocation.drops,
        });
    }

    let mut interest_share = ICUSD::new(0);
    if let Some(vault) = state.vault_id_to_vaults.get_mut(&request.vault_id) {
        if vault.accrued_interest.0 > 0 && vault.borrowed_icusd_amount.0 > 0 {
            let share = (Decimal::from(request.icusd_burned_e8s)
                * Decimal::from(vault.accrued_interest.0)
                / Decimal::from(vault.borrowed_icusd_amount.0))
            .to_u64()
            .unwrap_or(0);
            interest_share = ICUSD::new(share.min(vault.accrued_interest.0));
        }
        let debt_applied = ICUSD::new(request.icusd_burned_e8s).min(vault.borrowed_icusd_amount);
        let collateral_applied = preflight.total_to_seize_drops.min(vault.collateral_amount);
        vault.borrowed_icusd_amount = vault.borrowed_icusd_amount.saturating_sub(debt_applied);
        vault.collateral_amount = vault.collateral_amount.saturating_sub(collateral_applied);
        vault.accrued_interest = vault.accrued_interest.saturating_sub(interest_share);
    }
    crate::state::record_recent_liquidation(state, request.icusd_burned_e8s, now_ns);
    state.cleanup_if_drained(request.vault_id);
    state.consumed_writedown_proofs.insert(proof_key);
    state.sp_xrp_absorb_preflights.remove(&request.vault_id);

    let result = crate::XrpSpAbsorbResult {
        success: true,
        vault_id: request.vault_id,
        liquidated_debt_e8s: request.icusd_burned_e8s,
        collateral_received_drops: preflight.collateral_received_drops,
        payout_claims,
        block_index: request.proof.block_index,
        collateral_price_e8s: preflight.collateral_price_e8s,
    };
    let stored = crate::state::StoredXrpSpAbsorbResult {
        caller,
        vault_id: request.vault_id,
        icusd_burned_e8s: request.icusd_burned_e8s,
        proof_ledger: request.proof.ledger_kind,
        proof_block_index: request.proof.block_index,
        allocation_fingerprint: fingerprint,
        result: result.clone(),
        accepted_at_ns: now_ns,
    };
    record_sp_xrp_absorb_result_bounded(state, proof_key, stored);
    Ok(result)
}

/// P5: true iff `vault_id` is a native-XRP-collateral vault (custody on the XRP
/// Ledger). Such vaults are excluded from AUTOMATED liquidation (the unhealthy-vault
/// scan + the stability-pool / bot entry points): the SP and bot cannot settle an
/// XrpClaim, so native-XRP is liquidated only MANUALLY by an external liquidator who
/// provides an XRP address (via liquidate_vault_partial / partial_liquidate_vault).
pub fn vault_is_native_xrp(vault_id: u64) -> bool {
    read_state(|s| {
        s.vault_id_to_vaults
            .get(&vault_id)
            .and_then(|v| s.get_collateral_config(&v.collateral_type))
            .map(|c| c.is_native_xrp())
            .unwrap_or(false)
    })
}

/// Pure: drops to actually send when settling a claim — the claimant bears the XRPL
/// network fee (sends `drops - fee`). Errors if the claim cannot cover the fee.
pub(crate) fn xrp_claim_send_amount(drops: u64, fee: u64) -> Result<u64, ProtocolError> {
    match drops.checked_sub(fee) {
        Some(n) if n > 0 => Ok(n),
        _ => Err(ProtocolError::AmountTooLow {
            minimum_amount: fee.saturating_add(1),
        }),
    }
}

pub(crate) fn xrp_unresolved_claim_drops_for_custody(
    s: &crate::state::State,
    custody_owner: Principal,
    custody_nonce: u64,
) -> Result<u128, ProtocolError> {
    s.xrp_claims
        .values()
        .filter(|claim| {
            claim.custody_owner == custody_owner && claim.custody_nonce == custody_nonce
        })
        .try_fold(0u128, |total, claim| {
            total.checked_add(u128::from(claim.drops)).ok_or_else(|| {
                ProtocolError::GenericError(
                    "Aggregate XRP claims exceed supported drops range".to_string(),
                )
            })
        })
}

pub(crate) fn xrp_inflight_claims_for_custody(
    s: &crate::state::State,
    current_claim_id: u64,
    custody_owner: Principal,
    custody_nonce: u64,
) -> Vec<(u64, crate::state::XrpSettlement)> {
    s.xrp_claims
        .iter()
        .filter_map(|(claim_id, claim)| {
            if *claim_id != current_claim_id
                && claim.custody_owner == custody_owner
                && claim.custody_nonce == custody_nonce
            {
                claim
                    .settlement
                    .as_ref()
                    .map(|settlement| (*claim_id, settlement.clone()))
            } else {
                None
            }
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum XrpSettlementReconciliation {
    Paid,
    FailedFeeCharged,
    ExpiredNotFound,
}

pub(crate) fn reconcile_xrp_settlement_snapshot(
    s: &mut crate::state::State,
    claim_id: u64,
    tx_hash: &str,
    outcome: XrpSettlementReconciliation,
) -> bool {
    let hash_matches = s
        .xrp_claims
        .get(&claim_id)
        .and_then(|claim| claim.settlement.as_ref())
        .map(|settlement| settlement.tx_hash == tx_hash)
        .unwrap_or(false);
    if !hash_matches {
        return false;
    }

    match outcome {
        XrpSettlementReconciliation::Paid => {
            s.xrp_claims.remove(&claim_id);
        }
        XrpSettlementReconciliation::FailedFeeCharged => {
            if let Some(claim) = s.xrp_claims.get_mut(&claim_id) {
                claim.drops = claim
                    .drops
                    .saturating_sub(crate::chains::xrp::adapter::XRP_FEE_DROPS);
                claim.settlement = None;
            }
        }
        XrpSettlementReconciliation::ExpiredNotFound => {
            if let Some(claim) = s.xrp_claims.get_mut(&claim_id) {
                claim.settlement = None;
            }
        }
    }
    true
}

/// F-03: durably flag a claim as quarantined because a settlement divergence is
/// suspected (its custody Sequence advanced past the recorded `source_sequence` while
/// the recorded tx_hash is NotFound on-ledger). Idempotent: keeps the first reason set,
/// so repeated detection does not overwrite the most precise diagnostic. While a claim
/// is quarantined `settle_xrp_claim` refuses to sign; an admin clears it via
/// `admin_resolve_xrp_claim`. Returns true if the claim exists.
pub(crate) fn quarantine_xrp_claim_snapshot(
    s: &mut crate::state::State,
    claim_id: u64,
    reason: &str,
) -> bool {
    if let Some(claim) = s.xrp_claims.get_mut(&claim_id) {
        if claim.quarantine_reason.is_none() {
            claim.quarantine_reason = Some(reason.to_string());
        }
        true
    } else {
        false
    }
}

/// F-03: apply an admin resolution to a quarantined claim after off-ledger
/// reconciliation. `confirm_paid = true` means the admin verified the divergent Payment
/// DID deliver -> remove the claim (no re-pay). `false` means it did NOT deliver -> clear
/// the quarantine + settlement so the claimant can retry settle and be paid exactly once.
/// Errors WITHOUT mutating if the claim is absent or not quarantined, so a resolve can
/// never silently drop a healthy, still-settle-able claim. `pub` for the main.rs endpoint.
pub fn resolve_quarantined_xrp_claim_snapshot(
    s: &mut crate::state::State,
    claim_id: u64,
    confirm_paid: bool,
) -> Result<(), ProtocolError> {
    match s.xrp_claims.get(&claim_id) {
        Some(c) if c.quarantine_reason.is_some() => {}
        Some(_) => {
            return Err(ProtocolError::GenericError(format!(
                "XRP claim #{claim_id} is not quarantined; refusing to resolve a healthy claim"
            )))
        }
        None => {
            return Err(ProtocolError::GenericError(format!(
                "No such XRP claim #{claim_id}"
            )))
        }
    }
    if confirm_paid {
        s.xrp_claims.remove(&claim_id);
    } else if let Some(c) = s.xrp_claims.get_mut(&claim_id) {
        c.quarantine_reason = None;
        c.settlement = None;
    }
    Ok(())
}

async fn reconcile_xrp_other_inflight_claims(
    current_claim_id: u64,
    custody_owner: Principal,
    custody_nonce: u64,
    acct: &crate::chains::xrp::xrp_rpc::XrpAccountInfo,
) -> Result<Option<u64>, ProtocolError> {
    let in_flight = read_state(|s| {
        xrp_inflight_claims_for_custody(s, current_claim_id, custody_owner, custody_nonce)
    });
    for (other_claim_id, settlement) in in_flight {
        let status = crate::chains::xrp::xrp_rpc::fetch_tx_status(&settlement.tx_hash)
            .await
            .map_err(|e| {
                ProtocolError::GenericError(format!(
                    "xrp tx status for claim #{other_claim_id} failed: {e}"
                ))
            })?;
        match xrp_sibling_reconcile_decision(&status, &settlement, acct) {
            XrpSiblingReconcileDecision::Paid => {
                mutate_state(|s| {
                    reconcile_xrp_settlement_snapshot(
                        s,
                        other_claim_id,
                        &settlement.tx_hash,
                        XrpSettlementReconciliation::Paid,
                    );
                });
            }
            XrpSiblingReconcileDecision::FailedFeeCharged => {
                mutate_state(|s| {
                    reconcile_xrp_settlement_snapshot(
                        s,
                        other_claim_id,
                        &settlement.tx_hash,
                        XrpSettlementReconciliation::FailedFeeCharged,
                    );
                });
            }
            XrpSiblingReconcileDecision::StillInFlight => {
                return Ok(Some(other_claim_id));
            }
            XrpSiblingReconcileDecision::ExpiredSafeToClear => {
                mutate_state(|s| {
                    reconcile_xrp_settlement_snapshot(
                        s,
                        other_claim_id,
                        &settlement.tx_hash,
                        XrpSettlementReconciliation::ExpiredNotFound,
                    );
                });
            }
            // F-03: the sibling's Payment consumed its source Sequence under a hash that
            // differs from the one we recorded, so it may already have paid out. Durably
            // quarantine the sibling (so future settles refuse it too), refuse to clear the
            // blocker (which would let it be re-signed and double-paid), and fail the
            // current settle closed; an admin resolves the sibling via admin_resolve_xrp_claim.
            XrpSiblingReconcileDecision::QuarantineDiverged => {
                let reason = format!(
                    "settlement diverged: custody account sequence advanced past source \
                     sequence while tx_hash {} is NotFound on-ledger; Payment may already \
                     have settled under a different hash",
                    settlement.tx_hash
                );
                mutate_state(|s| {
                    quarantine_xrp_claim_snapshot(s, other_claim_id, &reason);
                });
                return Err(ProtocolError::GenericError(format!(
                    "xrp sibling claim #{other_claim_id} quarantined ({reason}). Refusing to \
                     clear the blocker to avoid a double-pay; manual reconciliation required."
                )));
            }
        }
    }
    Ok(None)
}

pub(crate) fn ensure_xrp_claim_aggregate_solvency(
    acct: &crate::chains::xrp::xrp_rpc::XrpAccountInfo,
    reserve_drops: u128,
    unresolved_claim_drops: u128,
) -> Result<(), ProtocolError> {
    if !acct.exists {
        return Err(ProtocolError::GenericError(
            "XRP custody account is unfunded; cannot settle claim.".to_string(),
        ));
    }
    let required = reserve_drops
        .checked_add(unresolved_claim_drops)
        .ok_or_else(|| {
            ProtocolError::GenericError(
                "Aggregate XRP claims plus reserve exceed supported drops range".to_string(),
            )
        })?;
    if acct.balance_drops < required {
        return Err(ProtocolError::GenericError(format!(
            "insufficient XRP for unresolved claims: balance {} drops < aggregate claims {} + reserve {}",
            acct.balance_drops, unresolved_claim_drops, reserve_drops
        )));
    }
    Ok(())
}

pub(crate) fn ensure_xrp_replacement_sequence_safe(
    prev: &crate::state::XrpSettlement,
    acct: &crate::chains::xrp::xrp_rpc::XrpAccountInfo,
) -> Result<(), ProtocolError> {
    let Some(source_sequence) = prev.source_sequence else {
        return Err(ProtocolError::GenericError(
            "Cannot replace XRP settlement from legacy state without source sequence.".to_string(),
        ));
    };
    if acct.sequence > source_sequence {
        return Err(ProtocolError::GenericError(format!(
            "Cannot replace XRP settlement: source sequence advanced from {} to {}.",
            source_sequence, acct.sequence
        )));
    }
    if acct.sequence < source_sequence {
        return Err(ProtocolError::GenericError(format!(
            "Cannot replace XRP settlement: live source sequence {} is behind stored sequence {}.",
            acct.sequence, source_sequence
        )));
    }
    Ok(())
}

/// How to reconcile an OTHER in-flight sibling settlement (one sharing a custody
/// address with the claim currently being settled), given the sibling's on-ledger tx
/// status and the live custody account. Split out of `reconcile_xrp_other_inflight_claims`
/// so the F-03 sibling-divergence guard is unit-testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum XrpSiblingReconcileDecision {
    /// Sibling Payment validated on-ledger -> finalize (remove the sibling claim).
    Paid,
    /// Sibling Payment validated but failed (`tec*`) -> charge the fee once, clear blocker.
    FailedFeeCharged,
    /// Sibling Payment may still land (its LastLedgerSequence has not passed) -> back off
    /// until it confirms or expires.
    StillInFlight,
    /// Sibling Payment expired AND the live custody Sequence still equals its
    /// `source_sequence`, proving nothing consumed that Sequence (the Payment never
    /// applied under ANY hash) -> safe to clear the blocker.
    ExpiredSafeToClear,
    /// Sibling Payment is NotFound by our local hash and expired, but the live custody
    /// Sequence has ADVANCED past its `source_sequence`. On the XRPL a sequence is
    /// consumed strictly in order and only by a transaction from that account, so the
    /// sibling's own Payment provably consumed it — under a hash that differs from the one
    /// we recorded (the F-03 codec/canonicalization divergence). It may already have paid
    /// the claimant, so the blocker must NOT be cleared (clearing would let the sibling be
    /// re-signed and double-paid). Quarantine for manual reconciliation.
    QuarantineDiverged,
}

/// Pure F-03 guard. The NotFound branch is gated on `ensure_xrp_replacement_sequence_safe`
/// exactly like the primary settle path (`settle_xrp_claim_with_tag`); previously the
/// sibling-reconcile path cleared the blocker on expiry WITHOUT a Sequence check, which
/// let a diverged-hash sibling be reset to `settlement = None` and re-paid a second time.
pub(crate) fn xrp_sibling_reconcile_decision(
    status: &crate::chains::xrp::xrp_rpc::XrpTxStatus,
    settlement: &crate::state::XrpSettlement,
    acct: &crate::chains::xrp::xrp_rpc::XrpAccountInfo,
) -> XrpSiblingReconcileDecision {
    use crate::chains::xrp::xrp_rpc::XrpTxStatus;
    match status {
        XrpTxStatus::Validated { .. } => XrpSiblingReconcileDecision::Paid,
        XrpTxStatus::Failed => XrpSiblingReconcileDecision::FailedFeeCharged,
        XrpTxStatus::NotFound => {
            if acct.ledger_index <= settlement.last_ledger_sequence {
                XrpSiblingReconcileDecision::StillInFlight
            } else if ensure_xrp_replacement_sequence_safe(settlement, acct).is_ok() {
                XrpSiblingReconcileDecision::ExpiredSafeToClear
            } else {
                XrpSiblingReconcileDecision::QuarantineDiverged
            }
        }
    }
}

pub(crate) fn remove_xrp_pending_deposit_if_unfunded_snapshot(
    s: &mut crate::state::State,
    vault_id: u64,
    expected: &crate::state::XrpPendingDeposit,
    acct: &crate::chains::xrp::xrp_rpc::XrpAccountInfo,
) -> Result<bool, ProtocolError> {
    if acct.exists {
        return Err(ProtocolError::GenericError(
            "XRP custody account is funded; confirm the deposit instead of cancelling.".to_string(),
        ));
    }
    match s.xrp_pending_deposits.get(&vault_id) {
        Some(current) if current == expected => {
            s.xrp_pending_deposits.remove(&vault_id);
            Ok(true)
        }
        Some(_) | None => Ok(false),
    }
}

const XRP_PENDING_CLEANUP_MIN_AGE_NS: u64 = 10 * 60 * 1_000_000_000;

pub(crate) fn ensure_xrp_pending_cleanup_age(
    pending: &crate::state::XrpPendingDeposit,
    now_ns: u64,
) -> Result<(), ProtocolError> {
    let age_ns = now_ns.saturating_sub(pending.opened_at_ns);
    if age_ns < XRP_PENDING_CLEANUP_MIN_AGE_NS {
        return Err(ProtocolError::GenericError(
            "XRP pending deposit is too new to cancel; wait for the XRPL funding window to pass."
                .to_string(),
        ));
    }
    Ok(())
}

/// XRP-006: owner cleanup for an unfunded native-XRP open. This never removes a
/// funded custody account; users must confirm funded deposits into real vaults.
pub async fn cancel_xrp_pending_open(vault_id: u64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal =
        GuardPrincipal::new(caller, &format!("cancel_xrp_pending_open_{}", vault_id))?;

    let pending = match read_state(|s| s.xrp_pending_deposits.get(&vault_id).cloned()) {
        Some(p) => p,
        None => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(
                "No pending XRP deposit for this vault.".to_string(),
            ));
        }
    };
    if pending.owner != caller {
        guard_principal.fail();
        return Err(ProtocolError::CallerNotOwner);
    }
    if let Err(e) = ensure_xrp_pending_cleanup_age(&pending, ic_cdk::api::time()) {
        guard_principal.fail();
        return Err(e);
    }

    let acct = match crate::chains::xrp::xrp_rpc::fetch_account_info(&pending.custody_address).await
    {
        Ok(a) => a,
        Err(e) => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(format!(
                "xrp account_info failed: {e}"
            )));
        }
    };

    let removed = mutate_state(|s| {
        ensure_xrp_pending_cleanup_age(&pending, ic_cdk::api::time())?;
        remove_xrp_pending_deposit_if_unfunded_snapshot(s, vault_id, &pending, &acct)
    })?;
    if !removed {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "XRP pending deposit changed concurrently; refresh and retry.".to_string(),
        ));
    }

    guard_principal.complete();
    Ok(())
}

/// XRP-006: developer cleanup for abandoned unfunded native-XRP opens. This is
/// deliberately unfunded-only; if XRP has reached the custody address the entry
/// must remain confirmable by its owner.
pub async fn sweep_xrp_pending_open(vault_id: u64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::api::caller();
    if !read_state(|s| s.developer_principal == caller) {
        return Err(ProtocolError::GenericError(
            "Only the developer can sweep XRP pending opens.".to_string(),
        ));
    }
    let guard_principal =
        GuardPrincipal::new(caller, &format!("sweep_xrp_pending_open_{}", vault_id))?;

    let pending = match read_state(|s| s.xrp_pending_deposits.get(&vault_id).cloned()) {
        Some(p) => p,
        None => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(
                "No pending XRP deposit for this vault.".to_string(),
            ));
        }
    };
    if let Err(e) = ensure_xrp_pending_cleanup_age(&pending, ic_cdk::api::time()) {
        guard_principal.fail();
        return Err(e);
    }

    let acct = match crate::chains::xrp::xrp_rpc::fetch_account_info(&pending.custody_address).await
    {
        Ok(a) => a,
        Err(e) => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(format!(
                "xrp account_info failed: {e}"
            )));
        }
    };

    let removed = mutate_state(|s| {
        ensure_xrp_pending_cleanup_age(&pending, ic_cdk::api::time())?;
        remove_xrp_pending_deposit_if_unfunded_snapshot(s, vault_id, &pending, &acct)
    })?;
    if !removed {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "XRP pending deposit changed concurrently; refresh and retry.".to_string(),
        ));
    }

    guard_principal.complete();
    Ok(())
}

/// P4: settle an XRP claim — sign + submit a `Payment` from the source vault's
/// custody address (re-derived from the claim) to `destination`, for
/// `claim.drops - fee` (the claimant bears the fee). Claimant-only. One in-flight
/// Payment per custody address (sequence serialization, keyed on the source vault
/// id). On success the claim is removed; returns the locally computed tx hash.
pub async fn settle_xrp_claim(claim_id: u64, destination: String) -> Result<String, ProtocolError> {
    settle_xrp_claim_with_tag(claim_id, destination, None).await
}

pub async fn settle_xrp_claim_with_tag(
    claim_id: u64,
    destination: String,
    destination_tag: Option<u32>,
) -> Result<String, ProtocolError> {
    let caller = ic_cdk::api::caller();
    require_xrp_production_key()?;
    let mut claim = match read_state(|s| s.xrp_claims.get(&claim_id).cloned()) {
        Some(c) => c,
        None => {
            return Err(ProtocolError::GenericError(
                "No such XRP claim (already settled or unknown).".to_string(),
            ))
        }
    };
    let mut replacement_destination = destination;
    let mut replacement_destination_tag = destination_tag;
    if claim.claimant != caller {
        return Err(ProtocolError::CallerNotOwner);
    }

    // F-03: a quarantined claim may already have been paid under a divergent hash.
    // Refuse to sign anything until an admin resolves it (admin_resolve_xrp_claim).
    if let Some(reason) = claim.quarantine_reason.clone() {
        return Err(ProtocolError::GenericError(format!(
            "XRP claim #{claim_id} is quarantined ({reason}); awaiting admin reconciliation."
        )));
    }

    let guard_principal = GuardPrincipal::new(caller, &format!("settle_xrp_claim_{}", claim_id))?;
    // Per-custody-address sequence serialization: custody_nonce == the source vault
    // id, so this per-vault lock prevents two concurrent Payments from one custody
    // address colliding on the XRPL Sequence.
    let _seq_guard = match VaultLiquidationGuard::new(claim.custody_nonce) {
        Ok(g) => g,
        Err(e) => {
            guard_principal.fail();
            return Err(e);
        }
    };

    let path = crate::chains::xrp::ted25519::custody_derivation_path(
        crate::chains::xrp::XRP_CHAIN_ID,
        claim.custody_owner,
        claim.custody_nonce,
    );

    // Idempotency (anti double-pay): if a settlement Payment was already
    // signed+submitted for this claim, CONFIRM it before signing a new one. A submit
    // outcall can error AFTER rippled already broadcast the tx; without this check a
    // retry would read the bumped account Sequence and send a second distinct
    // Payment, paying the claimant twice out of the custody address.
    if let Some(prev) = claim.settlement.clone() {
        match crate::chains::xrp::xrp_rpc::fetch_tx_status(&prev.tx_hash).await {
            Ok(crate::chains::xrp::xrp_rpc::XrpTxStatus::Validated { .. }) => {
                // Already paid on-chain — finalize by removing the claim.
                mutate_state(|s| {
                    s.xrp_claims.remove(&claim_id);
                });
                guard_principal.complete();
                return Ok(prev.tx_hash);
            }
            Ok(crate::chains::xrp::xrp_rpc::XrpTxStatus::NotFound) => {
                // Not validated yet. Only sign a fresh tx if the prior one can NEVER
                // apply anymore (its LastLedgerSequence has passed); otherwise it may
                // still land, so refuse to sign a second one.
                let addr =
                    match crate::chains::xrp::ted25519::derive_xrp_address(path.clone()).await {
                        Ok((_pk, addr)) => addr,
                        Err(e) => {
                            guard_principal.fail();
                            return Err(ProtocolError::GenericError(format!(
                                "xrp derive failed: {e}"
                            )));
                        }
                    };
                let acct = match crate::chains::xrp::xrp_rpc::fetch_account_info(&addr).await {
                    Ok(a) => a,
                    Err(e) => {
                        guard_principal.fail();
                        return Err(ProtocolError::GenericError(format!(
                            "xrp account_info failed: {e}"
                        )));
                    }
                };
                if acct.ledger_index <= prev.last_ledger_sequence {
                    guard_principal.fail();
                    return Err(ProtocolError::GenericError(
                        "XRP settlement already in flight; retry once it confirms or expires."
                            .to_string(),
                    ));
                }
                if let Err(e) = ensure_xrp_replacement_sequence_safe(&prev, &acct) {
                    // F-03: the prior Payment's source Sequence was consumed (under some
                    // hash) yet our recorded tx_hash is NotFound on-ledger — a divergence.
                    // Durably quarantine so future settles short-circuit, then fail closed.
                    let reason = format!(
                        "settlement diverged: {:?} (tx_hash {} NotFound on-ledger)",
                        e, prev.tx_hash
                    );
                    mutate_state(|s| {
                        quarantine_xrp_claim_snapshot(s, claim_id, &reason);
                    });
                    guard_principal.fail();
                    return Err(e);
                }
                if replacement_destination.trim().is_empty() {
                    replacement_destination = match prev.destination.clone() {
                        Some(dest) => dest,
                        None => {
                            guard_principal.fail();
                            return Err(ProtocolError::GenericError(
                                "XRP settlement replacement requires a destination address."
                                    .to_string(),
                            ));
                        }
                    };
                }
                if replacement_destination_tag.is_none() {
                    replacement_destination_tag = prev.destination_tag;
                }
                // Expired and never applied -> safe to sign a fresh settlement.
            }
            Ok(crate::chains::xrp::xrp_rpc::XrpTxStatus::Failed) => {
                // Validated but failed -> funds did not move, but XRPL still
                // consumed the source-account fee. Charge it to this claim once,
                // clear the failed settlement, and allow a replacement.
                let fee = crate::chains::xrp::adapter::XRP_FEE_DROPS;
                claim.drops = claim.drops.saturating_sub(fee);
                mutate_state(|s| {
                    if let Some(c) = s.xrp_claims.get_mut(&claim_id) {
                        if c.settlement
                            .as_ref()
                            .map(|settlement| settlement.tx_hash == prev.tx_hash)
                            .unwrap_or(false)
                        {
                            c.drops = claim.drops;
                            c.settlement = None;
                        }
                    }
                });
                if replacement_destination.trim().is_empty() {
                    replacement_destination = prev.destination.clone().unwrap_or_default();
                }
                if replacement_destination_tag.is_none() {
                    replacement_destination_tag = prev.destination_tag;
                }
            }
            Err(e) => {
                guard_principal.fail();
                return Err(ProtocolError::GenericError(format!(
                    "xrp tx status failed: {e}"
                )));
            }
        }
    }

    // Sign a fresh settlement Payment (claimant bears the XRPL fee).
    let send_drops =
        match xrp_claim_send_amount(claim.drops, crate::chains::xrp::adapter::XRP_FEE_DROPS) {
            Ok(n) => n,
            Err(e) => {
                guard_principal.fail();
                return Err(e);
            }
        };

    let (source_address, acct) =
        match crate::chains::xrp::ted25519::derive_xrp_address(path.clone()).await {
            Ok((_pk, addr)) => match crate::chains::xrp::xrp_rpc::fetch_account_info(&addr).await {
                Ok(a) => (addr, a),
                Err(e) => {
                    guard_principal.fail();
                    return Err(ProtocolError::GenericError(format!(
                        "xrp account_info failed: {e}"
                    )));
                }
            },
            Err(e) => {
                guard_principal.fail();
                return Err(ProtocolError::GenericError(format!(
                    "xrp derive failed: {e}"
                )));
            }
        };
    let reserve = match crate::chains::xrp::xrp_rpc::fetch_reserve_base().await {
        Ok(r) => r,
        Err(e) => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(format!(
                "xrp server_state failed: {e}"
            )));
        }
    };
    let blocking_other_claim = match reconcile_xrp_other_inflight_claims(
        claim_id,
        claim.custody_owner,
        claim.custody_nonce,
        &acct,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            guard_principal.fail();
            return Err(e);
        }
    };
    if let Some(other_claim_id) = blocking_other_claim {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(format!(
            "XRP settlement for claim #{other_claim_id} is already in flight for this custody address; confirm it before settling another claim."
        )));
    }
    let unresolved_claim_drops = match read_state(|s| {
        xrp_unresolved_claim_drops_for_custody(s, claim.custody_owner, claim.custody_nonce)
    }) {
        Ok(drops) => drops,
        Err(e) => {
            guard_principal.fail();
            return Err(e);
        }
    };
    if let Err(e) = ensure_xrp_claim_aggregate_solvency(&acct, reserve, unresolved_claim_drops) {
        log!(
            INFO,
            "[settle_xrp_claim] aggregate solvency rejected claim #{} from {}: {:?}",
            claim_id,
            source_address,
            e
        );
        guard_principal.fail();
        return Err(e);
    }

    let adapter = crate::chains::xrp::adapter::XrpAdapter::new(crate::chains::xrp::XRP_CHAIN_ID);
    let payment = match adapter
        .sign_xrp_payment_from(
            path,
            &replacement_destination,
            send_drops as u128,
            replacement_destination_tag,
        )
        .await
    {
        Ok(v) => v,
        Err(e) => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(format!(
                "xrp claim sign failed: {e:?}"
            )));
        }
    };
    let signed = payment.signed;

    // Record the in-flight settlement BEFORE submitting, so a submit whose outcall
    // errors after rippled broadcast is reconciled on the next settle (confirm) call
    // rather than double-paid.
    mutate_state(|s| {
        if let Some(c) = s.xrp_claims.get_mut(&claim_id) {
            c.settlement = Some(crate::state::XrpSettlement {
                tx_hash: signed.tx_hash.clone(),
                last_ledger_sequence: payment.last_ledger_sequence,
                source_sequence: Some(payment.source_sequence),
                destination: Some(replacement_destination.clone()),
                destination_tag: replacement_destination_tag,
            });
        }
    });

    if let Err(e) =
        crate::chains::xrp::xrp_rpc::submit_blob(&hex::encode_upper(&signed.raw_tx)).await
    {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(format!(
            "xrp claim submit failed (call settle again to confirm or retry): {e}"
        )));
    }

    // Submitted. The claim is removed only once the tx is confirmed Validated on a
    // later settle call; until then it keeps `settlement` set so a retry confirms
    // instead of re-paying. Return the locally computed hash.
    guard_principal.complete();
    Ok(signed.tx_hash)
}

#[cfg(test)]
mod xrp_p4_tests {
    use super::*;

    fn claim(
        claimant: Principal,
        custody_owner: Principal,
        custody_nonce: u64,
        drops: u64,
    ) -> crate::state::XrpClaim {
        crate::state::XrpClaim {
            claimant,
            drops,
            custody_owner,
            custody_nonce,
            created_at_ns: 0,
            settlement: None,
            quarantine_reason: None,
        }
    }

    fn settlement(tx_hash: &str) -> crate::state::XrpSettlement {
        crate::state::XrpSettlement {
            tx_hash: tx_hash.to_string(),
            last_ledger_sequence: 9_000_000,
            source_sequence: Some(41),
            destination: Some("rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh".to_string()),
            destination_tag: None,
        }
    }

    #[test]
    fn claim_send_amount_subtracts_fee() {
        assert_eq!(xrp_claim_send_amount(1_000_000, 20).unwrap(), 999_980);
    }

    #[test]
    fn claim_send_amount_rejects_at_or_below_fee() {
        assert!(matches!(
            xrp_claim_send_amount(20, 20),
            Err(ProtocolError::AmountTooLow { .. })
        ));
        assert!(matches!(
            xrp_claim_send_amount(5, 20),
            Err(ProtocolError::AmountTooLow { .. })
        ));
    }

    #[test]
    fn record_xrp_claim_allocates_incrementing_ids() {
        let mut s = crate::state::State::default();
        let owner = Principal::from_slice(&[0xaa; 16]);
        let liq = Principal::from_slice(&[0xbb; 16]);
        let id0 = record_xrp_claim(&mut s, liq, owner, 7, 4_000_000, 100);
        let id1 = record_xrp_claim(&mut s, owner, owner, 8, 1_000_000, 200);
        assert_eq!(id0, 0);
        assert_eq!(id1, 1);
        assert_eq!(s.next_xrp_claim_id, 2);
        let c0 = s.xrp_claims.get(&id0).unwrap();
        assert_eq!(c0.claimant, liq);
        assert_eq!(c0.custody_owner, owner);
        assert_eq!(c0.custody_nonce, 7);
        assert_eq!(c0.drops, 4_000_000);
    }

    #[test]
    fn sp_xrp_claim_status_requires_registered_pool() {
        let mut s = crate::state::State::default();
        let sp = Principal::from_slice(&[0x5a; 16]);
        let claimant = Principal::from_slice(&[0x11; 16]);
        let owner = Principal::from_slice(&[0xaa; 16]);
        s.stability_pool_canister = Some(sp);
        s.xrp_claims.insert(7, claim(claimant, owner, 1, 2_000_000));

        let err = stability_pool_xrp_claim_outstanding_in_state(
            &s,
            Principal::from_slice(&[0xee; 16]),
            7,
            claimant,
        )
        .unwrap_err();
        assert!(matches!(err, ProtocolError::GenericError(_)));
    }

    #[test]
    fn sp_xrp_claim_status_reports_matching_claim_only() {
        let mut s = crate::state::State::default();
        let sp = Principal::from_slice(&[0x5a; 16]);
        let claimant = Principal::from_slice(&[0x11; 16]);
        let other_claimant = Principal::from_slice(&[0x22; 16]);
        let owner = Principal::from_slice(&[0xaa; 16]);
        s.stability_pool_canister = Some(sp);
        s.xrp_claims.insert(7, claim(claimant, owner, 1, 2_000_000));

        assert_eq!(
            stability_pool_xrp_claim_outstanding_in_state(&s, sp, 7, claimant).unwrap(),
            true
        );
        assert_eq!(
            stability_pool_xrp_claim_outstanding_in_state(&s, sp, 8, claimant).unwrap(),
            false
        );
        assert!(stability_pool_xrp_claim_outstanding_in_state(&s, sp, 7, other_claimant).is_err());
    }

    #[test]
    fn native_xrp_withdraw_and_close_policy_preserves_custody_vault() {
        assert_eq!(
            withdraw_close_completion_policy(true),
            WithdrawCloseCompletionPolicy::KeepNativeXrpVaultOpen
        );
        assert_eq!(
            withdraw_close_completion_policy(false),
            WithdrawCloseCompletionPolicy::CloseVault
        );
    }

    #[test]
    fn unresolved_claim_drops_aggregates_only_same_custody_address() {
        let mut s = crate::state::State::default();
        let claimant = Principal::from_slice(&[0x11; 16]);
        let owner = Principal::from_slice(&[0xaa; 16]);
        let other_owner = Principal::from_slice(&[0xbb; 16]);
        s.xrp_claims.insert(0, claim(claimant, owner, 7, 2_000_000));
        s.xrp_claims.insert(1, claim(claimant, owner, 7, 3_000_000));
        s.xrp_claims.insert(2, claim(claimant, owner, 8, 5_000_000));
        s.xrp_claims
            .insert(3, claim(claimant, other_owner, 7, 7_000_000));

        assert_eq!(
            xrp_unresolved_claim_drops_for_custody(&s, owner, 7).unwrap(),
            5_000_000
        );
    }

    #[test]
    fn inflight_claims_for_custody_detects_same_custody_only() {
        let mut s = crate::state::State::default();
        let claimant = Principal::from_slice(&[0x11; 16]);
        let owner = Principal::from_slice(&[0xaa; 16]);
        let other_owner = Principal::from_slice(&[0xbb; 16]);
        s.xrp_claims.insert(0, claim(claimant, owner, 7, 2_000_000));
        s.xrp_claims.insert(1, {
            let mut c = claim(claimant, owner, 7, 3_000_000);
            c.settlement = Some(settlement("ABC"));
            c
        });
        s.xrp_claims.insert(2, {
            let mut c = claim(claimant, other_owner, 7, 5_000_000);
            c.settlement = Some(crate::state::XrpSettlement {
                tx_hash: "DEF".to_string(),
                last_ledger_sequence: 9_000_000,
                source_sequence: Some(12),
                destination: Some("rLUEXYuLiQptky37CqLcm9USQpPiz5rkpD".to_string()),
                destination_tag: Some(99),
            });
            c
        });

        assert_eq!(
            xrp_inflight_claims_for_custody(&s, 0, owner, 7)
                .into_iter()
                .map(|(id, _)| id)
                .collect::<Vec<_>>(),
            vec![1]
        );
        assert!(xrp_inflight_claims_for_custody(&s, 1, owner, 7).is_empty());
        assert!(xrp_inflight_claims_for_custody(&s, 0, owner, 8).is_empty());
    }

    #[test]
    fn inflight_claims_for_custody_returns_settlement_snapshots() {
        let mut s = crate::state::State::default();
        let claimant = Principal::from_slice(&[0x11; 16]);
        let owner = Principal::from_slice(&[0xaa; 16]);
        s.xrp_claims.insert(0, claim(claimant, owner, 7, 2_000_000));
        s.xrp_claims.insert(1, {
            let mut c = claim(claimant, owner, 7, 3_000_000);
            c.settlement = Some(settlement("ABC"));
            c
        });

        let in_flight = xrp_inflight_claims_for_custody(&s, 0, owner, 7);

        assert_eq!(in_flight.len(), 1);
        assert_eq!(in_flight[0].0, 1);
        assert_eq!(in_flight[0].1.tx_hash, "ABC");
    }

    #[test]
    fn reconcile_paid_settlement_removes_claim_only_for_matching_hash() {
        let mut s = crate::state::State::default();
        let claimant = Principal::from_slice(&[0x11; 16]);
        let owner = Principal::from_slice(&[0xaa; 16]);
        let mut c = claim(claimant, owner, 7, 3_000_000);
        c.settlement = Some(settlement("ABC"));
        s.xrp_claims.insert(1, c);

        assert!(!reconcile_xrp_settlement_snapshot(
            &mut s,
            1,
            "DEF",
            XrpSettlementReconciliation::Paid,
        ));
        assert!(s.xrp_claims.contains_key(&1));
        assert!(reconcile_xrp_settlement_snapshot(
            &mut s,
            1,
            "ABC",
            XrpSettlementReconciliation::Paid,
        ));
        assert!(!s.xrp_claims.contains_key(&1));
    }

    #[test]
    fn reconcile_failed_settlement_charges_fee_once_and_clears_blocker() {
        let mut s = crate::state::State::default();
        let claimant = Principal::from_slice(&[0x11; 16]);
        let owner = Principal::from_slice(&[0xaa; 16]);
        let mut c = claim(claimant, owner, 7, 3_000_000);
        c.settlement = Some(settlement("ABC"));
        s.xrp_claims.insert(1, c);

        assert!(reconcile_xrp_settlement_snapshot(
            &mut s,
            1,
            "ABC",
            XrpSettlementReconciliation::FailedFeeCharged,
        ));
        let claim = s.xrp_claims.get(&1).unwrap();
        assert_eq!(
            claim.drops,
            3_000_000 - crate::chains::xrp::adapter::XRP_FEE_DROPS
        );
        assert!(claim.settlement.is_none());
    }

    #[test]
    fn reconcile_expired_not_found_clears_blocker_without_charging_fee() {
        let mut s = crate::state::State::default();
        let claimant = Principal::from_slice(&[0x11; 16]);
        let owner = Principal::from_slice(&[0xaa; 16]);
        let mut c = claim(claimant, owner, 7, 3_000_000);
        c.settlement = Some(settlement("ABC"));
        s.xrp_claims.insert(1, c);

        assert!(reconcile_xrp_settlement_snapshot(
            &mut s,
            1,
            "ABC",
            XrpSettlementReconciliation::ExpiredNotFound,
        ));
        let claim = s.xrp_claims.get(&1).unwrap();
        assert_eq!(claim.drops, 3_000_000);
        assert!(claim.settlement.is_none());
    }

    #[test]
    fn aggregate_solvency_rejects_balance_that_only_covers_current_claim() {
        let acct = crate::chains::xrp::xrp_rpc::XrpAccountInfo {
            exists: true,
            sequence: 41,
            balance_drops: 4_000_020,
            ledger_index: 9_000_000,
        };

        assert!(matches!(
            ensure_xrp_claim_aggregate_solvency(&acct, 1_000_000, 5_000_000),
            Err(ProtocolError::GenericError(_))
        ));
    }

    #[test]
    fn aggregate_solvency_accepts_exact_balance_for_all_unresolved_claims() {
        let acct = crate::chains::xrp::xrp_rpc::XrpAccountInfo {
            exists: true,
            sequence: 41,
            balance_drops: 6_000_000,
            ledger_index: 9_000_000,
        };

        assert!(ensure_xrp_claim_aggregate_solvency(&acct, 1_000_000, 5_000_000).is_ok());
    }

    #[test]
    fn replacement_rejects_missing_prior_source_sequence() {
        let prev = crate::state::XrpSettlement {
            tx_hash: "ABC".to_string(),
            last_ledger_sequence: 9_000_000,
            source_sequence: None,
            destination: Some("rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh".to_string()),
            destination_tag: None,
        };
        let acct = crate::chains::xrp::xrp_rpc::XrpAccountInfo {
            exists: true,
            sequence: 41,
            balance_drops: 10_000_000,
            ledger_index: 9_000_100,
        };

        assert!(matches!(
            ensure_xrp_replacement_sequence_safe(&prev, &acct),
            Err(ProtocolError::GenericError(_))
        ));
    }

    #[test]
    fn replacement_rejects_when_live_sequence_advanced_past_prior_source_sequence() {
        let prev = crate::state::XrpSettlement {
            tx_hash: "ABC".to_string(),
            last_ledger_sequence: 9_000_000,
            source_sequence: Some(41),
            destination: Some("rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh".to_string()),
            destination_tag: None,
        };
        let acct = crate::chains::xrp::xrp_rpc::XrpAccountInfo {
            exists: true,
            sequence: 42,
            balance_drops: 10_000_000,
            ledger_index: 9_000_100,
        };

        assert!(matches!(
            ensure_xrp_replacement_sequence_safe(&prev, &acct),
            Err(ProtocolError::GenericError(_))
        ));
    }

    #[test]
    fn replacement_allows_same_live_sequence_after_expiry() {
        let prev = crate::state::XrpSettlement {
            tx_hash: "ABC".to_string(),
            last_ledger_sequence: 9_000_000,
            source_sequence: Some(41),
            destination: Some("rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh".to_string()),
            destination_tag: None,
        };
        let acct = crate::chains::xrp::xrp_rpc::XrpAccountInfo {
            exists: true,
            sequence: 41,
            balance_drops: 10_000_000,
            ledger_index: 9_000_100,
        };

        assert!(ensure_xrp_replacement_sequence_safe(&prev, &acct).is_ok());
    }

    #[test]
    fn settle_xrp_claim_with_tag_is_exposed_at_vault_level() {
        let _ = settle_xrp_claim_with_tag;
    }

    // ── F-03 sibling-reconcile divergence guard ──────────────────────────────
    //
    // `reconcile_xrp_other_inflight_claims` clears an expired, NotFound-by-local-hash
    // SIBLING settlement to `None` so the current claim can proceed. Before the guard,
    // that clear ran unconditionally on expiry (see `reconcile_expired_not_found_clears_
    // blocker_without_charging_fee`, which still encodes the pure snapshot behavior). The
    // primary settle path already refuses to re-sign once the custody Sequence advances
    // (`replacement_rejects_when_live_sequence_advanced_past_prior_source_sequence`); the
    // sibling path did not. These tests pin the now-symmetric decision.

    fn sibling_acct(
        sequence: u32,
        ledger_index: u32,
    ) -> crate::chains::xrp::xrp_rpc::XrpAccountInfo {
        crate::chains::xrp::xrp_rpc::XrpAccountInfo {
            exists: true,
            sequence,
            balance_drops: 100_000_000,
            ledger_index,
        }
    }

    #[test]
    fn sibling_reconcile_validated_is_paid() {
        let st = settlement("SIB"); // last_ledger_sequence = 9_000_000
        let acct = sibling_acct(41, 9_000_100);
        let status = crate::chains::xrp::xrp_rpc::XrpTxStatus::Validated {
            ledger_index: 9_000_050,
            delivered_drops: 1_000_000,
        };
        assert_eq!(
            xrp_sibling_reconcile_decision(&status, &st, &acct),
            XrpSiblingReconcileDecision::Paid
        );
    }

    #[test]
    fn sibling_reconcile_failed_charges_fee() {
        let st = settlement("SIB");
        let acct = sibling_acct(41, 9_000_100);
        let status = crate::chains::xrp::xrp_rpc::XrpTxStatus::Failed;
        assert_eq!(
            xrp_sibling_reconcile_decision(&status, &st, &acct),
            XrpSiblingReconcileDecision::FailedFeeCharged
        );
    }

    #[test]
    fn sibling_reconcile_notfound_unexpired_is_still_in_flight() {
        let st = settlement("SIB"); // last_ledger_sequence = 9_000_000
        let acct = sibling_acct(41, 9_000_000); // ledger_index == LLS -> not yet expired
        let status = crate::chains::xrp::xrp_rpc::XrpTxStatus::NotFound;
        assert_eq!(
            xrp_sibling_reconcile_decision(&status, &st, &acct),
            XrpSiblingReconcileDecision::StillInFlight
        );
    }

    #[test]
    fn sibling_reconcile_expired_sequence_unchanged_is_safe_to_clear() {
        let st = settlement("SIB"); // source_sequence = Some(41)
        let acct = sibling_acct(41, 9_000_100); // expired, sequence UNCHANGED -> never applied
        let status = crate::chains::xrp::xrp_rpc::XrpTxStatus::NotFound;
        assert_eq!(
            xrp_sibling_reconcile_decision(&status, &st, &acct),
            XrpSiblingReconcileDecision::ExpiredSafeToClear
        );
    }

    /// THE F-03 sibling double-pay guard. A sibling whose tx is NotFound by our local
    /// hash but whose custody Sequence ADVANCED (its Payment consumed the sequence under a
    /// diverged hash) must be QUARANTINED, not cleared. Pre-fix this case cleared the
    /// blocker, after which the sibling was treated as fresh, re-signed, and double-paid.
    #[test]
    fn sibling_reconcile_expired_sequence_advanced_quarantines_diverged() {
        let st = settlement("SIB"); // source_sequence = Some(41)
        let acct = sibling_acct(42, 9_000_100); // expired, sequence ADVANCED past source
        let status = crate::chains::xrp::xrp_rpc::XrpTxStatus::NotFound;
        assert_eq!(
            xrp_sibling_reconcile_decision(&status, &st, &acct),
            XrpSiblingReconcileDecision::QuarantineDiverged
        );
    }

    #[test]
    fn sibling_reconcile_legacy_no_source_sequence_quarantines() {
        let mut st = settlement("SIB");
        st.source_sequence = None; // legacy settlement cannot prove its sequence -> never clear
        let acct = sibling_acct(41, 9_000_100);
        let status = crate::chains::xrp::xrp_rpc::XrpTxStatus::NotFound;
        assert_eq!(
            xrp_sibling_reconcile_decision(&status, &st, &acct),
            XrpSiblingReconcileDecision::QuarantineDiverged
        );
    }

    // ── F-03 quarantine set + admin resolve ──────────────────────────────────

    #[test]
    fn quarantine_snapshot_sets_reason_idempotently() {
        let mut s = crate::state::State::default();
        let claimant = Principal::from_slice(&[0x11; 16]);
        let owner = Principal::from_slice(&[0xaa; 16]);
        s.xrp_claims.insert(7, claim(claimant, owner, 7, 2_000_000));

        assert!(quarantine_xrp_claim_snapshot(&mut s, 7, "first reason"));
        assert_eq!(
            s.xrp_claims.get(&7).unwrap().quarantine_reason.as_deref(),
            Some("first reason")
        );
        // Idempotent: a second detection keeps the first (most precise) reason.
        assert!(quarantine_xrp_claim_snapshot(&mut s, 7, "second reason"));
        assert_eq!(
            s.xrp_claims.get(&7).unwrap().quarantine_reason.as_deref(),
            Some("first reason")
        );
    }

    #[test]
    fn quarantine_snapshot_missing_claim_is_noop_false() {
        let mut s = crate::state::State::default();
        assert!(!quarantine_xrp_claim_snapshot(&mut s, 999, "x"));
    }

    #[test]
    fn resolve_confirm_paid_removes_quarantined_claim() {
        let mut s = crate::state::State::default();
        let claimant = Principal::from_slice(&[0x11; 16]);
        let owner = Principal::from_slice(&[0xaa; 16]);
        let mut c = claim(claimant, owner, 7, 2_000_000);
        c.settlement = Some(settlement("ABC"));
        c.quarantine_reason = Some("diverged".to_string());
        s.xrp_claims.insert(7, c);

        assert!(resolve_quarantined_xrp_claim_snapshot(&mut s, 7, true).is_ok());
        assert!(!s.xrp_claims.contains_key(&7));
    }

    #[test]
    fn resolve_release_for_retry_clears_quarantine_and_settlement() {
        let mut s = crate::state::State::default();
        let claimant = Principal::from_slice(&[0x11; 16]);
        let owner = Principal::from_slice(&[0xaa; 16]);
        let mut c = claim(claimant, owner, 7, 2_000_000);
        c.settlement = Some(settlement("ABC"));
        c.quarantine_reason = Some("diverged".to_string());
        s.xrp_claims.insert(7, c);

        assert!(resolve_quarantined_xrp_claim_snapshot(&mut s, 7, false).is_ok());
        let c = s.xrp_claims.get(&7).unwrap();
        assert!(c.quarantine_reason.is_none());
        assert!(c.settlement.is_none());
        assert_eq!(c.drops, 2_000_000); // claim preserved for a clean retry
    }

    #[test]
    fn resolve_refuses_healthy_claim() {
        let mut s = crate::state::State::default();
        let claimant = Principal::from_slice(&[0x11; 16]);
        let owner = Principal::from_slice(&[0xaa; 16]);
        s.xrp_claims.insert(7, claim(claimant, owner, 7, 2_000_000)); // not quarantined

        assert!(matches!(
            resolve_quarantined_xrp_claim_snapshot(&mut s, 7, true),
            Err(ProtocolError::GenericError(_))
        ));
        // The healthy claim is untouched (not dropped).
        assert!(s.xrp_claims.contains_key(&7));
    }

    #[test]
    fn resolve_missing_claim_errors() {
        let mut s = crate::state::State::default();
        assert!(matches!(
            resolve_quarantined_xrp_claim_snapshot(&mut s, 404, true),
            Err(ProtocolError::GenericError(_))
        ));
    }
}

#[cfg(test)]
mod xrp_p3_tests {
    use super::*;

    fn pending(owner: Principal, custody_address: &str) -> crate::state::XrpPendingDeposit {
        crate::state::XrpPendingDeposit {
            owner,
            custody_address: custody_address.to_string(),
            derivation_nonce: 7,
            opened_at_ns: 123,
            reserve_base_drops: 1_000_000,
        }
    }

    #[test]
    fn credit_nets_the_base_reserve() {
        // 5 XRP balance, 1 XRP reserve -> 4 XRP (drops) credited.
        assert_eq!(
            xrp_credit_amount(5_000_000, 1_000_000, 0).unwrap(),
            4_000_000
        );
    }

    #[test]
    fn credit_rejects_balance_at_or_below_reserve() {
        assert!(matches!(
            xrp_credit_amount(900_000, 1_000_000, 0),
            Err(ProtocolError::AmountTooLow { .. })
        ));
        assert!(matches!(
            xrp_credit_amount(1_000_000, 1_000_000, 0),
            Err(ProtocolError::AmountTooLow { .. })
        ));
    }

    #[test]
    fn pending_cleanup_removes_only_when_live_account_is_unfunded() {
        let owner = Principal::from_slice(&[0x44; 16]);
        let dep = pending(owner, "rLUEXYuLiQptky37CqLcm9USQpPiz5rkpD");
        let mut s = crate::state::State::default();
        s.xrp_pending_deposits.insert(7, dep.clone());
        let acct = crate::chains::xrp::xrp_rpc::XrpAccountInfo {
            exists: false,
            sequence: 0,
            balance_drops: 0,
            ledger_index: 9_000_000,
        };

        assert_eq!(
            remove_xrp_pending_deposit_if_unfunded_snapshot(&mut s, 7, &dep, &acct).unwrap(),
            true
        );
        assert!(!s.xrp_pending_deposits.contains_key(&7));
    }

    #[test]
    fn pending_cleanup_refuses_funded_account() {
        let owner = Principal::from_slice(&[0x44; 16]);
        let dep = pending(owner, "rLUEXYuLiQptky37CqLcm9USQpPiz5rkpD");
        let mut s = crate::state::State::default();
        s.xrp_pending_deposits.insert(7, dep.clone());
        let acct = crate::chains::xrp::xrp_rpc::XrpAccountInfo {
            exists: true,
            sequence: 1,
            balance_drops: 1_000_000,
            ledger_index: 9_000_000,
        };

        assert!(matches!(
            remove_xrp_pending_deposit_if_unfunded_snapshot(&mut s, 7, &dep, &acct),
            Err(ProtocolError::GenericError(_))
        ));
        assert!(s.xrp_pending_deposits.contains_key(&7));
    }

    #[test]
    fn pending_cleanup_rechecks_snapshot_after_await_before_removing() {
        let owner = Principal::from_slice(&[0x44; 16]);
        let dep = pending(owner, "rLUEXYuLiQptky37CqLcm9USQpPiz5rkpD");
        let changed = pending(owner, "rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh");
        let mut s = crate::state::State::default();
        s.xrp_pending_deposits.insert(7, changed);
        let acct = crate::chains::xrp::xrp_rpc::XrpAccountInfo {
            exists: false,
            sequence: 0,
            balance_drops: 0,
            ledger_index: 9_000_000,
        };

        assert_eq!(
            remove_xrp_pending_deposit_if_unfunded_snapshot(&mut s, 7, &dep, &acct).unwrap(),
            false
        );
        assert!(s.xrp_pending_deposits.contains_key(&7));
    }

    #[test]
    fn pending_cleanup_age_rejects_recent_entries() {
        let owner = Principal::from_slice(&[0x44; 16]);
        let dep = pending(owner, "rLUEXYuLiQptky37CqLcm9USQpPiz5rkpD");
        assert!(matches!(
            ensure_xrp_pending_cleanup_age(&dep, dep.opened_at_ns + 1),
            Err(ProtocolError::GenericError(_))
        ));
    }

    #[test]
    fn pending_cleanup_age_accepts_old_entries() {
        let owner = Principal::from_slice(&[0x44; 16]);
        let dep = pending(owner, "rLUEXYuLiQptky37CqLcm9USQpPiz5rkpD");
        assert!(ensure_xrp_pending_cleanup_age(
            &dep,
            dep.opened_at_ns + XRP_PENDING_CLEANUP_MIN_AGE_NS
        )
        .is_ok());
    }

    #[test]
    fn credit_rejects_net_below_min_deposit() {
        // net 500k but min 1M -> too low
        assert!(matches!(
            xrp_credit_amount(1_500_000, 1_000_000, 1_000_000),
            Err(ProtocolError::AmountTooLow { .. })
        ));
    }

    #[test]
    fn credit_ok_exactly_at_min_deposit() {
        assert_eq!(
            xrp_credit_amount(2_000_000, 1_000_000, 1_000_000).unwrap(),
            1_000_000
        );
    }

    #[test]
    fn credit_rejects_u64_overflow() {
        assert!(matches!(
            xrp_credit_amount(u128::MAX, 0, 0),
            Err(ProtocolError::GenericError(_))
        ));
    }
}

pub async fn open_vault_and_borrow(
    collateral_amount_raw: u64,
    borrow_amount_raw: u64,
    collateral_type_opt: Option<Principal>,
) -> Result<OpenVaultSuccess, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal = match GuardPrincipal::new(caller, "open_vault_and_borrow") {
        Ok(guard) => guard,
        Err(GuardError::AlreadyProcessing) => {
            log!(
                INFO,
                "[open_vault_and_borrow] Principal {:?} already has an ongoing operation",
                caller
            );
            return Err(ProtocolError::AlreadyProcessing);
        }
        Err(GuardError::StaleOperation) => {
            log!(
                INFO,
                "[open_vault_and_borrow] Principal {:?} has a stale operation being cleaned up",
                caller
            );
            return Err(ProtocolError::TemporarilyUnavailable(
                "Previous operation is being cleaned up. Please try again in a few seconds."
                    .to_string(),
            ));
        }
        Err(err) => return Err(err.into()),
    };

    // Resolve collateral type: default to ICP if not specified
    let collateral_type =
        collateral_type_opt.unwrap_or_else(|| read_state(|s| s.icp_collateral_type()));

    // Look up CollateralConfig; check status is Active
    let (config_ledger, config_status, min_deposit, is_native_xrp) =
        read_state(|s| match s.get_collateral_config(&collateral_type) {
            Some(config) => Ok((
                config.ledger_canister_id,
                config.status,
                config.min_collateral_deposit,
                config.is_native_xrp(),
            )),
            None => Err(ProtocolError::GenericError(
                "Collateral type not supported.".to_string(),
            )),
        })?;

    // P2: native-XRP collateral is custodied on the XRP Ledger (chains::xrp), not
    // pulled via an ICRC `transfer_from`. Its deposit flow (open-then-verify) is
    // wired in P3; until then reject opens through this ICRC path so XRP collateral
    // can never be silently mishandled as an ICRC token.
    if is_native_xrp {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "Native-XRP collateral uses the XRP deposit flow (not yet enabled).".to_string(),
        ));
    }

    if !config_status.allows_open() {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "Collateral type is not accepting new vaults.".to_string(),
        ));
    }

    let icp_margin_amount: ICP = collateral_amount_raw.into();

    if min_deposit > 0 && icp_margin_amount < ICP::new(min_deposit) {
        guard_principal.fail();
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: min_deposit,
        });
    }

    // Pull collateral via ICRC-2 transfer_from (caller must have approved first)
    let block_index =
        match transfer_collateral_from(collateral_amount_raw, caller, config_ledger).await {
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
                last_accrual_time: ic_cdk::api::time(),
                accrued_interest: ICUSD::new(0),
                bot_processing: false,
            },
            block_index,
        );
        vault_id
    });

    log!(
        INFO,
        "[open_vault_and_borrow] opened vault {vault_id}, now borrowing {borrow_amount_raw}"
    );

    // Borrow icUSD — reuse internal fn to avoid guard conflict
    if borrow_amount_raw > 0 {
        // AR-B-003: per-vault op lock across the borrow's mint await.
        let _vault_op_guard = match VaultLiquidationGuard::new(vault_id) {
            Ok(g) => g,
            Err(e) => {
                guard_principal.fail();
                return Err(ProtocolError::GenericError(format!(
                    "Vault created (id={}) but borrow of {} failed: {:?}. You can borrow separately.",
                    vault_id, borrow_amount_raw, e
                )));
            }
        };
        match borrow_from_vault_internal(
            caller,
            VaultArg {
                vault_id,
                amount: borrow_amount_raw,
            },
        )
        .await
        {
            Ok(borrow_result) => {
                log!(
                    INFO,
                    "[open_vault_and_borrow] vault {} borrow of {} succeeded (fee: {})",
                    vault_id,
                    borrow_amount_raw,
                    borrow_result.fee_amount_paid
                );
            }
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
    Ok(OpenVaultSuccess {
        vault_id,
        block_index,
    })
}

/// Internal borrow logic without guard management.
/// Called by both `borrow_from_vault` (which acquires its own guard) and
/// `open_vault_with_deposit` (which already holds a guard for the same principal).
async fn borrow_from_vault_internal(
    caller: Principal,
    arg: VaultArg,
) -> Result<SuccessWithFee, ProtocolError> {
    let amount: ICUSD = arg.amount.into();

    if amount < read_state(|s| s.min_icusd_amount) {
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: read_state(|s| s.min_icusd_amount).to_u64(),
        });
    }

    // Accrue interest on this vault before borrowing so CR check uses up-to-date debt.
    let now = ic_cdk::api::time();
    reject_active_xrp_sp_absorb_preflight(arg.vault_id, now)?;
    mutate_state(|s| s.accrue_single_vault(arg.vault_id, now));

    let (vault, collateral_price, config_decimals, is_native_xrp) =
        read_state(|s| match s.vault_id_to_vaults.get(&arg.vault_id) {
            Some(vault) => {
                let price = s
                    .get_collateral_price_decimal(&vault.collateral_type)
                    .ok_or("No price available for collateral. Price feed may be down.")?;
                let config = s
                    .get_collateral_config(&vault.collateral_type)
                    .ok_or("Collateral type not configured.")?;
                Ok((
                    vault.clone(),
                    price,
                    config.decimals,
                    config.is_native_xrp(),
                ))
            }
            None => Err("Vault not found. Please check the vault ID."),
        })
        .map_err(|msg: &str| ProtocolError::GenericError(msg.to_string()))?;

    require_vault_not_processing(&vault)?;

    // Check collateral status allows borrowing
    let collateral_status = read_state(|s| s.get_collateral_status(&vault.collateral_type));
    if let Some(status) = collateral_status {
        if !status.allows_borrow() {
            return Err(ProtocolError::GenericError(
                "Borrowing is not allowed for this collateral type.".to_string(),
            ));
        }
    }
    if is_native_xrp {
        require_xrp_production_key()?;
    }

    if caller != vault.owner {
        return Err(ProtocolError::CallerNotOwner);
    }

    // Check debt ceiling + global mint cap AND reserve the headroom atomically.
    //
    // BK-003 (audit 2026-06-05): these caps are checked here but the debt is not
    // recorded until after the `mint_icusd().await` below. Two borrows from
    // DIFFERENT owners both pass this check against the same committed aggregate,
    // both mint, and jointly exceed the cap (the per-caller GuardPrincipal does
    // not serialize distinct owners against the aggregate). The reservation guard
    // counts every in-flight borrow in the check and is held across the mint, so
    // a concurrent borrow sees this one's reserved amount. Released on Drop
    // (return or continuation-trap via ic-cdk cleanup).
    let current_debt = read_state(|s| s.total_debt_for_collateral(&vault.collateral_type));
    let debt_ceiling = read_state(|s| {
        s.get_collateral_config(&vault.collateral_type)
            .map(|c| c.debt_ceiling)
            .unwrap_or(u64::MAX)
    });
    let (global_cap, total_borrowed) =
        read_state(|s| (s.global_icusd_mint_cap, s.total_borrowed_icusd_amount()));
    let _borrow_reservation = crate::guard::BorrowReservationGuard::try_reserve(
        vault.collateral_type,
        amount.to_u64(),
        current_debt.to_u64(),
        debt_ceiling,
        total_borrowed.to_u64(),
        global_cap,
    )
    .map_err(ProtocolError::GenericError)?;

    let collateral_value = crate::numeric::collateral_usd_value(
        vault.collateral_amount,
        collateral_price,
        config_decimals,
    );
    let min_ratio = read_state(|s| {
        let base = s.get_min_collateral_ratio_for(&vault.collateral_type);
        if s.mode == Mode::Recovery {
            let recovery_cr = s.get_recovery_cr_for(&vault.collateral_type);
            if recovery_cr > base {
                recovery_cr
            } else {
                base
            }
        } else {
            base
        }
    });
    let max_borrowable_amount: ICUSD = collateral_value / min_ratio;

    if vault.borrowed_icusd_amount + amount > max_borrowable_amount {
        return Err(ProtocolError::GenericError(format!(
            "failed to borrow from vault, max borrowable: {max_borrowable_amount}, borrowed: {}, requested: {amount}",
            vault.borrowed_icusd_amount
        )));
    }

    // Compute projected vault CR after this borrow (for dynamic fee multiplier)
    let new_total_debt = vault.borrowed_icusd_amount + amount;
    let projected_cr = if new_total_debt.to_u64() == 0 {
        Ratio::new(dec!(999))
    } else {
        Ratio::from(
            Decimal::from_u64(collateral_value.to_u64()).unwrap_or(Decimal::ZERO)
                / Decimal::from_u64(new_total_debt.to_u64()).unwrap_or(Decimal::ONE),
        )
    };

    let fee: ICUSD = read_state(|s| {
        let base_fee = s.get_borrowing_fee_for(&vault.collateral_type);
        let multiplier = s.get_borrowing_fee_multiplier(projected_cr);
        let raw_fee: ICUSD = amount * base_fee * multiplier;
        // INT-003: clamp so `amount - fee >= 1 e8s`. Defense in depth: the
        // curve validator caps the multiplier, but a legacy or migrated curve
        // (or any future code path that writes the fee state outside
        // `set_borrowing_fee_curve`) cannot panic the borrow path.
        clamp_borrow_fee(amount, raw_fee)
    });

    match mint_icusd(amount - fee, caller).await {
        Ok(block_index) => {
            mutate_state(|s| {
                record_borrow_from_vault(s, arg.vault_id, amount, fee, block_index);
            });

            // Mint the borrowing fee to treasury (fire-and-forget)
            crate::treasury::mint_borrowing_fee_to_treasury(fee).await;

            Ok(SuccessWithFee {
                block_index,
                fee_amount_paid: fee.to_u64(),
                collateral_amount_received: None,
                debt_liquidated_e8s: None, // SP-101
                stable_pulled_e6s: None,   // SP-110
                xrp_claim_id: None,
            })
        }
        Err(mint_error) => Err(ProtocolError::TransferError(mint_error)),
    }
}

pub async fn borrow_from_vault(arg: VaultArg) -> Result<SuccessWithFee, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal =
        match GuardPrincipal::new(caller, &format!("borrow_vault_{}", arg.vault_id)) {
            Ok(guard) => guard,
            Err(GuardError::AlreadyProcessing) => {
                log!(
                    INFO,
                    "[borrow_from_vault] Principal {:?} already has an ongoing operation",
                    caller
                );
                return Err(ProtocolError::AlreadyProcessing);
            }
            Err(err) => return Err(err.into()),
        };

    // AR-B-003 (audit 2026-06-09): per-vault op lock. The per-caller guard
    // above does not exclude a concurrent liquidation/redemption of this
    // vault; this lock does (and the redemption water-fill skips locked
    // vaults). See guard.rs::VaultLiquidationGuard.
    let _vault_op_guard = match VaultLiquidationGuard::new(arg.vault_id) {
        Ok(g) => g,
        Err(e) => {
            guard_principal.fail();
            return Err(e);
        }
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

/// Internal repay logic without guard management.
///
/// Called by both `repay_to_vault` (which acquires its own `repay_vault_{id}`
/// guard) and `repay_and_close_vault` (which holds a single
/// `repay_and_close_{id}` guard spanning repay + withdraw + close).
///
/// Performs interest accrual, validates caller/state, pulls icUSD via
/// `icrc2_transfer_from`, records the repayment, and distributes the interest
/// share to treasury.
///
/// `is_full_close` signals that the caller is the `repay_and_close_vault`
/// compound endpoint and intends to zero the vault's debt in this call. When
/// true, the `MIN_ICUSD_AMOUNT` floor is bypassed so vaults stuck in the
/// `(DUST_DEBT_THRESHOLD, MIN_ICUSD_AMOUNT)` zone can be cleared. The floor
/// stays in force for the regular `repay_to_vault` path as an anti-spam
/// guarantee — an explicit flag (rather than `amount == debt` equality) avoids
/// brittleness from interest accruing between the caller's debt fetch and
/// this helper's read.
async fn repay_to_vault_internal(
    caller: Principal,
    arg: VaultArg,
    is_full_close: bool,
) -> Result<u64, ProtocolError> {
    let amount: ICUSD = arg.amount.into();

    // Accrue interest before repayment so the correct debt balance is used.
    let now = ic_cdk::api::time();
    reject_active_xrp_sp_absorb_preflight(arg.vault_id, now)?;
    mutate_state(|s| s.accrue_single_vault(arg.vault_id, now));

    let vault = read_state(|s| s.vault_id_to_vaults.get(&arg.vault_id).cloned())
        .ok_or_else(|| ProtocolError::GenericError("Vault not found".to_string()))?;

    require_vault_not_processing(&vault)?;

    // Check collateral status allows repayment
    let collateral_status = read_state(|s| s.get_collateral_status(&vault.collateral_type));
    if let Some(status) = collateral_status {
        if !status.allows_repay() {
            return Err(ProtocolError::GenericError(
                "Repayment is not allowed for this collateral type.".to_string(),
            ));
        }
    }

    if caller != vault.owner {
        return Err(ProtocolError::CallerNotOwner);
    }

    if !is_full_close && amount < read_state(|s| s.min_icusd_amount) {
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: read_state(|s| s.min_icusd_amount).to_u64(),
        });
    }

    // Cap repay amount to actual debt. Interest accrued between when the
    // frontend read the balance and now can push debt slightly above what
    // the user entered. If the requested amount exceeds or nearly matches
    // the current debt (within 1% or 0.01 icUSD — whichever is larger),
    // treat it as a full repayment to avoid leaving un-repayable dust.
    let debt = vault.borrowed_icusd_amount;
    let dust_threshold = std::cmp::max(debt.0 / 100, 1_000_000); // 1% or 0.01 icUSD
    let amount = if amount > debt {
        debt
    } else if debt.0.saturating_sub(amount.0) <= dust_threshold {
        debt
    } else {
        amount
    };

    check_min_vault_debt_after_repay(&vault, amount)?;

    match transfer_icusd_from(amount, caller).await {
        Ok(block_index) => {
            let interest_share =
                mutate_state(|s| record_repayed_to_vault(s, arg.vault_id, amount, block_index));
            // IC-B-002 (audit 2026-06-09): re-queue any unminted interest share so the
            // next flush retries it instead of silently dropping treasury revenue.
            let unminted_interest =
                crate::treasury::distribute_interest(interest_share, vault.collateral_type).await;
            if unminted_interest.to_u64() > 0 {
                mutate_state(|s| {
                    s.restore_pending_interest_for_pool(
                        vault.collateral_type,
                        unminted_interest.to_u64(),
                    )
                });
            }
            Ok(block_index)
        }
        Err(transfer_from_error) => Err(ProtocolError::TransferFromError(
            transfer_from_error,
            amount.to_u64(),
        )),
    }
}

pub async fn repay_to_vault(arg: VaultArg) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal = GuardPrincipal::new(caller, &format!("repay_vault_{}", arg.vault_id))?;
    // AR-B-003: per-vault op lock; see guard.rs::VaultLiquidationGuard.
    let _vault_op_guard = match VaultLiquidationGuard::new(arg.vault_id) {
        Ok(g) => g,
        Err(e) => {
            guard_principal.fail();
            return Err(e);
        }
    };

    match repay_to_vault_internal(caller, arg, false).await {
        Ok(block_index) => {
            guard_principal.complete();
            Ok(block_index)
        }
        Err(e) => {
            guard_principal.fail();
            Err(e)
        }
    }
}

/// Repay vault debt using ckUSDT or ckUSDC (1:1 with icUSD, plus configurable fee)
pub async fn repay_to_vault_with_stable(arg: VaultArgWithToken) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal =
        GuardPrincipal::new(caller, &format!("repay_vault_stable_{}", arg.vault_id))?;
    // AR-B-003: per-vault op lock; see guard.rs::VaultLiquidationGuard.
    let _vault_op_guard = match VaultLiquidationGuard::new(arg.vault_id) {
        Ok(g) => g,
        Err(e) => {
            guard_principal.fail();
            return Err(e);
        }
    };

    // Check if the selected stable token is enabled
    let is_enabled = read_state(|s| match arg.token_type {
        StableTokenType::CKUSDT => s.ckusdt_enabled,
        StableTokenType::CKUSDC => s.ckusdc_enabled,
    });
    if !is_enabled {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(format!(
            "{:?} repayments are currently disabled",
            arg.token_type
        )));
    }

    let now = ic_cdk::api::time();
    if let Err(e) = reject_active_xrp_sp_absorb_preflight(arg.vault_id, now) {
        guard_principal.fail();
        return Err(e);
    }

    // Depeg protection: fetch fresh stablecoin price and reject if outside $0.95–$1.05
    if let Err(e) = crate::xrc::ensure_stable_not_depegged(&arg.token_type).await {
        guard_principal.fail();
        return Err(e);
    }

    // Truncate to nearest 100 e8s for clean 8→6 decimal conversion
    let raw_amount_e8s = arg.amount - (arg.amount % 100);
    let amount: ICUSD = raw_amount_e8s.into();

    // Accrue interest before repayment so the correct debt balance is used.
    mutate_state(|s| s.accrue_single_vault(arg.vault_id, now));

    let vault = match read_state(|s| s.vault_id_to_vaults.get(&arg.vault_id).cloned()) {
        Some(v) => v,
        None => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError("Vault not found".to_string()));
        }
    };

    if let Err(e) = require_vault_not_processing(&vault) {
        guard_principal.fail();
        return Err(e);
    }

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

    if amount < read_state(|s| s.min_icusd_amount) {
        guard_principal.fail();
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: read_state(|s| s.min_icusd_amount).to_u64(),
        });
    }

    // Cap repay amount to actual debt. Interest accrued between when the
    // frontend read the balance and now can push debt slightly above what
    // the user entered. If the requested amount exceeds or nearly matches
    // the current debt (within 1% or 0.01 icUSD — whichever is larger),
    // treat it as a full repayment to avoid leaving un-repayable dust.
    let debt = vault.borrowed_icusd_amount;
    let dust_threshold = std::cmp::max(debt.0 / 100, 1_000_000); // 1% or 0.01 icUSD
    let amount = if amount > debt {
        debt
    } else if debt.0.saturating_sub(amount.0) <= dust_threshold {
        debt
    } else {
        amount
    };

    if let Err(e) = check_min_vault_debt_after_repay(&vault, amount) {
        guard_principal.fail();
        return Err(e);
    }

    // Convert e8s (icUSD) to e6s (ckstable) and add fee surcharge
    let base_stable_e6s = raw_amount_e8s / 100;
    let fee_rate = read_state(|s| s.ckstable_repay_fee);
    let fee_e6s = (rust_decimal::Decimal::from(base_stable_e6s) * fee_rate.0)
        .to_u64()
        .unwrap_or(0);
    let total_pull_e6s = base_stable_e6s + fee_e6s;

    // Transfer the stable token from user (in 6-decimal units)
    match transfer_stable_from(arg.token_type.clone(), total_pull_e6s, caller).await {
        Ok(block_index) => {
            let interest_share =
                mutate_state(|s| record_repayed_to_vault(s, arg.vault_id, amount, block_index));

            // Route interest via N-way split (stablecoin-denominated)
            if interest_share.to_u64() > 0 {
                crate::treasury::distribute_stablecoin_interest(
                    interest_share.to_u64(),
                    vault.collateral_type,
                    arg.token_type.clone(),
                )
                .await;
            }

            // Route fee surcharge to treasury as stablecoins
            if fee_e6s > 0 {
                let (treasury, stable_ledger) = read_state(|s| {
                    let ledger = match arg.token_type {
                        StableTokenType::CKUSDT => s.ckusdt_ledger_principal,
                        StableTokenType::CKUSDC => s.ckusdc_ledger_principal,
                    };
                    (s.treasury_principal, ledger)
                });
                if let (Some(treasury_principal), Some(stable_ledger)) = (treasury, stable_ledger) {
                    match management::transfer_collateral(
                        fee_e6s,
                        treasury_principal,
                        stable_ledger,
                    )
                    .await
                    {
                        Ok(block) => {
                            log!(
                                INFO,
                                "[repay_with_stable] Transferred {} e6s fee to treasury (block {})",
                                fee_e6s,
                                block
                            );
                        }
                        Err(e) => {
                            // Non-critical: fee stays in reserves if transfer fails
                            log!(INFO,
                                "[repay_with_stable] Fee transfer to treasury failed: {:?}. Fee remains in reserves.",
                                e
                            );
                        }
                    }
                }
            }

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
    let guard_principal =
        GuardPrincipal::new(caller, &format!("add_margin_vault_{}", arg.vault_id))?;
    // AR-B-003: per-vault op lock; see guard.rs::VaultLiquidationGuard.
    let _vault_op_guard = match VaultLiquidationGuard::new(arg.vault_id) {
        Ok(g) => g,
        Err(e) => {
            guard_principal.fail();
            return Err(e);
        }
    };
    let amount: ICP = arg.amount.into();

    let now = ic_cdk::api::time();
    if let Err(e) = reject_active_xrp_sp_absorb_preflight(arg.vault_id, now) {
        guard_principal.fail();
        return Err(e);
    }

    let (vault, config_ledger, min_deposit, is_native_xrp) =
        match read_state(|s| match s.vault_id_to_vaults.get(&arg.vault_id) {
            Some(v) => {
                let config = s
                    .get_collateral_config(&v.collateral_type)
                    .ok_or("Collateral type not configured")?;
                Ok((
                    v.clone(),
                    config.ledger_canister_id,
                    config.min_collateral_deposit,
                    config.is_native_xrp(),
                ))
            }
            None => Err("Vault not found"),
        }) {
            Ok(result) => result,
            Err(msg) => {
                guard_principal.fail();
                return Err(ProtocolError::GenericError(msg.to_string()));
            }
        };

    // P2: native-XRP collateral is not custodied via ICRC; its add-collateral flow
    // is wired with the XRP deposit path (P3). Reject so XRP collateral can never be
    // pulled as an ICRC token. (Latent until P5 enables XRP registration.)
    if is_native_xrp {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "Native-XRP collateral uses the XRP deposit flow (not yet enabled).".to_string(),
        ));
    }

    if let Err(e) = require_vault_not_processing(&vault) {
        guard_principal.fail();
        return Err(e);
    }

    if min_deposit > 0 && amount < ICP::new(min_deposit) {
        guard_principal.fail();
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: min_deposit,
        });
    }

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
            log!(
                INFO,
                "[open_vault_with_deposit] Principal {:?} already has an ongoing operation",
                caller
            );
            return Err(ProtocolError::AlreadyProcessing);
        }
        Err(err) => return Err(err.into()),
    };

    // Resolve collateral type: default to ICP if not specified
    let collateral_type =
        collateral_type_opt.unwrap_or_else(|| read_state(|s| s.icp_collateral_type()));

    // Look up CollateralConfig
    let (config_ledger, config_status, config_fee, min_deposit, is_native_xrp) =
        read_state(|s| match s.get_collateral_config(&collateral_type) {
            Some(config) => Ok((
                config.ledger_canister_id,
                config.status,
                config.ledger_fee,
                config.min_collateral_deposit,
                config.is_native_xrp(),
            )),
            None => Err(ProtocolError::GenericError(
                "Collateral type not supported.".to_string(),
            )),
        })?;

    // P2: native-XRP collateral is custodied on the XRP Ledger (chains::xrp), not
    // swept from an ICRC deposit subaccount. Reject until the XRP deposit flow (P3).
    if is_native_xrp {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "Native-XRP collateral uses the XRP deposit flow (not yet enabled).".to_string(),
        ));
    }

    if !config_status.allows_open() {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "Collateral type is not accepting new vaults.".to_string(),
        ));
    }

    // Sweep funds from the caller's deposit subaccount
    let (collateral_amount, sweep_block_index) = match management::sweep_deposit(
        &caller,
        config_ledger,
        config_fee,
    )
    .await
    {
        Ok(result) => result,
        Err(e) => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(
                format!("Push-deposit sweep failed: {}. Did you transfer collateral to your deposit account first?", e),
            ));
        }
    };

    let icp_margin_amount: ICP = collateral_amount.into();
    if min_deposit > 0 && icp_margin_amount < ICP::new(min_deposit) {
        guard_principal.fail();
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: min_deposit,
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
                last_accrual_time: ic_cdk::api::time(),
                accrued_interest: ICUSD::new(0),
                bot_processing: false,
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
        // AR-B-003: per-vault op lock across the borrow's mint await.
        let _vault_op_guard = VaultLiquidationGuard::new(vault_id)?;
        match borrow_from_vault_internal(
            caller,
            VaultArg {
                vault_id,
                amount: borrow_amount_raw,
            },
        )
        .await
        {
            Ok(borrow_result) => {
                log!(
                    INFO,
                    "[open_vault_with_deposit] vault {} initial borrow of {} succeeded (fee: {})",
                    vault_id,
                    borrow_amount_raw,
                    borrow_result.fee_amount_paid
                );
            }
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
    // AR-B-003: per-vault op lock; see guard.rs::VaultLiquidationGuard.
    let _vault_op_guard = match VaultLiquidationGuard::new(vault_id) {
        Ok(g) => g,
        Err(e) => {
            guard_principal.fail();
            return Err(e);
        }
    };

    let now = ic_cdk::api::time();
    if let Err(e) = reject_active_xrp_sp_absorb_preflight(vault_id, now) {
        guard_principal.fail();
        return Err(e);
    }

    let (vault, config_ledger, config_fee, min_deposit, is_native_xrp) =
        match read_state(|s| match s.vault_id_to_vaults.get(&vault_id) {
            Some(v) => {
                let config = s
                    .get_collateral_config(&v.collateral_type)
                    .ok_or("Collateral type not configured")?;
                Ok((
                    v.clone(),
                    config.ledger_canister_id,
                    config.ledger_fee,
                    config.min_collateral_deposit,
                    config.is_native_xrp(),
                ))
            }
            None => Err("Vault not found"),
        }) {
            Ok(result) => result,
            Err(msg) => {
                guard_principal.fail();
                return Err(ProtocolError::GenericError(msg.to_string()));
            }
        };

    // P2: native-XRP collateral is not custodied via ICRC; its add-collateral flow
    // is wired with the XRP deposit path (P3). Reject so XRP collateral can never be
    // swept as an ICRC token. (Latent until P5 enables XRP registration.)
    if is_native_xrp {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "Native-XRP collateral uses the XRP deposit flow (not yet enabled).".to_string(),
        ));
    }

    if let Err(e) = require_vault_not_processing(&vault) {
        guard_principal.fail();
        return Err(e);
    }

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
    let (collateral_amount, sweep_block_index) = match management::sweep_deposit(
        &caller,
        config_ledger,
        config_fee,
    )
    .await
    {
        Ok(result) => result,
        Err(e) => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(
                format!("Push-deposit sweep failed: {}. Did you transfer collateral to your deposit account first?", e),
            ));
        }
    };

    let margin_added: ICP = collateral_amount.into();
    if min_deposit > 0 && margin_added < ICP::new(min_deposit) {
        guard_principal.fail();
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: min_deposit,
        });
    }

    mutate_state(|s| record_add_margin_to_vault(s, vault_id, margin_added, sweep_block_index));

    log!(INFO, "[add_margin_with_deposit] added {} collateral to vault {} via push-deposit (sweep block {})",
        collateral_amount, vault_id, sweep_block_index);

    guard_principal.complete();
    Ok(sweep_block_index)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WithdrawCloseCompletionPolicy {
    CloseVault,
    KeepNativeXrpVaultOpen,
}

fn withdraw_close_completion_policy(is_native_xrp: bool) -> WithdrawCloseCompletionPolicy {
    if is_native_xrp {
        WithdrawCloseCompletionPolicy::KeepNativeXrpVaultOpen
    } else {
        WithdrawCloseCompletionPolicy::CloseVault
    }
}

fn native_xrp_reserve_locked_message() -> String {
    "Native-XRP vaults stay open because the XRP account reserve remains locked on XRPL."
        .to_string()
}

pub async fn close_vault(vault_id: u64) -> Result<Option<u64>, ProtocolError> {
    let caller = ic_cdk::caller();
    let _guard_principal = GuardPrincipal::new(caller, &format!("close_vault_{}", vault_id))?;
    // AR-B-003: per-vault op lock; see guard.rs::VaultLiquidationGuard.
    let _vault_op_guard = VaultLiquidationGuard::new(vault_id)?;
    reject_active_xrp_sp_absorb_preflight(vault_id, ic_cdk::api::time())?;

    // Check rate limits first
    mutate_state(|s| s.check_close_vault_rate_limit(caller))?;

    // Record the close request for rate limiting
    mutate_state(|s| s.record_close_vault_request(caller));

    // Accrue interest before closing so the full repayment amount is accurate.
    let now = ic_cdk::api::time();
    mutate_state(|s| s.accrue_single_vault(vault_id, now));

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
        return Err(ProtocolError::GenericError(format!(
            "Vault #{} not found",
            vault_id
        )));
    }

    // Get the vault
    let vault = read_state(|s| {
        s.vault_id_to_vaults
            .get(&vault_id)
            .cloned()
            .ok_or(ProtocolError::GenericError("Vault not found".to_string()))
    })?;

    require_vault_not_processing(&vault)?;

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

        // Record dust forgiveness (no real payment, no treasury routing)
        mutate_state(|s| {
            s.dust_forgiven_total += vault.borrowed_icusd_amount;
            let _ = s.repay_to_vault(vault_id, vault.borrowed_icusd_amount);
        });

        // Record dust forgiveness event
        crate::storage::record_event(&crate::event::Event::DustForgiven {
            vault_id,
            amount: vault.borrowed_icusd_amount,
            timestamp: Some(ic_cdk::api::time()),
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
            "Cannot close vault with outstanding debt. Repay all debt first.".to_string(),
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
            "Cannot close vault with remaining collateral. Withdraw collateral first.".to_string(),
        ));
    }

    let is_native_xrp = read_state(|s| {
        s.get_collateral_config(&vault.collateral_type)
            .map(|config| config.is_native_xrp())
            .unwrap_or(false)
    });
    if withdraw_close_completion_policy(is_native_xrp)
        == WithdrawCloseCompletionPolicy::KeepNativeXrpVaultOpen
    {
        mutate_state(|s| s.complete_close_vault_request());
        log!(
            INFO,
            "[close_vault] Keeping native-XRP vault #{} open because the XRPL reserve remains locked",
            vault_id
        );
        return Err(ProtocolError::GenericError(
            native_xrp_reserve_locked_message(),
        ));
    }

    // Simply close the vault - no transfers needed
    mutate_state(|s| {
        // Make sure vault exists before attempting to remove
        if s.vault_id_to_vaults.contains_key(&vault_id) {
            // The vault must still exist when record_close_vault runs:
            // state::close_vault inside it performs the removal (primary map
            // + every secondary index) and traps on an unknown vault. An
            // earlier version removed the vault inline here first, so the
            // recorder's close always hit that trap and rolled the whole
            // call back — the endpoint could never succeed.
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
    let _guard_principal =
        GuardPrincipal::new(caller, &format!("withdraw_collateral_{}", vault_id))?;
    // AR-B-003: per-vault op lock; see guard.rs::VaultLiquidationGuard.
    let _vault_op_guard = VaultLiquidationGuard::new(vault_id)?;
    reject_active_xrp_sp_absorb_preflight(vault_id, ic_cdk::api::time())?;

    log!(
        INFO,
        "[withdraw_collateral] Request to withdraw collateral from vault #{} by principal {}",
        vault_id,
        caller
    );

    // Check vault exists and caller is owner
    let vault = read_state(|state| {
        state
            .vault_id_to_vaults
            .get(&vault_id)
            .cloned()
            .ok_or(ProtocolError::GenericError("Vault not found".to_string()))
    })?;

    require_vault_not_processing(&vault)?;

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
        return Err(ProtocolError::GenericError(
            "No collateral to withdraw".to_string(),
        ));
    }

    // Look up per-collateral config (incl. custody kind for P4 native-XRP routing).
    let (ledger_canister_id, ledger_fee, is_native_xrp) =
        read_state(|s| {
            let config = s.get_collateral_config(&vault.collateral_type).ok_or(
                ProtocolError::GenericError("Collateral type not configured".to_string()),
            )?;
            Ok::<_, ProtocolError>((
                config.ledger_canister_id,
                config.ledger_fee,
                config.is_native_xrp(),
            ))
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
        // Wave-8b LIQ-002: collateral changed → re-key the index entry.
        state.reindex_vault_cr(vault_id);
    });

    // P4: native-XRP collateral leaves the vault into an XrpClaim (settled later via
    // settle_xrp_claim, signed from the vault's custody address) instead of an ICRC
    // transfer. Collateral is already zeroed above; the XRPL fee is taken at settle
    // time (claimant-bears-fee), so the full amount becomes the claim.
    if is_native_xrp {
        let now_ns = ic_cdk::api::time();
        let claim_id = mutate_state(|s| {
            crate::event::record_collateral_withdrawn(s, vault_id, amount_to_transfer, 0);
            record_xrp_claim(
                s,
                caller,
                caller,
                vault_id,
                amount_to_transfer.to_u64(),
                now_ns,
            )
        });
        log!(
            INFO,
            "[withdraw_collateral] vault #{} native-XRP collateral -> XRP claim #{}",
            vault_id,
            claim_id
        );
        return Ok(claim_id);
    }

    // Make the collateral transfer with appropriate fee deduction
    let fee = ICP::from(ledger_fee);
    let transfer_amount = amount_to_transfer - fee;

    log!(
        INFO,
        "[withdraw_collateral] Transferring {} (after fee deduction) to {}",
        transfer_amount,
        caller
    );

    match management::transfer_collateral(transfer_amount.to_u64(), caller, ledger_canister_id)
        .await
    {
        Ok(block_index) => {
            // Fix for the lifetime issue - we need to use a separate mutate_state call
            // Rather than passing a mutable reference to the state
            mutate_state(|s| {
                crate::event::record_collateral_withdrawn(
                    s,
                    vault_id,
                    amount_to_transfer,
                    block_index,
                )
            });

            log!(
                INFO,
                "[withdraw_collateral] Successfully withdrew {} from vault #{}, transfer block_index: {}",
                amount_to_transfer,
                vault_id,
                block_index
            );

            Ok(block_index)
        }
        Err(error) => {
            // If the transfer fails, we need to restore the collateral in the vault
            mutate_state(|state| {
                if let Some(vault) = state.vault_id_to_vaults.get_mut(&vault_id) {
                    vault.collateral_amount = amount_to_transfer.to_u64();
                }
                // Wave-8b LIQ-002: rollback restores collateral → re-key.
                state.reindex_vault_cr(vault_id);
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
    // AR-B-003: per-vault op lock. The post-await commit debits the vault by
    // the pre-await withdraw amount; without this lock a concurrent
    // liquidation/redemption could shrink the vault first, leaving phantom
    // collateral on the books after the transfer already paid the owner.
    let _vault_op_guard = VaultLiquidationGuard::new(vault_id)?;

    let withdraw_amount: ICP = ICP::new(amount);

    // INT-004: accrue interest on this vault before any CR-relevant read so
    // the withdrawal headroom is computed against fresh debt. Mirrors the
    // pattern already used in `borrow_from_vault_internal` and both
    // `repay_to_vault` entry points. `accrue_single_vault` is a no-op when
    // the vault has no debt or when the elapsed window is zero.
    let now = ic_cdk::api::time();
    reject_active_xrp_sp_absorb_preflight(vault_id, now)?;
    mutate_state(|s| s.accrue_single_vault(vault_id, now));

    // Read vault, per-collateral price + config from state
    let (
        vault,
        collateral_price,
        config_decimals,
        ledger_canister_id,
        ledger_fee,
        min_deposit,
        is_native_xrp,
    ) = match read_state(|s| match s.vault_id_to_vaults.get(&vault_id) {
        Some(vault) => {
            let price = s
                .get_collateral_price_decimal(&vault.collateral_type)
                .ok_or("No price available for collateral. Price feed may be down.")?;
            let config = s
                .get_collateral_config(&vault.collateral_type)
                .ok_or("Collateral type not configured.")?;
            Ok((
                vault.clone(),
                price,
                config.decimals,
                config.ledger_canister_id,
                config.ledger_fee,
                config.min_collateral_deposit,
                config.is_native_xrp(),
            ))
        }
        None => Err("Vault not found. Please check the vault ID."),
    }) {
        Ok(result) => result,
        Err(msg) => return Err(ProtocolError::GenericError(msg.to_string())),
    };

    require_vault_not_processing(&vault)?;

    if min_deposit > 0 && withdraw_amount < ICP::new(min_deposit) {
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: min_deposit,
        });
    }

    log!(
        INFO,
        "[withdraw_partial_collateral] Request to withdraw {} from vault #{} by principal {}",
        withdraw_amount,
        vault_id,
        caller
    );

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
        return Err(ProtocolError::GenericError(
            "No collateral to withdraw".to_string(),
        ));
    }

    // Forgive dust debt: if remaining debt is below threshold, zero it out
    let has_dust = vault.borrowed_icusd_amount.0 > 0
        && vault.borrowed_icusd_amount.0 <= crate::state::DUST_DEBT_THRESHOLD;
    if has_dust {
        log!(
            INFO,
            "[withdraw_partial_collateral] Forgiving dust debt of {} on vault #{}",
            vault.borrowed_icusd_amount,
            vault_id
        );
        mutate_state(|s| {
            if let Some(v) = s.vault_id_to_vaults.get_mut(&vault_id) {
                v.borrowed_icusd_amount = ICUSD::new(0);
                v.accrued_interest = ICUSD::new(0);
            }
            // Wave-8b LIQ-002: dust forgiveness changes debt → re-key.
            s.reindex_vault_cr(vault_id);
        });
    }

    // Calculate max withdrawable amount that keeps CR >= minimum
    let max_withdrawable = if vault.borrowed_icusd_amount == ICUSD::new(0) || has_dust {
        // No debt (or dust forgiven) — can withdraw everything
        vault_collateral
    } else {
        // min_collateral_value = debt * min_ratio
        // min_collateral_amount = icusd_to_collateral_amount(min_collateral_value, price, decimals)
        // max_withdrawable = current_collateral - min_collateral_amount
        let min_ratio = read_state(|s| {
            let base = s.get_min_collateral_ratio_for(&vault.collateral_type);
            if s.mode == Mode::Recovery {
                let recovery_cr = s.get_recovery_cr_for(&vault.collateral_type);
                if recovery_cr > base {
                    recovery_cr
                } else {
                    base
                }
            } else {
                base
            }
        });
        let min_collateral_value: ICUSD = vault.borrowed_icusd_amount * min_ratio;
        let min_collateral_raw = crate::numeric::icusd_to_collateral_amount(
            min_collateral_value,
            collateral_price,
            config_decimals,
        );
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

    // Note: margin is reduced in record_partial_collateral_withdrawn (via remove_margin_from_vault)
    // after the transfer succeeds. Do NOT also subtract here — that would double-deduct.

    // P4: native-XRP collateral leaves into an XrpClaim instead of an ICRC transfer.
    // Reduce the vault collateral (same as the ICRC success path) and record the
    // claim; the full withdraw_amount becomes the claim (XRPL fee taken at settle).
    if is_native_xrp {
        let now_ns = ic_cdk::api::time();
        let claim_id = mutate_state(|s| {
            crate::event::record_partial_collateral_withdrawn(s, vault_id, withdraw_amount, 0);
            record_xrp_claim(
                s,
                caller,
                caller,
                vault_id,
                withdraw_amount.to_u64(),
                now_ns,
            )
        });
        log!(
            INFO,
            "[withdraw_partial_collateral] vault #{} native-XRP collateral -> XRP claim #{}",
            vault_id,
            claim_id
        );
        return Ok(claim_id);
    }

    let fee = ICP::from(ledger_fee);
    let transfer_amount = withdraw_amount - fee;

    log!(
        INFO,
        "[withdraw_partial_collateral] Transferring {} (after fee) to {}",
        transfer_amount,
        caller
    );

    match management::transfer_collateral(transfer_amount.to_u64(), caller, ledger_canister_id)
        .await
    {
        Ok(block_index) => {
            mutate_state(|s| {
                crate::event::record_partial_collateral_withdrawn(
                    s,
                    vault_id,
                    withdraw_amount,
                    block_index,
                )
            });

            log!(
                INFO,
                "[withdraw_partial_collateral] Successfully withdrew {} from vault #{}, block_index: {}",
                withdraw_amount,
                vault_id,
                block_index
            );

            Ok(block_index)
        }
        Err(error) => {
            // No need to restore vault state — collateral is only deducted on success
            // (in record_partial_collateral_withdrawn via remove_margin_from_vault).

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

/// Internal withdraw-collateral-and-close logic without guard management.
///
/// Called by both `withdraw_and_close_vault` (which acquires its own
/// `withdraw_and_close_{id}` guard) and `repay_and_close_vault` (which holds
/// a single `repay_and_close_{id}` guard spanning repay + withdraw + close).
///
/// Forgives dust debt, validates collateral status, optimistically zeroes the
/// vault's collateral, and transfers it out. ICRC collateral closes the vault
/// after transfer; native-XRP collateral creates a claim and leaves the vault
/// open because the XRPL account reserve stays locked.
async fn withdraw_and_close_vault_internal(
    caller: Principal,
    vault_id: u64,
) -> Result<Option<u64>, ProtocolError> {
    log!(
        INFO,
        "[withdraw_and_close] Request for vault #{} by principal {}",
        vault_id,
        caller
    );
    reject_active_xrp_sp_absorb_preflight(vault_id, ic_cdk::api::time())?;

    // Check if the vault exists first
    let vault = read_state(|s| {
        s.vault_id_to_vaults
            .get(&vault_id)
            .cloned()
            .ok_or(ProtocolError::GenericError(format!(
                "Vault #{} not found",
                vault_id
            )))
    })?;

    require_vault_not_processing(&vault)?;

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

    // Forgive dust debt before checking
    if vault.borrowed_icusd_amount.0 > 0
        && vault.borrowed_icusd_amount.0 <= crate::state::DUST_DEBT_THRESHOLD
    {
        log!(
            INFO,
            "[withdraw_and_close] Forgiving dust debt of {} on vault #{}",
            vault.borrowed_icusd_amount,
            vault_id
        );
        mutate_state(|s| {
            if let Some(v) = s.vault_id_to_vaults.get_mut(&vault_id) {
                v.borrowed_icusd_amount = ICUSD::new(0);
                v.accrued_interest = ICUSD::new(0);
            }
            // Wave-8b LIQ-002: dust forgiveness changes debt → re-key.
            s.reindex_vault_cr(vault_id);
        });
    } else if vault.borrowed_icusd_amount > ICUSD::new(0) {
        log!(
            INFO,
            "[withdraw_and_close] Vault #{} has outstanding debt of {} icUSD",
            vault_id,
            vault.borrowed_icusd_amount
        );
        return Err(ProtocolError::GenericError(format!(
            "Cannot close vault while it has outstanding debt of {} icUSD. Please repay all debt first.",
            vault.borrowed_icusd_amount
        )));
    }

    // Look up per-collateral config
    let (ledger_canister_id, ledger_fee, is_native_xrp) =
        read_state(|s| {
            let config = s.get_collateral_config(&vault.collateral_type).ok_or(
                ProtocolError::GenericError("Collateral type not configured".to_string()),
            )?;
            Ok::<_, ProtocolError>((
                config.ledger_canister_id,
                config.ledger_fee,
                config.is_native_xrp(),
            ))
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
            // Wave-8b LIQ-002: collateral changed → re-key.
            state.reindex_vault_cr(vault_id);
        });

        // P4: native-XRP collateral leaves into an XrpClaim, not an ICRC transfer.
        if is_native_xrp {
            let now_ns = ic_cdk::api::time();
            let claim_id = mutate_state(|s| {
                crate::event::record_collateral_withdrawn(s, vault_id, amount_to_transfer, 0);
                record_xrp_claim(
                    s,
                    caller,
                    caller,
                    vault_id,
                    amount_to_transfer.to_u64(),
                    now_ns,
                )
            });
            log!(
                INFO,
                "[withdraw_and_close] vault #{} native-XRP collateral -> XRP claim #{}",
                vault_id,
                claim_id
            );
            block_index = Some(claim_id);
        } else {
            // Make the collateral transfer with appropriate fee deduction
            let fee = ICP::from(ledger_fee);
            let transfer_amount = amount_to_transfer - fee;

            log!(
                INFO,
                "[withdraw_and_close] Transferring {} (after fee deduction) to {}",
                transfer_amount,
                caller
            );

            match management::transfer_collateral(
                transfer_amount.to_u64(),
                caller,
                ledger_canister_id,
            )
            .await
            {
                Ok(idx) => {
                    // Record the withdrawal event
                    mutate_state(|s| {
                        crate::event::record_collateral_withdrawn(
                            s,
                            vault_id,
                            amount_to_transfer,
                            idx,
                        )
                    });

                    log!(
                    INFO,
                    "[withdraw_and_close] Successfully withdrew {} from vault #{}, block_index: {}",
                    amount_to_transfer,
                    vault_id,
                    idx
                );

                    block_index = Some(idx);
                }
                Err(error) => {
                    // CRITICAL: If the transfer fails, restore the collateral and exit WITHOUT closing the vault
                    mutate_state(|state| {
                        if let Some(vault) = state.vault_id_to_vaults.get_mut(&vault_id) {
                            vault.collateral_amount = amount_to_transfer.to_u64();
                        }
                        // Wave-8b LIQ-002: rollback restores collateral → re-key.
                        state.reindex_vault_cr(vault_id);
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
        } // end native-XRP `else` (the ICRC transfer path)
    } else {
        log!(
            INFO,
            "[withdraw_and_close] Vault #{} has no collateral to withdraw",
            vault_id
        );
    };

    if withdraw_close_completion_policy(is_native_xrp)
        == WithdrawCloseCompletionPolicy::KeepNativeXrpVaultOpen
    {
        log!(
            INFO,
            "[withdraw_and_close] Keeping native-XRP vault #{} open because the XRPL reserve remains locked",
            vault_id
        );
        return Ok(block_index);
    }

    // Now close the vault - only if we've successfully transferred any funds
    // or if there were no funds to transfer
    mutate_state(|s| {
        // Make sure vault exists before attempting to remove
        if s.vault_id_to_vaults.contains_key(&vault_id) {
            // Record the combined withdraw and close event
            crate::event::record_withdraw_and_close_vault(
                s,
                vault_id,
                amount_to_transfer,
                block_index,
            );

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

pub async fn withdraw_and_close_vault(vault_id: u64) -> Result<Option<u64>, ProtocolError> {
    let caller = ic_cdk::caller();
    // Use a specific name for better tracking
    let _guard_principal =
        GuardPrincipal::new(caller, &format!("withdraw_and_close_{}", vault_id))?;
    // AR-B-003: per-vault op lock; see guard.rs::VaultLiquidationGuard.
    let _vault_op_guard = VaultLiquidationGuard::new(vault_id)?;

    withdraw_and_close_vault_internal(caller, vault_id).await
}

/// Compound repay + withdraw + close in a single canister call.
///
/// Pulls icUSD via `icrc2_transfer_from` to zero the vault's debt, then
/// withdraws all collateral and deletes the vault — all under a single
/// `repay_and_close_{vault_id}` guard. This lets Oisy / ICRC-49 signer
/// wallets close a borrowed vault with 2 consent screens (approve + this
/// call) instead of 4 (approve + repay + approve + withdraw_and_close)
/// when calling the separate methods sequentially.
///
/// `arg.amount` is the icUSD amount to repay. Per `repay_to_vault_internal`,
/// the amount is capped to actual debt and snaps to full-repayment if within
/// 1% / 0.01 icUSD dust. If repay leaves any debt, the close phase fails
/// and the vault stays open (but the partial repay is preserved on-chain).
///
/// Returns the icUSD repay block index and the optional collateral-return
/// block index (None if the vault had no collateral, e.g. due to liquidation).
#[derive(candid::CandidType, candid::Deserialize, Clone, Debug)]
pub struct RepayAndCloseSuccess {
    pub repay_block_index: u64,
    pub collateral_return_block_index: Option<u64>,
}

pub async fn repay_and_close_vault(arg: VaultArg) -> Result<RepayAndCloseSuccess, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let vault_id = arg.vault_id;
    let guard_principal = GuardPrincipal::new(caller, &format!("repay_and_close_{}", vault_id))?;
    // AR-B-003: per-vault op lock spanning repay + withdraw + close.
    let _vault_op_guard = match VaultLiquidationGuard::new(vault_id) {
        Ok(g) => g,
        Err(e) => {
            guard_principal.fail();
            return Err(e);
        }
    };

    // Phase 1: repay. On failure the guard fails and we propagate the error —
    // no collateral movement attempted. `is_full_close=true` lets vaults stuck
    // in the (DUST_DEBT_THRESHOLD, MIN_ICUSD_AMOUNT) zone clear their debt
    // here, since the close phase below will zero the vault entirely.
    let repay_block_index = match repay_to_vault_internal(caller, arg, true).await {
        Ok(idx) => idx,
        Err(e) => {
            guard_principal.fail();
            return Err(e);
        }
    };

    // Phase 2: withdraw + close. If this fails (e.g. collateral transfer
    // bounces), the repay is already on-chain — the vault stays open with
    // debt=0 and full collateral, recoverable via the existing
    // `withdraw_and_close_vault` endpoint. Surface a descriptive error.
    match withdraw_and_close_vault_internal(caller, vault_id).await {
        Ok(collateral_return_block_index) => {
            guard_principal.complete();
            Ok(RepayAndCloseSuccess {
                repay_block_index,
                collateral_return_block_index,
            })
        }
        Err(e) => {
            guard_principal.fail();
            log!(
                INFO,
                "[repay_and_close_vault] Repay succeeded (block {}) but withdraw/close failed for vault #{}: {:?}. Vault is recoverable via withdraw_and_close_vault.",
                repay_block_index,
                vault_id,
                e
            );
            Err(e)
        }
    }
}

pub async fn liquidate_vault_partial(
    vault_id: u64,
    icusd_amount: u64,
) -> Result<SuccessWithFee, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal =
        GuardPrincipal::new(caller, &format!("liquidate_vault_partial_{}", vault_id))?;
    reject_if_bot_processing(vault_id)?; // LIQ-101: don't double-seize a bot-claimed vault
                                         // BK-001/002: per-vault lock so two different callers can't race this vault
                                         // and both be paid the full pre-state collateral from the shared pool.
    let _vault_liq_guard = VaultLiquidationGuard::new(vault_id)?;
    if let Err(e) = reject_active_xrp_sp_absorb_preflight(vault_id, ic_cdk::api::time()) {
        guard_principal.fail();
        return Err(e);
    }

    // Wave-8b LIQ-002 band gate deactivated 2026-05-18. The per-vault CR
    // check below remains the authoritative liquidatability test. See
    // `tests/audit_pocs_liq_002_sorted_troves_index.rs` ("Layer 2.5 —
    // band gate DEACTIVATION fence") for background. The helper
    // `state::is_within_liquidation_band` is preserved as dead code for
    // future MEV-resistance re-introduction (per-collateral index +
    // liquidatable-filtered floor).

    let liquidation_amount: ICUSD = icusd_amount.into();

    if liquidation_amount < read_state(|s| s.min_icusd_amount) {
        guard_principal.fail();
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: read_state(|s| s.min_icusd_amount).to_u64(),
        });
    }

    // Step 1: Validate vault is liquidatable and get partial liquidation amounts
    let (
        vault,
        collateral_price,
        config_decimals,
        collateral_price_usd,
        _mode,
        max_liquidatable_debt,
        collateral_to_liquidator,
        total_to_seize,
        protocol_cut,
    ) = match read_state(|s| {
        match s.vault_id_to_vaults.get(&vault_id) {
            Some(vault) => {
                // Check collateral status allows liquidation
                if let Some(status) = s.get_collateral_status(&vault.collateral_type) {
                    if !status.allows_liquidation() {
                        return Err(
                            "Liquidation is not allowed for this collateral type.".to_string()
                        );
                    }
                }

                let price = s
                    .get_collateral_price_decimal(&vault.collateral_type)
                    .ok_or_else(|| {
                        "No price available for collateral. Price feed may be down.".to_string()
                    })?;
                let decimals = s
                    .get_collateral_config(&vault.collateral_type)
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
                    let max_liquidatable =
                        s.compute_partial_liquidation_cap(vault, collateral_price_usd);

                    // Ensure requested amount doesn't exceed maximum
                    let capped_amount = liquidation_amount
                        .min(max_liquidatable)
                        .min(vault.borrowed_icusd_amount);

                    // LIQ-003: round residual up to full debt if it would land
                    // in (0, min_vault_debt). Mirrors the repay-side invariant.
                    let min_vault_debt = s
                        .get_collateral_config(&vault.collateral_type)
                        .map(|c| c.min_vault_debt)
                        .unwrap_or(ICUSD::new(0));
                    let actual_liquidation_amount =
                        round_up_partial_liq_dust(vault, capped_amount, min_vault_debt);

                    if actual_liquidation_amount == ICUSD::new(0) {
                        return Err("Cannot liquidate zero amount".to_string());
                    }

                    // Calculate collateral to transfer (debt + liquidation bonus)
                    let liq_bonus = s.get_liquidation_bonus_for(&vault.collateral_type);
                    let protocol_share = s.get_liquidation_protocol_share();
                    let collateral_raw = crate::numeric::icusd_to_collateral_amount(
                        actual_liquidation_amount,
                        price,
                        decimals,
                    );
                    let collateral_with_bonus = ICP::from(collateral_raw) * liq_bonus;
                    let total_to_seize =
                        collateral_with_bonus.min(ICP::from(vault.collateral_amount));

                    // Split: protocol gets a share of the bonus portion (liquidator's profit)
                    let bonus_portion = total_to_seize.to_u64().saturating_sub(collateral_raw);
                    let protocol_cut = (rust_decimal::Decimal::from(bonus_portion)
                        * protocol_share.0)
                        .to_u64()
                        .unwrap_or(0);
                    let collateral_to_liquidator =
                        ICP::from(total_to_seize.to_u64() - protocol_cut);

                    Ok((
                        vault.clone(),
                        price,
                        decimals,
                        collateral_price_usd,
                        s.mode,
                        actual_liquidation_amount,
                        collateral_to_liquidator,
                        total_to_seize,
                        protocol_cut,
                    ))
                }
            }
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
        "[liquidate_vault_partial] Vault #{}: liquidating {} icUSD (max: {}), getting {} ICP collateral (protocol fee: {} ICP)",
        vault_id,
        max_liquidatable_debt.to_u64(),
        vault.borrowed_icusd_amount.to_u64(),
        collateral_to_liquidator.to_u64(),
        protocol_cut
    );

    // Step 2: Take icUSD from liquidator
    let icusd_block_index = match transfer_icusd_from(max_liquidatable_debt, caller).await {
        Ok(block_index) => {
            log!(
                INFO,
                "[liquidate_vault_partial] Received {} icUSD from liquidator",
                max_liquidatable_debt.to_u64()
            );
            block_index
        }
        Err(transfer_from_error) => {
            guard_principal.fail();
            return Err(ProtocolError::TransferFromError(
                transfer_from_error,
                max_liquidatable_debt.to_u64(),
            ));
        }
    };

    // Step 3: Update protocol state (partial liquidation)
    let (interest_share, xrp_claim_id) = mutate_state(|s| {
        // Compute proportional interest share before reducing debt
        let interest_share = if let Some(vault) = s.vault_id_to_vaults.get(&vault_id) {
            if vault.accrued_interest.0 > 0 && vault.borrowed_icusd_amount.0 > 0 {
                let share = (rust_decimal::Decimal::from(max_liquidatable_debt.0)
                    * rust_decimal::Decimal::from(vault.accrued_interest.0)
                    / rust_decimal::Decimal::from(vault.borrowed_icusd_amount.0))
                .to_u64()
                .unwrap_or(0);
                ICUSD::new(share.min(vault.accrued_interest.0))
            } else {
                ICUSD::new(0)
            }
        } else {
            ICUSD::new(0)
        };

        // Reduce vault debt and collateral directly
        // Vault loses total_to_seize (liquidator + protocol cut)
        //
        // AR-B-001/BK-001 (audit 2026-06-09): the applied amounts captured
        // here also drive the PAYOUT below. Pre-fix, the payout used the
        // stale pre-await `collateral_to_liquidator`, so any concurrent
        // reduction of this vault paid the liquidator collateral the vault
        // no longer had, draining the shared pool. The per-vault op lock
        // makes such a reduction unreachable; the re-capped payout keeps any
        // residual drift solvency-safe.
        let mut debt_applied = max_liquidatable_debt;
        let mut collateral_applied = total_to_seize.to_u64();
        if let Some(vault) = s.vault_id_to_vaults.get_mut(&vault_id) {
            // ASYNC-001: cap each reduction to the CURRENT vault state and
            // saturating_sub. A concurrent partial liquidation may have reduced
            // this vault between our pre-await read and now; without the cap the
            // ICUSD Token::sub would underflow-PANIC and the raw u64 collateral
            // sub would WRAP, both after the liquidator's icUSD was already pulled.
            debt_applied = max_liquidatable_debt.min(vault.borrowed_icusd_amount);
            collateral_applied = total_to_seize.to_u64().min(vault.collateral_amount);
            let interest_applied = interest_share.min(vault.accrued_interest);
            vault.borrowed_icusd_amount = vault.borrowed_icusd_amount.saturating_sub(debt_applied);
            vault.collateral_amount = vault.collateral_amount.saturating_sub(collateral_applied);
            vault.accrued_interest = vault.accrued_interest.saturating_sub(interest_applied);
        }
        let payout_to_liquidator = ICP::from(collateral_applied.saturating_sub(protocol_cut));

        // Wave-10 LIQ-008: append the gross debt cleared to the rolling-
        // window log. Records all liquidations (healthy and underwater) so
        // the circuit breaker can pause auto-publishing during cascades.
        crate::event::record_liquidation_for_breaker(s, max_liquidatable_debt.to_u64());

        // Wave-8e LIQ-005: per-call deficit accrual against the APPLIED
        // amounts. Predicate: seized USD < debt cleared.
        let seized_usd = crate::numeric::collateral_usd_value(
            collateral_applied,
            collateral_price,
            config_decimals,
        );
        let shortfall = if seized_usd < debt_applied {
            debt_applied - seized_usd
        } else {
            ICUSD::new(0)
        };
        if shortfall.0 > 0 {
            crate::event::record_deficit_accrued(
                s,
                crate::event::DeficitSource::Liquidation { vault_id },
                shortfall,
                ic_cdk::api::time(),
            );
            if s.check_deficit_readonly_latch() {
                log!(INFO,
                    "[LIQ-005] deficit threshold {} crossed by partial vault #{} shortfall {}; auto-latched ReadOnly",
                    s.deficit_readonly_threshold_e8s, vault_id, shortfall.to_u64()
                );
            }
        }

        // Record the partial liquidation event (applied payout, so replay's
        // per-event deduction mirrors live state exactly)
        let event = crate::event::Event::PartialLiquidateVault {
            vault_id,
            liquidator_payment: max_liquidatable_debt,
            icp_to_liquidator: payout_to_liquidator,
            liquidator: Some(caller),
            icp_rate: Some(collateral_price_usd),
            protocol_fee_collateral: if protocol_cut > 0 {
                Some(protocol_cut.min(collateral_applied))
            } else {
                None
            },
            timestamp: Some(ic_cdk::api::time()),
            three_usd_reserves_e8s: None,
        };
        crate::storage::record_event(&event);

        // Liquidator-reward payout: PendingMarginTransfer for ICRC, XrpClaim for
        // native-XRP. Capture vault.owner (custody key) BEFORE cleanup_if_drained.
        let nonce = s.next_op_nonce();
        let xrp_claim_id = queue_collateral_payout(
            s,
            vault_id,
            vault.owner,
            caller,
            payout_to_liquidator,
            vault.collateral_type,
            nonce,
            ic_cdk::api::time(),
        );

        // Shared drain rule (see state::cleanup_if_drained): remove the vault
        // if this liquidation emptied it, else re-key its CR index entry.
        if s.cleanup_if_drained(vault_id) {
            log!(
                INFO,
                "[liquidate_vault_partial] Vault #{} fully liquidated — removed",
                vault_id
            );
        }

        log!(
            INFO,
            "[liquidate_vault_partial] Partial liquidation completed, {} pending transfers created",
            1
        );
        (interest_share, xrp_claim_id)
    });

    // Route interest share via N-way split
    // IC-B-002 (audit 2026-06-09): re-queue any unminted interest share so the
    // next flush retries it instead of silently dropping treasury revenue.
    let unminted_interest =
        crate::treasury::distribute_interest(interest_share, vault.collateral_type).await;
    if unminted_interest.to_u64() > 0 {
        mutate_state(|s| {
            s.restore_pending_interest_for_pool(vault.collateral_type, unminted_interest.to_u64())
        });
    }

    // Send protocol's liquidation fee cut to treasury (fire-and-forget)
    if protocol_cut > 0 {
        if vault.collateral_type == crate::state::xrp_collateral_principal() {
            // P5: native-XRP protocol fee -> a developer-settleable XrpClaim (the
            // ICRC treasury transfer cannot target the synthetic XRP ledger). Keyed
            // by collateral_type (not a vault lookup, since the vault may already be
            // drained/removed by cleanup_if_drained above).
            let dev = read_state(|s| s.developer_principal);
            let now_ns = ic_cdk::api::time();
            mutate_state(|s| {
                record_xrp_claim(
                    s,
                    dev,
                    vault.owner,
                    vault.vault_id,
                    protocol_cut.to_u64().unwrap_or(0),
                    now_ns,
                );
            });
        } else {
            let asset_type = crate::treasury::collateral_to_asset_type(&vault.collateral_type);
            crate::treasury::send_liquidation_fee_to_treasury(
                protocol_cut,
                vault.collateral_type,
                asset_type,
            )
            .await;
        }
    }

    // Step 4: Process transfer (same as complete liquidation)
    match try_process_pending_transfers_immediate(vault_id).await {
        Ok(processed_count) => {
            log!(
                INFO,
                "[liquidate_vault_partial] Successfully processed {} transfers immediately",
                processed_count
            );
        }
        Err(e) => {
            log!(INFO, "[liquidate_vault_partial] Immediate processing failed: {}. Transfers will be retried via timer", e);
            schedule_transfer_retry(vault_id, 0);
        }
    }

    ic_cdk_timers::set_timer(std::time::Duration::from_secs(2), move || {
        ic_cdk::spawn(async move {
            log!(
                INFO,
                "[liquidate_vault_partial] Backup timer processing transfers for vault #{}",
                vault_id
            );
            let _ = crate::process_pending_transfer().await;
        })
    });

    guard_principal.complete();

    // Calculate fee (liquidator bonus)
    let liquidator_value_received = crate::numeric::collateral_usd_value(
        collateral_to_liquidator.to_u64(),
        collateral_price,
        config_decimals,
    );
    let fee_amount = if liquidator_value_received > max_liquidatable_debt {
        liquidator_value_received - max_liquidatable_debt
    } else {
        ICUSD::new(0)
    };

    log!(INFO, "[liquidate_vault_partial] Partial liquidation completed. Block index: {}, Fee: {}, Collateral: {}",
         icusd_block_index, fee_amount.to_u64(), collateral_to_liquidator.to_u64());

    Ok(SuccessWithFee {
        block_index: icusd_block_index,
        fee_amount_paid: fee_amount.to_u64(),
        collateral_amount_received: Some(collateral_to_liquidator.to_u64()),
        debt_liquidated_e8s: Some(max_liquidatable_debt.to_u64()), // SP-101
        stable_pulled_e6s: None, // SP-110 (icUSD path: no stable surcharge)
        xrp_claim_id,
    })
}

/// Liquidate a vault using ckUSDT or ckUSDC (1:1 with icUSD, plus configurable fee)
pub async fn liquidate_vault_partial_with_stable(
    vault_id: u64,
    stable_amount: u64,
    token_type: StableTokenType,
) -> Result<SuccessWithFee, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal =
        GuardPrincipal::new(caller, &format!("liquidate_vault_stable_{}", vault_id))?;
    reject_if_bot_processing(vault_id)?; // LIQ-101: don't double-seize a bot-claimed vault
    let _vault_liq_guard = VaultLiquidationGuard::new(vault_id)?; // BK-001/002 per-vault lock
    if let Err(e) = reject_active_xrp_sp_absorb_preflight(vault_id, ic_cdk::api::time()) {
        guard_principal.fail();
        return Err(e);
    }

    // Wave-8b LIQ-002 band gate deactivated 2026-05-18 (see
    // `liquidate_vault_partial` above for rationale).

    // Check if the selected stable token is enabled
    let is_enabled = read_state(|s| match token_type {
        StableTokenType::CKUSDT => s.ckusdt_enabled,
        StableTokenType::CKUSDC => s.ckusdc_enabled,
    });
    if !is_enabled {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(format!(
            "{:?} liquidations are currently disabled",
            token_type
        )));
    }

    // Depeg protection: fetch fresh stablecoin price and reject if outside $0.95–$1.05
    if let Err(e) = crate::xrc::ensure_stable_not_depegged(&token_type).await {
        guard_principal.fail();
        return Err(e);
    }

    // Truncate to nearest 100 e8s for clean 8→6 decimal conversion
    let raw_amount_e8s = stable_amount - (stable_amount % 100);
    let liquidation_amount: ICUSD = raw_amount_e8s.into();

    if liquidation_amount < read_state(|s| s.min_icusd_amount) {
        guard_principal.fail();
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: read_state(|s| s.min_icusd_amount).to_u64(),
        });
    }

    // Step 1: Validate vault is liquidatable and get partial liquidation amounts
    let (
        vault,
        collateral_price,
        config_decimals,
        collateral_price_usd,
        _mode,
        max_liquidatable_debt,
        collateral_to_liquidator,
        total_to_seize,
        protocol_cut,
    ) = match read_state(|s| {
        match s.vault_id_to_vaults.get(&vault_id) {
            Some(vault) => {
                // Check collateral status allows liquidation
                if let Some(status) = s.get_collateral_status(&vault.collateral_type) {
                    if !status.allows_liquidation() {
                        return Err(
                            "Liquidation is not allowed for this collateral type.".to_string()
                        );
                    }
                }

                let price = s
                    .get_collateral_price_decimal(&vault.collateral_type)
                    .ok_or_else(|| {
                        "No price available for collateral. Price feed may be down.".to_string()
                    })?;
                let decimals = s
                    .get_collateral_config(&vault.collateral_type)
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
                    let max_liquidatable =
                        s.compute_partial_liquidation_cap(vault, collateral_price_usd);

                    let capped_amount = liquidation_amount
                        .min(max_liquidatable)
                        .min(vault.borrowed_icusd_amount);

                    // LIQ-003: round residual up to full debt if it would land
                    // in (0, min_vault_debt). Mirrors the repay-side invariant.
                    let min_vault_debt = s
                        .get_collateral_config(&vault.collateral_type)
                        .map(|c| c.min_vault_debt)
                        .unwrap_or(ICUSD::new(0));
                    let actual_liquidation_amount =
                        round_up_partial_liq_dust(vault, capped_amount, min_vault_debt);

                    if actual_liquidation_amount == ICUSD::new(0) {
                        return Err("Cannot liquidate zero amount".to_string());
                    }

                    let liq_bonus = s.get_liquidation_bonus_for(&vault.collateral_type);
                    let protocol_share = s.get_liquidation_protocol_share();
                    let collateral_raw = crate::numeric::icusd_to_collateral_amount(
                        actual_liquidation_amount,
                        price,
                        decimals,
                    );
                    let collateral_with_bonus = ICP::from(collateral_raw) * liq_bonus;
                    let total_to_seize =
                        collateral_with_bonus.min(ICP::from(vault.collateral_amount));

                    // Split: protocol gets a share of the bonus portion
                    let bonus_portion = total_to_seize.to_u64().saturating_sub(collateral_raw);
                    let protocol_cut = (rust_decimal::Decimal::from(bonus_portion)
                        * protocol_share.0)
                        .to_u64()
                        .unwrap_or(0);
                    let collateral_to_liquidator =
                        ICP::from(total_to_seize.to_u64() - protocol_cut);

                    Ok((
                        vault.clone(),
                        price,
                        decimals,
                        collateral_price_usd,
                        s.mode,
                        actual_liquidation_amount,
                        collateral_to_liquidator,
                        total_to_seize,
                        protocol_cut,
                    ))
                }
            }
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
        "[liquidate_vault_stable] Vault #{}: liquidating {} {:?} (max: {}), getting {} ICP collateral (protocol fee: {} ICP)",
        vault_id,
        max_liquidatable_debt.to_u64(),
        token_type,
        vault.borrowed_icusd_amount.to_u64(),
        collateral_to_liquidator.to_u64(),
        protocol_cut
    );

    // Step 2: Convert e8s to e6s and add fee surcharge, then take stable token from liquidator
    let debt_e8s = max_liquidatable_debt.to_u64();
    let base_stable_e6s = debt_e8s / 100;
    let fee_rate = read_state(|s| s.ckstable_repay_fee);
    let fee_e6s = (rust_decimal::Decimal::from(base_stable_e6s) * fee_rate.0)
        .to_u64()
        .unwrap_or(0);
    let total_pull_e6s = base_stable_e6s + fee_e6s;

    let stable_block_index =
        match transfer_stable_from(token_type.clone(), total_pull_e6s, caller).await {
            Ok(block_index) => {
                log!(
                    INFO,
                    "[liquidate_vault_stable] Received {} e6s {:?} from liquidator (fee: {} e6s)",
                    total_pull_e6s,
                    token_type,
                    fee_e6s
                );
                block_index
            }
            Err(transfer_from_error) => {
                guard_principal.fail();
                return Err(ProtocolError::TransferFromError(
                    transfer_from_error,
                    total_pull_e6s,
                ));
            }
        };

    // Step 3: Update protocol state (partial liquidation)
    let (interest_share, xrp_claim_id) = mutate_state(|s| {
        // Compute proportional interest share before reducing debt
        let interest_share = if let Some(vault) = s.vault_id_to_vaults.get(&vault_id) {
            if vault.accrued_interest.0 > 0 && vault.borrowed_icusd_amount.0 > 0 {
                let share = (rust_decimal::Decimal::from(max_liquidatable_debt.0)
                    * rust_decimal::Decimal::from(vault.accrued_interest.0)
                    / rust_decimal::Decimal::from(vault.borrowed_icusd_amount.0))
                .to_u64()
                .unwrap_or(0);
                ICUSD::new(share.min(vault.accrued_interest.0))
            } else {
                ICUSD::new(0)
            }
        } else {
            ICUSD::new(0)
        };

        // Reduce vault debt and collateral directly
        // Vault loses total_to_seize (liquidator + protocol cut)
        // AR-B-001/BK-001 (audit 2026-06-09): capture applied amounts and
        // re-cap the payout, mirroring `liquidate_vault_partial`.
        let mut debt_applied = max_liquidatable_debt;
        let mut collateral_applied = total_to_seize.to_u64();
        if let Some(vault) = s.vault_id_to_vaults.get_mut(&vault_id) {
            // ASYNC-001: cap each reduction to the CURRENT vault state and
            // saturating_sub. A concurrent partial liquidation may have reduced
            // this vault between our pre-await read and now; without the cap the
            // ICUSD Token::sub would underflow-PANIC and the raw u64 collateral
            // sub would WRAP, both after the liquidator's icUSD was already pulled.
            debt_applied = max_liquidatable_debt.min(vault.borrowed_icusd_amount);
            collateral_applied = total_to_seize.to_u64().min(vault.collateral_amount);
            let interest_applied = interest_share.min(vault.accrued_interest);
            vault.borrowed_icusd_amount = vault.borrowed_icusd_amount.saturating_sub(debt_applied);
            vault.collateral_amount = vault.collateral_amount.saturating_sub(collateral_applied);
            vault.accrued_interest = vault.accrued_interest.saturating_sub(interest_applied);
        }
        let payout_to_liquidator = ICP::from(collateral_applied.saturating_sub(protocol_cut));

        // Wave-10 LIQ-008: append the gross debt cleared to the rolling-
        // window log for the mass-liquidation circuit breaker.
        crate::event::record_liquidation_for_breaker(s, max_liquidatable_debt.to_u64());

        // Wave-8e LIQ-005: per-call deficit accrual against the APPLIED
        // amounts. The stablecoin path pulls ckUSDT/ckUSDC from the
        // liquidator (1:1 with icUSD plus a surcharge). Predicate measured
        // in icUSD-equivalent collateral USD value.
        let seized_usd = crate::numeric::collateral_usd_value(
            collateral_applied,
            collateral_price,
            config_decimals,
        );
        let shortfall = if seized_usd < debt_applied {
            debt_applied - seized_usd
        } else {
            ICUSD::new(0)
        };
        if shortfall.0 > 0 {
            crate::event::record_deficit_accrued(
                s,
                crate::event::DeficitSource::Liquidation { vault_id },
                shortfall,
                ic_cdk::api::time(),
            );
            if s.check_deficit_readonly_latch() {
                log!(INFO,
                    "[LIQ-005] deficit threshold {} crossed by stable-partial vault #{} shortfall {}; auto-latched ReadOnly",
                    s.deficit_readonly_threshold_e8s, vault_id, shortfall.to_u64()
                );
            }
        }

        // Record the partial liquidation event (applied payout, replay-exact)
        let event = crate::event::Event::PartialLiquidateVault {
            vault_id,
            liquidator_payment: max_liquidatable_debt,
            icp_to_liquidator: payout_to_liquidator,
            liquidator: Some(caller),
            icp_rate: Some(collateral_price_usd),
            protocol_fee_collateral: if protocol_cut > 0 {
                Some(protocol_cut.min(collateral_applied))
            } else {
                None
            },
            timestamp: Some(ic_cdk::api::time()),
            three_usd_reserves_e8s: None,
        };
        crate::storage::record_event(&event);

        // Create pending transfer for liquidator reward
        let nonce = s.next_op_nonce();
        let xrp_claim_id = queue_collateral_payout(
            s,
            vault_id,
            vault.owner,
            caller,
            payout_to_liquidator,
            vault.collateral_type,
            nonce,
            ic_cdk::api::time(),
        );

        // Shared drain rule (see state::cleanup_if_drained): remove the vault
        // if this liquidation emptied it, else re-key its CR index entry.
        if s.cleanup_if_drained(vault_id) {
            log!(
                INFO,
                "[liquidate_vault_stable] Vault #{} fully liquidated — removed",
                vault_id
            );
        }

        log!(
            INFO,
            "[liquidate_vault_stable] Partial liquidation completed, pending transfer created"
        );
        (interest_share, xrp_claim_id)
    });

    // Route interest via N-way split (stablecoin-denominated)
    if interest_share.to_u64() > 0 {
        crate::treasury::distribute_stablecoin_interest(
            interest_share.to_u64(),
            vault.collateral_type,
            token_type.clone(),
        )
        .await;
    }

    // Route fee surcharge to treasury as stablecoins (mirrors repay_to_vault_with_stable)
    if fee_e6s > 0 {
        let (treasury, stable_ledger) = read_state(|s| {
            let ledger = match token_type {
                StableTokenType::CKUSDT => s.ckusdt_ledger_principal,
                StableTokenType::CKUSDC => s.ckusdc_ledger_principal,
            };
            (s.treasury_principal, ledger)
        });
        if let (Some(treasury_principal), Some(stable_ledger)) = (treasury, stable_ledger) {
            match management::transfer_collateral(fee_e6s, treasury_principal, stable_ledger).await
            {
                Ok(block) => {
                    log!(INFO,
                        "[liquidate_vault_stable] Transferred {} e6s fee surcharge to treasury (block {})",
                        fee_e6s, block
                    );
                }
                Err(e) => {
                    log!(INFO,
                        "[liquidate_vault_stable] Fee surcharge transfer to treasury failed: {:?}. Fee remains in reserves.",
                        e
                    );
                }
            }
        }
    }

    // Send protocol's liquidation fee cut to treasury (fire-and-forget)
    if protocol_cut > 0 {
        if vault.collateral_type == crate::state::xrp_collateral_principal() {
            // P5: native-XRP protocol fee -> a developer-settleable XrpClaim (the
            // ICRC treasury transfer cannot target the synthetic XRP ledger). Keyed
            // by collateral_type (not a vault lookup, since the vault may already be
            // drained/removed by cleanup_if_drained above).
            let dev = read_state(|s| s.developer_principal);
            let now_ns = ic_cdk::api::time();
            mutate_state(|s| {
                record_xrp_claim(
                    s,
                    dev,
                    vault.owner,
                    vault.vault_id,
                    protocol_cut.to_u64().unwrap_or(0),
                    now_ns,
                );
            });
        } else {
            let asset_type = crate::treasury::collateral_to_asset_type(&vault.collateral_type);
            crate::treasury::send_liquidation_fee_to_treasury(
                protocol_cut,
                vault.collateral_type,
                asset_type,
            )
            .await;
        }
    }

    // Step 4: Process transfer
    match try_process_pending_transfers_immediate(vault_id).await {
        Ok(processed_count) => {
            log!(
                INFO,
                "[liquidate_vault_stable] Successfully processed {} transfers immediately",
                processed_count
            );
        }
        Err(e) => {
            log!(INFO, "[liquidate_vault_stable] Immediate processing failed: {}. Transfers will be retried via timer", e);
            schedule_transfer_retry(vault_id, 0);
        }
    }

    ic_cdk_timers::set_timer(std::time::Duration::from_secs(2), move || {
        ic_cdk::spawn(async move {
            log!(
                INFO,
                "[liquidate_vault_stable] Backup timer processing transfers for vault #{}",
                vault_id
            );
            let _ = crate::process_pending_transfer().await;
        })
    });

    guard_principal.complete();

    // Calculate fee (liquidator bonus)
    let liquidator_value_received = crate::numeric::collateral_usd_value(
        collateral_to_liquidator.to_u64(),
        collateral_price,
        config_decimals,
    );
    let fee_amount = if liquidator_value_received > max_liquidatable_debt {
        liquidator_value_received - max_liquidatable_debt
    } else {
        ICUSD::new(0)
    };

    log!(
        INFO,
        "[liquidate_vault_stable] Liquidation completed. Block index: {}, Fee: {}, Collateral: {}",
        stable_block_index,
        fee_amount.to_u64(),
        collateral_to_liquidator.to_u64()
    );

    Ok(SuccessWithFee {
        block_index: stable_block_index,
        fee_amount_paid: fee_amount.to_u64(),
        collateral_amount_received: Some(collateral_to_liquidator.to_u64()),
        debt_liquidated_e8s: Some(max_liquidatable_debt.to_u64()), // SP-101
        stable_pulled_e6s: Some(total_pull_e6s), // SP-110: base + repay-fee surcharge
        xrp_claim_id,
    })
}

/// Liquidate a vault when the debt has already been covered externally.
///
/// Two modes:
/// - `three_usd_received_e8s: None` — legacy burn path: icUSD was destroyed via 3pool.
/// - `three_usd_received_e8s: Some(amount)` — reserves path: 3USD was transferred to
///   the backend's protocol reserves subaccount. No icUSD was burned; the 3USD in
///   reserves serves as backing for the written-off debt.
///
/// Called by the stability pool canister.
///
/// Wave-8d LIQ-004 Phase 2: `proof` is required. Every writedown must
/// reference an on-chain ICRC-3 block (a real icUSD burn for the legacy
/// path, or a real 3USD transfer to the protocol's reserves subaccount for
/// the reserves path). The Wave-8c migration window where `None` was
/// accepted with a per-call WARN log has been retired.
pub async fn liquidate_vault_debt_already_burned(
    vault_id: u64,
    icusd_burned_e8s: u64,
    caller: Principal,
    three_usd_received_e8s: Option<u64>,
    proof: crate::icrc3_proof::SpWritedownProof,
) -> Result<StabilityPoolLiquidationResult, ProtocolError> {
    // Wave-8b LIQ-002 band gate is deactivated globally as of 2026-05-18.
    // This path was never gated to begin with: it is the stability-pool-
    // triggered writedown, with the caller gated on
    // `caller == stability_pool_canister` by the entry point in `main.rs`
    // (`stability_pool_liquidate_*`), and the SP has already committed
    // icUSD via the 3pool burn. The CR index is still kept fresh by the
    // post-mutation `reindex_vault_cr` call at the end of this function so
    // any future re-introduction of the band check sees a current CR.

    // Wave-8c LIQ-004 (kill switch): admin-toggleable disable of this path.
    // Independent of `frozen` and `liquidation_frozen`. Use during a
    // confirmed SP compromise / drift event.
    if read_state(|s| s.sp_writedown_disabled) {
        return Err(ProtocolError::TemporarilyUnavailable(
            "SP writedown path is disabled by admin".to_string(),
        ));
    }

    // Defense-in-depth: native-XRP collateral can NEVER be liquidated via the SP
    // write-down path. This path proof-verifies + settles against the icUSD/3pool
    // ledger, but the seized collateral here is XRP held on XRPL. The SP cannot
    // settle that, so a write-down would strand the seized XRP (it never becomes
    // an `XrpClaim`) and burn SP depositors. Native-XRP is liquidated only via the
    // manual paths (`liquidate_vault` / `liquidate_vault_partial` /
    // `partial_liquidate_vault` / `liquidate_vault_partial_with_stable`), which
    // route collateral into an `XrpClaim`. The two `main.rs` entry points are the
    // first line of defense; this in-function reject is the backstop so any future
    // third caller (or a refactor that drops the caller-side check) cannot reach
    // the write-down. Placed before `GuardPrincipal::new` so it returns without
    // touching any guard/state (and before any `ic_cdk::api::time()` call).
    if vault_is_native_xrp(vault_id) {
        return Err(ProtocolError::GenericError(
            "Native-XRP collateral cannot be liquidated via the SP write-down path".to_string(),
        ));
    }

    let guard_principal =
        GuardPrincipal::new(caller, &format!("liquidate_vault_debt_burned_{}", vault_id))?;
    reject_if_bot_processing(vault_id)?; // LIQ-101: don't double-seize a bot-claimed vault (SP path)
    let _vault_liq_guard = VaultLiquidationGuard::new(vault_id)?; // BK-001/002 per-vault lock

    let liquidation_amount: ICUSD = icusd_burned_e8s.into();

    if liquidation_amount < read_state(|s| s.min_icusd_amount) {
        guard_principal.fail();
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: read_state(|s| s.min_icusd_amount).to_u64(),
        });
    }

    // Wave-8d LIQ-004 Phase 2 (replay defense + ICRC-3 verification). Verify
    // the proof BEFORE touching any state. If the proof's
    // (ledger_kind, block_index) was already consumed, refuse — pre-
    // mutation — so the caller gets a clean error instead of a stale partial
    // success. Then validate the on-chain block matches expected accounts,
    // amount, and (for IcusdBurn) memo.
    //
    // The Wave-8c migration WARN-on-None branch has been retired in Phase 2.
    if read_state(|s| {
        s.consumed_writedown_proofs
            .contains(&(proof.ledger_kind, proof.block_index))
    }) {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(format!(
            "SP writedown proof replay rejected: ({:?}, block {}) already consumed",
            proof.ledger_kind, proof.block_index
        )));
    }

    let (ledger_principal, reserves_account) = read_state(|s| {
        let ledger = match proof.ledger_kind {
            crate::icrc3_proof::SpProofLedger::IcusdBurn => s.icusd_ledger_principal,
            crate::icrc3_proof::SpProofLedger::ThreePoolTransfer => {
                s.three_pool_canister.unwrap_or(Principal::anonymous())
            }
        };
        let reserves = icrc_ledger_types::icrc1::account::Account {
            owner: ic_cdk::id(),
            subaccount: Some(crate::management::protocol_3usd_reserves_subaccount()),
        };
        (ledger, reserves)
    });

    if ledger_principal == Principal::anonymous() {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "SP writedown proof references a ledger that is not configured".to_string(),
        ));
    }

    let expected_amount_e8s = match proof.ledger_kind {
        crate::icrc3_proof::SpProofLedger::IcusdBurn => icusd_burned_e8s,
        crate::icrc3_proof::SpProofLedger::ThreePoolTransfer => three_usd_received_e8s.unwrap_or(0),
    };

    let expectations = crate::icrc3_proof::ProofExpectations {
        ledger_kind: proof.ledger_kind,
        expected_amount_e8s,
        sp_principal: caller,
        reserves_account,
        vault_id_memo: vault_id,
    };

    if let Err(err) = crate::icrc3_proof::fetch_and_validate_block(
        ledger_principal,
        proof.block_index,
        &expectations,
    )
    .await
    {
        guard_principal.fail();
        log!(
            INFO,
            "[liquidate_vault_debt_burned] [LIQ-004] proof verification FAILED for vault #{} \
             ({:?} block {}): {}",
            vault_id,
            proof.ledger_kind,
            proof.block_index,
            err
        );
        return Err(ProtocolError::GenericError(format!(
            "SP writedown proof verification failed: {}",
            err
        )));
    }

    if vault_id != proof.vault_id_memo {
        // For IcusdBurn `validate_block` already enforces this against the
        // memo. For ThreePoolTransfer there is no memo on the block (3pool
        // ledger doesn't persist memos into ICRC-3); this assertion is the
        // single binding point against the call's vault_id, so any internal
        // misconstruction surfaces with a tight error before mutation.
        guard_principal.fail();
        return Err(ProtocolError::GenericError(format!(
            "SP writedown proof vault_id_memo {} does not match call vault_id {}",
            proof.vault_id_memo, vault_id
        )));
    }

    // Step 1: Validate vault is liquidatable and compute collateral to release
    let (
        vault,
        collateral_price,
        config_decimals,
        collateral_price_usd,
        max_liquidatable_debt,
        collateral_to_liquidator,
        total_to_seize,
        protocol_cut,
    ) = match read_state(|s| {
        match s.vault_id_to_vaults.get(&vault_id) {
            Some(vault) => {
                if let Some(status) = s.get_collateral_status(&vault.collateral_type) {
                    if !status.allows_liquidation() {
                        return Err(
                            "Liquidation is not allowed for this collateral type.".to_string()
                        );
                    }
                }

                let price = s
                    .get_collateral_price_decimal(&vault.collateral_type)
                    .ok_or_else(|| {
                        "No price available for collateral. Price feed may be down.".to_string()
                    })?;
                let decimals = s
                    .get_collateral_config(&vault.collateral_type)
                    .map(|c| c.decimals)
                    .unwrap_or(8);
                let collateral_price_usd = UsdIcp::from(price);

                // NO CR CHECK HERE — icUSD was already burned by the 3pool.
                // The backend MUST honor the write-down regardless of vault health.
                // Rejecting would leave burned icUSD unaccounted for.
                {
                    let actual_liquidation_amount =
                        liquidation_amount.min(vault.borrowed_icusd_amount);

                    if actual_liquidation_amount == ICUSD::new(0) {
                        return Err("Cannot liquidate zero amount".to_string());
                    }

                    let liq_bonus = s.get_liquidation_bonus_for(&vault.collateral_type);
                    let protocol_share = s.get_liquidation_protocol_share();
                    let collateral_raw = crate::numeric::icusd_to_collateral_amount(
                        actual_liquidation_amount,
                        price,
                        decimals,
                    );
                    let collateral_with_bonus = ICP::from(collateral_raw) * liq_bonus;
                    let total_to_seize =
                        collateral_with_bonus.min(ICP::from(vault.collateral_amount));

                    let bonus_portion = total_to_seize.to_u64().saturating_sub(collateral_raw);
                    let protocol_cut = (rust_decimal::Decimal::from(bonus_portion)
                        * protocol_share.0)
                        .to_u64()
                        .unwrap_or(0);
                    let collateral_to_liquidator =
                        ICP::from(total_to_seize.to_u64() - protocol_cut);

                    Ok((
                        vault.clone(),
                        price,
                        decimals,
                        collateral_price_usd,
                        actual_liquidation_amount,
                        collateral_to_liquidator,
                        total_to_seize,
                        protocol_cut,
                    ))
                }
            }
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
        "[liquidate_vault_debt_burned] Vault #{}: writing down {} icUSD (burned via 3pool), releasing {} collateral (protocol fee: {})",
        vault_id, max_liquidatable_debt.to_u64(), collateral_to_liquidator.to_u64(), protocol_cut
    );

    // Wave-8c LIQ-004 (sanity log on healthy vaults): if the pre-call CR is
    // above min_liq_ratio, a buggy or malicious SP is writing down a
    // non-underwater vault. Log loudly so an operator can investigate. Do
    // NOT reject — the SP has already burned icUSD (or moved 3USD into
    // reserves), so refusing here would orphan that token movement. The
    // proof verification + kill switch are the enforcement layers; this
    // is purely an alarm.
    let pre_call_cr = read_state(|s| compute_collateral_ratio(&vault, collateral_price_usd, s));
    let min_liq = read_state(|s| s.get_min_liquidation_ratio_for(&vault.collateral_type));
    if pre_call_cr >= min_liq {
        log!(INFO,
            "[liquidate_vault_debt_burned] [LIQ-004] WARN: SP writedown applied to vault #{} \
             whose pre-call CR ({}) is above min_liq_ratio ({}). Caller={} proof={:?}. Investigate.",
            vault_id, pre_call_cr.to_f64(), min_liq.to_f64(), caller, proof
        );
    }

    // Step 2: SKIPPED — icUSD was already burned atomically in the 3pool.
    // The icUSD supply has already been reduced by `icusd_burned_e8s`.

    // Step 3: Update protocol state (partial liquidation)
    let interest_share = mutate_state(|s| {
        // Wave-8c LIQ-004: record the proof as consumed atomically with the
        // writedown so a partial failure cannot leave the proof unconsumed
        // (replay risk) or consumed without an effect (orphan risk).
        s.consumed_writedown_proofs
            .insert((proof.ledger_kind, proof.block_index));

        let interest_share = if let Some(vault) = s.vault_id_to_vaults.get(&vault_id) {
            if vault.accrued_interest.0 > 0 && vault.borrowed_icusd_amount.0 > 0 {
                let share = (rust_decimal::Decimal::from(max_liquidatable_debt.0)
                    * rust_decimal::Decimal::from(vault.accrued_interest.0)
                    / rust_decimal::Decimal::from(vault.borrowed_icusd_amount.0))
                .to_u64()
                .unwrap_or(0);
                ICUSD::new(share.min(vault.accrued_interest.0))
            } else {
                ICUSD::new(0)
            }
        } else {
            ICUSD::new(0)
        };

        // AR-B-001/BK-001 (audit 2026-06-09): capture applied amounts and
        // re-cap the payout, mirroring `liquidate_vault_partial`.
        let mut debt_applied = max_liquidatable_debt;
        let mut collateral_applied = total_to_seize.to_u64();
        if let Some(vault) = s.vault_id_to_vaults.get_mut(&vault_id) {
            // ASYNC-001: cap each reduction to the CURRENT vault state and
            // saturating_sub. A concurrent partial liquidation may have reduced
            // this vault between our pre-await read and now; without the cap the
            // ICUSD Token::sub would underflow-PANIC and the raw u64 collateral
            // sub would WRAP, both after the liquidator's icUSD was already pulled.
            debt_applied = max_liquidatable_debt.min(vault.borrowed_icusd_amount);
            collateral_applied = total_to_seize.to_u64().min(vault.collateral_amount);
            let interest_applied = interest_share.min(vault.accrued_interest);
            vault.borrowed_icusd_amount = vault.borrowed_icusd_amount.saturating_sub(debt_applied);
            vault.collateral_amount = vault.collateral_amount.saturating_sub(collateral_applied);
            vault.accrued_interest = vault.accrued_interest.saturating_sub(interest_applied);
        }
        let payout_to_liquidator = ICP::from(collateral_applied.saturating_sub(protocol_cut));

        // Wave-10 LIQ-008: append the gross debt cleared to the rolling-
        // window log. SP writedowns count toward the breaker — a flood of
        // SP-absorbed liquidations is still a stress signal worth pausing
        // bot/SP auto-publishing on.
        crate::event::record_liquidation_for_breaker(s, max_liquidatable_debt.to_u64());

        // Wave-8e LIQ-005: per-call deficit accrual on the SP writedown
        // path, against the APPLIED amounts. Even though icUSD was burned
        // externally (legacy 3pool burn) or 3USD reserves were credited
        // (reserves path), the protocol's solvency invariant is still:
        // seized collateral USD value vs. debt cleared. If the SP absorbed
        // an underwater vault, the protocol records the shortfall here so
        // future fee revenue burns it down — this is what the audit
        // (LIQ-005) prescribes instead of socializing onto SP depositors.
        let seized_usd = crate::numeric::collateral_usd_value(
            collateral_applied,
            collateral_price,
            config_decimals,
        );
        let shortfall = if seized_usd < debt_applied {
            debt_applied - seized_usd
        } else {
            ICUSD::new(0)
        };
        if shortfall.0 > 0 {
            crate::event::record_deficit_accrued(
                s,
                crate::event::DeficitSource::Liquidation { vault_id },
                shortfall,
                ic_cdk::api::time(),
            );
            if s.check_deficit_readonly_latch() {
                log!(INFO,
                    "[LIQ-005] deficit threshold {} crossed by SP writedown vault #{} shortfall {}; auto-latched ReadOnly",
                    s.deficit_readonly_threshold_e8s, vault_id, shortfall.to_u64()
                );
            }
        }

        // AR-B-001/BK-001 (audit 2026-06-09): applied payout, replay-exact.
        let event = crate::event::Event::PartialLiquidateVault {
            vault_id,
            liquidator_payment: max_liquidatable_debt,
            icp_to_liquidator: payout_to_liquidator,
            liquidator: Some(caller),
            icp_rate: Some(collateral_price_usd),
            protocol_fee_collateral: if protocol_cut > 0 {
                Some(protocol_cut.min(collateral_applied))
            } else {
                None
            },
            timestamp: Some(ic_cdk::api::time()),
            three_usd_reserves_e8s: three_usd_received_e8s,
        };
        crate::storage::record_event(&event);

        // Track 3USD reserves at runtime (also persisted via event replay)
        if let Some(three_usd_e8s) = three_usd_received_e8s {
            s.protocol_3usd_reserves += three_usd_e8s;
        }

        let nonce = s.next_op_nonce();
        queue_collateral_payout(
            s,
            vault_id,
            vault.owner,
            caller,
            payout_to_liquidator,
            vault.collateral_type,
            nonce,
            ic_cdk::api::time(),
        );

        // Shared drain rule (see state::cleanup_if_drained): remove the vault
        // if this liquidation emptied it, else re-key its CR index entry.
        // The band gate that originally consumed the CR index was deactivated
        // 2026-05-18, but `check_vaults`' at-risk-band sharding (Wave-9c
        // DOS-005) still relies on accurate CR keys.
        if s.cleanup_if_drained(vault_id) {
            log!(
                INFO,
                "[liquidate_vault_debt_burned] Vault #{} fully liquidated — removed",
                vault_id
            );
        }

        interest_share
    });

    // Route interest share via N-way split
    // IC-B-002 (audit 2026-06-09): re-queue any unminted interest share so the
    // next flush retries it instead of silently dropping treasury revenue.
    let unminted_interest =
        crate::treasury::distribute_interest(interest_share, vault.collateral_type).await;
    if unminted_interest.to_u64() > 0 {
        mutate_state(|s| {
            s.restore_pending_interest_for_pool(vault.collateral_type, unminted_interest.to_u64())
        });
    }

    // Send protocol's liquidation fee cut to treasury
    if protocol_cut > 0 {
        if vault.collateral_type == crate::state::xrp_collateral_principal() {
            // P5: native-XRP protocol fee -> a developer-settleable XrpClaim (the
            // ICRC treasury transfer cannot target the synthetic XRP ledger). Keyed
            // by collateral_type (not a vault lookup, since the vault may already be
            // drained/removed by cleanup_if_drained above).
            let dev = read_state(|s| s.developer_principal);
            let now_ns = ic_cdk::api::time();
            mutate_state(|s| {
                record_xrp_claim(
                    s,
                    dev,
                    vault.owner,
                    vault.vault_id,
                    protocol_cut.to_u64().unwrap_or(0),
                    now_ns,
                );
            });
        } else {
            let asset_type = crate::treasury::collateral_to_asset_type(&vault.collateral_type);
            crate::treasury::send_liquidation_fee_to_treasury(
                protocol_cut,
                vault.collateral_type,
                asset_type,
            )
            .await;
        }
    }

    // Step 4: Process collateral transfer to stability pool
    match try_process_pending_transfers_immediate(vault_id).await {
        Ok(processed_count) => {
            log!(
                INFO,
                "[liquidate_vault_debt_burned] Processed {} transfers immediately",
                processed_count
            );
        }
        Err(e) => {
            log!(
                INFO,
                "[liquidate_vault_debt_burned] Immediate processing failed: {}. Retrying via timer",
                e
            );
            schedule_transfer_retry(vault_id, 0);
        }
    }

    ic_cdk_timers::set_timer(std::time::Duration::from_secs(2), move || {
        ic_cdk::spawn(async move {
            log!(
                INFO,
                "[liquidate_vault_debt_burned] Backup timer for vault #{}",
                vault_id
            );
            let _ = crate::process_pending_transfer().await;
        })
    });

    guard_principal.complete();

    let liquidator_value_received = crate::numeric::collateral_usd_value(
        collateral_to_liquidator.to_u64(),
        collateral_price,
        config_decimals,
    );
    let fee_amount = if liquidator_value_received > max_liquidatable_debt {
        liquidator_value_received - max_liquidatable_debt
    } else {
        ICUSD::new(0)
    };

    log!(
        INFO,
        "[liquidate_vault_debt_burned] Completed. Fee: {}, Collateral: {}",
        fee_amount.to_u64(),
        collateral_to_liquidator.to_u64()
    );

    Ok(StabilityPoolLiquidationResult {
        success: true,
        vault_id,
        liquidated_debt: max_liquidatable_debt.to_u64(),
        collateral_received: collateral_to_liquidator.to_u64(),
        collateral_type: vault.collateral_type.to_string(),
        block_index: 0, // No ledger block — icUSD was burned in 3pool
        fee: fee_amount.to_u64(),
        collateral_price_e8s: collateral_price_usd.to_e8s(),
    })
}

pub async fn liquidate_vault(vault_id: u64) -> Result<SuccessWithFee, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal = GuardPrincipal::new(caller, &format!("liquidate_vault_{}", vault_id))?;
    reject_if_bot_processing(vault_id)?; // LIQ-101: don't double-seize a bot-claimed vault
    let _vault_liq_guard = VaultLiquidationGuard::new(vault_id)?; // BK-001/002 per-vault lock
    if let Err(e) = reject_active_xrp_sp_absorb_preflight(vault_id, ic_cdk::api::time()) {
        guard_principal.fail();
        return Err(e);
    }

    // Wave-8b LIQ-002 band gate deactivated 2026-05-18 (see
    // `liquidate_vault_partial` above for rationale).

    // Step 1: Validate vault is liquidatable
    let (vault, collateral_price, config_decimals, collateral_price_usd, mode) =
        match read_state(|s| {
            match s.vault_id_to_vaults.get(&vault_id) {
                Some(vault) => {
                    // Check collateral status allows liquidation
                    if let Some(status) = s.get_collateral_status(&vault.collateral_type) {
                        if !status.allows_liquidation() {
                            return Err(
                                "Liquidation is not allowed for this collateral type.".to_string()
                            );
                        }
                    }

                    let price = s
                        .get_collateral_price_decimal(&vault.collateral_type)
                        .ok_or_else(|| {
                            "No price available for collateral. Price feed may be down.".to_string()
                        })?;
                    let decimals = s
                        .get_collateral_config(&vault.collateral_type)
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
                }
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
    let (
        debt_amount,
        collateral_to_liquidator,
        total_to_seize,
        protocol_cut,
        excess_collateral,
        is_recovery_partial,
    ) = read_state(|s| {
        let liq_bonus = s.get_liquidation_bonus_for(&vault.collateral_type);
        let protocol_share = s.get_liquidation_protocol_share();
        if let Some(repay_cap) = s.compute_recovery_repay_cap(&vault, collateral_price_usd) {
            // Recovery mode: only liquidate enough to restore CR to target
            let collateral_raw = crate::numeric::icusd_to_collateral_amount(
                repay_cap,
                collateral_price,
                config_decimals,
            );
            let total_to_seize = (ICP::from(collateral_raw) * liq_bonus).min(vault_collateral);
            // Split: protocol gets a share of the bonus portion (liquidator's profit)
            let bonus_portion = total_to_seize.to_u64().saturating_sub(collateral_raw);
            let protocol_cut = (rust_decimal::Decimal::from(bonus_portion) * protocol_share.0)
                .to_u64()
                .unwrap_or(0);
            let collateral_to_liquidator = ICP::from(total_to_seize.to_u64() - protocol_cut);
            (
                repay_cap,
                collateral_to_liquidator,
                total_to_seize,
                protocol_cut,
                ICP::new(0),
                true,
            )
        } else {
            // Normal full liquidation
            let debt = vault.borrowed_icusd_amount;
            let collateral_raw =
                crate::numeric::icusd_to_collateral_amount(debt, collateral_price, config_decimals);
            let icp_with_bonus = ICP::from(collateral_raw) * liq_bonus;
            let total_to_seize = icp_with_bonus.min(vault_collateral);
            // Split: protocol gets a share of the bonus portion (liquidator's profit)
            let bonus_portion = total_to_seize.to_u64().saturating_sub(collateral_raw);
            let protocol_cut = (rust_decimal::Decimal::from(bonus_portion) * protocol_share.0)
                .to_u64()
                .unwrap_or(0);
            let collateral_to_liquidator = ICP::from(total_to_seize.to_u64() - protocol_cut);
            let excess = vault_collateral.saturating_sub(total_to_seize);
            (
                debt,
                collateral_to_liquidator,
                total_to_seize,
                protocol_cut,
                excess,
                false,
            )
        }
    });

    log!(INFO,
        "[liquidate_vault] Vault #{}: debt_to_repay={} icUSD, liquidator gets {} ICP (protocol fee: {} ICP), excess={} ICP, recovery_partial={}",
        vault_id,
        debt_amount.to_u64(),
        collateral_to_liquidator.to_u64(),
        protocol_cut,
        excess_collateral.to_u64(),
        is_recovery_partial
    );

    // Step 3: Take icUSD from liquidator (this must succeed for liquidation to proceed)
    let icusd_block_index = match transfer_icusd_from(debt_amount, caller).await {
        Ok(block_index) => {
            log!(
                INFO,
                "[liquidate_vault] Received {} icUSD from liquidator",
                debt_amount.to_u64()
            );
            block_index
        }
        Err(transfer_from_error) => {
            guard_principal.fail();
            return Err(ProtocolError::TransferFromError(
                transfer_from_error,
                debt_amount.to_u64(),
            ));
        }
    };

    // Step 4: Update protocol state ATOMICALLY (this is the critical section).
    // ASYNC-002: a concurrent full liquidation may have removed this vault between
    // our pre-await read and the icUSD pull above. Detect it BEFORE any
    // irreversible state work and refund the liquidator (None branch below)
    // instead of trapping inside s.liquidate_vault()'s vault lookup.
    let (interest_share, xrp_claim_id) = match mutate_state(|s| {
        if !s.vault_id_to_vaults.contains_key(&vault_id) {
            return None;
        }
        // AR-B-001/BK-001 (audit 2026-06-09): re-cap every collateral payout
        // to the vault's LIVE collateral at commit time. The split below was
        // computed from the pre-await snapshot; the per-vault op lock makes a
        // concurrent reduction unreachable, and this clamp keeps any residual
        // drift solvency-safe (protocol cut first, then liquidator, then the
        // owner's excess — never more than the vault actually holds).
        let live_collateral = s
            .vault_id_to_vaults
            .get(&vault_id)
            .map(|v| v.collateral_amount)
            .unwrap_or(0);
        let cut_applied = protocol_cut.min(live_collateral);
        let liquidator_pay = ICP::from(
            collateral_to_liquidator
                .to_u64()
                .min(live_collateral.saturating_sub(cut_applied)),
        );
        let excess_pay = ICP::from(
            excess_collateral.to_u64().min(
                live_collateral
                    .saturating_sub(cut_applied)
                    .saturating_sub(liquidator_pay.to_u64()),
            ),
        );

        // Execute the liquidation in state first (this must happen)
        // liquidate_vault returns the interest share of the debt reduction
        let interest_share = s.liquidate_vault(vault_id, mode, collateral_price_usd);

        // Wave-10 LIQ-008: append the gross debt cleared to the rolling-
        // window log for the mass-liquidation circuit breaker.
        crate::event::record_liquidation_for_breaker(s, debt_amount.to_u64());

        // Wave-8e LIQ-005: if seized USD < debt cleared, the protocol
        // absorbed bad debt. Track the shortfall in `protocol_deficit_icusd`
        // and check the ReadOnly latch. The liquidator's icUSD payment was
        // already burned via `transfer_icusd_from` (the protocol IS the
        // icUSD minting account), so the supply side is consistent — the
        // liquidator effectively paid `debt_amount` icUSD for collateral
        // worth less, and the protocol now records the outstanding loss.
        let seized_usd = crate::numeric::collateral_usd_value(
            total_to_seize.to_u64().min(live_collateral),
            collateral_price,
            config_decimals,
        );
        let shortfall = if seized_usd < debt_amount {
            debt_amount - seized_usd
        } else {
            ICUSD::new(0)
        };
        if shortfall.0 > 0 {
            crate::event::record_deficit_accrued(
                s,
                crate::event::DeficitSource::Liquidation { vault_id },
                shortfall,
                ic_cdk::api::time(),
            );
            if s.check_deficit_readonly_latch() {
                log!(INFO,
                    "[LIQ-005] deficit threshold {} crossed by vault #{} shortfall {}; auto-latched ReadOnly",
                    s.deficit_readonly_threshold_e8s, vault_id, shortfall.to_u64()
                );
            }
        }

        // Record the liquidation event
        let event = crate::event::Event::LiquidateVault {
            vault_id,
            mode,
            icp_rate: collateral_price_usd,
            liquidator: Some(caller),
            timestamp: Some(ic_cdk::api::time()),
        };
        crate::storage::record_event(&event);

        // Create pending transfer for liquidator reward (minus protocol cut)
        let liquidator_nonce = s.next_op_nonce();
        let xrp_claim_id = queue_collateral_payout(
            s,
            vault_id,
            vault.owner,
            caller,
            liquidator_pay,
            vault.collateral_type,
            liquidator_nonce,
            ic_cdk::api::time(),
        );

        // Create pending transfer for excess collateral to vault owner (if any)
        // (only for full liquidations, not recovery partial)
        if !is_recovery_partial && excess_pay > ICP::new(0) {
            log!(
                INFO,
                "[liquidate_vault] Scheduling excess collateral return to vault owner"
            );
            // Native-XRP excess returns to the owner as an XrpClaim; ICRC excess
            // goes through the pending-excess transfer machinery.
            if s.get_collateral_config(&vault.collateral_type)
                .map(|c| c.is_native_xrp())
                .unwrap_or(false)
            {
                record_xrp_claim(
                    s,
                    vault.owner,
                    vault.owner,
                    vault_id,
                    excess_pay.to_u64(),
                    ic_cdk::api::time(),
                );
            } else {
                let excess_nonce = s.next_op_nonce();
                s.pending_excess_transfers.insert(
                    (vault_id, vault.owner),
                    PendingMarginTransfer {
                        owner: vault.owner,
                        margin: excess_pay,
                        collateral_type: vault.collateral_type,
                        retry_count: 0,
                        op_nonce: excess_nonce,
                    },
                );
            }
        }

        log!(
            INFO,
            "[liquidate_vault] Protocol state updated, {} pending transfers created",
            if !is_recovery_partial && excess_pay > ICP::new(0) {
                2
            } else {
                1
            }
        );
        Some((interest_share, xrp_claim_id))
    }) {
        Some(result) => result,
        None => {
            // ASYNC-002: the vault was liquidated by a concurrent op while our
            // icUSD pull was in flight. Refund the liquidator (mirrors the
            // redeem_reserves durable-refund saga) and return a clean error
            // instead of trapping with the liquidator's icUSD stuck.
            guard_principal.fail();
            log!(INFO,
                "[liquidate_vault] Vault #{} already liquidated by a concurrent op; refunding {} icUSD to {}",
                vault_id, debt_amount.to_u64(), caller);
            let refund_nonce = mutate_state(|s| s.next_op_nonce());
            match management::transfer_icusd_with_nonce(debt_amount, caller, refund_nonce).await {
                Ok(refund_block) => {
                    log!(
                        INFO,
                        "[liquidate_vault] Refunded {} icUSD to {} (block {})",
                        debt_amount.to_u64(),
                        caller,
                        refund_block
                    );
                }
                Err(refund_err) => {
                    log!(INFO,
                        "[liquidate_vault] Vault gone AND inline icUSD refund failed for {}: {:?}. \
                         Enqueueing durable refund (block {}).",
                        caller, refund_err, icusd_block_index);
                    mutate_state(|s| {
                        s.pending_refunds.insert(
                            icusd_block_index,
                            crate::state::PendingRefund {
                                user: caller,
                                amount_e8s: debt_amount.to_u64(),
                                retry_count: 0,
                                op_nonce: refund_nonce,
                            },
                        );
                    });
                    ic_cdk_timers::set_timer(std::time::Duration::from_secs(2), || {
                        ic_cdk::spawn(crate::process_pending_transfer())
                    });
                }
            }
            return Err(ProtocolError::GenericError(format!(
                "Vault #{} was already liquidated by a concurrent operation; your {} icUSD has been refunded",
                vault_id, debt_amount.to_u64()
            )));
        }
    };

    // Route interest share via N-way split
    // IC-B-002 (audit 2026-06-09): re-queue any unminted interest share so the
    // next flush retries it instead of silently dropping treasury revenue.
    let unminted_interest =
        crate::treasury::distribute_interest(interest_share, vault.collateral_type).await;
    if unminted_interest.to_u64() > 0 {
        mutate_state(|s| {
            s.restore_pending_interest_for_pool(vault.collateral_type, unminted_interest.to_u64())
        });
    }

    // Send protocol's liquidation fee cut to treasury (fire-and-forget)
    if protocol_cut > 0 {
        if vault.collateral_type == crate::state::xrp_collateral_principal() {
            // P5: native-XRP protocol fee -> a developer-settleable XrpClaim (the
            // ICRC treasury transfer cannot target the synthetic XRP ledger). Keyed
            // by collateral_type (not a vault lookup, since the vault may already be
            // drained/removed by cleanup_if_drained above).
            let dev = read_state(|s| s.developer_principal);
            let now_ns = ic_cdk::api::time();
            mutate_state(|s| {
                record_xrp_claim(
                    s,
                    dev,
                    vault.owner,
                    vault.vault_id,
                    protocol_cut.to_u64().unwrap_or(0),
                    now_ns,
                );
            });
        } else {
            let asset_type = crate::treasury::collateral_to_asset_type(&vault.collateral_type);
            crate::treasury::send_liquidation_fee_to_treasury(
                protocol_cut,
                vault.collateral_type,
                asset_type,
            )
            .await;
        }
    }

    // Step 5: Attempt immediate transfer processing (best effort)
    log!(
        INFO,
        "[liquidate_vault] Attempting immediate transfer processing..."
    );

    // Try to process transfers immediately
    match try_process_pending_transfers_immediate(vault_id).await {
        Ok(processed_count) => {
            log!(
                INFO,
                "[liquidate_vault] Successfully processed {} transfers immediately",
                processed_count
            );
        }
        Err(e) => {
            log!(INFO, "[liquidate_vault] Immediate processing failed: {}. Transfers will be retried via timer", e);

            // Schedule retry with exponential backoff
            schedule_transfer_retry(vault_id, 0);
        }
    }

    // Step 6: Always schedule a backup timer (in case immediate processing failed)
    ic_cdk_timers::set_timer(std::time::Duration::from_secs(2), move || {
        ic_cdk::spawn(async move {
            log!(
                INFO,
                "[liquidate_vault] Backup timer processing transfers for vault #{}",
                vault_id
            );
            let _ = crate::process_pending_transfer().await;
        })
    });

    // Step 7: Liquidation is successful (protocol state is consistent)
    guard_principal.complete();

    // Calculate fee
    let liquidator_value_received = crate::numeric::collateral_usd_value(
        collateral_to_liquidator.to_u64(),
        collateral_price,
        config_decimals,
    );
    let fee_amount = if liquidator_value_received > debt_amount {
        liquidator_value_received - debt_amount
    } else {
        ICUSD::new(0)
    };

    log!(INFO, "[liquidate_vault] Liquidation completed successfully. Block index: {}, Fee: {}, Collateral: {}",
         icusd_block_index, fee_amount.to_u64(), collateral_to_liquidator.to_u64());

    Ok(SuccessWithFee {
        block_index: icusd_block_index,
        fee_amount_paid: fee_amount.to_u64(),
        collateral_amount_received: Some(collateral_to_liquidator.to_u64()),
        debt_liquidated_e8s: None, // SP-101
        stable_pulled_e6s: None,   // SP-110
        xrp_claim_id,
    })
}

// Helper function to attempt immediate transfer processing
async fn try_process_pending_transfers_immediate(vault_id: u64) -> Result<u32, String> {
    let mut processed_count = 0;

    // Wave-4 LIQ-001: collect every pending margin/excess entry whose key matches
    // this vault_id. Concurrent liquidators on the same vault each have their own
    // (vault_id, owner) key, so we iterate rather than do a single point lookup.
    let transfers_to_process = read_state(|s| {
        let mut transfers = Vec::new();

        for ((vid, owner), transfer) in s.pending_margin_transfers.iter() {
            if *vid == vault_id {
                transfers.push(("margin", *vid, *owner, transfer.clone()));
            }
        }

        for ((vid, owner), transfer) in s.pending_excess_transfers.iter() {
            if *vid == vault_id {
                transfers.push(("excess", *vid, *owner, transfer.clone()));
            }
        }

        transfers
    });

    // Process each transfer
    for (transfer_type, transfer_vault_id, transfer_owner, transfer) in transfers_to_process {
        // Note: native-XRP collateral never reaches this ICRC processor — it is
        // converted to an XrpClaim at the moment of liquidation/withdrawal (see
        // `queue_collateral_payout`), so a NativeXrp `collateral_type` cannot appear
        // in pending_margin_transfers / pending_excess_transfers.
        // Look up per-collateral ledger fee and canister ID
        let (ledger_fee, ledger_canister_id) =
            read_state(
                |s| match s.get_collateral_config(&transfer.collateral_type) {
                    Some(config) => (ICP::from(config.ledger_fee), config.ledger_canister_id),
                    None => (s.icp_ledger_fee, s.icp_ledger_principal),
                },
            );

        if transfer.margin <= ledger_fee {
            log!(INFO, "[immediate_transfer] Skipping {} transfer {} owner {} - margin {} <= fee {}, removing",
                transfer_type, transfer_vault_id, transfer_owner, transfer.margin.to_u64(), ledger_fee.to_u64());
            mutate_state(|s| {
                let key = (transfer_vault_id, transfer_owner);
                match transfer_type {
                    "margin" => {
                        s.pending_margin_transfers.remove(&key);
                    }
                    "excess" => {
                        s.pending_excess_transfers.remove(&key);
                    }
                    _ => {}
                }
            });
            processed_count += 1;
            continue;
        }
        let transfer_amount = transfer.margin - ledger_fee;

        log!(
            INFO,
            "[immediate_transfer] Processing {} transfer {} of {} collateral to {}",
            transfer_type,
            transfer_vault_id,
            transfer_amount.to_u64(),
            transfer.owner
        );

        match management::transfer_collateral_with_nonce(
            transfer_amount.to_u64(),
            transfer.owner,
            ledger_canister_id,
            transfer.op_nonce,
        )
        .await
        {
            Ok(block_index) => {
                log!(
                    INFO,
                    "[immediate_transfer] Transfer {} owner {} successful, block: {}",
                    transfer_vault_id,
                    transfer_owner,
                    block_index
                );

                // Remove from the appropriate pending map
                mutate_state(|s| {
                    let key = (transfer_vault_id, transfer_owner);
                    match transfer_type {
                        "margin" => {
                            s.pending_margin_transfers.remove(&key);
                        }
                        "excess" => {
                            s.pending_excess_transfers.remove(&key);
                        }
                        _ => {}
                    }
                });

                processed_count += 1;
            }
            Err(error) => {
                log!(
                    INFO,
                    "[immediate_transfer] Transfer {} owner {} failed: {}. Will retry later",
                    transfer_vault_id,
                    transfer_owner,
                    error
                );
                // Leave in pending transfers for retry
                return Err(format!("Transfer {} failed: {}", transfer_vault_id, error));
            }
        }
    }

    Ok(processed_count)
}

// Helper function to schedule transfer retries with exponential backoff
fn schedule_transfer_retry(vault_id: u64, retry_count: u32) {
    let max_retries = 5;
    if retry_count >= max_retries {
        log!(
            INFO,
            "[retry_scheduler] Max retries reached for vault #{}",
            vault_id
        );
        return;
    }

    // Exponential backoff: 1s, 2s, 4s, 8s, 16s
    let delay_seconds = 1u64 << retry_count;

    log!(
        INFO,
        "[retry_scheduler] Scheduling retry #{} for vault #{} in {}s",
        retry_count + 1,
        vault_id,
        delay_seconds
    );

    ic_cdk_timers::set_timer(std::time::Duration::from_secs(delay_seconds), move || {
        ic_cdk::spawn(async move {
            log!(
                INFO,
                "[retry_scheduler] Retry #{} executing for vault #{}",
                retry_count + 1,
                vault_id
            );

            match try_process_pending_transfers_immediate(vault_id).await {
                Ok(processed) => {
                    log!(
                        INFO,
                        "[retry_scheduler] Retry #{} successful, processed {} transfers",
                        retry_count + 1,
                        processed
                    );
                }
                Err(_) => {
                    log!(
                        INFO,
                        "[retry_scheduler] Retry #{} failed, scheduling next retry",
                        retry_count + 1
                    );
                    schedule_transfer_retry(vault_id, retry_count + 1);
                }
            }
        })
    });
}

pub async fn partial_repay_to_vault(arg: VaultArg) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal =
        GuardPrincipal::new(caller, &format!("partial_repay_vault_{}", arg.vault_id))?;
    // AR-B-003: per-vault op lock; see guard.rs::VaultLiquidationGuard. This
    // endpoint pulls icUSD across an await then commits via repay_to_vault, so
    // it needs the same serialization vs liquidation/redemption as its siblings.
    let _vault_op_guard = match VaultLiquidationGuard::new(arg.vault_id) {
        Ok(g) => g,
        Err(e) => {
            guard_principal.fail();
            return Err(e);
        }
    };
    let amount: ICUSD = arg.amount.into();

    // Accrue interest before repayment so the correct debt balance is used.
    let now = ic_cdk::api::time();
    if let Err(e) = reject_active_xrp_sp_absorb_preflight(arg.vault_id, now) {
        guard_principal.fail();
        return Err(e);
    }
    mutate_state(|s| s.accrue_single_vault(arg.vault_id, now));

    let vault = match read_state(|s| s.vault_id_to_vaults.get(&arg.vault_id).cloned()) {
        Some(v) => v,
        None => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(format!(
                "Vault #{} not found",
                arg.vault_id
            )));
        }
    };

    // Check collateral status allows repayment
    let collateral_status = read_state(|s| s.get_collateral_status(&vault.collateral_type));
    if let Some(status) = collateral_status {
        if !status.allows_repay() {
            guard_principal.fail();
            return Err(ProtocolError::TemporarilyUnavailable(format!(
                "Collateral is {:?}, repayment not allowed",
                status
            )));
        }
    }

    if caller != vault.owner {
        guard_principal.fail();
        return Err(ProtocolError::CallerNotOwner);
    }

    if amount < read_state(|s| s.min_icusd_amount) {
        guard_principal.fail();
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: read_state(|s| s.min_icusd_amount).to_u64(),
        });
    }

    // Cap repay amount to actual debt. Interest accrued between when the
    // frontend read the balance and now can push debt slightly above what
    // the user entered. If the requested amount exceeds or nearly matches
    // the current debt (within 1% or 0.01 icUSD — whichever is larger),
    // treat it as a full repayment to avoid leaving un-repayable dust.
    let debt = vault.borrowed_icusd_amount;
    let dust_threshold = std::cmp::max(debt.0 / 100, 1_000_000); // 1% or 0.01 icUSD
    let amount = if amount > debt {
        debt
    } else if debt.0.saturating_sub(amount.0) <= dust_threshold {
        debt
    } else {
        amount
    };

    if let Err(e) = check_min_vault_debt_after_repay(&vault, amount) {
        guard_principal.fail();
        return Err(e);
    }

    match transfer_icusd_from(amount, caller).await {
        Ok(block_index) => {
            let interest_share =
                mutate_state(|s| record_repayed_to_vault(s, arg.vault_id, amount, block_index));
            // IC-B-002 (audit 2026-06-09): re-queue any unminted interest share so the
            // next flush retries it instead of silently dropping treasury revenue.
            let unminted_interest =
                crate::treasury::distribute_interest(interest_share, vault.collateral_type).await;
            if unminted_interest.to_u64() > 0 {
                mutate_state(|s| {
                    s.restore_pending_interest_for_pool(
                        vault.collateral_type,
                        unminted_interest.to_u64(),
                    )
                });
            }
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
    let guard_principal =
        GuardPrincipal::new(caller, &format!("partial_liquidate_vault_{}", arg.vault_id))?;
    reject_if_bot_processing(arg.vault_id)?; // LIQ-101: don't double-seize a bot-claimed vault
    let _vault_liq_guard = VaultLiquidationGuard::new(arg.vault_id)?; // BK-001/002 per-vault lock
    if let Err(e) = reject_active_xrp_sp_absorb_preflight(arg.vault_id, ic_cdk::api::time()) {
        guard_principal.fail();
        return Err(e);
    }

    // Wave-8b LIQ-002 band gate deactivated 2026-05-18 (see
    // `liquidate_vault_partial` above for rationale).

    let liquidator_payment: ICUSD = arg.amount.into();

    // Accrue interest before liquidation so CR check uses up-to-date debt.
    let now = ic_cdk::api::time();
    mutate_state(|s| s.accrue_single_vault(arg.vault_id, now));

    // Step 1: Validate vault is liquidatable
    let (vault, collateral_price, config_decimals, collateral_price_usd, _mode) =
        match read_state(|s| {
            match s.vault_id_to_vaults.get(&arg.vault_id) {
                Some(vault) => {
                    // Check collateral status allows liquidation
                    if let Some(status) = s.get_collateral_status(&vault.collateral_type) {
                        if !status.allows_liquidation() {
                            return Err(
                                "Liquidation is not allowed for this collateral type.".to_string()
                            );
                        }
                    }

                    let price = s
                        .get_collateral_price_decimal(&vault.collateral_type)
                        .ok_or_else(|| {
                            "No price available for collateral. Price feed may be down.".to_string()
                        })?;
                    let decimals = s
                        .get_collateral_config(&vault.collateral_type)
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
                }
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
    if liquidator_payment < read_state(|s| s.min_icusd_amount) {
        guard_principal.fail();
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: read_state(|s| s.min_icusd_amount).to_u64(),
        });
    }

    // Cap payment to recovery_target_cr, then round residual up to full debt
    // if it would land in (0, min_vault_debt) (LIQ-003: mirrors the repay-side
    // invariant in `check_min_vault_debt_after_repay`).
    let liquidator_payment = read_state(|s| {
        let cap = s.compute_partial_liquidation_cap(&vault, collateral_price_usd);
        let capped = liquidator_payment.min(cap);
        let min_vault_debt = s
            .get_collateral_config(&vault.collateral_type)
            .map(|c| c.min_vault_debt)
            .unwrap_or(ICUSD::new(0));
        round_up_partial_liq_dust(&vault, capped, min_vault_debt)
    });

    if liquidator_payment > vault.borrowed_icusd_amount {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(format!(
            "cannot liquidate more than borrowed: {} ICUSD, liquidate: {} ICUSD",
            vault.borrowed_icusd_amount, liquidator_payment
        )));
    }

    // Step 3: Calculate liquidation amounts with liquidation bonus and protocol fee
    let (liq_bonus, protocol_share) = read_state(|s| {
        (
            s.get_liquidation_bonus_for(&vault.collateral_type),
            s.get_liquidation_protocol_share(),
        )
    });
    let collateral_raw = crate::numeric::icusd_to_collateral_amount(
        liquidator_payment,
        collateral_price,
        config_decimals,
    );
    let icp_with_bonus = ICP::from(collateral_raw) * liq_bonus;
    let total_to_seize = icp_with_bonus.min(ICP::from(vault.collateral_amount));

    // Split: protocol gets a share of the bonus portion (liquidator's profit)
    let bonus_portion = total_to_seize.to_u64().saturating_sub(collateral_raw);
    let protocol_cut = (rust_decimal::Decimal::from(bonus_portion) * protocol_share.0)
        .to_u64()
        .unwrap_or(0);
    let collateral_to_liquidator = ICP::from(total_to_seize.to_u64() - protocol_cut);

    log!(INFO,
        "[partial_liquidate_vault] Vault #{}: liquidator pays {} icUSD, gets {} ICP (protocol fee: {} ICP, bonus: {})",
        arg.vault_id,
        liquidator_payment.to_u64(),
        collateral_to_liquidator.to_u64(),
        protocol_cut,
        liq_bonus.to_f64()
    );

    // Step 4: Take icUSD from liquidator
    let icusd_block_index = match transfer_icusd_from(liquidator_payment, caller).await {
        Ok(block_index) => {
            log!(
                INFO,
                "[partial_liquidate_vault] Received {} icUSD from liquidator",
                liquidator_payment.to_u64()
            );
            block_index
        }
        Err(transfer_from_error) => {
            guard_principal.fail();
            return Err(ProtocolError::TransferFromError(
                transfer_from_error,
                liquidator_payment.to_u64(),
            ));
        }
    };

    // Step 5: Update protocol state ATOMICALLY
    let (interest_share, xrp_claim_id) = mutate_state(|s| {
        // Compute proportional interest share before reducing debt
        let interest_share = if let Some(vault) = s.vault_id_to_vaults.get(&arg.vault_id) {
            if vault.accrued_interest.0 > 0 && vault.borrowed_icusd_amount.0 > 0 {
                let share = (rust_decimal::Decimal::from(liquidator_payment.0)
                    * rust_decimal::Decimal::from(vault.accrued_interest.0)
                    / rust_decimal::Decimal::from(vault.borrowed_icusd_amount.0))
                .to_u64()
                .unwrap_or(0);
                ICUSD::new(share.min(vault.accrued_interest.0))
            } else {
                ICUSD::new(0)
            }
        } else {
            ICUSD::new(0)
        };

        // Reduce the vault's debt by the liquidator payment amount
        // Vault loses total_to_seize (liquidator + protocol cut)
        //
        // AR-B-001/BK-001 (audit 2026-06-09): capture applied amounts and
        // re-cap the payout, mirroring `liquidate_vault_partial`.
        let mut debt_applied = liquidator_payment;
        let mut collateral_applied = total_to_seize.to_u64();
        if let Some(vault) = s.vault_id_to_vaults.get_mut(&arg.vault_id) {
            // ASYNC-001: cap each reduction to the CURRENT vault state and
            // saturating_sub (same race as the other partial-liq paths).
            debt_applied = liquidator_payment.min(vault.borrowed_icusd_amount);
            collateral_applied = total_to_seize.to_u64().min(vault.collateral_amount);
            let interest_applied = interest_share.min(vault.accrued_interest);
            vault.borrowed_icusd_amount = vault.borrowed_icusd_amount.saturating_sub(debt_applied);
            vault.collateral_amount = vault.collateral_amount.saturating_sub(collateral_applied);
            vault.accrued_interest = vault.accrued_interest.saturating_sub(interest_applied);
        }
        let payout_to_liquidator = ICP::from(collateral_applied.saturating_sub(protocol_cut));

        // Wave-10 LIQ-008: append the gross debt cleared to the rolling-
        // window log for the mass-liquidation circuit breaker.
        crate::event::record_liquidation_for_breaker(s, liquidator_payment.to_u64());

        // Wave-8e LIQ-005: per-call deficit accrual against the APPLIED amounts.
        let seized_usd = crate::numeric::collateral_usd_value(
            collateral_applied,
            collateral_price,
            config_decimals,
        );
        let shortfall = if seized_usd < debt_applied {
            debt_applied - seized_usd
        } else {
            ICUSD::new(0)
        };
        if shortfall.0 > 0 {
            crate::event::record_deficit_accrued(
                s,
                crate::event::DeficitSource::Liquidation {
                    vault_id: arg.vault_id,
                },
                shortfall,
                ic_cdk::api::time(),
            );
            if s.check_deficit_readonly_latch() {
                log!(INFO,
                    "[LIQ-005] deficit threshold {} crossed by partial_liquidate_vault #{} shortfall {}; auto-latched ReadOnly",
                    s.deficit_readonly_threshold_e8s, arg.vault_id, shortfall.to_u64()
                );
            }
        }

        // Record the partial liquidation event (applied payout, replay-exact)
        let event = crate::event::Event::PartialLiquidateVault {
            vault_id: arg.vault_id,
            liquidator_payment,
            icp_to_liquidator: payout_to_liquidator,
            liquidator: Some(caller),
            icp_rate: Some(collateral_price_usd),
            protocol_fee_collateral: if protocol_cut > 0 {
                Some(protocol_cut.min(collateral_applied))
            } else {
                None
            },
            timestamp: Some(ic_cdk::api::time()),
            three_usd_reserves_e8s: None,
        };
        crate::storage::record_event(&event);

        // Create pending transfer for liquidator reward (minus protocol cut)
        let nonce = s.next_op_nonce();
        let xrp_claim_id = queue_collateral_payout(
            s,
            arg.vault_id,
            vault.owner,
            caller,
            payout_to_liquidator,
            vault.collateral_type,
            nonce,
            ic_cdk::api::time(),
        );

        // Shared drain rule (see state::cleanup_if_drained): remove the vault
        // if this liquidation emptied it, else re-key its CR index entry.
        if s.cleanup_if_drained(arg.vault_id) {
            log!(
                INFO,
                "[partial_liquidate_vault] Vault #{} fully liquidated — removed",
                arg.vault_id
            );
        }

        log!(
            INFO,
            "[partial_liquidate_vault] Protocol state updated, pending transfer created"
        );
        (interest_share, xrp_claim_id)
    });

    // Route interest share via N-way split
    // IC-B-002 (audit 2026-06-09): re-queue any unminted interest share so the
    // next flush retries it instead of silently dropping treasury revenue.
    let unminted_interest =
        crate::treasury::distribute_interest(interest_share, vault.collateral_type).await;
    if unminted_interest.to_u64() > 0 {
        mutate_state(|s| {
            s.restore_pending_interest_for_pool(vault.collateral_type, unminted_interest.to_u64())
        });
    }

    // Send protocol's liquidation fee cut to treasury (fire-and-forget)
    if protocol_cut > 0 {
        if vault.collateral_type == crate::state::xrp_collateral_principal() {
            // P5: native-XRP protocol fee -> a developer-settleable XrpClaim (the
            // ICRC treasury transfer cannot target the synthetic XRP ledger). Keyed
            // by collateral_type (not a vault lookup, since the vault may already be
            // drained/removed by cleanup_if_drained above).
            let dev = read_state(|s| s.developer_principal);
            let now_ns = ic_cdk::api::time();
            mutate_state(|s| {
                record_xrp_claim(
                    s,
                    dev,
                    vault.owner,
                    vault.vault_id,
                    protocol_cut.to_u64().unwrap_or(0),
                    now_ns,
                );
            });
        } else {
            let asset_type = crate::treasury::collateral_to_asset_type(&vault.collateral_type);
            crate::treasury::send_liquidation_fee_to_treasury(
                protocol_cut,
                vault.collateral_type,
                asset_type,
            )
            .await;
        }
    }

    // Step 6: Attempt immediate transfer processing
    log!(
        INFO,
        "[partial_liquidate_vault] Attempting immediate transfer processing..."
    );

    match try_process_pending_transfers_immediate(arg.vault_id).await {
        Ok(processed_count) => {
            log!(
                INFO,
                "[partial_liquidate_vault] Successfully processed {} transfers immediately",
                processed_count
            );
        }
        Err(e) => {
            log!(INFO, "[partial_liquidate_vault] Immediate processing failed: {}. Transfers will be retried via timer", e);
            schedule_transfer_retry(arg.vault_id, 0);
        }
    }

    // Step 7: Schedule backup timer
    ic_cdk_timers::set_timer(std::time::Duration::from_secs(2), move || {
        ic_cdk::spawn(async move {
            log!(
                INFO,
                "[partial_liquidate_vault] Backup timer processing transfers for vault #{}",
                arg.vault_id
            );
            let _ = crate::process_pending_transfer().await;
        })
    });

    // Step 8: Liquidation is successful
    guard_principal.complete();

    // Calculate fee (the 10% discount is the fee)
    let liquidator_value_received = crate::numeric::collateral_usd_value(
        collateral_to_liquidator.to_u64(),
        collateral_price,
        config_decimals,
    );
    let fee_amount = if liquidator_value_received > liquidator_payment {
        liquidator_value_received - liquidator_payment
    } else {
        ICUSD::new(0)
    };

    log!(INFO, "[partial_liquidate_vault] Partial liquidation completed successfully. Block index: {}, Fee: {}, Collateral: {}",
         icusd_block_index, fee_amount.to_u64(), collateral_to_liquidator.to_u64());

    Ok(SuccessWithFee {
        block_index: icusd_block_index,
        fee_amount_paid: fee_amount.to_u64(),
        collateral_amount_received: Some(collateral_to_liquidator.to_u64()),
        debt_liquidated_e8s: None, // SP-101
        stable_pulled_e6s: None,   // SP-110
        xrp_claim_id,
    })
}

#[cfg(test)]
mod sp_writedown_native_xrp_guard_tests {
    use super::*;
    use crate::icrc3_proof::{SpProofLedger, SpWritedownProof};
    use crate::state::{replace_state, xrp_collateral_principal, CustodyKind, State};

    /// Install a thread-local state holding two vaults: an ICRC (ICP) vault at
    /// `icp_vault_id` and a native-XRP vault at `xrp_vault_id`. The native-XRP
    /// collateral config is the ICP config cloned with `custody_kind = NativeXrp`,
    /// mirroring the P4/P5 registration shape.
    fn install_two_collateral_state(xrp_vault_id: u64, icp_vault_id: u64) {
        let mut s = State::from(crate::InitArg {
            xrc_principal: Principal::anonymous(),
            icusd_ledger_principal: Principal::anonymous(),
            icp_ledger_principal: Principal::anonymous(),
            fee_e8s: 0,
            developer_principal: Principal::anonymous(),
            treasury_principal: None,
            stability_pool_principal: None,
            ckusdt_ledger_principal: None,
            ckusdc_ledger_principal: None,
        });
        // Deterministic: never trip the min-amount gate before reaching the guard.
        s.min_icusd_amount = ICUSD::new(0);

        let icp = s.icp_collateral_type();
        if let Some(c) = s.collateral_configs.get_mut(&icp) {
            c.last_price = Some(5.0);
        }
        s.open_vault(Vault {
            owner: Principal::anonymous(),
            vault_id: icp_vault_id,
            borrowed_icusd_amount: ICUSD::new(1_000_000_000),
            collateral_amount: 1_000_000_000,
            collateral_type: icp,
            accrued_interest: ICUSD::new(0),
            last_accrual_time: 0,
            bot_processing: false,
        });

        let xrp = xrp_collateral_principal();
        let mut xrp_cfg = s.collateral_configs.get(&icp).unwrap().clone();
        xrp_cfg.ledger_canister_id = xrp;
        xrp_cfg.custody_kind = Some(CustodyKind::NativeXrp);
        xrp_cfg.last_price = Some(0.5);
        s.collateral_configs.insert(xrp, xrp_cfg);
        s.open_vault(Vault {
            owner: Principal::anonymous(),
            vault_id: xrp_vault_id,
            borrowed_icusd_amount: ICUSD::new(1_000_000_000),
            collateral_amount: 5_000_000,
            collateral_type: xrp,
            accrued_interest: ICUSD::new(0),
            last_accrual_time: 0,
            bot_processing: false,
        });

        replace_state(s);
    }

    fn dummy_proof(vault_id_memo: u64) -> SpWritedownProof {
        SpWritedownProof {
            block_index: 0,
            ledger_kind: SpProofLedger::IcusdBurn,
            vault_id_memo,
        }
    }

    /// Defense-in-depth: the SP write-down core must reject a native-XRP vault
    /// before doing anything else, regardless of caller. The SP settles against
    /// the icUSD/3pool ledger but the seized XRP lives on XRPL as an XrpClaim the
    /// SP cannot settle, so a write-down here would strand the XRP and burn SP
    /// depositors. Native-XRP is liquidated only via the manual paths.
    #[test]
    fn sp_writedown_rejects_native_xrp_vault() {
        install_two_collateral_state(2, 1);
        let caller = Principal::from_slice(&[0xcc; 16]);
        let result = futures::executor::block_on(liquidate_vault_debt_already_burned(
            2,
            1_000_000_000,
            caller,
            None,
            dummy_proof(2),
        ));
        match result {
            Err(ProtocolError::GenericError(msg)) => assert!(
                msg.contains(
                    "Native-XRP collateral cannot be liquidated via the SP write-down path"
                ),
                "expected the native-XRP reject, got: {msg}"
            ),
            other => panic!("expected a native-XRP GenericError reject, got {other:?}"),
        }
    }

    /// The predicate is scoped to native-XRP custody: ICRC collateral and a
    /// missing vault must both read false, so the guard never short-circuits the
    /// legitimate ICRC write-down flow nor masks the normal not-found error.
    #[test]
    fn vault_is_native_xrp_is_scoped_to_xrp_custody() {
        install_two_collateral_state(2, 1);
        assert!(vault_is_native_xrp(2), "native-XRP vault must be flagged");
        assert!(
            !vault_is_native_xrp(1),
            "ICRC (ICP) vault must not be flagged"
        );
        assert!(
            !vault_is_native_xrp(999),
            "missing vault must not be flagged"
        );
    }
}

#[cfg(test)]
mod xrp_sp_absorb_contract_tests {
    use super::*;
    use crate::icrc3_proof::{SpProofLedger, SpWritedownProof};
    use crate::state::{
        xrp_collateral_principal, CollateralStatus, CustodyKind, State, StoredXrpSpAbsorbResult,
        MAX_SP_XRP_ABSORB_RESULTS_BY_PROOF,
    };
    use crate::{XrpSpAbsorbRequest, XrpSpPayoutAllocation, MAX_XRP_SP_PAYOUT_ALLOCATIONS};

    const E8: u64 = 100_000_000;
    const VAULT_ID: u64 = 7;

    fn principal(byte: u8) -> Principal {
        Principal::from_slice(&[byte; 29])
    }

    fn sp() -> Principal {
        principal(0x53)
    }

    fn depositor_a() -> Principal {
        principal(0xa1)
    }

    fn depositor_b() -> Principal {
        principal(0xb2)
    }

    fn test_state_with_xrp_vault() -> State {
        let mut state = State::from(crate::InitArg {
            xrc_principal: Principal::anonymous(),
            icusd_ledger_principal: principal(0x10),
            icp_ledger_principal: principal(0x11),
            fee_e8s: 0,
            developer_principal: principal(0xdd),
            treasury_principal: None,
            stability_pool_principal: Some(sp()),
            ckusdt_ledger_principal: None,
            ckusdc_ledger_principal: None,
        });
        state.min_icusd_amount = ICUSD::new(0);
        state.liquidation_protocol_share = Ratio::from(Decimal::ZERO);

        let icp = state.icp_collateral_type();
        if let Some(cfg) = state.collateral_configs.get_mut(&icp) {
            cfg.last_price = Some(10.0);
        }
        state.open_vault(Vault {
            owner: principal(0x99),
            vault_id: 1,
            borrowed_icusd_amount: ICUSD::new(100 * E8),
            collateral_amount: 2_000_000_000,
            collateral_type: icp,
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        });

        let xrp = xrp_collateral_principal();
        let mut xrp_cfg = crate::state::xrp_collateral_config(
            Ratio::from(Decimal::ZERO),
            Ratio::from(Decimal::ZERO),
            Ratio::new(dec!(1.033333333333333333)),
        );
        xrp_cfg.last_price = Some(0.5);
        xrp_cfg.status = CollateralStatus::Active;
        xrp_cfg.custody_kind = Some(CustodyKind::NativeXrp);
        state.collateral_configs.insert(xrp, xrp_cfg);
        state.open_vault(Vault {
            owner: principal(0x42),
            vault_id: VAULT_ID,
            borrowed_icusd_amount: ICUSD::new(100 * E8),
            collateral_amount: 100_000_000,
            collateral_type: xrp,
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        });
        state
    }

    fn proof(block_index: u64) -> SpWritedownProof {
        SpWritedownProof {
            block_index,
            ledger_kind: SpProofLedger::IcusdBurn,
            vault_id_memo: VAULT_ID,
        }
    }

    fn valid_request(block_index: u64) -> XrpSpAbsorbRequest {
        XrpSpAbsorbRequest {
            vault_id: VAULT_ID,
            icusd_burned_e8s: 100 * E8,
            proof: proof(block_index),
            allocations: vec![
                XrpSpPayoutAllocation {
                    claimant: depositor_a(),
                    payout_address: "rA".to_string(),
                    destination_tag: Some(7),
                    drops: 60_000_000,
                },
                XrpSpPayoutAllocation {
                    claimant: depositor_b(),
                    payout_address: "rB".to_string(),
                    destination_tag: None,
                    drops: 40_000_000,
                },
            ],
        }
    }

    fn preflight(state: &mut State, now_ns: u64) {
        stability_pool_preflight_xrp_absorb_in_state(state, sp(), VAULT_ID, 100 * E8, now_ns)
            .expect("preflight reservation");
    }

    fn assert_no_xrp_absorb_mutation(state: &State) {
        assert!(state.xrp_claims.is_empty());
        assert_eq!(state.next_xrp_claim_id, 0);
        assert!(state.sp_xrp_absorb_results_by_proof.is_empty());
        let vault = state.vault_id_to_vaults.get(&VAULT_ID).expect("vault");
        assert_eq!(vault.borrowed_icusd_amount, ICUSD::new(100 * E8));
        assert_eq!(vault.collateral_amount, 100_000_000);
    }

    #[test]
    fn xrp_sp_preflight_rejects_non_xrp_and_stores_reservation() {
        let mut state = test_state_with_xrp_vault();
        let err = stability_pool_preflight_xrp_absorb_in_state(&mut state, sp(), 1, 100 * E8, 1)
            .unwrap_err();
        assert!(
            format!("{err:?}").contains("native-XRP"),
            "unexpected error: {err:?}"
        );
        assert!(state.sp_xrp_absorb_preflights.is_empty());

        let result =
            stability_pool_preflight_xrp_absorb_in_state(&mut state, sp(), VAULT_ID, 100 * E8, 10)
                .expect("xrp preflight accepted");
        assert_eq!(result.vault_id, VAULT_ID);
        assert_eq!(result.icusd_burn_e8s, 100 * E8);
        assert_eq!(result.collateral_received_drops, 100_000_000);
        assert_eq!(result.collateral_price_e8s, 50_000_000);
        assert!(result.expires_at_ns > 10);

        let stored = state.sp_xrp_absorb_preflights.get(&VAULT_ID).unwrap();
        assert_eq!(stored.caller, sp());
        assert_eq!(stored.vault_id, VAULT_ID);
        assert_eq!(stored.icusd_burn_e8s, 100 * E8);
        assert_eq!(stored.collateral_received_drops, 100_000_000);
        assert_eq!(stored.collateral_price_e8s, 50_000_000);
        assert_eq!(stored.expires_at_ns, result.expires_at_ns);
    }

    #[test]
    fn xrp_sp_preflight_rejects_frozen_and_disabled_before_mutation() {
        let mut frozen = test_state_with_xrp_vault();
        frozen.frozen = true;
        assert!(stability_pool_preflight_xrp_absorb_in_state(
            &mut frozen,
            sp(),
            VAULT_ID,
            100 * E8,
            10,
        )
        .is_err());
        assert!(frozen.sp_xrp_absorb_preflights.is_empty());

        let mut disabled = test_state_with_xrp_vault();
        disabled.sp_writedown_disabled = true;
        assert!(stability_pool_preflight_xrp_absorb_in_state(
            &mut disabled,
            sp(),
            VAULT_ID,
            100 * E8,
            10,
        )
        .is_err());
        assert!(disabled.sp_xrp_absorb_preflights.is_empty());
    }

    #[test]
    fn xrp_sp_preflight_rejects_when_vault_operation_in_flight() {
        let mut state = test_state_with_xrp_vault();
        let guard = crate::guard::VaultLiquidationGuard::new(VAULT_ID).expect("lock vault");
        let err =
            stability_pool_preflight_xrp_absorb_in_state(&mut state, sp(), VAULT_ID, 100 * E8, 10)
                .unwrap_err();

        assert!(
            matches!(err, ProtocolError::TemporarilyUnavailable(_)),
            "unexpected error: {err:?}"
        );
        assert!(state.sp_xrp_absorb_preflights.is_empty());
        drop(guard);
    }

    #[test]
    fn xrp_sp_active_preflight_blocks_vault_mutations_until_expiry() {
        let mut state = test_state_with_xrp_vault();
        let pf =
            stability_pool_preflight_xrp_absorb_in_state(&mut state, sp(), VAULT_ID, 100 * E8, 10)
                .expect("preflight accepted");

        assert!(ensure_no_active_xrp_sp_absorb_preflight(&state, VAULT_ID, 20).is_err());
        assert!(
            ensure_no_active_xrp_sp_absorb_preflight(&state, VAULT_ID, pf.expires_at_ns + 1)
                .is_ok()
        );
    }

    #[test]
    fn xrp_sp_absorb_requires_registered_sp_and_unexpired_matching_preflight() {
        let mut no_preflight = test_state_with_xrp_vault();
        assert!(stability_pool_liquidate_xrp_vault_in_state(
            &mut no_preflight,
            sp(),
            valid_request(44),
            20,
        )
        .is_err());
        assert_no_xrp_absorb_mutation(&no_preflight);

        let mut wrong_caller = test_state_with_xrp_vault();
        preflight(&mut wrong_caller, 10);
        assert!(stability_pool_liquidate_xrp_vault_in_state(
            &mut wrong_caller,
            principal(0xee),
            valid_request(44),
            20,
        )
        .is_err());
        assert_no_xrp_absorb_mutation(&wrong_caller);

        let mut expired = test_state_with_xrp_vault();
        let pf = stability_pool_preflight_xrp_absorb_in_state(
            &mut expired,
            sp(),
            VAULT_ID,
            100 * E8,
            10,
        )
        .unwrap();
        assert!(stability_pool_liquidate_xrp_vault_in_state(
            &mut expired,
            sp(),
            valid_request(44),
            pf.expires_at_ns + 1,
        )
        .is_err());
        assert_no_xrp_absorb_mutation(&expired);

        let mut mismatched = test_state_with_xrp_vault();
        preflight(&mut mismatched, 10);
        let mut req = valid_request(44);
        req.icusd_burned_e8s -= 1;
        assert!(
            stability_pool_liquidate_xrp_vault_in_state(&mut mismatched, sp(), req, 20).is_err()
        );
        assert_no_xrp_absorb_mutation(&mismatched);
    }

    #[test]
    fn xrp_sp_absorb_rejects_non_xrp_before_mutation() {
        let mut non_xrp = test_state_with_xrp_vault();
        non_xrp.sp_xrp_absorb_preflights.insert(
            1,
            crate::state::StoredXrpSpAbsorbPreflight {
                caller: sp(),
                vault_id: 1,
                icusd_burn_e8s: 100 * E8,
                total_to_seize_drops: 100_000_000,
                collateral_received_drops: 100_000_000,
                collateral_price_e8s: 1_000_000_000,
                expires_at_ns: 100,
            },
        );
        let mut non_xrp_req = valid_request(44);
        non_xrp_req.vault_id = 1;
        non_xrp_req.proof.vault_id_memo = 1;
        assert!(
            stability_pool_liquidate_xrp_vault_in_state(&mut non_xrp, sp(), non_xrp_req, 20)
                .is_err()
        );
        assert!(non_xrp.xrp_claims.is_empty());
    }

    #[test]
    fn xrp_sp_preflight_rejects_healthy_vault_before_burn() {
        let mut healthy = test_state_with_xrp_vault();
        healthy
            .vault_id_to_vaults
            .get_mut(&VAULT_ID)
            .unwrap()
            .collateral_amount = 1_000_000_000_000;
        assert!(stability_pool_preflight_xrp_absorb_in_state(
            &mut healthy,
            sp(),
            VAULT_ID,
            100 * E8,
            10
        )
        .is_err());
        assert!(healthy.xrp_claims.is_empty());
        assert!(healthy.sp_xrp_absorb_preflights.is_empty());
    }

    #[test]
    fn xrp_sp_absorb_validates_allocations_before_mutation() {
        let invalid_cases: Vec<XrpSpAbsorbRequest> = {
            let mut sum_mismatch = valid_request(44);
            sum_mismatch.allocations[0].drops -= 1;

            let mut empty_address = valid_request(44);
            empty_address.allocations[0].payout_address = "  ".to_string();

            let mut zero_drops = valid_request(44);
            zero_drops.allocations[0].drops = 0;
            zero_drops.allocations[1].drops = 100_000_000;

            let mut too_many = valid_request(44);
            too_many.allocations = (0..=MAX_XRP_SP_PAYOUT_ALLOCATIONS)
                .map(|i| XrpSpPayoutAllocation {
                    claimant: principal((i % 200) as u8),
                    payout_address: format!("r{i}"),
                    destination_tag: None,
                    drops: 1,
                })
                .collect();

            vec![
                XrpSpAbsorbRequest {
                    allocations: vec![],
                    ..valid_request(44)
                },
                sum_mismatch,
                empty_address,
                zero_drops,
                too_many,
            ]
        };

        for request in invalid_cases {
            let mut state = test_state_with_xrp_vault();
            preflight(&mut state, 10);
            assert!(
                stability_pool_liquidate_xrp_vault_in_state(&mut state, sp(), request, 20).is_err()
            );
            assert_no_xrp_absorb_mutation(&state);
        }
    }

    #[test]
    fn xrp_sp_absorb_uses_reserved_preflight_amount_for_write_down() {
        let mut state = test_state_with_xrp_vault();
        state.sp_xrp_absorb_preflights.insert(
            VAULT_ID,
            crate::state::StoredXrpSpAbsorbPreflight {
                caller: sp(),
                vault_id: VAULT_ID,
                icusd_burn_e8s: 100 * E8,
                total_to_seize_drops: 80_000_000,
                collateral_received_drops: 80_000_000,
                collateral_price_e8s: 50_000_000,
                expires_at_ns: 100,
            },
        );
        let mut request = valid_request(44);
        request.allocations[0].drops = 48_000_000;
        request.allocations[1].drops = 32_000_000;

        let result = stability_pool_liquidate_xrp_vault_in_state(&mut state, sp(), request, 20)
            .expect("absorb accepted");

        assert_eq!(result.collateral_received_drops, 80_000_000);
        let vault = state
            .vault_id_to_vaults
            .get(&VAULT_ID)
            .expect("non-drained vault remains for excess collateral");
        assert_eq!(vault.borrowed_icusd_amount, ICUSD::new(0));
        assert_eq!(vault.collateral_amount, 20_000_000);
        assert_eq!(state.xrp_claims.get(&0).unwrap().drops, 48_000_000);
        assert_eq!(state.xrp_claims.get(&1).unwrap().drops, 32_000_000);
    }

    #[test]
    fn xrp_sp_absorb_submit_honors_reserved_preflight_after_vault_recovers() {
        let mut state = test_state_with_xrp_vault();
        preflight(&mut state, 10);
        state
            .vault_id_to_vaults
            .get_mut(&VAULT_ID)
            .unwrap()
            .collateral_amount = 1_000_000_000_000;

        let result =
            stability_pool_liquidate_xrp_vault_in_state(&mut state, sp(), valid_request(44), 20)
                .expect("post-burn submit consumes reservation");

        assert_eq!(result.collateral_received_drops, 100_000_000);
        let vault = state
            .vault_id_to_vaults
            .get(&VAULT_ID)
            .expect("recovered vault remains with excess collateral");
        assert_eq!(vault.borrowed_icusd_amount, ICUSD::new(0));
        assert_eq!(vault.collateral_amount, 999_900_000_000);
        assert_eq!(state.xrp_claims.get(&0).unwrap().drops, 60_000_000);
        assert_eq!(state.xrp_claims.get(&1).unwrap().drops, 40_000_000);
        assert!(state.sp_xrp_absorb_preflights.get(&VAULT_ID).is_none());
    }

    #[test]
    fn xrp_sp_absorb_writes_down_claims_and_exact_replay_returns_same_claim_ids() {
        let mut state = test_state_with_xrp_vault();
        preflight(&mut state, 10);

        let result =
            stability_pool_liquidate_xrp_vault_in_state(&mut state, sp(), valid_request(44), 20)
                .expect("absorb accepted");
        assert!(result.success);
        assert_eq!(result.vault_id, VAULT_ID);
        assert_eq!(result.liquidated_debt_e8s, 100 * E8);
        assert_eq!(result.collateral_received_drops, 100_000_000);
        assert_eq!(result.payout_claims.len(), 2);
        assert_eq!(result.payout_claims[0].claimant, depositor_a());
        assert_eq!(result.payout_claims[0].claim_id, 0);
        assert_eq!(result.payout_claims[1].claimant, depositor_b());
        assert_eq!(result.payout_claims[1].claim_id, 1);
        assert_eq!(state.next_xrp_claim_id, 2);
        assert!(state.vault_id_to_vaults.get(&VAULT_ID).is_none());
        assert!(state.sp_xrp_absorb_preflights.get(&VAULT_ID).is_none());
        assert!(state
            .sp_xrp_absorb_results_by_proof
            .contains_key(&(SpProofLedger::IcusdBurn, 44,)));
        assert_eq!(state.xrp_claims.get(&0).unwrap().claimant, depositor_a());
        assert_eq!(state.xrp_claims.get(&1).unwrap().claimant, depositor_b());

        let replay =
            stability_pool_liquidate_xrp_vault_in_state(&mut state, sp(), valid_request(44), 999)
                .expect("exact replay returns cached result");
        assert_eq!(replay, result);
        assert_eq!(state.next_xrp_claim_id, 2);
        assert_eq!(state.xrp_claims.len(), 2);
    }

    #[test]
    fn xrp_sp_absorb_conflicting_replay_rejects_without_mutation() {
        let mut state = test_state_with_xrp_vault();
        preflight(&mut state, 10);
        let result =
            stability_pool_liquidate_xrp_vault_in_state(&mut state, sp(), valid_request(44), 20)
                .unwrap();
        let claims_before = state.xrp_claims.clone();
        let next_before = state.next_xrp_claim_id;
        let results_before = state.sp_xrp_absorb_results_by_proof.clone();

        let mut conflicting = valid_request(44);
        conflicting.allocations[0].payout_address = "rDifferent".to_string();
        assert!(
            stability_pool_liquidate_xrp_vault_in_state(&mut state, sp(), conflicting, 30).is_err()
        );
        assert_eq!(state.xrp_claims, claims_before);
        assert_eq!(state.next_xrp_claim_id, next_before);
        assert_eq!(state.sp_xrp_absorb_results_by_proof, results_before);
        assert_eq!(
            state
                .sp_xrp_absorb_results_by_proof
                .get(&(SpProofLedger::IcusdBurn, 44))
                .unwrap()
                .result,
            result,
        );
    }

    fn stored_result_for(block_index: u64) -> StoredXrpSpAbsorbResult {
        StoredXrpSpAbsorbResult {
            caller: sp(),
            vault_id: block_index,
            icusd_burned_e8s: 100 * E8,
            proof_ledger: SpProofLedger::IcusdBurn,
            proof_block_index: block_index,
            allocation_fingerprint: vec![block_index as u8; 32],
            result: crate::XrpSpAbsorbResult {
                success: true,
                vault_id: block_index,
                liquidated_debt_e8s: 100 * E8,
                collateral_received_drops: 100_000_000,
                payout_claims: vec![],
                block_index,
                collateral_price_e8s: 50_000_000,
            },
            accepted_at_ns: block_index,
        }
    }

    #[test]
    fn xrp_sp_absorb_result_cache_keeps_just_accepted_proof() {
        let mut state = State::default();
        for block_index in 1..=(MAX_SP_XRP_ABSORB_RESULTS_BY_PROOF as u64) {
            record_sp_xrp_absorb_result_bounded(
                &mut state,
                (SpProofLedger::IcusdBurn, block_index),
                stored_result_for(block_index),
            );
        }

        record_sp_xrp_absorb_result_bounded(
            &mut state,
            (SpProofLedger::IcusdBurn, 0),
            stored_result_for(0),
        );

        assert_eq!(
            state.sp_xrp_absorb_results_by_proof.len(),
            MAX_SP_XRP_ABSORB_RESULTS_BY_PROOF,
        );
        assert!(
            state
                .sp_xrp_absorb_results_by_proof
                .contains_key(&(SpProofLedger::IcusdBurn, 0)),
            "the just-accepted proof must remain replayable",
        );
        assert!(!state
            .sp_xrp_absorb_results_by_proof
            .contains_key(&(SpProofLedger::IcusdBurn, 1)));
    }
}
