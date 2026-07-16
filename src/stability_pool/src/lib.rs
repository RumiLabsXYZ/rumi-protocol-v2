use candid::Principal;
use ic_canister_log::log;
use ic_cdk::{init, post_upgrade, pre_upgrade, query, update};
use std::collections::BTreeMap;
use std::time::Duration;

pub mod deposits;
pub mod liquidation;
pub mod logs;
pub mod pool_guard;
pub mod state;
pub mod types;

use crate::logs::INFO;
use crate::state::{mutate_state, read_state};
use crate::types::*;

const CHAIN_ABSORB_AUTO_TIMER_POLL_SECONDS: u64 = 60;
const UNALLOCATED_INTEREST_FORWARD_RETRY_SECONDS: u64 = 60;
/// How often the pool reconciles its tracked aggregate against live ledger
/// balances and logs any shortfall. Hourly: a handful of balance queries, so
/// negligible cycle cost, while still surfacing drift long before it can trip a
/// depositor's withdrawal.
const LEDGER_RECONCILIATION_CHECK_SECONDS: u64 = 3600;

pub(crate) fn pool_balance_mutation_blocked() -> bool {
    crate::pool_guard::liquidation_in_progress() || read_state(|s| s.has_pending_pool_absorbs())
}

pub(crate) fn ensure_pool_balance_mutation_allowed() -> Result<(), StabilityPoolError> {
    if pool_balance_mutation_blocked() {
        return Err(StabilityPoolError::SystemBusy);
    }
    Ok(())
}

pub(crate) fn ensure_no_pool_balance_async_in_flight() -> Result<(), StabilityPoolError> {
    if crate::pool_guard::balance_async_in_flight() {
        return Err(StabilityPoolError::SystemBusy);
    }
    Ok(())
}

// ─── Init / Upgrade ───

#[init]
fn init(args: StabilityPoolInitArgs) {
    // UPG-006: refuse to init with non-empty stable memory. Catches accidental
    // reinstalls of a canister that already has persisted state. Reinstall mode
    // wipes stable memory before init runs (per IC spec), so this primarily
    // documents intent and guards against future IC behavior changes.
    assert!(
        ic_cdk::api::stable::stable64_size() == 0,
        "refusing to init: stable memory non-empty; use upgrade mode not reinstall"
    );
    mutate_state(|s| s.initialize(args));
    log!(
        INFO,
        "Stability Pool initialized. Protocol: {}",
        read_state(|s| s.protocol_canister_id)
    );
    ic_cdk_timers::set_timer(Duration::ZERO, || {
        setup_virtual_price_timer();
        setup_chain_absorb_auto_timer();
        setup_unallocated_interest_forward_retry_timer();
        setup_ledger_reconciliation_timer();
    });
}

#[pre_upgrade]
fn pre_upgrade() {
    log!(
        INFO,
        "Stability Pool pre-upgrade: saving state to stable memory"
    );
    state::save_to_stable_memory();
}

#[post_upgrade]
fn post_upgrade(_args: StabilityPoolInitArgs) {
    state::load_from_stable_memory();
    log!(
        INFO,
        "Stability Pool post-upgrade: state restored. {} depositors, {} liquidations",
        read_state(|s| s.deposits.len()),
        read_state(|s| s.total_liquidations_executed)
    );

    if let Err(error) = read_state(|s| s.validate_state()) {
        ic_cdk::trap(&format!("State validation failed after upgrade: {}", error));
    }

    let corrected_fees = mutate_state(|s| s.normalize_registered_stablecoin_transfer_fees());
    log!(
        INFO,
        "Migration: normalized {} stablecoin transfer fee values",
        corrected_fees
    );

    // Defer timer setup to avoid ic0_call_new restriction during upgrade
    ic_cdk_timers::set_timer(Duration::ZERO, || {
        setup_virtual_price_timer();
        setup_chain_absorb_auto_timer();
        setup_unallocated_interest_forward_retry_timer();
        setup_ledger_reconciliation_timer();
    });
}

// ─── Virtual Price Timer ───

fn setup_virtual_price_timer() {
    // Fetch immediately on startup, then every 5 minutes.
    ic_cdk::spawn(fetch_virtual_prices());
    ic_cdk_timers::set_timer_interval(Duration::from_secs(300), || {
        ic_cdk::spawn(fetch_virtual_prices());
    });
}

fn setup_chain_absorb_auto_timer() {
    ic_cdk_timers::set_timer_interval(
        Duration::from_secs(CHAIN_ABSORB_AUTO_TIMER_POLL_SECONDS),
        || {
            ic_cdk::spawn(async {
                if let Err(error) = crate::liquidation::run_chain_absorb_auto_tick().await {
                    log!(INFO, "chain absorb auto tick skipped: {:?}", error);
                }
            });
        },
    );
}

/// A successful backend notification only means the SP has durably received
/// the source receipt. This timer advances/retries the receipt so concurrent
/// notifications and temporary ledger/treasury failures cannot strand funds.
fn setup_unallocated_interest_forward_retry_timer() {
    ic_cdk_timers::set_timer_interval(
        Duration::from_secs(UNALLOCATED_INTEREST_FORWARD_RETRY_SECONDS),
        || {
            ic_cdk::spawn(async {
                let next = read_state(|s| {
                    s.pending_unallocated_interest_forwards()
                        .into_iter()
                        .map(|batch| batch.id)
                        .next()
                });
                if let Some(batch_id) = next {
                    if let Err(error) = process_unallocated_interest_forward(batch_id).await {
                        log!(
                            INFO,
                            "unallocated interest forward {} still pending: {:?}",
                            batch_id,
                            error
                        );
                    }
                }
            });
        },
    );
}

