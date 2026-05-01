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
// Wave-8e LIQ-005: fee → deficit routing planner
// ---------------------------------------------------------------------------

/// Outcome of routing a fee through the deficit-repayment path.
///
/// `to_repay` is the icUSD applied to deficit repayment (mint foregone for
/// borrowing fees, supply already reduced for redemption fees).
/// `to_remainder` is what flows to the existing destination — for borrowing
/// fees that's the treasury mint amount; for redemption fees that's the
/// portion of fee revenue that accrues as protocol equity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeeRoutingOutcome {
    pub to_repay: ICUSD,
    pub to_remainder: ICUSD,
}

/// Plan how a fee splits between deficit repayment and its existing
/// destination. Mutates state to apply the repayment + emit
/// `DeficitRepaid` (so callers stay one-liners), then returns the split
/// for the caller to act on.
///
/// `anchor_block_index` on the emitted event is `None`; the ledger op
/// (treasury mint for borrowing fee, the redeemer's burn for redemption
/// fee) hasn't happened yet at this point. Callers can correlate via the
/// op_nonce on subsequent ledger entries.
///
/// In production the canister wrapper [`plan_fee_routing`] passes
/// `ic_cdk::api::time()`; the inner `_at` form takes the timestamp
/// explicitly so unit tests can drive it without a canister context.
pub fn plan_fee_routing_at(
    state: &mut crate::state::State,
    fee: crate::numeric::ICUSD,
    source: crate::event::FeeSource,
    timestamp: u64,
) -> FeeRoutingOutcome {
    if fee.0 == 0 {
        return FeeRoutingOutcome {
            to_repay: crate::numeric::ICUSD::new(0),
            to_remainder: crate::numeric::ICUSD::new(0),
        };
    }
    let to_repay = state.compute_deficit_repay_amount(fee);
    if to_repay.0 > 0 {
        crate::event::record_deficit_repaid(state, to_repay, source, None, timestamp);
    }
    let to_remainder = crate::numeric::ICUSD::new(fee.0 - to_repay.0);
    FeeRoutingOutcome { to_repay, to_remainder }
}

/// Production wrapper around [`plan_fee_routing_at`] that captures the
/// canister time. Call sites stay one-liners.
pub fn plan_fee_routing(
    state: &mut crate::state::State,
    fee: crate::numeric::ICUSD,
    source: crate::event::FeeSource,
) -> FeeRoutingOutcome {
    plan_fee_routing_at(state, fee, source, ic_cdk::api::time())
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
///
/// Returns `Ok(())` when the icUSD mint itself succeeded — the
/// downstream `notify_treasury_deposit` call is bookkeeping and its
/// failure does not roll back the mint. Returns `Err(interest_share)`
/// when the mint did not happen (no treasury configured, or the ledger
/// call failed); the caller is expected to re-queue this amount via
/// the snapshot-then-decrement restore path so revenue is not lost.
pub async fn mint_interest_to_treasury(interest_share: ICUSD) -> Result<(), ICUSD> {
    if interest_share.0 == 0 {
        return Ok(());
    }
    let treasury = read_state(|s| s.treasury_principal);
    let Some(tp) = treasury else {
        log!(
            INFO,
            "[treasury] WARNING: no treasury principal configured; {} icUSD interest unminted",
            interest_share.to_u64()
        );
        return Err(interest_share);
    };
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
            Ok(())
        }
        Err(e) => {
            log!(
                INFO,
                "[treasury] WARNING: interest mint failed: {:?}",
                e
            );
            Err(interest_share)
        }
    }
}

