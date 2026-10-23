use candid::Principal;
use ic_canister_log::log;
use ic_cdk::{init, post_upgrade, pre_upgrade, query, update};
use std::collections::BTreeMap;
use std::time::Duration;

pub mod deposits;
pub mod conversion;
pub mod withdrawals;
pub mod liquidation;
pub mod logs;
pub mod pool_guard;
pub mod receipt_store;
pub mod interest_recovery;
pub mod ordinary_liquidation;
pub mod state;
pub mod types;

use crate::logs::INFO;
use crate::state::{mutate_state, read_state};
use crate::types::*;

const CHAIN_ABSORB_AUTO_TIMER_POLL_SECONDS: u64 = 60;
/// Native-XRP payout settlement sweep cadence. Each settlement is a tEd25519
/// signature + XRPL submit outcall on the backend, so the sweep is deliberately
/// slow and bounded: at 2 claims per 10-minute tick a normal absorb fan-out
/// clears within the hour, and cycle cost stays negligible.
const NATIVE_XRP_SETTLE_SWEEP_POLL_SECONDS: u64 = 600;
const NATIVE_XRP_SETTLE_SWEEP_MAX_PER_TICK: usize = 2;
/// Reconcile exact native-XRP burn intents independently of the liquidatable
/// vault list. A backend/refund response may have been lost after the burn.
const NATIVE_XRP_ABSORB_RECOVERY_POLL_SECONDS: u64 = 60;
const NATIVE_XRP_ABSORB_RECOVERY_MAX_PER_TICK: usize = 2;
const UNALLOCATED_INTEREST_FORWARD_RETRY_SECONDS: u64 = 60;
const ORDINARY_LIQUIDATION_RECOVERY_POLL_SECONDS: u64 = 30;
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

fn cache_virtual_price(
    state: &mut state::StabilityPoolState,
    lp_ledger: Principal,
    virtual_price: u128,
) {
    let cache = state
        .cached_virtual_prices
        .get_or_insert_with(BTreeMap::new);
    if virtual_price == 0 {
        // Zero is the 3pool's legacy sentinel for an unavailable VP. Remove
        // any older value so collateral accounting fails closed until a
        // valid price is fetched again.
        cache.remove(&lp_ledger);
    } else {
        cache.insert(lp_ledger, virtual_price);
    }
}

/// A pending absorb only apportions the exact stablecoin ledgers captured in
/// its durable intent. Unrelated-token deposits and withdrawals remain safe;
/// live liquidation and any intersecting ledger stay globally serialized.
pub(crate) fn pool_token_balance_mutation_blocked(ledgers: &[Principal]) -> bool {
    crate::pool_guard::liquidation_in_progress()
        || read_state(|state| {
            let chain_intersects = state.pending_chain_absorbs().iter().any(|intent| {
                ledgers
                    .iter()
                    .any(|ledger| intent.stables_consumed.contains_key(ledger))
            });
            let native_xrp_intersects = state.pending_native_xrp_absorbs().iter().any(|intent| {
                ledgers
                    .iter()
                    .any(|ledger| intent.stables_consumed.contains_key(ledger))
            });
            let three_usd_intersects = state
                .pending_three_usd_absorbs
                .as_ref()
                .map_or(false, |pending| {
                    pending
                        .values()
                        .any(|intent| ledgers.contains(&intent.ledger))
                });
            let ordinary_intersects = state
                .pending_ordinary_liquidations
                .as_ref()
                .map_or(false, |pending| {
                    pending.values().any(|intent| {
                        ledgers.contains(&intent.stable_ledger)
                            || intent
                                .approval
                                .as_ref()
                                .is_some_and(|approval| ledgers.contains(&approval.ledger))
                    })
                });
            chain_intersects || native_xrp_intersects || three_usd_intersects || ordinary_intersects
        })
}