async fn fetch_virtual_prices() {
    let lp_configs: Vec<(Principal, Principal)> = read_state(|s| {
        s.stablecoin_registry
            .iter()
            .filter(|(_, c)| c.is_lp_token.unwrap_or(false))
            .filter_map(|(ledger, c)| c.underlying_pool.map(|pool| (*ledger, pool)))
            .collect()
    });

    for (lp_ledger, pool_canister) in lp_configs {
        let result: Result<(ThreePoolStatus,), _> =
            ic_cdk::call(pool_canister, "get_pool_status", ()).await;

        match result {
            Ok((status,)) => {
                mutate_state(|s| {
                    s.cached_virtual_prices
                        .get_or_insert_with(BTreeMap::new)
                        .insert(lp_ledger, status.virtual_price);
                });
            }
            Err(e) => {
                log!(
                    INFO,
                    "Failed to fetch virtual price from {}: {:?}",
                    pool_canister,
                    e
                );
            }
        }
    }
}

// ─── Deposit / Withdraw / Claim ───

#[update]
pub async fn deposit(token_ledger: Principal, amount: u64) -> Result<(), StabilityPoolError> {
    crate::deposits::deposit(token_ledger, amount).await
}

#[update]
pub async fn withdraw(token_ledger: Principal, amount: u64) -> Result<(), StabilityPoolError> {
    crate::deposits::withdraw(token_ledger, amount).await
}

#[update]
pub async fn claim_collateral(collateral_ledger: Principal) -> Result<u64, StabilityPoolError> {
    crate::deposits::claim_collateral(collateral_ledger).await
}

#[update]
pub async fn claim_all_collateral() -> Result<BTreeMap<Principal, u64>, StabilityPoolError> {
    crate::deposits::claim_all_collateral().await
}

/// Convenience: deposit a stablecoin (icUSD, ckUSDT, ckUSDC) and have the pool
/// mint 3USD on the user's behalf by depositing into the 3pool.
#[update]
pub async fn deposit_as_3usd(
    token_ledger: Principal,
    amount: u64,
) -> Result<u64, StabilityPoolError> {
    crate::deposits::deposit_as_3usd(token_ledger, amount).await
}

/// Recover tokens the pool owes after a failed `deposit_as_3usd` refund
/// (audit IC-S-001). Callable by the original user or a pool admin. Returns
/// the net amount sent (gross minus the ledger transfer fee).
#[update]
pub async fn claim_pending_refund(refund_id: u64) -> Result<u64, StabilityPoolError> {
    crate::deposits::claim_pending_refund(refund_id).await
}

#[update]
pub async fn claim_cfx(
    chain_sentinel: Principal,
    dest_evm: String,
) -> Result<u128, StabilityPoolError> {
    crate::liquidation::claim_cfx(chain_sentinel, dest_evm).await
}

#[update]
pub fn recredit_failed_cfx_claim_payout(
    recovery: CfxClaimPayoutRecovery,
) -> Result<bool, StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    let expected = read_state(|s| s.protocol_canister_id);
    if caller != expected {
        return Err(StabilityPoolError::Unauthorized);
    }
    let now = ic_cdk::api::time();
    mutate_state(|s| {
        crate::liquidation::recredit_failed_cfx_claim_payout_in_state_at(s, recovery, now)
    })
}

// ─── Opt-in / Opt-out ───

#[update]
pub fn opt_out_collateral(collateral_type: Principal) -> Result<(), StabilityPoolError> {
    // SP-102 / AR-S-002: opt-in/out changes the apportionment denominator, so
    // it must not land between a liquidation's snapshot and its apportionment
    // (escape-the-burn + aggregate drift above the ledger balance).
    // `pool_balance_mutation_blocked` includes `liquidation_in_progress`.
    if pool_balance_mutation_blocked() {
        return Err(StabilityPoolError::SystemBusy);
    }
    let caller = ic_cdk::api::caller();
    let result = mutate_state(|s| s.opt_out_collateral(&caller, collateral_type));
    if result.is_ok() {
        mutate_state(|s| s.push_event(caller, PoolEventType::OptOutCollateral { collateral_type }));
    }
    result
}

#[update]
pub fn opt_in_collateral(collateral_type: Principal) -> Result<(), StabilityPoolError> {
    // SP-102 / AR-S-002: see opt_out_collateral.
    // `pool_balance_mutation_blocked` includes `liquidation_in_progress`.
    if pool_balance_mutation_blocked() {
        return Err(StabilityPoolError::SystemBusy);
    }
    let caller = ic_cdk::api::caller();
    let result = mutate_state(|s| s.opt_in_collateral(&caller, collateral_type));
    if result.is_ok() {
        mutate_state(|s| s.push_event(caller, PoolEventType::OptInCollateral { collateral_type }));
    }
    result
}

#[update]
pub fn opt_in_cfx(chain_sentinel: Principal) -> Result<(), StabilityPoolError> {
    // SP-102 / AR-S-002: see opt_out_collateral.
    if pool_balance_mutation_blocked() {
        return Err(StabilityPoolError::SystemBusy);
    }
    let caller = ic_cdk::api::caller();
    let result = mutate_state(|s| s.opt_in_cfx(&caller, chain_sentinel));
    if result.is_ok() {
        mutate_state(|s| {
            s.push_event(
                caller,
                PoolEventType::OptInCollateral {
                    collateral_type: chain_sentinel,
                },
            )
        });
    }
    result
}

#[update]
pub fn opt_out_cfx(chain_sentinel: Principal) -> Result<(), StabilityPoolError> {
    // SP-102 / AR-S-002: see opt_out_collateral.
    if pool_balance_mutation_blocked() {
        return Err(StabilityPoolError::SystemBusy);
    }
    let caller = ic_cdk::api::caller();
    let result = mutate_state(|s| s.opt_out_cfx(&caller, chain_sentinel));
    if result.is_ok() {
        mutate_state(|s| {
            s.push_event(
                caller,
                PoolEventType::OptOutCollateral {
                    collateral_type: chain_sentinel,
                },
            )
        });
    }
    result
}

#[update]
pub fn opt_in_native_collateral(
    collateral_type: Principal,
    payout_address: String,
) -> Result<(), StabilityPoolError> {
    opt_in_native_collateral_with_tag(collateral_type, payout_address, None)
}