/// Mint icUSD interest revenue to the stability pool canister.
/// The stability pool distributes this pro-rata to depositors.
///
/// `collateral_type` identifies which collateral's vault generated this interest.
/// The pool uses it to exclude depositors who opted out of that collateral.
///
/// Returns `Ok(())` when the icUSD mint succeeded — the post-mint
/// `receive_interest_revenue` notification is bookkeeping and its
/// failure does not roll back the mint. Returns `Err(interest_share)`
/// when the mint did not happen (no pool configured, or the ledger
/// call failed); the caller re-queues this via the snapshot-then-
/// decrement restore path.
pub async fn mint_interest_to_stability_pool(
    interest_share: ICUSD,
    collateral_type: Principal,
) -> Result<(), ICUSD> {
    if interest_share.0 == 0 {
        return Ok(());
    }
    let (stability_pool, icusd_ledger) =
        read_state(|s| (s.stability_pool_canister, s.icusd_ledger_principal));
    let Some(pool_principal) = stability_pool else {
        log!(
            INFO,
            "[treasury] WARNING: no stability pool configured; {} icUSD interest unminted",
            interest_share.to_u64()
        );
        return Err(interest_share);
    };
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
            Ok(())
        }
        Err(e) => {
            log!(
                INFO,
                "[treasury] WARNING: stability pool interest mint failed ({} icUSD): {:?}",
                interest_share.to_u64(),
                e
            );
            Err(interest_share)
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
///
/// Returns the total icUSD that failed to mint across all recipients.
/// The caller (e.g. `flush_pending_interest`) re-queues this via the
/// snapshot-then-decrement restore path so the failed share is replayed
/// on the next tick rather than silently lost.
pub async fn distribute_interest(interest: ICUSD, collateral_type: Principal) -> ICUSD {
    if interest.0 == 0 {
        return ICUSD::new(0);
    }

    let (split, three_pool) = read_state(|s| {
        (s.interest_split.clone(), s.three_pool_canister)
    });

    let total_e8s = interest.to_u64();
    let mut unminted_e8s: u64 = 0;

    for recipient in &split {
        let share_e8s = ((total_e8s as u128) * (recipient.bps as u128) / 10_000) as u64;
        if share_e8s == 0 {
            continue;
        }
        let share = ICUSD::from(share_e8s);

        match &recipient.destination {
            crate::state::InterestDestination::StabilityPool => {
                if let Err(unsent) =
                    mint_interest_to_stability_pool(share, collateral_type).await
                {
                    unminted_e8s = unminted_e8s.saturating_add(unsent.to_u64());
                }
            }
            crate::state::InterestDestination::Treasury => {
                if let Err(unsent) = mint_interest_to_treasury(share).await {
                    unminted_e8s = unminted_e8s.saturating_add(unsent.to_u64());
                }
            }
            crate::state::InterestDestination::ThreePool => {
                if let Some(pool_canister) = three_pool {
                    if let Err(unsent_e8s) = donate_to_three_pool(pool_canister, share_e8s).await {
                        unminted_e8s = unminted_e8s.saturating_add(unsent_e8s);
                    }
                } else {
                    log!(INFO, "[treasury] WARNING: 3pool interest share ({} icUSD) has no target canister configured, sending to treasury instead", share_e8s);
                    if let Err(unsent) = mint_interest_to_treasury(share).await {
                        unminted_e8s = unminted_e8s.saturating_add(unsent.to_u64());
                    }
                }
            }
        }
    }
    ICUSD::from(unminted_e8s)
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
                // Stablecoin path keeps legacy "log + drop" on mint failure.
                // No bucket to re-queue against (the funds live in
                // collateral reserves, not a pending field). Audit
                // INT-002 covered only the icUSD-denominated path.
                let _ = mint_interest_to_stability_pool(pool_icusd, collateral_type).await;
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
                    let _ = donate_to_three_pool(pool_canister, share_e8s).await;
                } else {
                    // Fallback: mint icUSD to treasury (same as distribute_interest)
                    log!(INFO, "[treasury] WARNING: 3pool interest share ({} icUSD) has no target canister, sending to treasury instead", share_e8s);
                    let _ = mint_interest_to_treasury(ICUSD::from(share_e8s)).await;
                }
            }
        }
    }
}

