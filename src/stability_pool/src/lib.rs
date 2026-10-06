use candid::Principal;
use ic_canister_log::log;
use ic_cdk::{init, post_upgrade, pre_upgrade, query, update};
use std::collections::BTreeMap;
use std::cell::Cell;
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
const CHAIN_BURN_RECOVERY_POLL_SECONDS: u64 = 600;
const CHAIN_BURN_RECOVERY_MAX_PER_TICK: usize = 2;
const NATIVE_XRP_BURN_RECOVERY_POLL_SECONDS: u64 = 600;
const NATIVE_XRP_BURN_RECOVERY_MAX_PER_TICK: usize = 2;
thread_local! {
    static NATIVE_XRP_BURN_RECOVERY_CURSOR: Cell<u64> = const { Cell::new(0) };
}
/// Native-XRP payout settlement sweep cadence. Each settlement is a tEd25519
/// signature + XRPL submit outcall on the backend, so the sweep is deliberately
/// slow and bounded: at 2 claims per 10-minute tick a normal absorb fan-out
/// clears within the hour, and cycle cost stays negligible.
const NATIVE_XRP_SETTLE_SWEEP_POLL_SECONDS: u64 = 600;
const NATIVE_XRP_SETTLE_SWEEP_MAX_PER_TICK: usize = 2;
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

/// A retained burn intent fences only the exact stablecoin ledgers committed
/// to its draw. The transient live-liquidation guard remains pool-wide while
/// a snapshot is being formed or a callback is applying accounting.
pub(crate) fn pool_balance_mutation_blocked_for_ledger(ledger: Principal) -> bool {
    crate::pool_guard::liquidation_in_progress()
        || read_state(|s| {
            s.pending_chain_absorbs.as_ref().is_some_and(|intents| {
                intents.values().any(|intent| {
                    intent.icusd_ledger == ledger || intent.stables_consumed.contains_key(&ledger)
                })
            }) || s
                .pending_native_xrp_absorbs
                .as_ref()
                .is_some_and(|intents| {
                    intents.values().any(|intent| {
                        intent.icusd_ledger == ledger
                            || intent.stables_consumed.contains_key(&ledger)
                    })
                })
                || s.pending_refunds.as_ref().is_some_and(|refunds| {
                    refunds.values().any(|refund| refund.token_ledger == ledger)
                })
        })
}