#[update]
pub fn opt_in_native_collateral_with_tag(
    collateral_type: Principal,
    payout_address: String,
    destination_tag: Option<u32>,
) -> Result<(), StabilityPoolError> {
    // SP-102 / AR-S-002: see opt_out_collateral.
    if pool_balance_mutation_blocked() {
        return Err(StabilityPoolError::SystemBusy);
    }
    let caller = ic_cdk::api::caller();
    let result = mutate_state(|s| {
        s.opt_in_native_collateral_with_tag(
            &caller,
            collateral_type,
            payout_address,
            destination_tag,
        )
    });
    if result.is_ok() {
        mutate_state(|s| s.push_event(caller, PoolEventType::OptInCollateral { collateral_type }));
    }
    result
}

#[query]
pub fn get_my_native_xrp_payouts() -> Vec<NativeXrpPendingPayout> {
    let caller = ic_cdk::api::caller();
    read_state(|s| s.native_xrp_pending_payouts_for(&caller))
}

#[query]
pub fn cycles_status() -> rumi_cycle_manager::CycleManagerCyclesStatus {
    let operational = !pool_balance_mutation_blocked();
    rumi_cycle_manager::self_cycles_status(
        3_000_000_000_000,
        operational,
        rumi_cycle_manager::DEFAULT_FREEZE_THRESHOLD_SECS,
    )
}

#[query]
pub fn cycle_manager_metrics() -> Vec<rumi_cycle_manager::CycleManagerMetric> {
    read_state(|s| {
        vec![
            rumi_cycle_manager::metric(
                "op:depositors:count",
                s.deposits.len() as u64,
                s.deposits.len() as u64,
                Some("stability pool depositor records"),
            ),
            rumi_cycle_manager::metric(
                "op:liquidation:count",
                s.total_liquidations_executed,
                s.total_liquidations_executed,
                Some("cumulative stability pool liquidations"),
            ),
            rumi_cycle_manager::metric(
                "op:chain_absorb:count",
                s.pending_chain_absorb_count() as u64,
                s.pending_chain_absorb_count() as u64,
                Some("pending native-chain absorbs"),
            ),
        ]
    })
}

#[update]
pub async fn ack_native_xrp_payout_settled(claim_id: u64) -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    let protocol_canister_id = read_state(|s| {
        s.native_xrp_pending_payout_for(&caller, claim_id)
            .map(|_| s.protocol_canister_id)
    })
    .ok_or(StabilityPoolError::RefundClaimNotFound)?;

    ensure_backend_xrp_claim_absent(protocol_canister_id, claim_id, caller).await?;
    mutate_state(|s| s.ack_native_xrp_payout_settled(&caller, claim_id))
}

async fn ensure_backend_xrp_claim_absent(
    protocol_canister_id: Principal,
    claim_id: u64,
    claimant: Principal,
) -> Result<(), StabilityPoolError> {
    let method = "stability_pool_xrp_claim_outstanding";
    let response: Result<(Result<bool, rumi_protocol_backend::ProtocolError>,), _> =
        ic_cdk::call(protocol_canister_id, method, (claim_id, claimant)).await;

    match response {
        Ok((Ok(false),)) => Ok(()),
        Ok((Ok(true),)) => Err(StabilityPoolError::XrpClaimStillOutstanding { claim_id }),
        Ok((Err(err),)) => Err(StabilityPoolError::XrpClaimStatusCheckFailed {
            reason: format!("{err:?}"),
        }),
        Err((code, message)) => Err(StabilityPoolError::XrpClaimStatusCheckFailed {
            reason: format!("{method} rejected by {protocol_canister_id}: {code:?}: {message}"),
        }),
    }
}

// ─── Liquidation (Push + Fallback) ───

/// Called by the backend to push liquidatable vault notifications.
///
/// Restricted to the registered protocol canister (audit 2026-04-22-28e9896
/// Wave 2, AUTH-001 / SP-004 / DOS-009). Any other caller would otherwise
/// be able to feed fabricated `LiquidatableVaultInfo` entries through the
/// SP's liquidation pipeline (cycle DoS + event-log pollution + interaction
/// with the per-token bookkeeping path), so the gate matches the pattern
/// used by `receive_interest_revenue` below.
#[update]
pub async fn notify_liquidatable_vaults(
    vaults: Vec<LiquidatableVaultInfo>,
) -> Vec<LiquidationResult> {
    let caller = ic_cdk::api::caller();
    let expected = read_state(|s| s.protocol_canister_id);
    if caller != expected {
        log!(
            INFO,
            "notify_liquidatable_vaults: rejected caller {} (expected protocol {})",
            caller,
            expected
        );
        return Vec::new();
    }
    let vault_count = vaults.len() as u64;
    mutate_state(|s| {
        s.push_event(
            caller,
            PoolEventType::LiquidationNotification { vault_count },
        )
    });
    crate::liquidation::notify_liquidatable_vaults(vaults).await
}

/// Public fallback: trigger liquidation for a specific vault.
#[update]
pub async fn execute_liquidation(vault_id: u64) -> Result<LiquidationResult, StabilityPoolError> {
    crate::liquidation::execute_liquidation(vault_id).await
}

#[update]
pub async fn sp_absorb_chain_vault(
    vault_id: u64,
) -> Result<ChainSpAbsorbResult, StabilityPoolError> {
    crate::liquidation::sp_absorb_chain_vault(vault_id).await
}

#[update]
pub async fn scan_chain_absorb_candidates(
    max_per_chain: Option<u64>,
) -> Result<Vec<ChainSpAbsorbCandidate>, StabilityPoolError> {
    crate::liquidation::scan_chain_absorb_candidates(max_per_chain).await
}

#[update]
pub fn set_chain_absorb_auto_config(
    config: ChainAbsorbAutoConfig,
) -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    if caller == Principal::anonymous() {
        return Err(StabilityPoolError::Unauthorized);
    }
    if !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    mutate_state(|s| {
        s.set_chain_absorb_auto_config(config)?;
        s.push_event(caller, PoolEventType::ConfigurationUpdated);
        Ok(())
    })
}

