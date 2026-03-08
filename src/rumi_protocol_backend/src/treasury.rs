//! Treasury inter-canister helpers.
//!
//! Mint/transfer protocol revenue to the treasury canister and call
//! `treasury.deposit()` for categorized bookkeeping.
//!
//! All treasury operations are **non-critical**: failures are logged but
//! never block user-facing operations (borrow, repay, liquidation).

use candid::{CandidType, Deserialize, Principal};
use ic_canister_log::log;
use serde::Serialize;

use crate::logs::INFO;
use crate::management;
use crate::numeric::ICUSD;
use crate::state::read_state;

// ---------------------------------------------------------------------------
// Mirror types matching rumi_treasury::types (can't depend on cdylib crate)
// ---------------------------------------------------------------------------

/// Mirrors `rumi_treasury::types::DepositType`.
#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub enum DepositType {
    BorrowingFee,
    RedemptionFee,
    LiquidationFee,
    InterestRevenue,
}

/// Mirrors `rumi_treasury::types::AssetType`.
#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub enum AssetType {
    ICUSD,
    ICP,
    CKBTC,
    CKUSDT,
    CKUSDC,
}

/// Mirrors `rumi_treasury::types::DepositArgs`.
#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub struct DepositArgs {
    pub deposit_type: DepositType,
    pub asset_type: AssetType,
    pub amount: u64,
    pub block_index: u64,
    pub memo: Option<String>,
}

// ---------------------------------------------------------------------------
// Helper: map collateral ledger principal → AssetType
// ---------------------------------------------------------------------------

/// Map a collateral ledger principal to the treasury's AssetType enum.
/// Uses known ckStable ledger principals from state config, plus ICP ledger.
pub fn collateral_to_asset_type(ct: &Principal) -> AssetType {
    read_state(|s| {
        if *ct == s.icp_ledger_principal {
            return AssetType::ICP;
        }
        if let Some(ckusdt) = s.ckusdt_ledger_principal {
            if *ct == ckusdt {
                return AssetType::CKUSDT;
            }
        }
        if let Some(ckusdc) = s.ckusdc_ledger_principal {
            if *ct == ckusdc {
                return AssetType::CKUSDC;
            }
        }
        // For any other collateral (ckBTC, ckETH, etc.), default to ICP
        // since treasury AssetType only has ICP/CKBTC/CKUSDT/CKUSDC.
        // TODO: Expand AssetType when new collaterals are added.
        AssetType::ICP
    })
}

// ---------------------------------------------------------------------------
// Inter-canister call to treasury.deposit()
// ---------------------------------------------------------------------------

/// Notify the treasury canister about a deposit (for bookkeeping).
/// Non-critical: failures are logged but don't affect protocol operation.
pub async fn notify_treasury_deposit(
    treasury: Principal,
    deposit_type: DepositType,
    asset_type: AssetType,
    amount: u64,
    block_index: u64,
) -> Result<u64, String> {
    let args = DepositArgs {
        deposit_type,
        asset_type,
        amount,
        block_index,
        memo: None,
    };
    let result: Result<(Result<u64, String>,), _> =
        ic_cdk::call(treasury, "deposit", (args,)).await;
    match result {
        Ok((Ok(deposit_id),)) => {
            log!(INFO, "[treasury] Deposit recorded: id={}", deposit_id);
            Ok(deposit_id)
        }
        Ok((Err(e),)) => {
            log!(
                INFO,
                "[treasury] WARNING: deposit recording failed: {}",
                e
            );
            Err(e)
        }
        Err((code, msg)) => {
            log!(
                INFO,
                "[treasury] WARNING: inter-canister call failed: {:?} {}",
                code,
                msg
            );
            Err(msg)
        }
    }
}

// ---------------------------------------------------------------------------
// Public helpers — mint/transfer + notify
// ---------------------------------------------------------------------------

/// Mint icUSD interest revenue to treasury and record the deposit.
pub async fn mint_interest_to_treasury(interest_share: ICUSD) {
    if interest_share.0 == 0 {
        return;
    }
    let treasury = read_state(|s| s.treasury_principal);
    if let Some(tp) = treasury {
        match management::mint_icusd(interest_share, tp).await {
            Ok(block_index) => {
                log!(
                    INFO,
                    "[treasury] Minted {} icUSD interest revenue (block {})",
                    interest_share.to_u64(),
                    block_index
                );
                let _ = notify_treasury_deposit(
                    tp,
                    DepositType::InterestRevenue,
                    AssetType::ICUSD,
                    interest_share.to_u64(),
                    block_index,
                )
                .await;
            }
            Err(e) => log!(
                INFO,
                "[treasury] WARNING: interest mint failed: {:?}",
                e
            ),
        }
    }
}