pub(crate) fn ensure_pool_balance_mutation_allowed_for_ledger(
    ledger: Principal,
) -> Result<(), StabilityPoolError> {
    if pool_balance_mutation_blocked_for_ledger(ledger) {
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
        setup_chain_burn_recovery_timer();
        setup_native_xrp_burn_recovery_timer();
        setup_native_xrp_settle_sweep_timer();
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
    let (indexed, retained) = mutate_state(|s| {
        s.reconcile_pending_deposit_attempts_after_upgrade();
        s.initialize_unallocated_interest_mint_index();
        (
            s.unallocated_interest_mint_index.is_some(),
            s.unallocated_interest_mint_index.as_ref().map(BTreeMap::len).unwrap_or(0),
        )
    });
    if !indexed {
        log!(
            INFO,
            "CL-10 migration: legacy unallocated-interest receipt history exceeds the bounded index; new notifications will remain pending for reconciliation"
        );
    } else {
        log!(INFO, "CL-10 migration: indexed {} retained unallocated-interest receipts", retained);
    }
    mutate_state(|s| s.normalize_pending_refund_fee_state());
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
        setup_chain_burn_recovery_timer();
        setup_native_xrp_burn_recovery_timer();
        setup_native_xrp_settle_sweep_timer();
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

/// Auto-settle pending native-XRP payouts to depositors' registered XRPL
/// addresses. Depositors opted in with an address; without this sweep a
/// liquidation's proceeds sat as claims until each depositor manually clicked
/// settle (most never knew they had one — vault 195, 2026-08-15). The cursor
/// rotates so a failing claim cannot starve the rest; `emergency_pause` stops
/// the sweep with the rest of the pool.
fn setup_native_xrp_settle_sweep_timer() {
    thread_local! {
        static SWEEP_CURSOR: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
    }
    ic_cdk_timers::set_timer_interval(
        Duration::from_secs(NATIVE_XRP_SETTLE_SWEEP_POLL_SECONDS),
        || {
            ic_cdk::spawn(async {
                let cursor = SWEEP_CURSOR.with(|c| c.get());
                let summary = crate::liquidation::run_native_xrp_settle_sweep_with_io(
                    &mut crate::liquidation::CdkNativeXrpSettleSweepIo,
                    cursor,
                    NATIVE_XRP_SETTLE_SWEEP_MAX_PER_TICK,
                )
                .await;
                if summary.examined > 0 {
                    log!(
                        INFO,
                        "[xrp-settle-sweep] examined {} acked {} submitted {} \
                         pending-confirm {} failed {}",
                        summary.examined,
                        summary.acked,
                        summary.submitted,
                        summary.pending_confirmation,
                        summary.failed
                    );
                    SWEEP_CURSOR.with(|c| c.set(summary.last_claim_id));
                }
            });
        },
    );
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

/// Resume retained chain burns independently of the liquidatable-vault feed.
/// The recovery path only reuses a persisted exact burn proof.
fn setup_chain_burn_recovery_timer() {
    ic_cdk_timers::set_timer_interval(
        Duration::from_secs(CHAIN_BURN_RECOVERY_POLL_SECONDS),
        || {
            ic_cdk::spawn(async {
                let resumed = crate::liquidation::run_chain_burn_recovery_tick(
                    CHAIN_BURN_RECOVERY_MAX_PER_TICK,
                )
                .await;
                if resumed > 0 {
                    log!(INFO, "chain burn recovery tick inspected {} retained intent(s)", resumed);
                }
            });
        },
    );
}

async fn reconcile_pending_native_xrp_absorb_from_status(
    vault_id: u64,
) -> Result<(), StabilityPoolError> {
    let _liquidation_guard = crate::pool_guard::SpLiquidationGuard::new()?;
    if read_state(|s| !s.in_flight_liquidations.is_empty()) {
        return Err(StabilityPoolError::SystemBusy);
    }
    let intent = read_state(|s| s.get_pending_native_xrp_absorb(vault_id))
        .ok_or_else(|| StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: "missing pending native XRP absorb intent".to_string(),
        })?;
    let proof = intent.burn_proof.clone().ok_or_else(|| {
        StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: "pending native XRP absorb has no exact burn proof".to_string(),
        }
    })?;
    if intent.backend_result.is_none()
        && !matches!(intent.status,
            NativeXrpAbsorbIntentStatus::Burned
                | NativeXrpAbsorbIntentStatus::BackendRejected)
    {
        return Err(StabilityPoolError::SystemBusy);
    }
    mutate_state(|s| {
        s.in_flight_liquidations.insert(vault_id);
    });

    let recovery = async {
        let protocol_id = read_state(|s| s.protocol_canister_id);
        let status = if let Some(result) = intent.backend_result.clone() {
            rumi_protocol_backend::XrpSpAbsorbStatus::Accepted(result)
        } else {
            let request = crate::liquidation::native_xrp_request_from_intent(&intent, proof.clone());
            let status_result: Result<
                (Result<rumi_protocol_backend::XrpSpAbsorbStatus, rumi_protocol_backend::ProtocolError>,),
                _,
            > = ic_cdk::call(protocol_id, "stability_pool_xrp_absorb_status", (request,)).await;
            match status_result {
                Ok((Ok(status),)) => status,
                Ok((Err(error),)) => {
                    return Err(StabilityPoolError::LiquidationFailed {
                        vault_id,
                        reason: format!("backend could not resolve native XRP absorb status: {error:?}"),
                    });
                }
                Err(_) => {
                    return Err(StabilityPoolError::InterCanisterCallFailed {
                        target: format!("{protocol_id}"),
                        method: "stability_pool_xrp_absorb_status".to_string(),
                    });
                }
            }
        };
        if read_state(|s| s.get_pending_native_xrp_absorb(vault_id)) != Some(intent.clone()) {
            return Err(StabilityPoolError::LiquidationFailed {
                vault_id,
                reason: "pending native XRP intent changed during backend status lookup".to_string(),
            });
        }

        match status {
            rumi_protocol_backend::XrpSpAbsorbStatus::Accepted(result) => {
                crate::liquidation::validate_xrp_absorb_backend_result(
                    vault_id,
                    intent.icusd_to_burn_e8s,
                    intent.collateral_received_drops,
                    &intent.allocations,
                    &result,
                )?;
                let accepted = mutate_state(|s| {
                    crate::liquidation::mark_native_xrp_absorb_backend_result_in_state(
                        s, vault_id, result, ic_cdk::api::time(),
                    )
                })?;
                mutate_state(|s| {
                    crate::liquidation::apply_native_xrp_absorb_success_in_state_at(
                        s, &accepted, ic_cdk::api::time(),
                    )
                })?;
                Ok(())
            }
            rumi_protocol_backend::XrpSpAbsorbStatus::Unseen
            | rumi_protocol_backend::XrpSpAbsorbStatus::RefundJournaled => {
                let receipt = crate::liquidation::refund_and_verify_sp_burn(
                    protocol_id,
                    vault_id,
                    intent.icusd_to_burn_e8s,
                    intent.icusd_ledger,
                    proof.clone(),
                )
                .await?;
                if read_state(|s| s.get_pending_native_xrp_absorb(vault_id))
                    != Some(intent.clone())
                {
                    return Err(StabilityPoolError::LiquidationFailed {
                        vault_id,
                        reason: "pending native XRP intent changed while verifying its refund".into(),
                    });
                }
                let cleared = mutate_state(|s| {
                    crate::liquidation::clear_refunded_native_xrp_absorb_in_state(s, &intent)
                });
                if !cleared {
                    return Err(StabilityPoolError::LiquidationFailed {
                        vault_id,
                        reason: format!(
                            "refund block {} verified but the exact native XRP intent could not be cleared",
                            receipt.refund_block_index
                        ),
                    });
                }
                Ok(())
            }
            rumi_protocol_backend::XrpSpAbsorbStatus::ConsumedWithoutResult => {
                mutate_state(|s| {
                    crate::liquidation::mark_native_xrp_absorb_error_in_state(
                        s,
                        vault_id,
                        NativeXrpAbsorbIntentStatus::Burned,
                        "backend reports proof consumed without an exact absorb or refund result".to_string(),
                        ic_cdk::api::time(),
                    );
                });
                Err(StabilityPoolError::LiquidationFailed {
                    vault_id,
                    reason: "backend consumed this proof without an exact recoverable result; intent remains held".to_string(),
                })
            }
        }
    }
    .await;

    mutate_state(|s| {
        s.in_flight_liquidations.remove(&vault_id);
    });
    recovery
}


fn setup_native_xrp_burn_recovery_timer() {
    ic_cdk_timers::set_timer_interval(
        Duration::from_secs(NATIVE_XRP_BURN_RECOVERY_POLL_SECONDS),
        || {
            ic_cdk::spawn(async {
                let mut ids = read_state(|state| {
                    state.pending_native_xrp_absorbs()
                        .into_iter()
                        .filter(|intent| intent.burn_proof.is_some()
                            && (intent.backend_result.is_some()
                                || matches!(intent.status,
                                    NativeXrpAbsorbIntentStatus::Burned
                                        | NativeXrpAbsorbIntentStatus::BackendRejected)))
                        .map(|intent| intent.vault_id)
                        .collect::<Vec<_>>()
                });
                ids.sort_unstable();
                let cursor = NATIVE_XRP_BURN_RECOVERY_CURSOR.with(Cell::get);
                let mut batch = ids.iter().copied().filter(|id| *id > cursor)
                    .take(NATIVE_XRP_BURN_RECOVERY_MAX_PER_TICK).collect::<Vec<_>>();
                if batch.len() < NATIVE_XRP_BURN_RECOVERY_MAX_PER_TICK {
                    batch.extend(ids.iter().copied().filter(|id| *id <= cursor)
                        .take(NATIVE_XRP_BURN_RECOVERY_MAX_PER_TICK - batch.len()));
                }
                for vault_id in batch {
                    NATIVE_XRP_BURN_RECOVERY_CURSOR.with(|value| value.set(vault_id));
                    if let Err(error) = reconcile_pending_native_xrp_absorb_from_status(vault_id).await {
                        log!(INFO, "native XRP burn recovery {} remains pending: {:?}", vault_id, error);
                    }
                }
            });
        },
    );
}

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

/// Temporarily fail-closed: 3pool add-liquidity lacks a durable receipt, so this
/// endpoint returns a typed error before pulling any input tokens.
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

/// Attach an exact ledger receipt to a pending refund after an ambiguous call.
/// This is evidence-only: it never emits a transfer.
#[update]
pub async fn reconcile_pending_refund(
    refund_id: u64,
    block_index: u64,
) -> Result<u64, StabilityPoolError> {
    crate::deposits::reconcile_pending_refund(refund_id, block_index).await
}

/// Advance one bounded archive-aware scan page after a typed TooOld response.
/// A fresh tuple is enabled only after the full configured icUSD prefix proves
/// no exact payout block exists.
#[update]
pub async fn reconcile_pending_refund_history(
    refund_id: u64,
) -> Result<(), StabilityPoolError> {
    crate::deposits::reconcile_pending_refund_history(refund_id).await
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

/// Legacy notification entry point retained for Candid compatibility.
///
/// It cannot safely credit interest because it has no source mint block for
/// replay protection. The backend must use `receive_interest_revenue_v2`.
#[update]
pub fn receive_interest_revenue(
    _token_ledger: Principal,
    _amount: u64,
    _collateral_type: Option<Principal>,
) -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    let expected = read_state(|s| s.protocol_canister_id);
    if caller != expected {
        return Err(StabilityPoolError::Unauthorized);
    }
    Err(StabilityPoolError::SystemBusy)
}

/// V2 interest notification carries the backend mint block, which supplies a
/// durable source receipt for the no-eligible-recipient treasury route. The
/// legacy V1 method above remains signature-compatible but fails closed because
/// it lacks that key. The backend must use V2 before interest notifications
/// can be acknowledged by this pool.
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

    match read_state(|s| s.interest_mint_receipt_status(source_mint_block)) {
        state::InterestMintReceiptStatus::PendingForward(batch_id) => {
            return process_unallocated_interest_forward(batch_id).await;
        }
        state::InterestMintReceiptStatus::Duplicate => return Ok(()),
        state::InterestMintReceiptStatus::OutsideReplayWindow => {
            // Keep the backend's durable notification pending. A stale receipt
            // must never be acknowledged as a new distribution.
            return Err(StabilityPoolError::SystemBusy);
        }
        state::InterestMintReceiptStatus::New => {}
    }

    if read_state(|s| s.has_eligible_interest_recipient(collateral_type.as_ref())) {
        return match mutate_state(|s| {
            match s.record_interest_mint_receipt(source_mint_block) {
                state::InterestMintReceiptStatus::New => {
                    s.distribute_interest_revenue(token_ledger, amount, collateral_type);
                    s.push_event(caller, PoolEventType::InterestReceived { token_ledger, amount });
                    Ok(())
                }
                state::InterestMintReceiptStatus::Duplicate => Ok(()),
                _ => Err(StabilityPoolError::SystemBusy),
            }
        }) {
            Ok(()) => Ok(()),
            Err(error) => Err(error),
        };
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

/// Credit a protocol-paid refund fee reserve from an exact admin funding
/// transfer. The caller first transfers `amount` from its default account to
/// the pool's default account, then supplies the ICRC-3 receipt tuple.
#[update]
pub async fn fund_pending_refund_fee_reserve(
    ledger: Principal,
    amount: u64,
    funding_block_index: u64,
    created_at_time_ns: u64,
    memo: Vec<u8>,
) -> Result<u64, StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    if caller == Principal::anonymous() || !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    if amount == 0 {
        return Err(StabilityPoolError::AmountTooLow { minimum_e8s: 1 });
    }
    if !read_state(|s| s.stablecoin_registry.contains_key(&ledger)) {
        return Err(StabilityPoolError::TokenNotAccepted { ledger });
    }
    let source = icrc_ledger_types::icrc1::account::Account {
        owner: caller,
        subaccount: None,
    };
    let destination = icrc_ledger_types::icrc1::account::Account {
        owner: ic_cdk::id(),
        subaccount: None,
    };
    rumi_protocol_backend::icrc3_proof::verify_icrc3_direct_transfer_block(
        ledger,
        funding_block_index,
        source,
        destination,
        amount,
        Some(&memo),
        Some(created_at_time_ns),
    )
    .await
    .map_err(|reason| StabilityPoolError::LedgerTransferFailed {
        reason: format!("refund fee funding receipt did not verify: {reason}"),
    })?;
    let balance = mutate_state(|s| {
        s.credit_pending_refund_fee_reserve(ledger, funding_block_index, amount)
            .map_err(|reason| StabilityPoolError::LedgerTransferFailed {
                reason: reason.to_string(),
            })?;
        Ok::<u64, StabilityPoolError>(
            s.pending_refund_fee_reserves
                .as_ref()
                .and_then(|reserves| reserves.get(&ledger).copied())
                .unwrap_or(0),
        )
    })?;
    Ok(balance)
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
pub async fn get_ledger_reconciliation(
) -> Result<Vec<LedgerReconciliationEntry>, StabilityPoolError> {
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


const MAX_PENDING_ICUSD_BURN_HISTORY_BLOCKS: u64 = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
enum PendingIcusdBurnIntentSnapshot {
    Chain(ChainSpAbsorbIntent),
    NativeXrp(NativeXrpAbsorbIntent),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingBurnReconciliationPhase {
    Prepared,
    Burned,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PendingBurnHistoryScanError {
    InvalidRange,
    RangePastLedgerTip { end_exclusive: u64, log_length: u64 },
    IncompleteAt { block_index: u64, reason: String },
    MultipleMatches,
}

/// Exact positive ledger proof may resolve legacy attempts whose dispatch marker
/// is missing, but a known no-dispatch marker, advanced phase, or competing
/// result/proof must never be overwritten.
fn pending_burn_reconciliation_eligible(
    attempted: Option<bool>,
    phase: PendingBurnReconciliationPhase,
    existing_proof: Option<&rumi_protocol_backend::icrc3_proof::SpWritedownProof>,
    backend_result_exists: bool,
    requested_proof: &rumi_protocol_backend::icrc3_proof::SpWritedownProof,
) -> bool {
    if attempted == Some(false) {
        return false;
    }
    // Exact proof read-back is safe in every later phase, including a backend
    // rejection followed by refund reconciliation. It must not rewrite phase.
    if existing_proof == Some(requested_proof) {
        return true;
    }
    if existing_proof.is_some() {
        return false;
    }
    phase == PendingBurnReconciliationPhase::Prepared && !backend_result_exists
}

fn burn_snapshot_matches<T: PartialEq>(
    expected: &T,
    current: Option<&T>,
    competing_intent_exists: bool,
) -> bool {
    !competing_intent_exists && current == Some(expected)
}

fn pending_burn_history_scan_eligible(
    attempted: Option<bool>,
    phase: PendingBurnReconciliationPhase,
    has_proof: bool,
    backend_result_exists: bool,
) -> bool {
    attempted != Some(false)
        && phase == PendingBurnReconciliationPhase::Prepared
        && !has_proof
        && !backend_result_exists
}

fn capture_pending_icusd_burn(
    vault_id: u64,
) -> Result<PendingIcusdBurnIntentSnapshot, StabilityPoolError> {
    read_state(|s| {
        match (
            s.get_pending_chain_absorb(vault_id),
            s.get_pending_native_xrp_absorb(vault_id),
        ) {
            (Some(chain), None) => Ok(PendingIcusdBurnIntentSnapshot::Chain(chain)),
            (None, Some(xrp)) => Ok(PendingIcusdBurnIntentSnapshot::NativeXrp(xrp)),
            _ => Err(StabilityPoolError::LiquidationFailed {
                vault_id,
                reason: "expected exactly one pending chain or native XRP absorb intent".to_string(),
            }),
        }
    })
}

fn pending_burn_identity(
    pending: &PendingIcusdBurnIntentSnapshot,
) -> (Principal, u64, icrc_ledger_types::icrc1::account::Account, u64) {
    match pending {
        PendingIcusdBurnIntentSnapshot::Chain(intent) => (
            intent.icusd_ledger,
            intent.icusd_to_burn_e8s,
            intent.icusd_minting_account,
            intent.burn_created_at_time_ns,
        ),
        PendingIcusdBurnIntentSnapshot::NativeXrp(intent) => (
            intent.icusd_ledger,
            intent.icusd_to_burn_e8s,
            intent.icusd_minting_account,
            intent.burn_created_at_time_ns,
        ),
    }
}

fn pending_burn_is_eligible(
    pending: &PendingIcusdBurnIntentSnapshot,
    requested_proof: &rumi_protocol_backend::icrc3_proof::SpWritedownProof,
) -> bool {
    match pending {
        PendingIcusdBurnIntentSnapshot::Chain(intent) => pending_burn_reconciliation_eligible(
            intent.burn_attempted,
            match intent.status {
                ChainSpAbsorbIntentStatus::Prepared => PendingBurnReconciliationPhase::Prepared,
                ChainSpAbsorbIntentStatus::Burned => PendingBurnReconciliationPhase::Burned,
                _ => PendingBurnReconciliationPhase::Other,
            },
            intent.burn_proof.as_ref(),
            intent.backend_result.is_some(),
            requested_proof,
        ),
        PendingIcusdBurnIntentSnapshot::NativeXrp(intent) => pending_burn_reconciliation_eligible(
            intent.burn_attempted,
            match intent.status {
                NativeXrpAbsorbIntentStatus::Prepared => PendingBurnReconciliationPhase::Prepared,
                NativeXrpAbsorbIntentStatus::Burned => PendingBurnReconciliationPhase::Burned,
                _ => PendingBurnReconciliationPhase::Other,
            },
            intent.burn_proof.as_ref(),
            intent.backend_result.is_some(),
            requested_proof,
        ),
    }
}

/// Commit only against the exact full intent captured before any await. The
/// equality includes phase, proof, backend result, error, timestamps, and all
/// immutable burn-plan fields, preventing same-tuple phase races.
fn commit_pending_icusd_burn_reconciliation(
    vault_id: u64,
    expected: PendingIcusdBurnIntentSnapshot,
    proof: rumi_protocol_backend::icrc3_proof::SpWritedownProof,
) -> Result<(), StabilityPoolError> {
    mutate_state(|s| match expected {
        PendingIcusdBurnIntentSnapshot::Chain(expected) => {
            let current = s.get_pending_chain_absorb(vault_id);
            if !burn_snapshot_matches(
                &expected,
                current.as_ref(),
                s.get_pending_native_xrp_absorb(vault_id).is_some(),
            ) {
                return Err(StabilityPoolError::LiquidationFailed {
                    vault_id,
                    reason: "pending chain absorb full snapshot changed during burn reconciliation".to_string(),
                });
            }
            let current = current.expect("snapshot comparator established presence");
            if !pending_burn_is_eligible(
                &PendingIcusdBurnIntentSnapshot::Chain(current.clone()),
                &proof,
            ) {
                return Err(StabilityPoolError::LiquidationFailed {
                    vault_id,
                    reason: "pending chain absorb is no longer eligible for burn reconciliation".to_string(),
                });
            }
            if current.burn_proof.as_ref() == Some(&proof) {
                return Ok(()); // verified same proof; preserve the advanced phase
            }
            crate::liquidation::mark_chain_absorb_burned_in_state(
                s,
                vault_id,
                proof,
                ic_cdk::api::time(),
            )?;
            Ok(())
        }
        PendingIcusdBurnIntentSnapshot::NativeXrp(expected) => {
            let current = s.get_pending_native_xrp_absorb(vault_id);
            if !burn_snapshot_matches(
                &expected,
                current.as_ref(),
                s.get_pending_chain_absorb(vault_id).is_some(),
            ) {
                return Err(StabilityPoolError::LiquidationFailed {
                    vault_id,
                    reason: "pending native XRP absorb full snapshot changed during burn reconciliation".to_string(),
                });
            }
            let current = current.expect("snapshot comparator established presence");
            if !pending_burn_is_eligible(
                &PendingIcusdBurnIntentSnapshot::NativeXrp(current.clone()),
                &proof,
            ) {
                return Err(StabilityPoolError::LiquidationFailed {
                    vault_id,
                    reason: "pending native XRP absorb is no longer eligible for burn reconciliation".to_string(),
                });
            }
            if current.burn_proof.as_ref() == Some(&proof) {
                return Ok(()); // verified same proof; preserve the advanced phase
            }
            crate::liquidation::mark_native_xrp_absorb_burned_in_state(
                s,
                vault_id,
                proof,
                ic_cdk::api::time(),
            )?;
            Ok(())
        }
    })
}

async fn scan_icusd_burn_history<F, Fut>(
    start: u64,
    length: u64,
    log_length: u64,
    sp_principal: Principal,
    minting_account: icrc_ledger_types::icrc1::account::Account,
    amount_e8s: u64,
    vault_id: u64,
    created_at_time_ns: u64,
    mut fetch_block: F,
) -> Result<Option<u64>, PendingBurnHistoryScanError>
where
    F: FnMut(u64) -> Fut,
    Fut: std::future::Future<Output = Result<rumi_protocol_backend::icrc3_proof::DecodedBlock, String>>,
{
    let end_exclusive = start.checked_add(length).ok_or(PendingBurnHistoryScanError::InvalidRange)?;
    if length == 0 || length > MAX_PENDING_ICUSD_BURN_HISTORY_BLOCKS {
        return Err(PendingBurnHistoryScanError::InvalidRange);
    }
    if end_exclusive > log_length {
        return Err(PendingBurnHistoryScanError::RangePastLedgerTip { end_exclusive, log_length });
    }

    let mut found = None;
    for index in start..end_exclusive {
        let block = fetch_block(index).await.map_err(|reason| {
            PendingBurnHistoryScanError::IncompleteAt { block_index: index, reason }
        })?;
        if crate::liquidation::validate_icusd_burn_reconciliation_block(
            &block,
            sp_principal,
            minting_account,
            amount_e8s,
            vault_id,
            created_at_time_ns,
        ).is_ok() {
            if found.replace(index).is_some() {
                return Err(PendingBurnHistoryScanError::MultipleMatches);
            }
        }
    }
    Ok(found)
}

fn pending_burn_history_error(vault_id: u64, error: PendingBurnHistoryScanError) -> StabilityPoolError {
    let reason = match error {
        PendingBurnHistoryScanError::InvalidRange => format!(
            "invalid icUSD burn history range; length must be 1..={MAX_PENDING_ICUSD_BURN_HISTORY_BLOCKS} and end must not overflow"
        ),
        PendingBurnHistoryScanError::RangePastLedgerTip { end_exclusive, log_length } => format!(
            "icUSD burn history range is incomplete: end {end_exclusive} exceeds ledger log_length {log_length}"
        ),
        PendingBurnHistoryScanError::IncompleteAt { block_index, reason } => format!(
            "icUSD burn history scan is incomplete at block {block_index} (ledger/archive lookup failed: {reason}); no state changed"
        ),
        PendingBurnHistoryScanError::MultipleMatches =>
            "icUSD burn history range contains multiple exact matches; no state changed".to_string(),
    };
    StabilityPoolError::LiquidationFailed { vault_id, reason }
}

fn authorize_icusd_burn_reconciliation(vault_id: u64) -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    if caller == Principal::anonymous() || !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    if vault_id == 0 {
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: "vault id must be nonzero".to_string(),
        });
    }
    Ok(())
}

/// Attach an independently verified icUSD burn block to a pending chain or
/// native-XRP absorb whose original ledger reply was lost. Admin-only; the
/// complete intent snapshot must still match after the ledger query.
#[update]
pub async fn reconcile_pending_icusd_burn(
    vault_id: u64,
    burn_block_index: u64,
) -> Result<(), StabilityPoolError> {
    authorize_icusd_burn_reconciliation(vault_id)?;
    // Serialize recovery with liquidation replay after upgrades. The durable
    // pending intent remains the upgrade-safe balance-mutation fence.
    let _liquidation_guard = crate::pool_guard::SpLiquidationGuard::new()?;
    let expected = capture_pending_icusd_burn(vault_id)?;
    let proof = crate::liquidation::build_icusd_burn_proof(burn_block_index, vault_id);
    if !pending_burn_is_eligible(&expected, &proof) {
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: "pending burn is not in an unresolved reconciliation phase".to_string(),
        });
    }
    let (ledger, amount, minting_account, created_at_time_ns) = pending_burn_identity(&expected);
    let block = rumi_protocol_backend::icrc3_proof::fetch_icrc3_block(ledger, burn_block_index)
        .await
        .map_err(|reason| StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: format!("supplied burn block could not be decoded: {reason}"),
        })?;
    crate::liquidation::validate_icusd_burn_reconciliation_block(
        &block,
        ic_cdk::id(),
        minting_account,
        amount,
        vault_id,
        created_at_time_ns,
    )
    .map_err(|reason| StabilityPoolError::LiquidationFailed {
        vault_id,
        reason: format!("supplied burn block failed exact ledger verification: {reason}"),
    })?;
    commit_pending_icusd_burn_reconciliation(vault_id, expected, proof)
}