#[query]
pub fn get_chain_absorb_auto_status() -> ChainAbsorbAutoStatus {
    read_state(|s| ChainAbsorbAutoStatus {
        config: s.chain_absorb_auto_config(),
        tick_in_flight: crate::pool_guard::chain_absorb_auto_tick_in_flight(),
        last_tick: s.chain_absorb_auto_last_tick(),
    })
}

// ─── Interest Revenue ───

/// Receive interest revenue from the protocol backend and distribute pro-rata to depositors.
/// Only callable by the protocol canister.
///
/// `collateral_type` identifies which collateral's vault generated the interest.
/// Depositors who opted out of that collateral are excluded from the distribution.
/// The parameter is optional for backward compatibility with older backend versions.
#[update]
pub fn receive_interest_revenue(
    token_ledger: Principal,
    amount: u64,
    collateral_type: Option<Principal>,
) -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    let expected = read_state(|s| s.protocol_canister_id);
    if caller != expected {
        return Err(StabilityPoolError::Unauthorized);
    }
    ensure_pool_balance_mutation_allowed()?;

    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }

    if !read_state(|s| s.stablecoin_registry.contains_key(&token_ledger)) {
        return Err(StabilityPoolError::TokenNotAccepted {
            ledger: token_ledger,
        });
    }

    mutate_state(|s| {
        s.distribute_interest_revenue(token_ledger, amount, collateral_type);
        s.push_event(
            caller,
            PoolEventType::InterestReceived {
                token_ledger,
                amount,
            },
        );
    });

    log!(
        INFO,
        "Distributed {} interest for token {} (collateral: {:?}) from backend",
        amount,
        token_ledger,
        collateral_type
    );
    Ok(())
}

/// V2 interest notification carries the backend mint block, which supplies a
/// durable source receipt for the no-eligible-recipient treasury route. The
/// legacy V1 method above remains available during rollout but deliberately
/// retains its original distribution-only behavior because it lacks that key.
#[update]
pub async fn receive_interest_revenue_v2(
    token_ledger: Principal,
    amount: u64,
    collateral_type: Option<Principal>,
    source_mint_block: u64,
) -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    let expected = read_state(|s| s.protocol_canister_id);
    if caller != expected {
        return Err(StabilityPoolError::Unauthorized);
    }
    ensure_pool_balance_mutation_allowed()?;
    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }
    if !read_state(|s| s.stablecoin_registry.contains_key(&token_ledger)) {
        return Err(StabilityPoolError::TokenNotAccepted {
            ledger: token_ledger,
        });
    }

    if read_state(|s| s.has_eligible_interest_recipient(collateral_type.as_ref())) {
        mutate_state(|s| {
            s.distribute_interest_revenue(token_ledger, amount, collateral_type);
            s.push_event(
                caller,
                PoolEventType::InterestReceived {
                    token_ledger,
                    amount,
                },
            );
        });
        return Ok(());
    }

    let batch_id = mutate_state(|s| {
        s.queue_unallocated_interest_forward(source_mint_block, token_ledger, amount)
    })?;
    process_unallocated_interest_forward(batch_id).await
}

async fn process_unallocated_interest_forward(batch_id: u64) -> Result<(), StabilityPoolError> {
    let _guard = crate::pool_guard::UnallocatedInterestForwardGuard::new()?;
    let batch = read_state(|s| s.unallocated_interest_forward_batch(batch_id))
        .ok_or(StabilityPoolError::RefundClaimNotFound)?;
    if batch.treasury_recorded {
        return Ok(());
    }
    let Some(treasury) = batch.treasury else {
        // The backend notification has a durable receipt, but deployment or
        // operator configuration has not selected a destination yet.
        return Ok(());
    };

    let batch = if batch.transfer_block_index.is_none() && batch.fee.is_none() {
        let fee = deposits::unallocated_interest_transfer_fee(batch.token_ledger).await;
        mutate_state(|s| s.prepare_unallocated_interest_forward(batch_id, fee))?
    } else {
        batch
    };
    let fee = batch
        .fee
        .ok_or_else(|| StabilityPoolError::LedgerTransferFailed {
            reason: "unallocated interest forward missing persisted fee".to_string(),
        })?;
    if batch.gross_amount <= fee {
        // This is durable fee dust. A later no-recipient mint appends to this
        // unattempted batch and automatically carries it over the threshold.
        return Ok(());
    }
    let net_amount = batch.gross_amount - fee;
    let transfer_block_index = match batch.transfer_block_index {
        Some(block) => block,
        None => {
            let created_at = batch.transfer_created_at_ns.ok_or_else(|| {
                StabilityPoolError::LedgerTransferFailed {
                    reason: "unallocated interest forward missing timestamp".to_string(),
                }
            })?;
            let mut memo = b"RUMI-SP-INT-FWD".to_vec();
            memo.extend_from_slice(&batch.id.to_be_bytes());
            match deposits::transfer_unallocated_interest_to_treasury(
                batch.token_ledger,
                treasury,
                net_amount,
                fee,
                created_at,
                memo,
            )
            .await?
            {
                deposits::UnallocatedInterestTransferResult::Sent(block) => {
                    mutate_state(|s| {
                        s.mark_unallocated_interest_forward_transferred(batch_id, block)
                    });
                    block
                }
                deposits::UnallocatedInterestTransferResult::BadFee(expected_fee) => {
                    mutate_state(|s| {
                        s.update_unallocated_interest_forward_fee(batch_id, expected_fee)
                    });
                    return Err(StabilityPoolError::LedgerTransferFailed {
                        reason: "ledger transfer fee changed; retry queued treasury forward"
                            .to_string(),
                    });
                }
                deposits::UnallocatedInterestTransferResult::TooOld => {
                    mutate_state(|s| {
                        s.record_unallocated_interest_forward_error(
                            batch_id,
                            "ICRC dedup window expired; verify the ledger transfer and confirm its block before retrying".to_string(),
                        )
                    });
                    return Err(StabilityPoolError::LedgerTransferFailed {
                        reason: "unallocated interest transfer is too old; reconciliation required"
                            .to_string(),
                    });
                }
            }
        }
    };

    let (result,): (Result<u64, String>,) = ic_cdk::call(
        treasury,
        "record_stability_pool_unallocated_interest",
        (
            net_amount,
            transfer_block_index,
            batch.source_mint_blocks.clone(),
        ),
    )
    .await
    .map_err(|_| StabilityPoolError::InterCanisterCallFailed {
        target: format!("{}", treasury),
        method: "record_stability_pool_unallocated_interest".to_string(),
    })?;
    result.map_err(|reason| StabilityPoolError::LedgerTransferFailed { reason })?;

    mutate_state(|s| {
        s.mark_unallocated_interest_forward_recorded(batch_id);
    });
    Ok(())
}