/// Mint icUSD interest revenue to the stability pool canister.
/// The stability pool distributes this pro-rata to depositors.
pub async fn mint_interest_to_stability_pool(interest_share: ICUSD) {
    if interest_share.0 == 0 {
        return;
    }
    let (stability_pool, icusd_ledger) =
        read_state(|s| (s.stability_pool_canister, s.icusd_ledger_principal));
    if let Some(pool_principal) = stability_pool {
        match management::mint_icusd(interest_share, pool_principal).await {
            Ok(block_index) => {
                log!(
                    INFO,
                    "[treasury] Minted {} icUSD interest to stability pool (block {})",
                    interest_share.to_u64(),
                    block_index
                );

                // Notify pool to distribute interest pro-rata to depositors.
                // Fire-and-forget: failure is logged but does not block repayment.
                let amount = interest_share.to_u64();
                let result: Result<(candid::IDLValue,), _> = ic_cdk::call(
                    pool_principal,
                    "receive_interest_revenue",
                    (icusd_ledger, amount),
                )
                .await;
                match result {
                    Ok(_) => {
                        log!(
                            INFO,
                            "[treasury] Pool acknowledged interest distribution ({} icUSD)",
                            amount
                        );
                    }
                    Err(e) => {
                        log!(
                            INFO,
                            "[treasury] WARNING: pool interest notification call failed: {:?}",
                            e
                        );
                    }
                }
            }
            Err(e) => {
                log!(
                    INFO,
                    "[treasury] WARNING: stability pool interest mint failed ({} icUSD): {:?}",
                    interest_share.to_u64(),
                    e
                );
            }
        }
    }
}

/// Mint icUSD borrowing fee to treasury and record the deposit.
pub async fn mint_borrowing_fee_to_treasury(fee: ICUSD) {
    if fee.0 == 0 {
        return;
    }
    let treasury = read_state(|s| s.treasury_principal);
    if let Some(tp) = treasury {
        match management::mint_icusd(fee, tp).await {
            Ok(block_index) => {
                log!(
                    INFO,
                    "[treasury] Minted {} icUSD borrowing fee (block {})",
                    fee.to_u64(),
                    block_index
                );
                let _ = notify_treasury_deposit(
                    tp,
                    DepositType::BorrowingFee,
                    AssetType::ICUSD,
                    fee.to_u64(),
                    block_index,
                )
                .await;
            }
            Err(e) => log!(
                INFO,
                "[treasury] WARNING: borrowing fee mint failed: {:?}",
                e
            ),
        }
    }
}

/// Transfer collateral (liquidation fee) to treasury and record the deposit.
pub async fn send_liquidation_fee_to_treasury(
    amount: u64,
    collateral_ledger: Principal,
    asset_type: AssetType,
) {
    if amount == 0 {
        return;
    }
    let treasury = read_state(|s| s.treasury_principal);
    if let Some(tp) = treasury {
        match management::transfer_collateral(amount, tp, collateral_ledger).await {
            Ok(block_index) => {
                log!(
                    INFO,
                    "[treasury] Sent {} collateral liquidation fee (block {})",
                    amount,
                    block_index
                );
                let _ = notify_treasury_deposit(
                    tp,
                    DepositType::LiquidationFee,
                    asset_type,
                    amount,
                    block_index,
                )
                .await;
            }
            Err(e) => log!(
                INFO,
                "[treasury] WARNING: liquidation fee transfer failed: {:?}",
                e
            ),
        }
    }
}

/// Mint pending treasury interest accumulated from sync liquidations.
/// Called from the XRC timer tick to drain `pending_treasury_interest`.
pub async fn drain_pending_treasury_interest() {
    let pending = read_state(|s| s.pending_treasury_interest);
    if pending.0 == 0 {
        return;
    }
    let treasury = read_state(|s| s.treasury_principal);
    if let Some(tp) = treasury {
        match management::mint_icusd(pending, tp).await {
            Ok(block_index) => {
                crate::state::mutate_state(|s| {
                    s.pending_treasury_interest = ICUSD::new(0);
                });
                log!(
                    INFO,
                    "[treasury] Drained {} pending interest (block {})",
                    pending.to_u64(),
                    block_index
                );
                let _ = notify_treasury_deposit(
                    tp,
                    DepositType::InterestRevenue,
                    AssetType::ICUSD,
                    pending.to_u64(),
                    block_index,
                )
                .await;
            }
            Err(e) => log!(
                INFO,
                "[treasury] WARNING: pending interest drain failed: {:?}",
                e
            ),
        }
    }
}

/// Transfer pending collateral fees to treasury.
/// Called from the XRC timer tick to drain `pending_treasury_collateral`.
pub async fn drain_pending_treasury_collateral() {
    let pending: Vec<(u64, Principal)> =
        read_state(|s| s.pending_treasury_collateral.clone());
    if pending.is_empty() {
        return;
    }
    let treasury = read_state(|s| s.treasury_principal);
    if let Some(tp) = treasury {
        let mut drained = Vec::new();
        for (amount, ledger) in &pending {
            let asset_type = collateral_to_asset_type(ledger);
            match management::transfer_collateral(*amount, tp, *ledger).await {
                Ok(block_index) => {
                    log!(
                        INFO,
                        "[treasury] Drained {} collateral fee for ledger {} (block {})",
                        amount,
                        ledger,
                        block_index
                    );
                    let _ = notify_treasury_deposit(
                        tp,
                        DepositType::LiquidationFee,
                        asset_type,
                        *amount,
                        block_index,
                    )
                    .await;
                    drained.push((*amount, *ledger));
                }
                Err(e) => log!(
                    INFO,
                    "[treasury] WARNING: collateral drain failed for {}: {:?}",
                    ledger,
                    e
                ),
            }
        }
        // Remove successfully drained entries
        if !drained.is_empty() {
            crate::state::mutate_state(|s| {
                s.pending_treasury_collateral
                    .retain(|entry| !drained.contains(entry));
            });
        }
    }
}