/// Search a caller-supplied bounded range for a lost-reply burn. Every index
/// is fetched exactly once through the existing ICRC-3 direct/archive resolver.
/// Only one exact match across the fully fetched range is attached. A complete
/// range with no match returns an error that explicitly limits absence to that
/// range; it never clears/reissues the burn. Any gap, malformed block, or
/// archive failure is incomplete and leaves the pending intent unchanged.
#[update]
pub async fn scan_pending_icusd_burn_history(
    vault_id: u64,
    start: u64,
    length: u64,
) -> Result<u64, StabilityPoolError> {
    authorize_icusd_burn_reconciliation(vault_id)?;
    let _liquidation_guard = crate::pool_guard::SpLiquidationGuard::new()?;
    if length == 0 || length > MAX_PENDING_ICUSD_BURN_HISTORY_BLOCKS || start.checked_add(length).is_none() {
        return Err(pending_burn_history_error(vault_id, PendingBurnHistoryScanError::InvalidRange));
    }
    let expected = capture_pending_icusd_burn(vault_id)?;
    let (ledger, amount, minting_account, created_at_time_ns) = pending_burn_identity(&expected);
    let history_scan_eligible = match &expected {
        PendingIcusdBurnIntentSnapshot::Chain(i) => pending_burn_history_scan_eligible(
            i.burn_attempted,
            match i.status {
                ChainSpAbsorbIntentStatus::Prepared => PendingBurnReconciliationPhase::Prepared,
                ChainSpAbsorbIntentStatus::Burned => PendingBurnReconciliationPhase::Burned,
                _ => PendingBurnReconciliationPhase::Other,
            },
            i.burn_proof.is_some(),
            i.backend_result.is_some(),
        ),
        PendingIcusdBurnIntentSnapshot::NativeXrp(i) => pending_burn_history_scan_eligible(
            i.burn_attempted,
            match i.status {
                NativeXrpAbsorbIntentStatus::Prepared => PendingBurnReconciliationPhase::Prepared,
                NativeXrpAbsorbIntentStatus::Burned => PendingBurnReconciliationPhase::Burned,
                _ => PendingBurnReconciliationPhase::Other,
            },
            i.burn_proof.is_some(),
            i.backend_result.is_some(),
        ),
    };
    if !history_scan_eligible {
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: "pending burn is not an unresolved prepared attempt for history discovery".to_string(),
        });
    }

    let log_length = rumi_protocol_backend::icrc3_proof::icrc3_log_length(ledger)
        .await
        .map_err(|reason| StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: format!("icUSD history log_length query failed: {reason}; no state changed"),
        })?;

    let matched = scan_icusd_burn_history(
        start,
        length,
        log_length,
        ic_cdk::id(),
        minting_account,
        amount,
        vault_id,
        created_at_time_ns,
        |index| rumi_protocol_backend::icrc3_proof::fetch_icrc3_block(ledger, index),
    )
    .await
    .map_err(|error| pending_burn_history_error(vault_id, error))?;
    let Some(block_index) = matched else {
        let still_current = read_state(|s| match &expected {
            PendingIcusdBurnIntentSnapshot::Chain(e) => burn_snapshot_matches(
                e,
                s.get_pending_chain_absorb(vault_id).as_ref(),
                s.get_pending_native_xrp_absorb(vault_id).is_some(),
            ),
            PendingIcusdBurnIntentSnapshot::NativeXrp(e) => burn_snapshot_matches(
                e,
                s.get_pending_native_xrp_absorb(vault_id).as_ref(),
                s.get_pending_chain_absorb(vault_id).is_some(),
            ),
        });
        if !still_current {
            return Err(StabilityPoolError::LiquidationFailed {
                vault_id,
                reason: "pending burn snapshot changed during complete history scan; no state changed".to_string(),
            });
        }
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: format!(
                "complete range [{start}, {}) at log_length {log_length} contains no exact burn; this is not proof of absence outside the scanned range, and no state changed",
                start + length
            ),
        });
    };

    let proof = crate::liquidation::build_icusd_burn_proof(block_index, vault_id);
    commit_pending_icusd_burn_reconciliation(vault_id, expected, proof)?;
    Ok(block_index)
}