// ─── Queries ───

#[query]
pub fn get_pool_status() -> StabilityPoolStatus {
    read_state(|s| s.get_pool_status())
}

#[query]
pub fn get_user_position(user: Option<Principal>) -> Option<UserStabilityPosition> {
    let target = user.unwrap_or_else(ic_cdk::api::caller);
    read_state(|s| s.get_user_position(&target))
}

#[query]
pub fn get_liquidation_history(limit: Option<u64>) -> Vec<PoolLiquidationRecord> {
    let limit = limit.unwrap_or(50).min(100) as usize;
    read_state(|s| {
        s.liquidation_history
            .iter()
            .rev()
            .take(limit)
            .cloned()
            .collect()
    })
}

/// Server-side cap on `length` for `get_pool_events`. Audit Wave 9a
/// (DOS-008): without this cap a caller could pass `length = u64::MAX`
/// and force the canister to slice up to the full pool-event log on a
/// single query — the same cycle-DoS pattern fixed for the backend's
/// `get_events`.
pub const MAX_POOL_EVENTS_PAGE: u64 = 500;

/// Pure paging helper for `get_pool_events`. Clamps `length` to
/// `MAX_POOL_EVENTS_PAGE` before slicing so a single call's reply size
/// is bounded regardless of caller input. Extracted from the `#[query]`
/// wrapper so the audit fence can exercise the clamp without spinning
/// up a canister fixture.
pub fn pool_events_page(events: &[PoolEvent], start: u64, length: u64) -> Vec<PoolEvent> {
    let length = length.min(MAX_POOL_EVENTS_PAGE);
    let total = events.len() as u64;
    if start >= total {
        return Vec::new();
    }
    let end = (start + length).min(total) as usize;
    events[start as usize..end].to_vec()
}

/// Paginated pool-event log. `length` is server-side clamped via
/// `pool_events_page` so a caller cannot request the entire log in a
/// single call, regardless of input. Audit Wave 9a (DOS-008).
#[query]
pub fn get_pool_events(start: u64, length: u64) -> Vec<PoolEvent> {
    read_state(|s| pool_events_page(s.pool_events(), start, length))
}

#[query]
pub fn get_pool_event_count() -> u64 {
    read_state(|s| s.pool_event_count())
}

/// Enumerate every principal currently holding a deposit. The frontend's
/// "Current depositors" card needs this because the analytics shadow log
/// (`evt_stability`) misses depositors whose Deposit events predate the
/// analytics tailer (or were dropped while the tailer was decoding broken
/// shadow types). Pool's `deposits` map is the source of truth — its keys
/// equal `total_depositors` from `get_pool_status`.
#[query]
pub fn list_depositor_principals() -> Vec<Principal> {
    read_state(|s| s.deposits.keys().copied().collect())
}

/// Outstanding failed-refund records for `user` (defaults to the caller).
/// Audit IC-S-001: pairs with `claim_pending_refund`.
#[query]
pub fn get_pending_refunds(user: Option<Principal>) -> Vec<PendingRefund> {
    let target = user.unwrap_or_else(ic_cdk::api::caller);
    read_state(|s| s.pending_refunds_for(&target))
}

/// Durable no-recipient interest routes that still need a ledger transfer or
/// treasury bookkeeping acknowledgement. Public for transparent operations.
#[query]
pub fn get_pending_unallocated_interest_forwards() -> Vec<UnallocatedInterestForwardBatch> {
    read_state(|s| s.pending_unallocated_interest_forwards())
}

#[query]
pub fn get_pending_chain_absorbs() -> Vec<ChainSpAbsorbIntent> {
    read_state(|s| s.pending_chain_absorbs())
}

#[query]
pub fn get_completed_chain_absorbs(limit: Option<u64>) -> Vec<ChainSpAbsorbCompletion> {
    let limit = limit.unwrap_or(50).min(500) as usize;
    read_state(|s| s.completed_chain_absorbs(limit))
}

#[query]
pub fn get_chain_collateral_sentinel(chain_id: u32) -> Principal {
    crate::state::chain_collateral_sentinel(chain_id)
}

#[query]
pub fn check_pool_capacity(collateral_type: Principal, debt_amount_e8s: u64) -> bool {
    read_state(|s| s.effective_pool_for_collateral(&collateral_type) >= debt_amount_e8s)
}

#[query]
pub fn check_chain_absorb_capacity(chain_sentinel: Principal, debt_amount_e8s: u64) -> bool {
    read_state(|s| {
        s.is_chain_collateral_sentinel(&chain_sentinel)
            && s.effective_icusd_pool_for_collateral(&chain_sentinel) >= debt_amount_e8s
    })
}

#[query]
pub fn validate_pool_state() -> Result<String, String> {
    read_state(|s| {
        s.validate_state()
            .map(|_| "Pool state is consistent".to_string())
    })
}

/// Compare each registered stablecoin's tracked aggregate against its live
/// on-ledger balance. Read-only: queries every ledger and returns the deltas,
/// never mutates state. Ledgers whose balance query fails are omitted (logged)
/// rather than reported as a false shortfall.
async fn compute_ledger_reconciliation() -> Vec<LedgerReconciliationEntry> {
    let tokens: Vec<(Principal, String)> = read_state(|s| {
        s.stablecoin_registry
            .iter()
            .map(|(ledger, config)| (*ledger, config.symbol.clone()))
            .collect()
    });

    let mut entries = Vec::with_capacity(tokens.len());
    for (ledger, symbol) in tokens {
        let Some(live_e8s) = crate::deposits::ledger_pool_balance(ledger).await else {
            continue;
        };
        let recorded_e8s = read_state(|s| {
            s.total_stablecoin_balances
                .get(&ledger)
                .copied()
                .unwrap_or(0)
        });
        entries.push(LedgerReconciliationEntry {
            ledger,
            symbol,
            recorded_e8s,
            live_e8s,
            delta_e8s: live_e8s as i64 - recorded_e8s as i64,
            healthy: live_e8s >= recorded_e8s,
        });
    }
    entries
}