pub(crate) fn ensure_pool_token_balance_mutation_allowed(
    ledgers: &[Principal],
) -> Result<(), StabilityPoolError> {
    if pool_token_balance_mutation_blocked(ledgers) {
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
    receipt_store::init_layout();
    state::save_to_stable_memory();
    receipt_store::set_layout_version(receipt_store::LAYOUT_VERSION);
    log!(
        INFO,
        "Stability Pool initialized. Protocol: {}",
        read_state(|s| s.protocol_canister_id)
    );
    ic_cdk_timers::set_timer(Duration::ZERO, || {
        setup_virtual_price_timer();
        setup_chain_absorb_auto_timer();
        setup_native_xrp_settle_sweep_timer();
        setup_native_xrp_absorb_recovery_timer();
        setup_three_usd_absorb_recovery_timer();
        setup_ordinary_liquidation_recovery_timer();
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
    let conversion_reconciled = conversion::reconcile_after_upgrade();
    if conversion_reconciled > 0 {
        log!(
            INFO,
            "SP conversion migration: retained {} in-flight conversion intents for exact-tuple reconciliation",
            conversion_reconciled
        );
    }
    mutate_state(|s| { if s.interest_recovery.is_none() { s.interest_recovery_bootstrap_hold=Some(true); } });
    let legacy_ambiguous_chain_absorbs = mutate_state(|s| {
        s.hold_legacy_prepared_chain_absorbs(ic_cdk::api::time())
    });
    if legacy_ambiguous_chain_absorbs > 0 {
        log!(
            INFO,
            "SP chain burn migration: held {} legacy Prepared intents as ambiguous; inspect burn_proof/legacy_ambiguous_burn before reconciliation",
            legacy_ambiguous_chain_absorbs
        );
    }
    mutate_state(|s| s.reconcile_pending_withdrawals_after_upgrade());
    let recovered_payouts = mutate_state(|s| {
        s.initialize_pending_outbound_payouts();
        let pending = s.pending_outbound_payouts.as_ref().map_or(0, |p| p.len());
        s.reconcile_pending_outbound_payouts_after_upgrade();
        pending
    });
    if recovered_payouts > 0 {
        log!(
            INFO,
            "SP payout migration: held {} outbound payout tuples for same-tuple reconciliation",
            recovered_payouts
        );
    }
    let (indexed, retained) = mutate_state(|s| {
        s.initialize_unallocated_interest_mint_index();
        (
            s.unallocated_interest_mint_index.is_some(),
            s.unallocated_interest_mint_index
                .as_ref()
                .map(BTreeMap::len)
                .unwrap_or(0),
        )
    });
    if !indexed {
        log!(
            INFO,
            "CL-10 migration: legacy unallocated-interest receipt history exceeds the bounded index; new notifications will remain pending for reconciliation"
        );
    } else {
        log!(
            INFO,
            "CL-10 migration: indexed {} retained unallocated-interest receipts",
            retained
        );
    }
    let migration = mutate_state(|s| receipt_store::migrate_interest_mint_receipts(s));
    let (receipt_count, legacy_floor) = match migration {
        Ok(count) => count,
        Err(reason) => ic_cdk::trap(&format!("SP interest receipt migration failed closed: {reason}")),
    };
    receipt_store::set_layout_version(receipt_store::LAYOUT_VERSION);
    if receipt_count >= state::INTEREST_MINT_RECEIPT_WARNING_THRESHOLD as u64 {
        log!(INFO, "CL-10 interest receipt capacity warning: {} permanent rows (legacy floor {:?}); 80% threshold {}, hard capacity {}. Never pruned; admission fails closed at capacity.", receipt_count, legacy_floor, state::INTEREST_MINT_RECEIPT_WARNING_THRESHOLD, state::MAX_INTEREST_MINT_RECEIPTS);
    }
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
    state::save_to_stable_memory();

    // Defer timer setup to avoid ic0_call_new restriction during upgrade
    ic_cdk_timers::set_timer(Duration::ZERO, || {
        setup_virtual_price_timer();
        setup_chain_absorb_auto_timer();
        setup_native_xrp_settle_sweep_timer();
        setup_native_xrp_absorb_recovery_timer();
        setup_three_usd_absorb_recovery_timer();
        setup_ordinary_liquidation_recovery_timer();
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
            if read_state(|s| s.configuration.emergency_pause) || interest_recovery::locked() { return; }
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

/// Resolve proof-bearing pending native-XRP burns by vault id. This is
/// intentionally separate from liquidation discovery: a vault can disappear
/// from the live liquidatable list after its burn, while its refund/absorb
/// outcome still requires reconciliation.
fn setup_native_xrp_absorb_recovery_timer() {
    thread_local! {
        static RECOVERY_CURSOR: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
    }
    ic_cdk_timers::set_timer_interval(
        Duration::from_secs(NATIVE_XRP_ABSORB_RECOVERY_POLL_SECONDS),
        || {
            if read_state(|s| s.configuration.emergency_pause) || interest_recovery::locked() { return; }
            ic_cdk::spawn(async {
                let cursor = RECOVERY_CURSOR.with(|c| c.get());
                let vault_ids = crate::liquidation::pending_native_xrp_recovery_vault_ids(
                    cursor,
                    NATIVE_XRP_ABSORB_RECOVERY_MAX_PER_TICK,
                );
                let mut last = None;
                for vault_id in vault_ids {
                    last = Some(vault_id);
                    if let Err(error) =
                        crate::liquidation::reconcile_pending_native_xrp_absorb_from_status(
                            vault_id,
                        )
                        .await
                    {
                        log!(
                            INFO,
                            "native XRP burn recovery for vault {} remains pending: {:?}",
                            vault_id,
                            error
                        );
                    }
                }
                if let Some(last) = last {
                    RECOVERY_CURSOR.with(|c| c.set(Some(last)));
                }
            });
        },
    );
}

fn setup_chain_absorb_auto_timer() {
    ic_cdk_timers::set_timer_interval(
        Duration::from_secs(CHAIN_ABSORB_AUTO_TIMER_POLL_SECONDS),
        || {
            if read_state(|s| s.configuration.emergency_pause) || interest_recovery::locked() { return; }
            ic_cdk::spawn(async {
                if let Err(error) = crate::liquidation::run_chain_absorb_auto_tick().await {
                    log!(INFO, "chain absorb auto tick skipped: {:?}", error);
                }
            });
        },
    );
}

/// Recover terminal 3USD reserve intents independently of the protocol's
/// current liquidatable-vault scan, which may omit a vault after a partial
/// absorb or deletion.
fn setup_three_usd_absorb_recovery_timer() {
    ic_cdk_timers::set_timer_interval(Duration::from_secs(30), || {
        if read_state(|s| s.configuration.emergency_pause) || interest_recovery::locked() { return; }
        ic_cdk::spawn(crate::liquidation::recover_pending_three_usd_absorbs());
    });
}

/// Retry only retained ordinary request identities. A status NotFound or
/// transport failure remains held; this timer never allocates a fresh nonce.
fn setup_ordinary_liquidation_recovery_timer() {
    ic_cdk_timers::set_timer_interval(
        Duration::from_secs(ORDINARY_LIQUIDATION_RECOVERY_POLL_SECONDS),
        || {
            if read_state(|s| s.configuration.emergency_pause) || interest_recovery::locked() {
                return;
            }
            ic_cdk::spawn(crate::liquidation::recover_pending_ordinary_liquidations());
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
            if read_state(|s| s.configuration.emergency_pause) || interest_recovery::locked() { return; }
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
                    cache_virtual_price(s, lp_ledger, status.virtual_price);
                });
                if status.virtual_price == 0 {
                    log!(
                        INFO,
                        "Rejected zero virtual price from {}; cache invalidated",
                        pool_canister
                    );
                }
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
pub async fn deposit(_token_ledger: Principal, _amount: u64) -> Result<(), StabilityPoolError> {
    interest_recovery::ensure_unlocked();
    // The legacy call has no operation identity. A client retry after an
    // ambiguous ledger reply can pull and credit a second time, so keep its
    // Candid signature for old clients but reject before any ledger call.
    Err(StabilityPoolError::SystemBusy)
}

/// Caller-scoped idempotent deposit. Reuse `intent_seq` and the identical
/// payload on every retry until a terminal result is returned.
#[update]
pub async fn deposit_with_intent(
    intent_seq: u64,
    token_ledger: Principal,
    amount: u64,
) -> Result<DepositIntentResult, DepositIntentError> {
    interest_recovery::ensure_unlocked();
    crate::deposits::deposit_with_intent(intent_seq, token_ledger, amount).await
}

/// Authoritative status update for recovery after reload or on another device.
#[update]
pub fn get_deposit_intent(intent_seq: u64) -> DepositIntentStatus {
    crate::deposits::deposit_intent_status(ic_cdk::api::caller(), intent_seq)
}

#[update]
pub async fn withdraw(token_ledger: Principal, amount: u64) -> Result<(), StabilityPoolError> {
    interest_recovery::ensure_unlocked();
    crate::deposits::withdraw(token_ledger, amount).await
}

/// Exact unresolved stablecoin withdrawal transfers for the caller.
#[query]
pub fn get_my_pending_withdrawals() -> Vec<PendingOutboundPayoutStatus> {
    read_state(|s| s.pending_withdrawals_for(&ic_cdk::api::caller()))
}

/// Settle an aged/ambiguous withdrawal from its exact ledger receipt.
#[update]
pub async fn reconcile_withdrawal(
    token_ledger: Principal,
    withdrawal_owner: Principal,
    block_index: u64,
) -> Result<u64, StabilityPoolError> {
    interest_recovery::ensure_unlocked();
    crate::withdrawals::reconcile(token_ledger, withdrawal_owner, block_index).await
}

#[update]
pub async fn claim_collateral(collateral_ledger: Principal) -> Result<u64, StabilityPoolError> {
    interest_recovery::ensure_unlocked();
    crate::deposits::claim_collateral(collateral_ledger).await
}

#[update]
pub async fn claim_all_collateral() -> Result<BTreeMap<Principal, u64>, StabilityPoolError> {
    interest_recovery::ensure_unlocked();
    crate::deposits::claim_all_collateral().await
}

/// Confirm an aged ambiguous collateral claim from an exact direct-ledger ICRC-3 block.
/// The claimant or an admin supplies the original owner, ledger, and candidate block index.
#[update]
pub async fn reconcile_collateral_claim(
    collateral_ledger: Principal,
    claim_owner: Principal,
    block_index: u64,
) -> Result<u64, StabilityPoolError> {
    interest_recovery::ensure_unlocked();
    crate::deposits::reconcile_collateral_claim(collateral_ledger, claim_owner, block_index).await
}

/// Caller-scoped status for exact collateral payout tuples held for retry.
#[query]
pub fn get_my_pending_collateral_claims() -> Vec<PendingOutboundPayoutStatus> {
    let caller = ic_cdk::api::caller();
    read_state(|s| s.pending_outbound_payouts_for(&caller))
}

/// Compatibility view of the first 10,000 caller-scoped settled collateral
/// receipts, preserving the prior endpoint bound. This returns the oldest
/// rows by transfer timestamp and cannot represent a larger history; use the
/// page method to traverse the complete permanent receipt journal.
#[query]
pub fn get_my_completed_collateral_claims() -> Vec<CompletedOutboundPayoutStatus> {
    let caller = ic_cdk::api::caller();
    receipt_store::all_for_owner_compat(caller)
}

/// Caller-scoped bounded page over every settled collateral-claim receipt.
#[query]
pub fn get_my_completed_collateral_claims_page(
    after_timestamp: Option<u64>,
    limit: u16,
) -> CompletedOutboundPayoutPage {
    let caller = ic_cdk::api::caller();
    let page = receipt_store::page(caller, after_timestamp, limit.into());
    CompletedOutboundPayoutPage {
        items: page.items,
        next_cursor: page.next_cursor,
        has_more: page.has_more,
    }
}


/// Convenience: deposit a stablecoin (icUSD, ckUSDT, ckUSDC) and have the pool
/// mint 3USD on the user's behalf by depositing into the 3pool.
#[update]
pub async fn deposit_as_3usd(
    token_ledger: Principal,
    amount: u64,
) -> Result<u64, StabilityPoolError> {
    interest_recovery::ensure_unlocked();
    crate::deposits::deposit_as_3usd(token_ledger, amount).await
}

/// Start or resume a durable escrow-backed conversion. The caller owns the
/// sequence and explicit downstream fee budget; a duplicate request returns
/// the retained status without allocating another escrow account.
#[update]
pub async fn start_3usd_conversion(
    request: conversion::ConversionRequest,
) -> Result<conversion::ConversionStatus, conversion::ConversionError> {
    interest_recovery::ensure_unlocked();
    conversion::start_conversion(request).await
}

/// Resume one retained conversion using its exact persisted identities.
#[update]
pub async fn resume_3usd_conversion(
    intent_seq: u64,
) -> Result<conversion::ConversionStatus, conversion::ConversionError> {
    interest_recovery::ensure_unlocked();
    conversion::resume_conversion(intent_seq).await
}

/// Authenticate an aged ambiguous ingress pull against an exact ledger block,
/// then resume the same durable conversion intent.
#[update]
pub async fn reconcile_3usd_conversion_pull_v1(
    intent_seq: u64,
    candidate_block: u64,
) -> Result<conversion::ConversionStatus, conversion::ConversionError> {
    interest_recovery::ensure_unlocked();
    conversion::reconcile_pull(intent_seq, candidate_block).await
}

/// Reconcile only this caller's saved pool input using positive ledger proof.
#[update]
pub async fn reconcile_3usd_conversion_pool_input_v1(
    intent_seq: u64,
    leg_index: u8,
    candidate_block: u64,
) -> Result<conversion::ConversionStatus, conversion::ConversionError> {
    interest_recovery::ensure_unlocked();
    conversion::reconcile_pool_input(intent_seq, leg_index, candidate_block).await
}

/// Reconcile an aged escrow approval only from its exact directly served
/// ICRC-3 approval block. The caller selects only its own durable intent.
#[update]
pub async fn reconcile_3usd_conversion_approval_v1(
    intent_seq: u64,
    candidate_block: u64,
) -> Result<conversion::ConversionStatus, conversion::ConversionError> {
    interest_recovery::ensure_unlocked();
    conversion::reconcile_approval(intent_seq, candidate_block).await
}

/// Reconcile an aged allowance revoke only from its exact directly served
/// ICRC-3 approval block.
#[update]
pub async fn reconcile_3usd_conversion_revoke_v1(
    intent_seq: u64,
    candidate_block: u64,
) -> Result<conversion::ConversionStatus, conversion::ConversionError> {
    interest_recovery::ensure_unlocked();
    conversion::reconcile_revoke(intent_seq, candidate_block).await
}

/// Reconcile an aged escrow refund only from its exact directly served ICRC-3
/// transfer block.
#[update]
pub async fn reconcile_3usd_conversion_refund_v1(
    intent_seq: u64,
    candidate_block: u64,
) -> Result<conversion::ConversionStatus, conversion::ConversionError> {
    interest_recovery::ensure_unlocked();
    conversion::reconcile_refund(intent_seq, candidate_block).await
}

/// Read the caller's exact conversion status. Missing rows are not interpreted
/// as evidence that a transfer had no effect.
#[query]
pub fn get_3usd_conversion_status(
    intent_seq: u64,
) -> Result<conversion::ConversionStatus, conversion::ConversionError> {
    conversion::get_status(intent_seq)
}

#[query]
pub fn get_active_3usd_conversion() -> Option<conversion::ConversionStatus> {
    conversion::get_active_status()
}

/// Return the next caller-scoped sequence from the durable conversion
/// watermark. Clients must use this value instead of guessing after a lost
/// response.
#[query]
pub fn get_3usd_conversion_next_seq() -> Result<u64, conversion::ConversionError> {
    conversion::next_sequence()
}

/// Return the active request metadata together with its status so a client can
/// resume the exact durable intent across devices.
#[query]
pub fn get_active_3usd_conversion_v1() -> Option<conversion::ConversionView> {
    conversion::get_active_view()
}

/// Cancel only a journal row that has not dispatched a user pull. Once a value
/// moving tuple exists, the caller must resume/reconcile that exact row.
#[update]
pub fn cancel_3usd_conversion(
    intent_seq: u64,
) -> Result<conversion::ConversionStatus, conversion::ConversionError> {
    interest_recovery::ensure_unlocked();
    conversion::cancel_conversion(intent_seq)
}

/// Read-only quote/admission helper. It performs only ledger/3pool queries and
/// never creates a conversion intent or pulls funds.
#[update]
pub async fn quote_3usd_conversion(
    token_ledger: Principal,
    principal_raw: u64,
    min_lp: u128,
) -> Result<conversion::ConversionQuote, conversion::ConversionError> {
    conversion::quote_conversion(token_ledger, principal_raw, min_lp).await
}

/// Recover tokens the pool owes after a failed `deposit_as_3usd` refund
/// (audit IC-S-001). Callable by the original user or a pool admin. Returns
/// the net amount sent (gross minus the ledger transfer fee).
#[update]
pub async fn claim_pending_refund(refund_id: u64) -> Result<u64, StabilityPoolError> {
    interest_recovery::ensure_unlocked();
    crate::deposits::claim_pending_refund(refund_id).await
}

#[update]
pub async fn claim_cfx(
    chain_sentinel: Principal,
    dest_evm: String,
) -> Result<u128, StabilityPoolError> {
    interest_recovery::ensure_unlocked();
    crate::liquidation::claim_cfx(chain_sentinel, dest_evm).await
}

#[update]
pub fn recredit_failed_cfx_claim_payout(
    recovery: CfxClaimPayoutRecovery,
) -> Result<bool, StabilityPoolError> {
    interest_recovery::ensure_unlocked();
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
    interest_recovery::ensure_unlocked();
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
    interest_recovery::ensure_unlocked();
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
    interest_recovery::ensure_unlocked();
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
    interest_recovery::ensure_unlocked();
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
    interest_recovery::ensure_unlocked();
    opt_in_native_collateral_with_tag(collateral_type, payout_address, None)
}

#[update]
pub fn opt_in_native_collateral_with_tag(
    collateral_type: Principal,
    payout_address: String,
    destination_tag: Option<u32>,
) -> Result<(), StabilityPoolError> {
    interest_recovery::ensure_unlocked();
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
    interest_recovery::ensure_unlocked();
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
    interest_recovery::ensure_unlocked();
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
    interest_recovery::ensure_unlocked();
    crate::liquidation::execute_liquidation(vault_id).await
}

/// Read the durable ordinary request by its exact 32-byte identity. Unknown
/// requests return `None`; a status miss never authorizes a fresh request ID.
#[query]
pub fn get_ordinary_liquidation_status(
    request_id: Vec<u8>,
) -> Option<ordinary_liquidation::OrdinaryLiquidationStatus> {
    crate::liquidation::ordinary_liquidation_status(request_id)
}

/// Retry one ordinary request using its persisted approval/backend tuples.
/// This endpoint never allocates a new request ID after an unknown callback.
#[update]
pub async fn retry_ordinary_liquidation(
    request_id: Vec<u8>,
) -> Result<LiquidationResult, StabilityPoolError> {
    interest_recovery::ensure_unlocked();
    crate::liquidation::retry_ordinary_liquidation(request_id).await
}

/// Cancel only a prepared ordinary request that has not dispatched approval.
/// The completion tombstone is written before the pending row is released.
#[update]
pub async fn cancel_ordinary_liquidation(
    request_id: Vec<u8>,
) -> Result<LiquidationResult, StabilityPoolError> {
    interest_recovery::ensure_unlocked();
    let caller = ic_cdk::api::caller();
    if caller == Principal::anonymous() || !read_state(|state| state.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    crate::liquidation::cancel_ordinary_liquidation(request_id).await
}

/// Resolve an aged approval using a block served by its configured ledger.
/// The candidate is never accepted without the complete persisted tuple.
#[update]
pub async fn reconcile_ordinary_liquidation_approval(
    request_id: Vec<u8>,
    candidate_block_index: u64,
) -> Result<LiquidationResult, StabilityPoolError> {
    interest_recovery::ensure_unlocked();
    let caller = ic_cdk::api::caller();
    if caller == Principal::anonymous() || !read_state(|state| state.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    crate::liquidation::reconcile_ordinary_liquidation_approval(request_id, candidate_block_index).await
}

#[update]
pub async fn sp_absorb_chain_vault(
    vault_id: u64,
) -> Result<ChainSpAbsorbResult, StabilityPoolError> {
    interest_recovery::ensure_unlocked();
    crate::liquidation::sp_absorb_chain_vault(vault_id).await
}

/// Admin recovery for a native-XRP burn whose ledger call reply was lost.
/// The supplied block index is only a candidate: it is accepted only after
/// the pinned ledger returns the exact direct ICRC-3 block for the persisted
/// burn tuple. When omitted, recovery checks a bounded direct tail of history.
/// Archived candidates remain held because archive callbacks are not accepted
/// as proof of canonical ledger membership.
#[update]
pub async fn admin_recover_ambiguous_native_xrp_burn(
    vault_id: u64,
    candidate_block_index: Option<u64>,
) -> Result<(), StabilityPoolError> {
    interest_recovery::ensure_unlocked();
    let caller = ic_cdk::api::caller();
    if caller == Principal::anonymous() || !read_state(|state| state.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    crate::liquidation::recover_ambiguous_native_xrp_burn_proof(vault_id, candidate_block_index)
        .await?;
    crate::liquidation::reconcile_pending_native_xrp_absorb_from_status(vault_id).await
}

/// Admin recovery for an ambiguous chain-vault icUSD burn. The optional index
/// is only a candidate: direct ledger evidence must match the retained intent.
/// On success, the existing idempotent absorb path resumes backend accounting
/// from the proof without issuing another burn.
#[update]
pub async fn admin_recover_ambiguous_chain_burn(
    vault_id: u64,
    candidate_block_index: Option<u64>,
) -> Result<ChainSpAbsorbResult, StabilityPoolError> {
    interest_recovery::ensure_unlocked();
    let caller = ic_cdk::api::caller();
    if caller == Principal::anonymous() || !read_state(|state| state.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    crate::liquidation::recover_ambiguous_chain_burn_proof(vault_id, candidate_block_index).await?;
    crate::liquidation::sp_absorb_chain_vault(vault_id).await
}

#[update]
pub async fn scan_chain_absorb_candidates(
    max_per_chain: Option<u64>,
) -> Result<Vec<ChainSpAbsorbCandidate>, StabilityPoolError> {
    interest_recovery::ensure_unlocked();
    crate::liquidation::scan_chain_absorb_candidates(max_per_chain).await
}

#[update]
pub fn set_chain_absorb_auto_config(
    config: ChainAbsorbAutoConfig,
) -> Result<(), StabilityPoolError> {
    interest_recovery::ensure_unlocked();
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
    interest_recovery::ensure_unlocked();
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
    let payload = state::InterestMintReceiptPayload {
        token_ledger,
        amount,
        collateral_type: collateral_type.clone(),
    };
    match read_state(|s| receipt_store::interest_mint_receipt_status(s, source_mint_block, &payload)) {
        state::InterestMintReceiptStatus::PendingForward(batch_id) => {
            if read_state(|s| s.configuration.emergency_pause) || interest_recovery::locked() { return Err(StabilityPoolError::SystemBusy); }
            return process_unallocated_interest_forward(batch_id).await;
        }
        state::InterestMintReceiptStatus::Duplicate => return Ok(()),
        state::InterestMintReceiptStatus::LegacyUnknown
        | state::InterestMintReceiptStatus::PayloadConflict
        | state::InterestMintReceiptStatus::CapacityReached
        | state::InterestMintReceiptStatus::OutsideReplayWindow => {
            // Keep the backend's durable notification pending. A stale receipt
            // must never be acknowledged as a new distribution.
            return Err(StabilityPoolError::SystemBusy);
        }
        state::InterestMintReceiptStatus::New => {}
    }

    interest_recovery::ensure_unlocked();
    ensure_pool_balance_mutation_allowed()?;
    let paused=read_state(|s| s.configuration.emergency_pause);
    if paused && !interest_recovery::live() { return Err(StabilityPoolError::EmergencyPaused); }
    if !read_state(|s| s.stablecoin_registry.contains_key(&token_ledger)) { return Err(StabilityPoolError::TokenNotAccepted { ledger: token_ledger }); }
    if paused && !read_state(|s| s.stablecoin_registry.get(&token_ledger).is_some_and(|c|c.symbol=="icUSD" && c.decimals==8)
        && collateral_type.is_some_and(|c|s.collateral_registry.contains_key(&c))
        && s.has_eligible_interest_recipient(collateral_type.as_ref())) { return Err(StabilityPoolError::SystemBusy); }
    if read_state(|s| s.has_eligible_interest_recipient(collateral_type.as_ref())) {
        return match mutate_state(
            |s| match receipt_store::record_interest_mint_receipt(s, source_mint_block, state::InterestMintReceipt {
                payload: Some(payload.clone()),
                disposition: state::InterestMintReceiptDisposition::Distributed,
            }) {
                state::InterestMintReceiptStatus::New => {
                    s.record_interest_mint_legacy_cache(source_mint_block);
                    s.distribute_interest_revenue(token_ledger, amount, collateral_type);
                    s.push_event(
                        caller,
                        PoolEventType::InterestReceived {
                            token_ledger,
                            amount,
                        },
                    );
                    Ok(())
                }
                state::InterestMintReceiptStatus::Duplicate => Ok(()),
                _ => Err(StabilityPoolError::SystemBusy),
            },
        ) {
            Ok(()) => Ok(()),
            Err(error) => Err(error),
        };
    }

    let batch_id = mutate_state(|s| {
        s.queue_unallocated_interest_forward_at_with_admission(
            source_mint_block, token_ledger, amount, ic_cdk::api::time(),
            |pool, id| match receipt_store::record_interest_mint_receipt(pool, source_mint_block, state::InterestMintReceipt {
                payload: Some(payload), disposition: state::InterestMintReceiptDisposition::ForwardPending(id),
            }) {
                state::InterestMintReceiptStatus::New => Ok(()),
                _ => Err(StabilityPoolError::SystemBusy),
            },
        )
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

    let source_blocks = read_state(|s| s.unallocated_interest_forward_batch(batch_id).map(|b| b.source_mint_blocks).unwrap_or_default());
    mutate_state(|s| {
        s.mark_unallocated_interest_forward_recorded(batch_id);
    });
    receipt_store::mark_interest_forwarded(&source_blocks, batch_id);
    Ok(())
}

// ─── Queries ───

#[query]
pub fn get_pool_status() -> StabilityPoolStatus {
    read_state(|s| s.get_pool_status())
}

#[query]
pub fn get_interest_mint_receipt_capacity() -> state::InterestMintReceiptCapacity {
    read_state(receipt_store::interest_mint_receipt_capacity)
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

/// Authorized-admin inventory of pending 3USD intents and retained completed IDs.
#[query]
pub fn admin_get_three_usd_absorb_inventory_page(
    after: Option<ThreeUsdAbsorbInventoryCursor>,
    limit: u16,
) -> Result<ThreeUsdAbsorbInventoryPage, StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    let authorized = read_state(|state| {
        inventory_admin_authorized(caller, &state.configuration.authorized_admins)
    });
    if !authorized {
        return Err(StabilityPoolError::Unauthorized);
    }
    Ok(read_state(|state| {
        state.three_usd_absorb_inventory_page(after, limit)
    }))
}

/// Register this deployed SP as the V2 reserve client without starting a
/// liquidation or reserving any depositor funds.
#[update]
pub async fn admin_acknowledge_three_usd_reserve_v2_client() -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    let protocol = read_state(|state| {
        if !inventory_admin_authorized(caller, &state.configuration.authorized_admins) {
            return Err(StabilityPoolError::Unauthorized);
        }
        Ok(state.protocol_canister_id)
    })?;
    let response: Result<(Result<(), rumi_protocol_backend::ProtocolError>,), _> =
        ic_cdk::call(protocol, "acknowledge_three_usd_reserve_v2_client", ()).await;
    read_state(|state| {
        if !inventory_admin_authorized(caller, &state.configuration.authorized_admins)
            || state.protocol_canister_id != protocol
        {
            return Err(StabilityPoolError::Unauthorized);
        }
        Ok(())
    })?;
    match response {
        Ok((Ok(()),)) => Ok(()),
        other => Err(StabilityPoolError::LedgerTransferFailed {
            reason: format!("V2 client acknowledgment: {:?}", other),
        }),
    }
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
    interest_recovery::ensure_unlocked();
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
    interest_recovery::ensure_unlocked();
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
    interest_recovery::ensure_unlocked();
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
    interest_recovery::ensure_unlocked();
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
    interest_recovery::ensure_unlocked();
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
    interest_recovery::ensure_unlocked();
    let message_text = match request.method.as_str() {
        "deposit" => {
            "## Legacy Stability Pool Deposit Unavailable\n\n\
             This older deposit method is disabled. Update the app to use a retry-safe deposit intent."
                .to_string()
        }
        "deposit_with_intent" => {
            match candid::decode_args::<(u64, Principal, u64)>(&request.arg) {
                Ok((intent_seq, token_ledger, amount)) => {
                    let (symbol, decimals) = read_state(|s| {
                        s.stablecoin_registry
                            .get(&token_ledger)
                            .map(|c| (c.symbol.clone(), c.decimals))
                            .unwrap_or_else(|| (format!("token {}", token_ledger), 8))
                    });
                    format!(
                        "## Deposit to Stability Pool\n\n\
                         You are depositing **{} {}** into the Rumi Protocol Stability Pool. \
                         Intent #{} keeps retries tied to this same transfer.",
                        format_token_amount(amount, decimals),
                        symbol,
                        intent_seq
                    )
                }
                Err(_) => "Deposit stablecoins into the Rumi Protocol Stability Pool with a retry-safe intent.".to_string(),
            }
        }
        "get_deposit_intent" => {
            "## Check Stability Pool Deposit\n\n\
             You are checking the status of your pending deposit so the app can safely resume or finish it."
                .to_string()
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
        // The conversion actor is deliberately created through Oisy.  Keep
        // these calls in the consent surface even though quote is economically
        // read-only: Oisy asks for ICRC-21 consent for update methods, and the
        // recovery calls must describe the retained journal row rather than
        // presenting an opaque, generic approval.
        method if conversion_consent_method(method) => conversion_consent_message(
            method,
            &request.arg,
            ic_cdk::api::caller(),
        ),
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

/// Methods in the durable 3USD conversion flow that may be invoked through
/// the user's Oisy actor.  This list is intentionally explicit so a new
/// update method cannot silently acquire a generic wallet approval message.
fn conversion_consent_method(method: &str) -> bool {
    matches!(
        method,
        "quote_3usd_conversion"
            | "start_3usd_conversion"
            | "resume_3usd_conversion"
            | "cancel_3usd_conversion"
            | "reconcile_3usd_conversion_pull_v1"
            | "reconcile_3usd_conversion_pool_input_v1"
            | "reconcile_3usd_conversion_approval_v1"
            | "reconcile_3usd_conversion_revoke_v1"
            | "reconcile_3usd_conversion_refund_v1"
    )
}

/// Render consent for the actual conversion wire arguments.  Each malformed
/// argument path remains bounded to the named method and makes no claim about
/// an amount, ledger, or transfer.  That is safer than showing a generic
/// approval when an Oisy caller supplied bytes that did not decode.
fn conversion_consent_message(method: &str, arg: &[u8], caller: Principal) -> String {
    let malformed = || match method {
        "quote_3usd_conversion" => {
            "## Quote 3USD Conversion\n\n\
             You are requesting a read-only 3USD conversion quote. Verify the \
             exact token ledger, amount, and minimum LP output shown by the \
             wallet. This call does not create an intent or move funds."
                .to_string()
        }
        "start_3usd_conversion" => {
            "## Start 3USD Conversion\n\n\
             You are authorizing a durable 3USD conversion intent. Verify the \
             exact caller, token ledger, amount, downstream fee budget, and \
             minimum LP output shown by the wallet. Retries remain bound to the \
             same stored intent; this message does not authorize an arbitrary \
             transfer."
                .to_string()
        }
        "resume_3usd_conversion" => {
            "## Resume 3USD Conversion\n\n\
             You are authorizing recovery of one stored 3USD conversion intent. \
             Verify the exact intent shown by the wallet. This resumes the same \
             persisted request and does not create a new sequence or fresh spend."
                .to_string()
        }
        "cancel_3usd_conversion" => {
            "## Cancel 3USD Conversion\n\n\
             You are requesting cancellation of one stored 3USD conversion \
             intent. Cancellation is limited to that journaled request and does \
             not authorize a new transfer or arbitrary refund."
                .to_string()
        }
        "reconcile_3usd_conversion_pull_v1" => {
            "## Reconcile 3USD Conversion Pull\n\n\
             You are reconciling one stored conversion intent against a candidate \
             token-ledger block. This verifies the same ingress pull and does not \
             create a new intent or fresh spend."
                .to_string()
        }
        "reconcile_3usd_conversion_pool_input_v1" => {
            "## Reconcile 3USD Pool Input\n\n\
             You are reconciling one stored conversion intent and pool leg against \
             a candidate block. This verifies the same pool input and does not \
             create a new intent or fresh spend."
                .to_string()
        }
        "reconcile_3usd_conversion_approval_v1" => {
            "## Reconcile 3USD Conversion Approval\n\n\
             You are reconciling the approval proof for one stored conversion \
             intent against a candidate ledger block. This verifies the same \
             approval and does not authorize a new transfer."
                .to_string()
        }
        "reconcile_3usd_conversion_revoke_v1" => {
            "## Reconcile 3USD Conversion Revoke\n\n\
             You are reconciling the allowance revoke for one stored conversion \
             intent against a candidate ledger block. This verifies the same \
             revoke and does not authorize a new transfer."
                .to_string()
        }
        "reconcile_3usd_conversion_refund_v1" => {
            "## Reconcile 3USD Conversion Refund\n\n\
             You are reconciling the refund proof for one stored conversion \
             intent against a candidate ledger block. This verifies the same \
             refund and does not authorize a new transfer or arbitrary refund."
                .to_string()
        }
        _ => "This conversion method is not supported for wallet consent.".to_string(),
    };

    let token_details = |token_ledger: Principal| {
        read_state(|s| {
            s.stablecoin_registry
                .get(&token_ledger)
                .map(|config| (config.symbol.clone(), config.decimals))
                .unwrap_or_else(|| (format!("token {}", token_ledger), 8))
        })
    };

    match method {
        "quote_3usd_conversion" => {
            match candid::decode_args::<(Principal, u64, u128)>(arg) {
                Ok((token_ledger, principal_raw, min_lp)) => {
                    let (symbol, decimals) = token_details(token_ledger);
                    format!(
                        "## Quote 3USD Conversion\n\n\
                         You are requesting a read-only 3USD conversion quote for **{} {}** \
                         from token ledger `{}` with a minimum output of **{} LP base units**. \
                         This call only queries the configured route; it does not create a \
                         durable intent or move funds. Any wallet ingress ledger fee is \
                         reported separately from a downstream fee budget.",
                        format_token_amount(principal_raw, decimals),
                        symbol,
                        token_ledger,
                        min_lp
                    )
                }
                Err(_) => malformed(),
            }
        }
        "start_3usd_conversion" => {
            match candid::decode_args::<(conversion::ConversionRequest,)>(arg) {
                Ok((request,)) => {
                    let (symbol, decimals) = token_details(request.token_ledger);
                    format!(
                        "## Start 3USD Conversion\n\n\
                         Caller principal `{}` is authorizing durable conversion intent **#{}** \
                         for **{} {}** from token ledger `{}`. The explicit downstream fee \
                         budget is **{} {}** and the minimum LP output is **{} LP base units**. \
                         The wallet ingress transfer fee is charged separately by the token \
                         ledger and is not included in this downstream escrow fee budget. \
                         Retries remain bound to this same intent and request.",
                        caller,
                        request.intent_seq,
                        format_token_amount(request.principal_raw, decimals),
                        symbol,
                        request.token_ledger,
                        format_token_amount(request.max_fee_budget_raw, decimals),
                        symbol,
                        request.min_lp
                    )
                }
                Err(_) => malformed(),
            }
        }
        "resume_3usd_conversion" => match candid::decode_args::<(u64,)>(arg) {
            Ok((intent_seq,)) => format!(
                "## Resume 3USD Conversion\n\n\
                 Caller principal `{}` is authorizing recovery of stored conversion intent \
                 **#{}**. This resumes the exact persisted token, amount, fee budget, and \
                 minimum LP request; it does not create a new sequence or authorize a fresh \
                 spend.",
                caller, intent_seq
            ),
            Err(_) => malformed(),
        },
        "cancel_3usd_conversion" => match candid::decode_args::<(u64,)>(arg) {
            Ok((intent_seq,)) => format!(
                "## Cancel 3USD Conversion\n\n\
                 Caller principal `{}` is requesting cancellation of stored conversion intent \
                 **#{}**. Cancellation is limited to the exact journaled request and is only \
                 effective before value movement; otherwise that same intent remains for \
                 status or reconciliation. This call does not authorize a new transfer or \
                 arbitrary refund.",
                caller, intent_seq
            ),
            Err(_) => malformed(),
        },
        "reconcile_3usd_conversion_pull_v1" => {
            match candid::decode_args::<(u64, u64)>(arg) {
                Ok((intent_seq, candidate_block)) => format!(
                    "## Reconcile 3USD Conversion Pull\n\n\
                     Caller principal `{}` is reconciling the exact stored conversion intent \
                     **#{}** using candidate token-ledger block **#{}**. This verifies or \
                     resumes recovery of the same ingress pull; it does not create a new \
                     intent or fresh spend.",
                    caller, intent_seq, candidate_block
                ),
                Err(_) => malformed(),
            }
        }
        "reconcile_3usd_conversion_pool_input_v1" => {
            match candid::decode_args::<(u64, u8, u64)>(arg) {
                Ok((intent_seq, leg_index, candidate_block)) => format!(
                    "## Reconcile 3USD Pool Input\n\n\
                     Caller principal `{}` is reconciling the exact stored conversion intent \
                     **#{}**, pool leg **#{}**, using candidate block **#{}**. This verifies \
                     the same pool input; it does not create a new intent or fresh spend.",
                    caller, intent_seq, leg_index, candidate_block
                ),
                Err(_) => malformed(),
            }
        }
        "reconcile_3usd_conversion_approval_v1"
        | "reconcile_3usd_conversion_revoke_v1"
        | "reconcile_3usd_conversion_refund_v1" => {
            let title = match method {
                "reconcile_3usd_conversion_approval_v1" => "Approval",
                "reconcile_3usd_conversion_revoke_v1" => "Allowance Revoke",
                _ => "Refund",
            };
            match candid::decode_args::<(u64, u64)>(arg) {
                Ok((intent_seq, candidate_block)) => format!(
                    "## Reconcile 3USD Conversion {}\n\n\
                     Caller principal `{}` is reconciling the exact stored conversion intent \
                     **#{}** against candidate ledger block **#{}**. This verifies the same \
                     {} operation and does not authorize a new transfer.",
                    title.to_string(),
                    caller,
                    intent_seq,
                    candidate_block,
                    title.to_lowercase()
                ),
                Err(_) => malformed(),
            }
        }
        _ => malformed(),
    }
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
    interest_recovery::ensure_unlocked();
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
    interest_recovery::ensure_unlocked();
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
    interest_recovery::ensure_unlocked();
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
    interest_recovery::ensure_unlocked();
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
    interest_recovery::ensure_unlocked();
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
    interest_recovery::ensure_unlocked();
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

    #[test]
    fn zero_virtual_price_invalidates_cached_pool_value() {
        let lp_ledger = principal(31);
        let mut state = state::StabilityPoolState::default();

        cache_virtual_price(&mut state, lp_ledger, 1_000_000_000_000_000_000);
        assert_eq!(
            state.virtual_prices().get(&lp_ledger),
            Some(&1_000_000_000_000_000_000)
        );

        cache_virtual_price(&mut state, lp_ledger, 0);
        assert_eq!(state.virtual_prices().get(&lp_ledger), None);
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
            burn_receipt_nat: Some(candid::Nat::from(44u8)),
            legacy_ambiguous_burn: None,
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
    fn pending_three_usd_absorb_blocks_its_ledger_balance_mutations() {
        crate::state::replace_state(crate::state::StabilityPoolState::default());
        let three_usd = principal(11);
        let unrelated = principal(12);
        mutate_state(|state| {
            state
                .prepare_three_usd_absorb(77, 100, 100, three_usd, principal(90), 100_000_000)
                .unwrap();
        });

        assert!(pool_token_balance_mutation_blocked(&[three_usd]));
        assert!(pool_token_balance_mutation_blocked(&[unrelated, three_usd]));
        assert!(!pool_token_balance_mutation_blocked(&[unrelated]));
        assert!(matches!(
            futures::executor::block_on(crate::deposits::deposit(three_usd, 10)),
            Err(StabilityPoolError::SystemBusy)
        ));
        assert!(matches!(
            futures::executor::block_on(crate::deposits::withdraw(three_usd, 10)),
            Err(StabilityPoolError::SystemBusy)
        ));
        assert!(matches!(
            futures::executor::block_on(crate::deposits::claim_collateral(three_usd)),
            Err(StabilityPoolError::SystemBusy)
        ));
        assert!(matches!(
            futures::executor::block_on(crate::deposits::claim_all_collateral()),
            Err(StabilityPoolError::SystemBusy)
        ));
        assert_eq!(read_state(|state| state.deposits.len()), 0);

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

    #[test]
    fn conversion_consent_dispatch_is_bounded_and_quote_is_read_only() {
        let token = principal(41);
        let args = candid::encode_args((token, 125_000_000u64, 99u128)).unwrap();
        let message = conversion_consent_message("quote_3usd_conversion", &args, principal(7));

        assert!(conversion_consent_method("quote_3usd_conversion"));
        assert!(!conversion_consent_method("some_future_update"));
        assert!(message.contains("1.25"));
        assert!(message.contains(token.to_text().as_str()));
        assert!(message.contains("99 LP base units"));
        assert!(message.contains("read-only"));
        assert!(message.contains("does not create a durable intent or move funds"));
    }

    #[test]
    fn conversion_start_consent_decodes_wire_request_and_discloses_budget() {
        let request = conversion::ConversionRequest {
            intent_seq: 7,
            token_ledger: principal(42),
            principal_raw: 12_500,
            max_fee_budget_raw: 275,
            min_lp: 888,
        };
        let args = candid::encode_args((request.clone(),)).unwrap();
        let caller = principal(9);
        let message = conversion_consent_message("start_3usd_conversion", &args, caller);

        assert!(message.contains(caller.to_text().as_str()));
        assert!(message.contains("intent **#7**"));
        assert!(message.contains("0.000125"));
        assert!(message.contains("0.00000275"));
        assert!(message.contains(request.token_ledger.to_text().as_str()));
        assert!(message.contains("888 LP base units"));
        assert!(message.contains("ingress transfer fee is charged separately"));
        assert!(message.contains("same intent and request"));
    }

    #[test]
    fn conversion_recovery_consent_binds_same_intent_and_candidate_proof() {
        let caller = principal(10);
        let pull_args = candid::encode_args((7u64, 123u64)).unwrap();
        let pull_message = conversion_consent_message(
            "reconcile_3usd_conversion_pull_v1",
            &pull_args,
            caller,
        );
        assert!(pull_message.contains("intent **#7**"));
        assert!(pull_message.contains("block **#123**"));
        assert!(pull_message.contains("same ingress pull"));
        assert!(pull_message.contains("does not create a new intent or fresh spend"));

        let pool_args = candid::encode_args((7u64, 2u8, 456u64)).unwrap();
        let pool_message = conversion_consent_message(
            "reconcile_3usd_conversion_pool_input_v1",
            &pool_args,
            caller,
        );
        assert!(pool_message.contains("pool leg **#2**"));
        assert!(pool_message.contains("candidate block **#456**"));

        let resume_args = candid::encode_args((7u64,)).unwrap();
        let resume_message =
            conversion_consent_message("resume_3usd_conversion", &resume_args, caller);
        assert!(resume_message.contains("exact persisted token, amount, fee budget"));
        assert!(resume_message.contains("does not create a new sequence or authorize a fresh spend"));

        let cancel_args = candid::encode_args((7u64,)).unwrap();
        let cancel_message =
            conversion_consent_message("cancel_3usd_conversion", &cancel_args, caller);
        assert!(cancel_message.contains("stored conversion intent"));
        assert!(cancel_message.contains("not authorize a new transfer or arbitrary refund"));
    }

    #[test]
    fn conversion_consent_malformed_args_do_not_claim_amount_or_opaque_approval() {
        let message = conversion_consent_message("start_3usd_conversion", &[], principal(11));
        assert!(message.contains("exact caller, token ledger, amount"));
        assert!(message.contains("does not authorize an arbitrary transfer"));
        assert!(!message.contains("0 LP base units"));
        assert!(!message.contains("0.0"));
    }
}

// Scoped recovery endpoints bind the exact reviewed allocation seal; execution needs separate authority.
#[update]
pub fn stage_interest_recovery(manifest: interest_recovery::RecoveryManifest) -> Result<interest_recovery::RecoveryStatus, String> { interest_recovery::stage(manifest) }
#[update]
pub fn continue_interest_recovery(manifest_hash: Vec<u8>, expected_next: u16, limit: u8) -> Result<interest_recovery::RecoveryStatus, String> { interest_recovery::continue_batch(manifest_hash,expected_next,limit) }
#[update]
pub fn finish_interest_recovery(manifest_hash: Vec<u8>) -> Result<interest_recovery::RecoveryStatus, String> { interest_recovery::finish(manifest_hash) }
#[query]
pub fn get_interest_recovery_status() -> Result<Option<interest_recovery::RecoveryStatus>, String> { interest_recovery::get_status() }
/// Replicated read for the configured backend to prove an already-applied source credit.
#[update]
pub fn get_interest_recovery_packet_receipt(source_mint_block: u64) -> Result<Option<interest_recovery::RecoveryPacketReceipt>, String> { interest_recovery::get_receipt(source_mint_block) }