/// Mint icUSD directly to the 3pool canister, then call `receive_donation`
/// so the pool updates its internal balances.
/// Non-critical: failures are logged but don't block protocol operations.
///
/// Returns `Ok(())` when the icUSD mint succeeded — the subsequent
/// `receive_donation` notification is bookkeeping and its failure does
/// not roll back the mint. Returns `Err(amount_e8s)` when the mint
/// itself failed; the caller re-queues this via the snapshot-then-
/// decrement restore path.
async fn donate_to_three_pool(pool_canister: Principal, amount_e8s: u64) -> Result<(), u64> {
    // 1. Mint icUSD directly to the 3pool canister
    let icusd = ICUSD::from(amount_e8s);
    match crate::management::mint_icusd(icusd, pool_canister).await {
        Ok(block_index) => {
            log!(INFO, "[treasury] Minted {} icUSD to 3pool for donation (block {})", amount_e8s, block_index);
        }
        Err(e) => {
            log!(INFO, "[treasury] WARNING: 3pool donation mint failed: {:?}", e);
            return Err(amount_e8s);
        }
    }

    // 2. Call receive_donation(0, amount) so 3pool updates internal balances
    let donate_amount = candid::Nat::from(amount_e8s);
    let result: Result<(Result<(), ThreePoolDonateError>,), _> =
        ic_cdk::call(pool_canister, "receive_donation", (0u8, donate_amount)).await;
    match result {
        Ok((Ok(()),)) => {
            log!(INFO, "[treasury] 3pool acknowledged donation of {} icUSD", amount_e8s);
        }
        Ok((Err(e),)) => {
            log!(INFO, "[treasury] WARNING: 3pool receive_donation returned error: {:?}", e);
        }
        Err((code, msg)) => {
            log!(INFO, "[treasury] WARNING: 3pool receive_donation call failed: {:?} {}", code, msg);
        }
    }
    Ok(())
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
///
/// Wave-8e LIQ-005: routes a configurable fraction of the fee to deficit
/// repayment first via `plan_fee_routing`. The "repayment" is supply-
/// conserving — we mint `to_remainder` instead of the full `fee`, so the
/// skipped `to_repay` mint is the foregone-revenue that pays down the
/// deficit. No separate ledger op is required.
pub async fn mint_borrowing_fee_to_treasury(fee: ICUSD) {
    if fee.0 == 0 {
        return;
    }
    let outcome = crate::state::mutate_state(|s| {
        plan_fee_routing(s, fee, crate::event::FeeSource::BorrowingFee)
    });
    if outcome.to_remainder.0 == 0 {
        log!(
            INFO,
            "[treasury] Borrowing fee {} fully routed to deficit repayment (no treasury mint)",
            fee.to_u64()
        );
        return;
    }
    let treasury = read_state(|s| s.treasury_principal);
    if let Some(tp) = treasury {
        match management::mint_icusd(outcome.to_remainder, tp).await {
            Ok(block_index) => {
                if outcome.to_repay.0 > 0 {
                    log!(
                        INFO,
                        "[treasury] Minted {} icUSD borrowing fee (deficit repay {}, block {})",
                        outcome.to_remainder.to_u64(),
                        outcome.to_repay.to_u64(),
                        block_index
                    );
                } else {
                    log!(
                        INFO,
                        "[treasury] Minted {} icUSD borrowing fee (block {})",
                        outcome.to_remainder.to_u64(),
                        block_index
                    );
                }
                let _ = notify_treasury_deposit(
                    tp,
                    DepositType::BorrowingFee,
                    AssetType::ICUSD,
                    outcome.to_remainder.to_u64(),
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
///
/// Audit INT-002: uses the snapshot-then-decrement pattern so a mint
/// failure inside `distribute_interest` re-queues the unminted portion
/// via `restore_pending_interest_for_pool` (saturating_add). Concurrent
/// harvest credits that land during the await accumulate against zero
/// rather than against the old snapshot, so neither side is silently
/// overwritten.
pub async fn flush_pending_interest() {
    let (pending, threshold) = read_state(|s| {
        (s.pending_interest_for_pools.clone(), s.interest_flush_threshold_e8s)
    });

    for (collateral_type, amount) in pending {
        if amount < threshold {
            continue;
        }
        log!(
            INFO,
            "[treasury] Flushing {} icUSD interest for collateral {}",
            amount,
            collateral_type
        );
        // Atomic snapshot+take. The actual amount taken may be larger
        // than the cloned `amount` if a concurrent harvest landed
        // between the clone and this mutate; we mint what is currently
        // in the bucket regardless.
        let snapshot_e8s = crate::state::mutate_state(|s| {
            s.take_pending_interest_for_pool(collateral_type)
        });
        if snapshot_e8s == 0 {
            continue;
        }
        let unminted = distribute_interest(
            ICUSD::from(snapshot_e8s),
            collateral_type,
        )
        .await;
        if unminted.0 > 0 {
            crate::state::mutate_state(|s| {
                s.restore_pending_interest_for_pool(collateral_type, unminted.to_u64());
            });
            log!(
                INFO,
                "[treasury] CRITICAL: re-queued {} icUSD for collateral {} after partial mint failure (snapshot {})",
                unminted.to_u64(),
                collateral_type,
                snapshot_e8s,
            );
        }
    }
}

/// Mint pending treasury interest accumulated from sync liquidations.
/// Called from the XRC timer tick to drain `pending_treasury_interest`.
///
/// Audit INT-006: uses the snapshot-then-decrement pattern so a
/// concurrent credit landing during the await is preserved on both
/// arms. The pre-await `take` zeroes the field so any concurrent
/// increment accumulates against zero; on mint failure the snapshot is
/// restored via `saturating_add`, merging with whatever landed.
pub async fn drain_pending_treasury_interest() {
    let treasury = read_state(|s| s.treasury_principal);
    let Some(tp) = treasury else {
        return;
    };
    // Atomic snapshot+zero before the await.
    let snapshot = crate::state::mutate_state(|s| s.take_pending_treasury_interest());
    if snapshot.0 == 0 {
        return;
    }
    match management::mint_icusd(snapshot, tp).await {
        Ok(block_index) => {
            log!(
                INFO,
                "[treasury] Drained {} pending interest (block {})",
                snapshot.to_u64(),
                block_index
            );
            let _ = notify_treasury_deposit(
                tp,
                DepositType::InterestRevenue,
                AssetType::ICUSD,
                snapshot.to_u64(),
                block_index,
            )
            .await;
        }
        Err(e) => {
            crate::state::mutate_state(|s| s.restore_pending_treasury_interest(snapshot));
            log!(
                INFO,
                "[treasury] CRITICAL: pending interest drain failed, re-queued {} icUSD: {:?}",
                snapshot.to_u64(),
                e
            );
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