/// Admin-only, read-only ledger reconciliation. Reveals, per stablecoin, the
/// tracked aggregate vs. the live ledger balance so a shortfall (books above
/// ledger) can be spotted and remediated (top up the pool, or `admin_correct_balance`)
/// before it blocks withdrawals. Admin-gated because it triggers one
/// inter-canister balance query per token.
#[update]
pub async fn get_ledger_reconciliation() -> Result<Vec<LedgerReconciliationEntry>, StabilityPoolError>
{
    let caller = ic_cdk::api::caller();
    if !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    Ok(compute_ledger_reconciliation().await)
}

/// Periodic self-check: reconcile tracked aggregates against live ledger
/// balances and log any shortfall so operators are alerted proactively. Never
/// auto-corrects — writing books down to a transient in-flight balance (e.g.
/// mid-liquidation) would socialize a phantom loss; remediation stays a
/// deliberate admin action.
fn setup_ledger_reconciliation_timer() {
    ic_cdk_timers::set_timer_interval(
        Duration::from_secs(LEDGER_RECONCILIATION_CHECK_SECONDS),
        || {
            ic_cdk::spawn(async {
                for entry in compute_ledger_reconciliation().await {
                    if !entry.healthy {
                        log!(
                            INFO,
                            "[ledger-reconciliation] SHORTFALL {} ({}): live {} < recorded {} (delta {})",
                            entry.symbol,
                            entry.ledger,
                            entry.live_e8s,
                            entry.recorded_e8s,
                            entry.delta_e8s
                        );
                    }
                }
            });
        },
    );
}

// ─── Admin: Registry Management ───

#[update]
pub fn register_stablecoin(config: StablecoinConfig) -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    if !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    let ledger = config.ledger_id;
    let symbol = config.symbol.clone();
    mutate_state(|s| {
        s.register_stablecoin(config);
        s.push_event(
            caller,
            PoolEventType::StablecoinRegistered { ledger, symbol },
        );
    });
    Ok(())
}

#[update]
pub fn register_collateral(info: CollateralInfo) -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    if !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    let ledger = info.ledger_id;
    let symbol = info.symbol.clone();
    mutate_state(|s| {
        s.register_collateral(info);
        s.push_event(
            caller,
            PoolEventType::CollateralRegistered { ledger, symbol },
        );
    });
    Ok(())
}

#[update]
pub fn register_cfx_collateral(chain_id: u32) -> Result<Principal, StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    if !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    let sentinel = mutate_state(|s| s.register_chain_collateral(chain_id, "CFX".to_string(), 18))?;
    mutate_state(|s| {
        s.push_event(
            caller,
            PoolEventType::CollateralRegistered {
                ledger: sentinel,
                symbol: "CFX".to_string(),
            },
        );
    });
    Ok(sentinel)
}

// ─── Admin: Configuration ───

#[update]
pub fn update_pool_configuration(new_config: PoolConfiguration) -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    if !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    mutate_state(|s| {
        s.configuration = new_config;
        s.push_event(caller, PoolEventType::ConfigurationUpdated);
    });
    Ok(())
}

// ─── ICRC-21: Canister Call Consent Messages ───