fn pending_burn_refund_request(
    pending: &PendingIcusdBurnIntentSnapshot,
) -> Result<(u64, rumi_protocol_backend::icrc3_proof::SpWritedownProof), StabilityPoolError> {
    let (vault_id, amount, attempted, proof, backend_result_exists) = match pending {
        PendingIcusdBurnIntentSnapshot::Chain(intent) => (
            intent.vault_id,
            intent.icusd_to_burn_e8s,
            intent.burn_attempted,
            intent.burn_proof.clone(),
            intent.backend_result.is_some(),
        ),
        PendingIcusdBurnIntentSnapshot::NativeXrp(intent) => (
            intent.vault_id,
            intent.icusd_to_burn_e8s,
            intent.burn_attempted,
            intent.burn_proof.clone(),
            intent.backend_result.is_some(),
        ),
    };
    if attempted == Some(false) || backend_result_exists {
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: "refund recovery requires a dispatched burn with no accepted backend absorb result".into(),
        });
    }
    let proof = proof.ok_or_else(|| StabilityPoolError::LiquidationFailed {
        vault_id,
        reason: "refund recovery requires an exact persisted burn proof".into(),
    })?;
    if proof.vault_id_memo != vault_id
        || proof.ledger_kind != rumi_protocol_backend::icrc3_proof::SpProofLedger::IcusdBurn
    {
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: "persisted burn proof does not match the pending vault".into(),
        });
    }
    Ok((amount, proof))
}

