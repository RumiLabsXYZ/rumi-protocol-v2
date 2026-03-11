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
///
/// `collateral_type` identifies which collateral's vault generated this interest.
/// The pool uses it to exclude depositors who opted out of that collateral.
pub async fn mint_interest_to_stability_pool(interest_share: ICUSD, collateral_type: Principal) {
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
                let ct: Option<Principal> = Some(collateral_type);
                let result: Result<(candid::IDLValue,), _> = ic_cdk::call(
                    pool_principal,
                    "receive_interest_revenue",
                    (icusd_ledger, amount, ct),
                )
                .await;
                match result {
                    Ok(_) => {
                        log!(
                            INFO,
                            "[treasury] Pool acknowledged interest distribution ({} icUSD, collateral {})",
                            amount,
                            collateral_type
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

// ---------------------------------------------------------------------------
// Interest distribution — N-way split
// ---------------------------------------------------------------------------

/// Distribute interest revenue according to the configured interest_split.
/// Mints icUSD to each destination: stability pool, treasury, and/or 3pool.
///
/// For the 3pool destination, mints icUSD to self, approves the 3pool canister,
/// then calls `donate(0, amount)` to inject yield into the pool.
///
/// `collateral_type` is needed for stability pool interest routing.
pub async fn distribute_interest(interest: ICUSD, collateral_type: Principal) {
    if interest.0 == 0 {
        return;
    }

    let (split, three_pool) = read_state(|s| {
        (s.interest_split.clone(), s.three_pool_canister)
    });

    let total_e8s = interest.to_u64();

    for recipient in &split {
        let share_e8s = ((total_e8s as u128) * (recipient.bps as u128) / 10_000) as u64;
        if share_e8s == 0 {
            continue;
        }
        let share = ICUSD::from(share_e8s);

        match &recipient.destination {
            crate::state::InterestDestination::StabilityPool => {
                mint_interest_to_stability_pool(share, collateral_type).await;
            }
            crate::state::InterestDestination::Treasury => {
                mint_interest_to_treasury(share).await;
            }
            crate::state::InterestDestination::ThreePool => {
                if let Some(pool_canister) = three_pool {
                    donate_to_three_pool(pool_canister, share_e8s).await;
                } else {
                    log!(INFO, "[treasury] WARNING: 3pool interest share ({} icUSD) has no target canister configured, sending to treasury instead", share_e8s);
                    mint_interest_to_treasury(share).await;
                }
            }
        }
    }
}

/// Distribute stablecoin-denominated interest revenue.
/// Similar to `distribute_interest` but handles the stablecoin-specific case:
/// - StabilityPool: mint icUSD (backed by stablecoins in reserves)
/// - Treasury: transfer actual stablecoins
/// - ThreePool: mint icUSD + donate to pool
///
/// `interest_e8s` is in icUSD-equivalent e8s (8 decimals).
/// `token_type` and the corresponding ledger are used for treasury stablecoin transfers.
pub async fn distribute_stablecoin_interest(
    interest_e8s: u64,
    collateral_type: Principal,
    token_type: crate::StableTokenType,
) {
    if interest_e8s == 0 {
        return;
    }

    let (split, three_pool) = read_state(|s| {
        (s.interest_split.clone(), s.three_pool_canister)
    });

    for recipient in &split {
        let share_e8s = ((interest_e8s as u128) * (recipient.bps as u128) / 10_000) as u64;
        if share_e8s == 0 {
            continue;
        }

        match &recipient.destination {
            crate::state::InterestDestination::StabilityPool => {
                let pool_icusd = ICUSD::from(share_e8s);
                mint_interest_to_stability_pool(pool_icusd, collateral_type).await;
            }
            crate::state::InterestDestination::Treasury => {
                // Transfer stablecoins (not icUSD) to treasury
                let treasury_e6s = share_e8s / 100; // e8s → e6s
                if treasury_e6s > 0 {
                    let (treasury_principal, stable_ledger) = read_state(|s| {
                        let ledger = match token_type {
                            crate::StableTokenType::CKUSDT => s.ckusdt_ledger_principal,
                            crate::StableTokenType::CKUSDC => s.ckusdc_ledger_principal,
                        };
                        (s.treasury_principal, ledger)
                    });
                    if let (Some(tp), Some(ledger)) = (treasury_principal, stable_ledger) {
                        match crate::management::transfer_collateral(treasury_e6s, tp, ledger).await {
                            Ok(block_index) => {
                                log!(INFO, "[treasury] Transferred {} {:?} interest to treasury (block {})", treasury_e6s, token_type, block_index);
                                let asset_type = match token_type {
                                    crate::StableTokenType::CKUSDT => AssetType::CKUSDT,
                                    crate::StableTokenType::CKUSDC => AssetType::CKUSDC,
                                };
                                let _ = notify_treasury_deposit(tp, DepositType::InterestRevenue, asset_type, treasury_e6s, block_index).await;
                            }
                            Err(e) => log!(INFO, "[treasury] WARNING: stablecoin interest transfer failed: {:?}", e),
                        }
                    }
                }
            }
            crate::state::InterestDestination::ThreePool => {
                if let Some(pool_canister) = three_pool {
                    donate_to_three_pool(pool_canister, share_e8s).await;
                } else {
                    // Fallback: mint icUSD to treasury (same as distribute_interest)
                    log!(INFO, "[treasury] WARNING: 3pool interest share ({} icUSD) has no target canister, sending to treasury instead", share_e8s);
                    mint_interest_to_treasury(ICUSD::from(share_e8s)).await;
                }
            }
        }
    }
}

/// Mint icUSD to self, approve 3pool, and call `donate(0, amount)`.
/// Non-critical: failures are logged but don't block protocol operations.
async fn donate_to_three_pool(pool_canister: Principal, amount_e8s: u64) {
    let protocol_id = ic_cdk::id();
    let icusd_fee: u64 = 10_000; // icUSD ledger fee (0.0001 icUSD)

    // 1. Mint icUSD to self (the backend canister)
    // Must mint extra to cover: approve fee + transfer_from fee
    let mint_amount = amount_e8s + 2 * icusd_fee;
    let icusd = ICUSD::from(mint_amount);
    match crate::management::mint_icusd(icusd, protocol_id).await {
        Ok(block_index) => {
            log!(INFO, "[treasury] Minted {} icUSD to self for 3pool donation (block {})", mint_amount, block_index);
        }
        Err(e) => {
            log!(INFO, "[treasury] WARNING: 3pool donation mint failed: {:?}", e);
            return;
        }
    }

    // 2. Approve 3pool canister to spend it
    // Allowance must cover amount + transfer_from fee
    let approve_amount = amount_e8s + icusd_fee;
    match crate::management::approve_icusd(pool_canister, approve_amount).await {
        Ok(_) => {
            log!(INFO, "[treasury] Approved 3pool {} for {} icUSD", pool_canister, approve_amount);
        }
        Err(e) => {
            log!(INFO, "[treasury] WARNING: 3pool approval failed: {:?}", e);
            return;
        }
    }

    // 3. Call donate(0, amount) on the 3pool canister
    // token_index 0 = icUSD, amount in e8s (icUSD uses 8 decimals)
    let donate_amount = candid::Nat::from(amount_e8s);
    let result: Result<(Result<(), ThreePoolDonateError>,), _> =
        ic_cdk::call(pool_canister, "donate", (0u8, donate_amount)).await;
    match result {
        Ok((Ok(()),)) => {
            log!(INFO, "[treasury] Donated {} icUSD to 3pool", amount_e8s);
        }
        Ok((Err(e),)) => {
            log!(INFO, "[treasury] WARNING: 3pool donate call returned error: {:?}", e);
        }
        Err((code, msg)) => {
            log!(INFO, "[treasury] WARNING: 3pool donate inter-canister call failed: {:?} {}", code, msg);
        }
    }
}

/// Mirror of the 3pool ThreePoolError for the donate response.
/// We only need this for deserialization of the Result.
#[derive(CandidType, Deserialize, Clone, Debug)]
enum ThreePoolDonateError {
    InsufficientOutput { expected_min: candid::Nat, actual: candid::Nat },
    InsufficientLiquidity,
    InvalidCoinIndex,
    ZeroAmount,
    PoolEmpty,
    SlippageExceeded,
    TransferFailed { token: String, reason: String },
    Unauthorized,
    MathOverflow,
    InvariantNotConverged,
    PoolPaused,
}

// ---------------------------------------------------------------------------
// Public helpers — mint/transfer + notify
// ---------------------------------------------------------------------------

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

/// Flush accumulated interest from periodic harvesting to pools/treasury.
/// For each collateral bucket that has reached the threshold, calls
/// `distribute_interest` which handles the N-way split (3pool, stability pool, treasury).
pub async fn flush_pending_interest() {
    let (pending, threshold) = read_state(|s| {
        (s.pending_interest_for_pools.clone(), s.interest_flush_threshold_e8s)
    });

    for (collateral_type, amount) in pending {
        if amount >= threshold {
            log!(INFO, "[treasury] Flushing {} icUSD interest for collateral {}", amount, collateral_type);
            // Zero out this bucket BEFORE the async call to prevent double-flush
            crate::state::mutate_state(|s| {
                s.pending_interest_for_pools.remove(&collateral_type);
            });
            distribute_interest(ICUSD::from(amount), collateral_type).await;
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