#[update]
pub fn icrc21_canister_call_consent_message(
    request: Icrc21ConsentMessageRequest,
) -> Icrc21ConsentMessageResponse {
    let message_text = match request.method.as_str() {
        "deposit" => {
            match candid::decode_args::<(Principal, u64)>(&request.arg) {
                Ok((token_ledger, amount)) => {
                    let (symbol, decimals) = read_state(|s| {
                        s.stablecoin_registry
                            .get(&token_ledger)
                            .map(|c| (c.symbol.clone(), c.decimals))
                            .unwrap_or_else(|| (format!("token {}", token_ledger), 8))
                    });
                    let formatted = format_token_amount(amount, decimals);
                    format!(
                        "## Deposit to Stability Pool\n\n\
                         You are depositing **{} {}** into the Rumi Protocol Stability Pool.\n\n\
                         Your deposit earns liquidation rewards proportional to your share of the pool.",
                        formatted, symbol
                    )
                }
                Err(_) => "Deposit stablecoins into the Rumi Protocol Stability Pool.".to_string(),
            }
        }
        "withdraw" => {
            match candid::decode_args::<(Principal, u64)>(&request.arg) {
                Ok((token_ledger, amount)) => {
                    let (symbol, decimals, fee) = read_state(|s| {
                        let Some(config) = s.stablecoin_registry.get(&token_ledger) else {
                            return (format!("token {}", token_ledger), 8, 0);
                        };
                        let known_fee = crate::state::known_stablecoin_transfer_fee(
                            &config.symbol,
                            config.decimals,
                        )
                        .unwrap_or(0);
                        (
                            config.symbol.clone(),
                            config.decimals,
                            config.transfer_fee.unwrap_or(0).max(known_fee),
                        )
                    });
                    let gross_formatted = format_token_amount(amount, decimals);
                    let net_amount = amount.saturating_sub(fee);
                    let net_formatted = format_token_amount(net_amount, decimals);
                    format!(
                        "## Withdraw from Stability Pool\n\n\
                         You are withdrawing **{} {}** from your Rumi Protocol Stability Pool position. \
                         After the ledger transfer fee, you receive **{} {}**.",
                        gross_formatted, symbol, net_formatted, symbol
                    )
                }
                Err(_) => "Withdraw stablecoins from the Rumi Protocol Stability Pool.".to_string(),
            }
        }
        "claim_collateral" => {
            match candid::decode_args::<(Principal,)>(&request.arg) {
                Ok((collateral_ledger,)) => {
                    let symbol = read_state(|s| {
                        s.collateral_registry
                            .get(&collateral_ledger)
                            .map(|c| c.symbol.clone())
                            .unwrap_or_else(|| format!("collateral {}", collateral_ledger))
                    });
                    format!(
                        "## Claim Collateral Rewards\n\n\
                         You are claiming your **{}** collateral rewards from the Rumi Protocol Stability Pool.",
                        symbol
                    )
                }
                Err(_) => "Claim collateral rewards from the Rumi Protocol Stability Pool.".to_string(),
            }
        }
        "claim_all_collateral" => {
            "## Claim All Collateral Rewards\n\n\
             You are claiming **all** of your collateral rewards from the Rumi Protocol Stability Pool."
                .to_string()
        }
        "claim_cfx" => {
            "## Claim CFX Rewards\n\n\
             You are claiming CFX rewards from chain-vault liquidations."
                .to_string()
        }
        "opt_out_collateral" => {
            "## Opt Out of Collateral\n\n\
             You are opting out of receiving a specific collateral type from future liquidations."
                .to_string()
        }
        "opt_in_collateral" => {
            "## Opt In to Collateral\n\n\
             You are opting back in to receiving a specific collateral type from future liquidations."
                .to_string()
        }
        "opt_in_cfx" => {
            "## Opt In to CFX\n\n\
             You are opting in to receiving CFX from future chain-vault liquidations."
                .to_string()
        }
        "opt_out_cfx" => {
            "## Opt Out of CFX\n\n\
             You are opting out of receiving CFX from future chain-vault liquidations."
                .to_string()
        }
        "opt_in_native_collateral" => {
            match candid::decode_args::<(Principal, String)>(&request.arg) {
                Ok((collateral_ledger, payout_address)) => {
                    let symbol = read_state(|s| {
                        s.collateral_registry
                            .get(&collateral_ledger)
                            .map(|c| c.symbol.clone())
                            .unwrap_or_else(|| format!("collateral {}", collateral_ledger))
                    });
                    format!(
                        "## Opt In to Native Collateral\n\n\
                         You are opting in to receive **{}** from future liquidations. \
                         Payouts will be sent to XRP Ledger address `{}`.",
                        symbol, payout_address
                    )
                }
                Err(_) => {
                    "Opt in to native collateral liquidations with a payout address.".to_string()
                }
            }
        }
        "opt_in_native_collateral_with_tag" => {
            match candid::decode_args::<(Principal, String, Option<u32>)>(&request.arg) {
                Ok((collateral_ledger, payout_address, destination_tag)) => {
                    let symbol = read_state(|s| {
                        s.collateral_registry
                            .get(&collateral_ledger)
                            .map(|c| c.symbol.clone())
                            .unwrap_or_else(|| format!("collateral {}", collateral_ledger))
                    });
                    let tag_text = destination_tag
                        .map(|tag| format!(" with destination tag `{}`", tag))
                        .unwrap_or_default();
                    format!(
                        "## Opt In to Native Collateral\n\n\
                         You are opting in to receive **{}** from future liquidations. \
                         Payouts will be sent to XRP Ledger address `{}`{}.",
                        symbol, payout_address, tag_text
                    )
                }
                Err(_) => {
                    "Opt in to native collateral liquidations with a payout address and optional destination tag."
                        .to_string()
                }
            }
        }
        "ack_native_xrp_payout_settled" => {
            "## Clear Settled XRP Payout\n\n\
             You are clearing a settled native XRP payout reminder from the Stability Pool."
                .to_string()
        }
        "deposit_as_3usd" => {
            match candid::decode_args::<(Principal, u64)>(&request.arg) {
                Ok((token_ledger, amount)) => {
                    let (symbol, decimals) = read_state(|s| {
                        s.stablecoin_registry
                            .get(&token_ledger)
                            .map(|c| (c.symbol.clone(), c.decimals))
                            .unwrap_or_else(|| (format!("token {}", token_ledger), 8))
                    });
                    let formatted = format_token_amount(amount, decimals);
                    format!(
                        "## Deposit as 3USD\n\n\
                         You are depositing **{} {}** into the Rumi Protocol Stability Pool \
                         via the 3pool. Your tokens will be converted to 3USD LP tokens, \
                         which earn swap fees while backing liquidations.",
                        formatted, symbol
                    )
                }
                Err(_) => "Deposit stablecoins into the Stability Pool via the 3pool as 3USD LP tokens.".to_string(),
            }
        }
        _ => {
            return Icrc21ConsentMessageResponse::Err(Icrc21Error::UnsupportedCanisterCall(
                Icrc21ErrorInfo {
                    description: format!(
                        "Method '{}' is not a supported user-facing call.",
                        request.method
                    ),
                },
            ));
        }
    };

    Icrc21ConsentMessageResponse::Ok(Icrc21ConsentInfo {
        consent_message: Icrc21ConsentMessage::GenericDisplayMessage(message_text),
        metadata: Icrc21ConsentMessageResponseMetadata {
            language: request.user_preferences.metadata.language.clone(),
            utc_offset_minutes: request.user_preferences.metadata.utc_offset_minutes,
        },
    })
}

// ─── ICRC-10: Supported Standards ───

#[query]
pub fn icrc10_supported_standards() -> Vec<Icrc10SupportedStandard> {
    vec![
        Icrc10SupportedStandard {
            name: "ICRC-21".to_string(),
            url: "https://github.com/dfinity/ICRC/blob/main/ICRCs/ICRC-21/ICRC-21.md".to_string(),
        },
        Icrc10SupportedStandard {
            name: "ICRC-10".to_string(),
            url: "https://github.com/dfinity/ICRC/blob/main/ICRCs/ICRC-10/ICRC-10.md".to_string(),
        },
    ]
}