async fn clear_pending_burn_after_verified_refund(
    vault_id: u64,
    expected: &PendingIcusdBurnIntentSnapshot,
    receipt: &rumi_protocol_backend::sp_burn_refund::SpBurnRefundReceipt,
) -> Result<(), StabilityPoolError> {
    let (amount, proof) = pending_burn_refund_request(expected)?;
    let (ledger, _, _, _) = pending_burn_identity(expected);
    let mut expected_memo = b"RSPRFND:".to_vec();
    expected_memo.extend_from_slice(&proof.block_index.to_be_bytes());
    expected_memo.extend_from_slice(&vault_id.to_be_bytes());
    if receipt.vault_id != vault_id
        || receipt.amount_e8s != amount
        || receipt.ledger != ledger
        || receipt.recipient != ic_cdk::id()
        || receipt.burn_block_index != proof.block_index
        || receipt.refund_memo != expected_memo
    {
        return Err(StabilityPoolError::LedgerTransferFailed {
            reason: "backend refund receipt does not match the complete pending burn identity".into(),
        });
    }
    rumi_protocol_backend::icrc3_proof::verify_icrc3_transfer_block(
        ledger,
        receipt.refund_block_index,
        None,
        icrc_ledger_types::icrc1::account::Account {
            owner: ic_cdk::id(),
            subaccount: None,
        },
        amount,
        Some(&expected_memo),
        Some(receipt.refund_created_at_time),
    )
    .await
    .map_err(|reason| StabilityPoolError::LedgerTransferFailed {
        reason: format!("backend refund receipt failed local ledger verification: {reason}"),
    })?;
    let cleared = mutate_state(|state| match expected {
        PendingIcusdBurnIntentSnapshot::Chain(intent) => {
            if state.get_pending_chain_absorb(vault_id).as_ref() != Some(intent)
                || state.get_pending_native_xrp_absorb(vault_id).is_some()
            {
                return false;
            }
            crate::liquidation::clear_refunded_chain_absorb_in_state(state, intent)
        }
        PendingIcusdBurnIntentSnapshot::NativeXrp(intent) => {
            if state.get_pending_native_xrp_absorb(vault_id).as_ref() != Some(intent)
                || state.get_pending_chain_absorb(vault_id).is_some()
            {
                return false;
            }
            crate::liquidation::clear_refunded_native_xrp_absorb_in_state(state, intent)
        }
    });
    if !cleared {
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: "refund receipt verified but pending burn intent changed; intent remains held".into(),
        });
    }
    Ok(())
}

