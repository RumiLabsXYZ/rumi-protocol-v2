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

    // Migration: fix stablecoin transfer_fee values
    mutate_state(|s| {
        for config in s.stablecoin_registry.values_mut() {
            match config.symbol.as_str() {
                "icUSD" => {
                    config.transfer_fee = Some(100_000);
                }
                "3USD" => {
                    config.transfer_fee = Some(0);
                }
                _ => {}
            }
        }
    });
    log!(INFO, "Migration: corrected icUSD and 3USD transfer fees");

    // Defer timer setup to avoid ic0_call_new restriction during upgrade
    ic_cdk_timers::set_timer(Duration::ZERO, || {
        setup_virtual_price_timer();
        setup_chain_absorb_auto_timer();
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
                    let symbol = read_state(|s| {
                        s.stablecoin_registry
                            .get(&token_ledger)
                            .map(|c| c.symbol.clone())
                            .unwrap_or_else(|| format!("token {}", token_ledger))
                    });
                    let formatted = format_token_amount(amount);
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
                    let symbol = read_state(|s| {
                        s.stablecoin_registry
                            .get(&token_ledger)
                            .map(|c| c.symbol.clone())
                            .unwrap_or_else(|| format!("token {}", token_ledger))
                    });
                    let formatted = format_token_amount(amount);
                    format!(
                        "## Withdraw from Stability Pool\n\n\
                         You are withdrawing **{} {}** from the Rumi Protocol Stability Pool.",
                        formatted, symbol
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
                    let symbol = read_state(|s| {
                        s.stablecoin_registry
                            .get(&token_ledger)
                            .map(|c| c.symbol.clone())
                            .unwrap_or_else(|| format!("token {}", token_ledger))
                    });
                    let formatted = format_token_amount(amount);
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

/// Format an e8s token amount as a human-readable string.
fn format_token_amount(amount_e8s: u64) -> String {
    let whole = amount_e8s / 100_000_000;
    let frac = amount_e8s % 100_000_000;
    if frac == 0 {
        format!("{}", whole)
    } else {
        let frac_str = format!("{:08}", frac);
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