/// Format a token amount in native units as a human-readable string.
fn format_token_amount(amount: u64, decimals: u8) -> String {
    let divisor = 10u64.checked_pow(decimals as u32).unwrap_or(100_000_000);
    let whole = amount / divisor;
    let frac = amount % divisor;
    if frac == 0 {
        format!("{}", whole)
    } else {
        let frac_str = format!("{:0width$}", frac, width = decimals as usize);
        let trimmed = frac_str.trim_end_matches('0');
        format!("{}.{}", whole, trimmed)
    }
}

// ─── Admin: Configuration ───

#[update]
pub fn emergency_pause() -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    if !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    mutate_state(|s| {
        s.configuration.emergency_pause = true;
        s.push_event(caller, PoolEventType::EmergencyPauseActivated);
    });
    log!(INFO, "Emergency pause activated by {}", caller);
    Ok(())
}

#[update]
pub fn resume_operations() -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    if !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    mutate_state(|s| {
        s.configuration.emergency_pause = false;
        s.push_event(caller, PoolEventType::OperationsResumed);
    });
    log!(INFO, "Operations resumed by {}", caller);
    Ok(())
}

/// Set the sole treasury destination for interest which cannot be credited to
/// an opted-in icUSD depositor. Destination changes are rejected while any
/// route is unsettled, so a persisted receipt can never be retargeted.
#[update]
pub fn set_interest_treasury(treasury: Option<Principal>) -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    if !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    mutate_state(|s| {
        s.set_interest_treasury(treasury)?;
        s.push_event(caller, PoolEventType::ConfigurationUpdated);
        Ok(())
    })
}

/// Retry an individual durable treasury forward. The original ledger transfer
/// timestamp/memo is reused, so a retry after an ambiguous response is safe.
#[update]
pub async fn retry_unallocated_interest_forward(batch_id: u64) -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    if !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    process_unallocated_interest_forward(batch_id).await
}

/// Admin reconciliation after an ICRC-003 window has expired: the caller must
/// first verify the immutable ledger transaction externally, then supplies its
/// block so the receipt can finish treasury bookkeeping without a second send.
#[update]
pub async fn confirm_unallocated_interest_forward_transfer(
    batch_id: u64,
    transfer_block_index: u64,
) -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    if !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    mutate_state(|s| {
        s.mark_unallocated_interest_forward_transferred(batch_id, transfer_block_index)
    });
    process_unallocated_interest_forward(batch_id).await
}

/// Admin: correct a depositor's stablecoin balance to match actual ledger state.
/// Use when internal state tracks tokens that were never actually transferred on-chain.
#[update]
pub fn admin_correct_balance(
    user: Principal,
    token_ledger: Principal,
    correct_amount: u64,
) -> Result<String, StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    if !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    ensure_pool_balance_mutation_allowed()?;
    let msg = mutate_state(|s| {
        let result = s.correct_balance(user, token_ledger, correct_amount);
        s.push_event(
            caller,
            PoolEventType::BalanceCorrected {
                user,
                token_ledger,
                new_amount: correct_amount,
            },
        );
        result
    });
    log!(INFO, "Admin balance correction by {}: {}", caller, msg);
    Ok(msg)
}

#[update]
pub fn admin_correct_collateral_gain(
    user: Principal,
    collateral_ledger: Principal,
    correct_amount: u64,
) -> Result<String, StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    if !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    let msg = mutate_state(|s| {
        let result = s.correct_collateral_gain(user, collateral_ledger, correct_amount);
        s.push_event(
            caller,
            PoolEventType::CollateralGainCorrected {
                user,
                collateral_ledger,
                new_amount: correct_amount,
            },
        );
        result
    });
    log!(
        INFO,
        "Admin collateral gain correction by {}: {}",
        caller,
        msg
    );
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use icrc_ledger_types::icrc1::account::Account;
    use rumi_protocol_backend::chains::config::ChainId;

    fn principal(byte: u8) -> Principal {
        Principal::from_slice(&[byte])
    }

    fn pending_intent() -> ChainSpAbsorbIntent {
        let mut stables_consumed = BTreeMap::new();
        stables_consumed.insert(principal(10), 100_00000000);
        ChainSpAbsorbIntent {
            vault_id: 77,
            chain_id: ChainId(1030),
            chain_sentinel: crate::state::chain_collateral_sentinel(1030),
            icusd_ledger: principal(10),
            icusd_minting_account: Account {
                owner: principal(90),
                subaccount: None,
            },
            icusd_to_burn_e8s: 100_00000000,
            stables_consumed,
            burn_created_at_time_ns: 123,
            status: ChainSpAbsorbIntentStatus::Burned,
            burn_proof: Some(rumi_protocol_backend::icrc3_proof::SpWritedownProof {
                block_index: 44,
                ledger_kind: rumi_protocol_backend::icrc3_proof::SpProofLedger::IcusdBurn,
                vault_id_memo: 77,
            }),
            backend_result: None,
            last_error: None,
            created_at_ns: 123,
            updated_at_ns: 456,
        }
    }

    #[test]
    fn pending_chain_absorb_blocks_pool_balance_mutations() {
        crate::state::replace_state(crate::state::StabilityPoolState::default());
        assert!(
            ensure_pool_balance_mutation_allowed().is_ok(),
            "empty journal does not block ordinary mutations",
        );

        mutate_state(|s| s.put_pending_chain_absorb(pending_intent()).unwrap());
        assert!(
            matches!(
                ensure_pool_balance_mutation_allowed(),
                Err(StabilityPoolError::SystemBusy)
            ),
            "interest revenue and admin balance correction must not mutate live denominator while pending",
        );

        crate::state::replace_state(crate::state::StabilityPoolState::default());
    }

    #[test]
    fn in_flight_balance_async_blocks_chain_absorb_start() {
        assert!(ensure_no_pool_balance_async_in_flight().is_ok());
        let guard = crate::pool_guard::PoolBalanceAsyncGuard::new();
        assert!(
            matches!(
                ensure_no_pool_balance_async_in_flight(),
                Err(StabilityPoolError::SystemBusy)
            ),
            "SP chain absorb must not start while withdrawal rollback could still restore balances",
        );
        drop(guard);
        assert!(ensure_no_pool_balance_async_in_flight().is_ok());
    }
}