/// Retry compensation for a pending exact burn. The backend owns the durable
/// refund tuple and either resumes that same operation or returns its verified
/// receipt; this endpoint never creates a caller-selected transfer identity.
#[update]
pub async fn retry_pending_icusd_burn_refund(
    vault_id: u64,
) -> Result<rumi_protocol_backend::sp_burn_refund::SpBurnRefundReceipt, StabilityPoolError> {
    authorize_icusd_burn_reconciliation(vault_id)?;
    let _liquidation_guard = crate::pool_guard::SpLiquidationGuard::new()?;
    let expected = capture_pending_icusd_burn(vault_id)?;
    let (amount, proof) = pending_burn_refund_request(&expected)?;
    let (ledger, _, _, _) = pending_burn_identity(&expected);
    let receipt = crate::liquidation::refund_and_verify_sp_burn(
        read_state(|s| s.protocol_canister_id),
        vault_id,
        amount,
        ledger,
        proof,
    )
    .await?;
    clear_pending_burn_after_verified_refund(vault_id, &expected, &receipt).await?;
    Ok(receipt)
}

/// Attach an exact positive ledger receipt to the backend's already-journaled
/// compensation tuple, then clear only the unchanged pending SP burn intent.
#[update]
pub async fn reconcile_pending_icusd_burn_refund(
    vault_id: u64,
    refund_block_index: u64,
) -> Result<rumi_protocol_backend::sp_burn_refund::SpBurnRefundReceipt, StabilityPoolError> {
    authorize_icusd_burn_reconciliation(vault_id)?;
    let _liquidation_guard = crate::pool_guard::SpLiquidationGuard::new()?;
    let expected = capture_pending_icusd_burn(vault_id)?;
    let (amount, proof) = pending_burn_refund_request(&expected)?;
    let (result,): (Result<rumi_protocol_backend::sp_burn_refund::SpBurnRefundReceipt, rumi_protocol_backend::ProtocolError>,) =
        ic_cdk::call(
            read_state(|s| s.protocol_canister_id),
            "reconcile_stability_pool_burn_refund",
            (vault_id, amount, proof, refund_block_index),
        )
        .await
        .map_err(|_| StabilityPoolError::InterCanisterCallFailed {
            target: read_state(|s| s.protocol_canister_id).to_string(),
            method: "reconcile_stability_pool_burn_refund".into(),
        })?;
    let receipt = result.map_err(|error| StabilityPoolError::LiquidationFailed {
        vault_id,
        reason: format!("backend rejected exact refund receipt reconciliation: {error:?}"),
    })?;
    clear_pending_burn_after_verified_refund(vault_id, &expected, &receipt).await?;
    Ok(receipt)
}

/// Ask the backend's archive-aware refund journal to find an exact positive
/// receipt. History gaps and absence remain held; this route never remints or
/// rotates a compensation tuple.
#[update]
pub async fn reconcile_pending_icusd_burn_refund_from_history(
    vault_id: u64,
) -> Result<rumi_protocol_backend::sp_burn_refund::SpBurnRefundReceipt, StabilityPoolError> {
    authorize_icusd_burn_reconciliation(vault_id)?;
    let _liquidation_guard = crate::pool_guard::SpLiquidationGuard::new()?;
    let expected = capture_pending_icusd_burn(vault_id)?;
    let (amount, proof) = pending_burn_refund_request(&expected)?;
    let (result,): (Result<rumi_protocol_backend::sp_burn_refund::SpBurnRefundReceipt, rumi_protocol_backend::ProtocolError>,) =
        ic_cdk::call(
            read_state(|s| s.protocol_canister_id),
            "reconcile_stability_pool_burn_refund_from_history",
            (vault_id, amount, proof),
        )
        .await
        .map_err(|_| StabilityPoolError::InterCanisterCallFailed {
            target: read_state(|s| s.protocol_canister_id).to_string(),
            method: "reconcile_stability_pool_burn_refund_from_history".into(),
        })?;
    let receipt = result.map_err(|error| StabilityPoolError::LiquidationFailed {
        vault_id,
        reason: format!("backend could not reconcile exact refund history: {error:?}"),
    })?;
    clear_pending_burn_after_verified_refund(vault_id, &expected, &receipt).await?;
    Ok(receipt)
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
            burn_attempted: Some(true),
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
        let guard = crate::pool_guard::PoolBalanceAsyncGuard::new()
            .expect("balance async operation should acquire without liquidation");
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


#[cfg(test)]
mod pending_burn_history_scan_tests {
    use super::*;
    use rumi_protocol_backend::icrc3_proof::{DecodedBlock, SpProofLedger, SpWritedownProof};

    const VAULT: u64 = 77;
    const AMOUNT: u64 = 123_000_000;
    const CAT: u64 = 987_654;

    fn exact_burn() -> DecodedBlock {
        DecodedBlock {
            btype: Some("1burn".to_string()),
            op: "burn".to_string(),
            from: Some(icrc_ledger_types::icrc1::account::Account {
                owner: Principal::anonymous(),
                subaccount: None,
            }),
            to: None,
            spender: None,
            amount: AMOUNT as u128,
            transaction_fee: None,
            fee: None,
            memo: Some(crate::liquidation::encode_chain_writedown_memo(VAULT)),
            created_at_time: Some(CAT),
            expected_allowance: None,
            expires_at: None,
        }
    }

    fn unrelated_block() -> DecodedBlock {
        let mut block = exact_burn();
        block.memo = Some(b"other operation".to_vec());
        block
    }

    fn scan<F, Fut>(log_length: u64, fetch: F) -> Result<Option<u64>, PendingBurnHistoryScanError>
    where
        F: FnMut(u64) -> Fut,
        Fut: std::future::Future<Output = Result<DecodedBlock, String>>,
    {
        futures::executor::block_on(scan_icusd_burn_history(
            10,
            3,
            log_length,
            Principal::anonymous(),
            icrc_ledger_types::icrc1::account::Account {
                owner: Principal::anonymous(),
                subaccount: None,
            },
            AMOUNT,
            VAULT,
            CAT,
            fetch,
        ))
    }

    #[test]
    fn complete_bounded_range_finds_one_exact_burn() {
        let result = scan(13, |index| async move {
            Ok(if index == 11 { exact_burn() } else { unrelated_block() })
        });
        assert_eq!(result, Ok(Some(11)));
    }

    #[test]
    fn complete_range_without_match_reports_absence_only_in_range() {
        let result = scan(13, |_| async { Ok(unrelated_block()) });
        assert_eq!(result, Ok(None));
    }

    #[test]
    fn truncated_or_invalid_ranges_do_not_fetch_or_claim_absence() {
        assert_eq!(scan(12, |_| async { panic!("truncated range must not fetch") }),
            Err(PendingBurnHistoryScanError::RangePastLedgerTip { end_exclusive: 13, log_length: 12 }));
        let over_bound = futures::executor::block_on(scan_icusd_burn_history(
            10, MAX_PENDING_ICUSD_BURN_HISTORY_BLOCKS + 1, 100, Principal::anonymous(),
            icrc_ledger_types::icrc1::account::Account {
                owner: Principal::anonymous(),
                subaccount: None,
            },
            AMOUNT, VAULT, CAT, |_| async { panic!("invalid range must not fetch") },
        ));
        assert_eq!(over_bound, Err(PendingBurnHistoryScanError::InvalidRange));
    }

    #[test]
    fn archive_or_block_fetch_error_is_incomplete() {
        let result = scan(13, |index| async move {
            if index == 11 { Err("advertised archive callback failed".to_string()) }
            else { Ok(unrelated_block()) }
        });
        assert_eq!(result, Err(PendingBurnHistoryScanError::IncompleteAt {
            block_index: 11,
            reason: "advertised archive callback failed".to_string(),
        }));
    }

    #[test]
    fn multiple_exact_burns_in_the_complete_range_are_conflicting() {
        let result = scan(13, |_| async { Ok(exact_burn()) });
        assert_eq!(result, Err(PendingBurnHistoryScanError::MultipleMatches));
    }

    #[test]
    fn history_discovery_requires_prepared_unresolved_intent_but_accepts_legacy_unknown() {
        assert!(pending_burn_history_scan_eligible(
            None, PendingBurnReconciliationPhase::Prepared, false, false
        ));
        assert!(pending_burn_history_scan_eligible(
            Some(true), PendingBurnReconciliationPhase::Prepared, false, false
        ));
        assert!(!pending_burn_history_scan_eligible(
            Some(false), PendingBurnReconciliationPhase::Prepared, false, false
        ));
        assert!(!pending_burn_history_scan_eligible(
            Some(true), PendingBurnReconciliationPhase::Burned, true, false
        ));
        assert!(!pending_burn_history_scan_eligible(
            Some(true), PendingBurnReconciliationPhase::Prepared, false, true
        ));
    }

    #[test]
    fn stale_full_snapshot_or_competing_intent_fails_the_commit_cas() {
        #[derive(Clone, Debug, PartialEq, Eq)]
        struct Snapshot { phase: u8, proof: Option<u64>, backend_result: Option<u64> }
        let expected = Snapshot { phase: 0, proof: None, backend_result: None };
        assert!(burn_snapshot_matches(&expected, Some(&expected), false));
        let advanced = Snapshot { phase: 1, ..expected.clone() };
        assert!(!burn_snapshot_matches(&expected, Some(&advanced), false));
        let proof_attached = Snapshot { proof: Some(44), ..expected.clone() };
        assert!(!burn_snapshot_matches(&expected, Some(&proof_attached), false));
        let backend_accepted = Snapshot { backend_result: Some(1), ..expected.clone() };
        assert!(!burn_snapshot_matches(&expected, Some(&backend_accepted), false));
        assert!(!burn_snapshot_matches(&expected, Some(&expected), true));
        assert!(!burn_snapshot_matches::<Snapshot>(&expected, None, false));
    }

    #[test]
    fn scan_eligibility_allows_legacy_only_for_positive_proof_and_rejects_no_dispatch() {
        let proof = SpWritedownProof {
            block_index: 11,
            ledger_kind: SpProofLedger::IcusdBurn,
            vault_id_memo: VAULT,
        };
        assert!(pending_burn_reconciliation_eligible(
            None, PendingBurnReconciliationPhase::Prepared, None, false, &proof
        ));
        assert!(!pending_burn_reconciliation_eligible(
            Some(false), PendingBurnReconciliationPhase::Prepared, None, false, &proof
        ));
        assert!(!pending_burn_reconciliation_eligible(
            Some(true), PendingBurnReconciliationPhase::Other, None, false, &proof
        ));
        assert!(pending_burn_reconciliation_eligible(
            Some(true), PendingBurnReconciliationPhase::Other, Some(&proof), false, &proof
        ), "exact proof read-back must preserve BackendRejected while refund is pending");
        assert!(pending_burn_reconciliation_eligible(
            Some(true), PendingBurnReconciliationPhase::Burned, Some(&proof), true, &proof
        ), "identical positive burn proof remains idempotent after backend response");
        let conflicting = SpWritedownProof { block_index: 12, ..proof.clone() };
        assert!(!pending_burn_reconciliation_eligible(
            Some(true), PendingBurnReconciliationPhase::Burned, Some(&conflicting), true, &proof
        ));
    }
}
