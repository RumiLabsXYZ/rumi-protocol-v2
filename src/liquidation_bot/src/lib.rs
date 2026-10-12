use candid::{CandidType, Deserialize, Nat, Principal};
use ic_cdk_macros::{init, post_upgrade, pre_upgrade, query, update};
use ic_canister_log::{declare_log_buffer, log};
use icrc_ledger_types::icrc1::account::Account;
use std::cell::RefCell;

mod history;
mod icpswap;
mod memory;
mod native_icp_blocks;
mod process;
mod state;
mod swap;

use state::{BotAdminAction, BotAdminEvent, BotConfig, BotState, LiquidatableVaultInfo};

declare_log_buffer!(name = INFO, capacity = 1000);

thread_local! {
    /// Reentrancy guard: true while process_pending or an admin async endpoint is running.
    static PROCESSING: RefCell<bool> = RefCell::new(false);
}

struct ProcessingGuard;

impl ProcessingGuard {
    fn acquire() -> Result<Self, &'static str> {
        PROCESSING.with(|p| {
            let mut flag = p.borrow_mut();
            if *flag {
                return Err("Already processing");
            }
            *flag = true;
            Ok(Self)
        })
    }
}

impl Drop for ProcessingGuard {
    fn drop(&mut self) {
        PROCESSING.with(|p| *p.borrow_mut() = false);
    }
}

#[derive(CandidType, Deserialize)]
pub struct BotInitArgs {
    pub config: BotConfig,
}

#[init]
fn init(args: BotInitArgs) {
    // UPG-006: refuse to init with non-empty stable memory. Catches accidental
    // reinstalls of a canister that already has persisted state. Must run BEFORE
    // memory::init_memory_manager() because that call writes the MGR magic header
    // at offset 0, which would defeat the check on subsequent inits.
    assert!(
        ic_cdk::api::stable::stable64_size() == 0,
        "refusing to init: stable memory non-empty; use upgrade mode not reinstall"
    );
    memory::init_memory_manager();
    history::init_history();
    state::init_state(BotState {
        config: Some(args.config),
        migrated_to_stable_structures: true,
        ..Default::default()
    });
    setup_timer();
}

#[pre_upgrade]
fn pre_upgrade() {
    // UPG-001: the legacy raw-offset-0 save must be unreachable once the
    // MemoryManager layout exists at offset 0 (it would clobber the MGR header
    // and corrupt every virtual region), even if the heap migration flag was
    // somehow reset. The raw-magic check does not trust heap state.
    let migrated = state::read_state(|s| s.migrated_to_stable_structures);
    match state::pre_upgrade_save_path(migrated, memory::memory_manager_layout_exists()) {
        state::PreUpgradeSavePath::Config => state::save_config_to_stable(),
        state::PreUpgradeSavePath::LegacyRaw => state::save_to_stable_memory(),
    }
}

#[post_upgrade]
fn post_upgrade() {
    // STEP 1: Rescue legacy JSON blob BEFORE MemoryManager::init.
    // Raw stable64_read is safe here because MemoryManager hasn't been initialized yet.
    // On second+ upgrades, offset 0 contains the MemoryManager header (magic b"MGR" + version).
    // Interpreted as a little-endian u64, this exceeds 10_000_000, so the len check below
    // correctly returns None and we fall through to load_config_from_stable().
    let size = ic_cdk::api::stable::stable64_size();
    let legacy_state: Option<BotState> = if size > 0 {
        let mut len_bytes = [0u8; 8];
        ic_cdk::api::stable::stable64_read(0, &mut len_bytes);
        let len = u64::from_le_bytes(len_bytes) as usize;
        if len > 0 && len < 10_000_000 {
            let mut bytes = vec![0u8; len];
            ic_cdk::api::stable::stable64_read(8, &mut bytes);
            serde_json::from_slice(&bytes).ok()
        } else {
            None
        }
    } else {
        None
    };

    // STEP 2: Initialize MemoryManager. On first migration this writes a new header
    // at offset 0 (fine, we already rescued above). On subsequent upgrades it reads
    // the existing header (non-destructive, idempotent).
    memory::init_memory_manager();
    history::init_history();

    // STEP 3: Decide migration path.
    if let Some(ref state) = legacy_state {
        if !state.migrated_to_stable_structures {
            // First upgrade after migration: move legacy events into stable map
            log!(INFO, "Migrating {} legacy events to stable map", state.liquidation_events.len());
            history::migrate_legacy_events(&state.liquidation_events);
        }
    }

    // STEP 4: Load state.
    if let Some(legacy) = legacy_state {
        if legacy.migrated_to_stable_structures {
            // Already migrated, but the pre_upgrade wrote config to MEM_ID_CONFIG.
            // The legacy_state we read from offset 0 is stale. Load from StableCell instead.
            state::load_config_from_stable();
        } else {
            // First migration: use the rescued state, mark as migrated.
            state::init_state(legacy);
            state::mutate_state(|s| s.migrated_to_stable_structures = true);
        }
    } else {
        // No legacy state found at all. Try new-format config.
        state::load_config_from_stable();
    }

    // Stop automatic liquidation after every upgrade. Persist the pause before
    // reinstalling the 30-second worker timer so no queued work can run before
    // an operator has inspected state and explicitly resumed processing.
    state::mutate_state(state::pause_after_upgrade);
    state::save_config_to_stable();
    setup_timer();
}

fn setup_timer() {
    ic_cdk_timers::set_timer_interval(
        std::time::Duration::from_secs(30),
        || ic_cdk::spawn(process::process_pending()),
    );
}

// ---- Inspect message (cycle optimization, NOT a security boundary) ----

#[ic_cdk_macros::inspect_message]
fn inspect_message() {
    let method = ic_cdk::api::call::method_name();
    match method.as_str() {
        // Admin/auth methods: reject anonymous to save cycles on Candid decoding
        "set_config" | "admin_resolve_pool_ordering" | "admin_approve_pool"
        | "admin_sweep_ckusdc" | "admin_retry_stuck_claim"
        | "set_processing_paused" | "admin_reconcile_payment_block" | "admin_reconcile_return_block"
        | "admin_authorize_shortfall_topup" | "admin_reconcile_shortfall_topup_block"
        | "admin_refresh_fees" | "admin_test_swap" | "admin_reconcile_treasury_block" => {
            if ic_cdk::api::caller() != Principal::anonymous() {
                ic_cdk::api::call::accept_message();
            }
        }
        // All other methods (queries, notify from backend): accept
        _ => ic_cdk::api::call::accept_message(),
    }
}

// ---- Core endpoints ----

#[update]
fn notify_liquidatable_vaults(vaults: Vec<LiquidatableVaultInfo>) {
    let caller = ic_cdk::api::caller();
    let backend = state::read_state(|s| s.config.as_ref().map(|c| c.backend_principal));
    if Some(caller) != backend {
        log!(INFO, "Rejected notification from unauthorized caller: {}", caller);
        return;
    }
    let count = vaults.len();
    state::mutate_state(|s| {
        // Replace (not append): backend sends the full current set each cycle.
        s.pending_vaults = vaults;
        s.admin_events.push(BotAdminEvent {
            timestamp: ic_cdk::api::time(),
            caller: caller.to_text(),
            action: BotAdminAction::VaultsNotified { count: count as u64 },
        });
        trim_admin_events(&mut s.admin_events);
    });
    log!(INFO, "Received {} liquidatable vaults from backend", count);
}

/// Pause/resume the timer-driven liquidation worker. Pausing is refused while
/// an update is suspended in an external call; retry after it has settled.
#[update]
fn set_processing_paused(paused: bool) -> Result<(), String> {
    require_admin();
    if paused && PROCESSING.with(|p| *p.borrow()) {
        return Err("cannot pause while a liquidation or admin operation is in flight".into());
    }
    state::mutate_state(|s| s.processing_paused = paused);
    state::save_config_to_stable();
    Ok(())
}

#[query]
fn get_processing_paused() -> bool {
    state::read_state(|s| s.processing_paused)
}

#[query]
fn get_bot_stats() -> state::BotStats {
    state::read_state(|s| s.stats.clone())
}

#[query]
fn cycles_status() -> rumi_cycle_manager::CycleManagerCyclesStatus {
    let operational = PROCESSING.with(|p| !*p.borrow());
    rumi_cycle_manager::self_cycles_status(
        2_000_000_000_000,
        operational,
        rumi_cycle_manager::DEFAULT_FREEZE_THRESHOLD_SECS,
    )
}

#[query]
fn cycle_manager_metrics() -> Vec<rumi_cycle_manager::CycleManagerMetric> {
    state::read_state(|s| {
        vec![
            rumi_cycle_manager::metric(
                "op:liquidation:count",
                s.stats.events_count,
                s.stats.events_count,
                Some("cumulative liquidation bot events"),
            ),
            rumi_cycle_manager::metric(
                "op:pending_vaults:count",
                s.pending_vaults.len() as u64,
                s.pending_vaults.len() as u64,
                Some("currently queued liquidatable vaults"),
            ),
            rumi_cycle_manager::metric(
                "ledger:debt_covered:e8s",
                s.stats.events_count,
                s.stats.total_debt_covered_e8s,
                Some("total debt covered by bot"),
            ),
        ]
    })
}

#[query]
fn get_admin_events(offset: u64, limit: u64) -> Vec<state::BotAdminEvent> {
    let limit = limit.min(1000);
    state::read_state(|s| {
        let len = s.admin_events.len();
        let start = (len as u64).saturating_sub(offset.saturating_add(limit)) as usize;
        let end = (len as u64).saturating_sub(offset) as usize;
        if start >= end {
            return vec![];
        }
        s.admin_events[start..end].to_vec()
    })
}

#[query]
fn get_admin_event_count() -> u64 {
    state::read_state(|s| s.admin_events.len() as u64)
}

/// Cap admin_events to prevent unbounded heap growth.
/// Keeps the most recent MAX entries, discards oldest.
const MAX_ADMIN_EVENTS: usize = 10_000;

fn trim_admin_events(events: &mut Vec<BotAdminEvent>) {
    if events.len() > MAX_ADMIN_EVENTS {
        let drain_count = events.len() - MAX_ADMIN_EVENTS;
        events.drain(..drain_count);
    }
}

#[update]
fn set_config(config: BotConfig) {
    require_admin();
    let (has_payment, has_claim, has_treasury) = state::read_state(|s| (
        !s.pending_payments.is_empty(), !s.pending_claims.is_empty(), !s.pending_treasury.is_empty(),
    ));
    if !admin_config_update_allowed(has_payment, has_claim, has_treasury) {
        log!(INFO, "Rejected config change while a claim, payment, or treasury recovery journal is pending");
        return;
    }
    state::mutate_state(|s| {
        s.config = Some(config);
        s.admin_events.push(BotAdminEvent {
            timestamp: ic_cdk::api::time(),
            caller: ic_cdk::api::caller().to_text(),
            action: BotAdminAction::ConfigUpdated,
        });
        trim_admin_events(&mut s.admin_events);
    });
}

#[query]
fn get_pending_payment_journals() -> Vec<state::BotPaymentJournal> {
    require_admin();
    state::read_state(|s| s.pending_payments.values().cloned().collect())
}

#[query]
fn get_pending_claim_journals() -> Vec<state::BotClaimJournal> {
    require_admin();
    state::read_state(|s| s.pending_claims.values().cloned().collect())
}

/// Return one immutable pending treasury intent for operator block reconciliation.
/// The record ID makes the query bounded regardless of the journal's total size.
#[query]
fn get_pending_treasury_intent(record_id: u64) -> Option<state::BotTreasuryIntentView> {
    require_admin();
    state::read_state(|s| s.pending_treasury.get(&record_id).map(Into::into))
}

// ---- History query endpoints ----

#[query]
fn get_liquidation(id: u64) -> Option<history::LiquidationRecordVersioned> {
    history::get_record(id)
}

#[query]
fn get_liquidations(offset: u64, limit: u64) -> Vec<history::LiquidationRecordVersioned> {
    history::get_records(offset, limit)
}

#[query]
fn get_liquidation_count() -> u64 {
    history::record_count()
}

#[query]
fn get_stuck_liquidations() -> Vec<history::LiquidationRecordVersioned> {
    history::get_stuck_records()
}

// ---- Legacy query (backward compat, delegates to new history) ----

#[query]
fn get_liquidation_events(offset: u64, limit: u64) -> Vec<history::LiquidationRecordVersioned> {
    history::get_records(offset, limit)
}

// ---- Admin endpoints ----

fn require_admin() {
    let caller = ic_cdk::api::caller();
    if caller == Principal::anonymous() {
        ic_cdk::trap("Anonymous caller not allowed");
    }
    let is_admin = state::read_state(|s| {
        s.config.as_ref().map(|c| c.admin == caller).unwrap_or(false)
    });
    if !is_admin {
        ic_cdk::trap("Unauthorized: only admin can call this function");
    }
}

/// One-time: fetch pool metadata to determine if ICP is token0 or token1.
#[update]
async fn admin_resolve_pool_ordering() {
    require_admin();
    let _guard = ProcessingGuard::acquire()
        .unwrap_or_else(|_| ic_cdk::trap("Another operation is in progress"));
    let (pool, icp_ledger) = state::read_state(|s| {
        let c = s.config.as_ref().expect("Config not set");
        (c.icpswap_pool, c.icp_ledger)
    });

    let metadata = icpswap::fetch_metadata(pool)
        .await
        .unwrap_or_else(|e| ic_cdk::trap(&format!("Failed to fetch metadata: {}", e)));

    let icp_text = icp_ledger.to_text();
    let zero_for_one = metadata.token0.address == icp_text;

    state::mutate_state(|s| {
        if let Some(ref mut config) = s.config {
            config.icpswap_zero_for_one = Some(zero_for_one);
        }
    });

    log!(
        INFO,
        "Pool ordering resolved: ICP is token{}, zeroForOne={}",
        if zero_for_one { "0" } else { "1" },
        zero_for_one
    );
}

/// One-time: set up infinite ICRC-2 approve for ICP to the ICPSwap pool.
#[update]
async fn admin_approve_pool() {
    require_admin();
    let _guard = ProcessingGuard::acquire()
        .unwrap_or_else(|_| ic_cdk::trap("Another operation is in progress"));
    let (icp_ledger, pool) = state::read_state(|s| {
        let c = s.config.as_ref().expect("Config not set");
        (c.icp_ledger, c.icpswap_pool)
    });

    swap::approve_infinite(icp_ledger, pool)
        .await
        .unwrap_or_else(|e| ic_cdk::trap(&format!("Approve failed: {}", e)));

    log!(INFO, "Infinite approve set: ICP ledger {} -> pool {}", icp_ledger, pool);
}

/// Emergency: transfer all bot ckUSDC to a target principal.
/// Optionally mark an associated history record as AdminResolved.
#[update]
async fn admin_sweep_ckusdc(target: Principal, record_id: Option<u64>) {
    require_admin();
    let _guard = ProcessingGuard::acquire()
        .unwrap_or_else(|_| ic_cdk::trap("Another operation is in progress"));
    if state::read_state(|s| !s.pending_payments.is_empty() || !s.pending_claims.is_empty()) {
        ic_cdk::trap("Cannot sweep assets while a claim/payment recovery journal is pending");
    }
    let ckusdc_ledger = state::read_state(|s| {
        s.config.as_ref().expect("Config not set").ckusdc_ledger
    });

    let balance_result: Result<(Nat,), _> = ic_cdk::call(
        ckusdc_ledger,
        "icrc1_balance_of",
        (Account {
            owner: ic_cdk::id(),
            subaccount: None,
        },),
    )
    .await;

    let balance = match balance_result {
        Ok((b,)) => {
            let val: u64 = b.0.to_string().parse().unwrap_or(u64::MAX);
            if val == 0 {
                ic_cdk::trap("Bot has zero ckUSDC balance");
            }
            val
        }
        Err((code, msg)) => ic_cdk::trap(&format!("Balance query failed: {:?} {}", code, msg)),
    };

    let fee = state::read_state(|s| {
        s.config
            .as_ref()
            .expect("Config not set")
            .ckusdc_fee_e6
            .unwrap_or(10_000)
    });
    let send_amount = balance.saturating_sub(fee);

    let transfer_args = icrc_ledger_types::icrc1::transfer::TransferArg {
        from_subaccount: None,
        to: Account {
            owner: target,
            subaccount: None,
        },
        amount: Nat::from(send_amount),
        fee: None,
        memo: None,
        created_at_time: Some(ic_cdk::api::time()),
    };

    let result: Result<
        (Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError>,),
        _,
    > = ic_cdk::call(ckusdc_ledger, "icrc1_transfer", (transfer_args,)).await;

    match result {
        Ok((Ok(block),)) => {
            log!(INFO, "Swept {} ckUSDC e6 to {}, block {}", send_amount, target, block);
            if let Some(id) = record_id {
                history::update_record_status(id, history::LiquidationStatus::AdminResolved);
                log!(INFO, "Marked record #{} as AdminResolved", id);
            }
        }
        Ok((Err(e),)) => ic_cdk::trap(&format!("Sweep transfer failed: {:?}", e)),
        Err((code, msg)) => ic_cdk::trap(&format!("Sweep call failed: {:?} {}", code, msg)),
    }
}

/// Refresh the cached ICRC-1 transfer fees from each ledger and store them on
/// the in-memory BotConfig. This exists because ICPSwap's `depositFromAndSwap`
/// requires the caller to pass the exact `tokenInFee` / `tokenOutFee` values
/// the pool has cached; a stale `BotConfig.ckusdc_fee_e6` is what caused every
/// swap to fail with "Wrong fee cache (expected: 10000, received: 10)".
/// Returns the resolved (icp_fee_e8s, ckusdc_fee_e6) pair.
#[update]
async fn admin_refresh_fees() -> (u64, u64) {
    require_admin();
    let _guard = ProcessingGuard::acquire()
        .unwrap_or_else(|_| ic_cdk::trap("Another operation is in progress"));
    let (icp_ledger, ckusdc_ledger) = state::read_state(|s| {
        let c = s.config.as_ref().expect("Config not set");
        (c.icp_ledger, c.ckusdc_ledger)
    });

    let icp_fee = swap::fetch_ledger_fee(icp_ledger)
        .await
        .unwrap_or_else(|e| ic_cdk::trap(&format!("Failed to fetch ICP fee: {}", e)));
    let ckusdc_fee = swap::fetch_ledger_fee(ckusdc_ledger)
        .await
        .unwrap_or_else(|e| ic_cdk::trap(&format!("Failed to fetch ckUSDC fee: {}", e)));

    state::mutate_state(|s| {
        if let Some(ref mut config) = s.config {
            config.icp_fee_e8s = Some(icp_fee);
            config.ckusdc_fee_e6 = Some(ckusdc_fee);
        }
    });

    log!(
        INFO,
        "Refreshed ledger fees: ICP={} e8s, ckUSDC={} e6",
        icp_fee,
        ckusdc_fee
    );

    (icp_fee, ckusdc_fee)
}

/// Run the live swap path (quote -> apply slippage -> `depositFromAndSwap`)
/// against the configured ICPSwap pool using `amount_e8s` of ICP from the
/// bot's own balance. Admin-only. Designed for end-to-end verification of the
/// swap leg without waiting for an organic liquidation.
///
/// The resulting ckUSDC stays in the bot canister; retrieve via
/// `admin_sweep_ckusdc`.
#[update]
async fn admin_test_swap(amount_e8s: u64) -> Result<swap::SwapResult, String> {
    require_admin();
    let _guard = ProcessingGuard::acquire()
        .map_err(|_| "Another operation is in progress".to_string())?;
    let (has_claim_recovery, has_payment_recovery) = state::read_state(|s| {
        (!s.pending_claims.is_empty(), !s.pending_payments.is_empty())
    });
    let has_treasury_recovery = state::read_state(|s| !s.pending_treasury.is_empty());
    if !admin_test_swap_allowed(has_claim_recovery, has_payment_recovery, has_treasury_recovery) {
        return Err("cannot run admin_test_swap while claim, payment, or treasury recovery is pending".into());
    }
    let config = state::read_state(|s| s.config.clone())
        .ok_or_else(|| "Config not set".to_string())?;

    log!(INFO, "admin_test_swap: attempting to swap {} ICP e8s", amount_e8s);
    let result = swap::swap_icp_for_ckusdc(&config, amount_e8s)
        .await
        .map_err(|error| error.to_string());
    match &result {
        Ok(r) => log!(
            INFO,
            "admin_test_swap succeeded: received {} ckUSDC e6 at effective price {} e8s",
            r.ckusdc_received_e6,
            r.effective_price_e8s
        ),
        Err(e) => log!(INFO, "admin_test_swap failed: {}", e),
    }
    result
}

/// Retry only a payment with a durable definitive no-effect response. Every
/// other case requires exact proof reconciliation; never fall back to the
/// vault-id-only legacy confirm endpoint.
#[update]
async fn admin_retry_stuck_claim(vault_id: u64) {
    require_admin();
    let _guard = ProcessingGuard::acquire()
        .unwrap_or_else(|_| ic_cdk::trap("Another operation is in progress"));
    let config = state::read_state(|s| s.config.clone()).expect("Not configured");
    let disposition = state::read_state(|s| {
        admin_retry_disposition(
            s.pending_payments.get(&vault_id).map(|journal| journal.status.clone()),
            s.pending_claims.contains_key(&vault_id),
        )
    });
    match disposition {
        AdminRetryDisposition::RetryNoEffectPayment => {
            if let Err(error) = process::admin_retry_no_effect_payment(&config, vault_id).await {
                ic_cdk::trap(&format!("Payment remains held for vault #{}: {}", vault_id, error));
            }
            log!(INFO, "admin_retry_stuck_claim: retried definitive no-effect payment for vault #{}", vault_id);
        }
        AdminRetryDisposition::ReconcilePaymentProof => ic_cdk::trap(&format!(
            "Payment for vault #{} requires exact ICRC-3 block reconciliation; legacy confirmation is disabled",
            vault_id
        )),
        AdminRetryDisposition::HoldClaim => ic_cdk::trap(&format!(
            "Claim for vault #{} is held without an accepted payment proof; legacy confirmation is disabled",
            vault_id
        )),
        AdminRetryDisposition::HoldLegacy => ic_cdk::trap(&format!(
            "Vault #{} has no durable proof journal; legacy confirmation is disabled",
            vault_id
        )),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AdminRetryDisposition {
    RetryNoEffectPayment,
    ReconcilePaymentProof,
    HoldClaim,
    HoldLegacy,
}

fn admin_retry_disposition(
    payment_status: Option<state::BotPaymentStatus>,
    has_pending_claim: bool,
) -> AdminRetryDisposition {
    match payment_status {
        Some(state::BotPaymentStatus::NoEffect) => AdminRetryDisposition::RetryNoEffectPayment,
        Some(_) => AdminRetryDisposition::ReconcilePaymentProof,
        None if has_pending_claim => AdminRetryDisposition::HoldClaim,
        None => AdminRetryDisposition::HoldLegacy,
    }
}

fn admin_test_swap_allowed(has_claim_recovery: bool, has_payment_recovery: bool, has_treasury_recovery: bool) -> bool {
    !has_claim_recovery && !has_payment_recovery && !has_treasury_recovery
}

fn admin_config_update_allowed(has_payment_recovery: bool, has_claim_recovery: bool, has_treasury_recovery: bool) -> bool {
    !has_payment_recovery && !has_claim_recovery && !has_treasury_recovery
}

#[cfg(test)]
mod admin_retry_tests {
    use super::*;

    #[test]
    fn short_payment_claim_cannot_fall_through_to_legacy_confirmation() {
        assert_eq!(
            admin_retry_disposition(None, true),
            AdminRetryDisposition::HoldClaim,
        );
    }

    #[test]
    fn missing_journal_also_fails_closed() {
        assert_eq!(
            admin_retry_disposition(None, false),
            AdminRetryDisposition::HoldLegacy,
        );
    }

    #[test]
    fn admin_test_swap_is_blocked_by_any_pending_recovery_journal() {
        assert!(admin_test_swap_allowed(false, false, false));
        assert!(!admin_test_swap_allowed(true, false, false));
        assert!(!admin_test_swap_allowed(false, true, false));
        assert!(!admin_test_swap_allowed(true, true, false));
        assert!(!admin_test_swap_allowed(false, false, true));
    }

    #[test]
    fn config_change_is_blocked_by_pending_treasury_obligation() {
        assert!(admin_config_update_allowed(false, false, false));
        assert!(!admin_config_update_allowed(false, false, true));
    }
}

/// Reconcile an ambiguous payment by an operator-supplied ICRC-3 block.
/// Backend validation remains authoritative before any debt write-down.
#[update]
async fn admin_reconcile_payment_block(vault_id: u64, block_index: u64) -> Result<(), String> {
    require_admin();
    let _guard = ProcessingGuard::acquire()
        .map_err(|_| "Another operation is in progress".to_string())?;
    let config = state::read_state(|s| s.config.clone())
        .ok_or_else(|| "Config not set".to_string())?;
    process::admin_reconcile_payment_block(&config, vault_id, block_index).await
}

/// Reconcile the exact residual ckUSDC transfer for a previously short claim.
#[update]
async fn admin_reconcile_shortfall_topup_block(vault_id: u64, block_index: u64) -> Result<(), String> {
    require_admin();
    let _guard = ProcessingGuard::acquire()
        .map_err(|_| "Another operation is in progress".to_string())?;
    let config = state::read_state(|s| s.config.clone())
        .ok_or_else(|| "Config not set".to_string())?;
    process::admin_reconcile_shortfall_topup_block(&config, vault_id, block_index).await
}

/// Reconcile one unresolved treasury bonus using an operator-supplied candidate
/// ICP ledger block. The stored transfer tuple remains authoritative; this path
/// verifies and records the block without dispatching another transfer.
#[update]
async fn admin_reconcile_treasury_block(record_id: u64, block_index: u64) -> Result<(), String> {
    require_admin();
    let _guard = ProcessingGuard::acquire()
        .map_err(|_| "Another operation is in progress".to_string())?;
    process::admin_reconcile_treasury_block(record_id, block_index).await
}

/// Authorize one exact residual payment after allocating residual plus fee.
#[update]
async fn admin_authorize_shortfall_topup(
    vault_id: u64,
    funding_allocation_e6: u64,
) -> Result<(), String> {
    require_admin();
    let _guard = ProcessingGuard::acquire()
        .map_err(|_| "Another operation is in progress".to_string())?;
    let config = state::read_state(|s| s.config.clone())
        .ok_or_else(|| "Config not set".to_string())?;
    process::admin_authorize_shortfall_topup(&config, vault_id, funding_allocation_e6).await
}

/// Reconcile a collateral return by an operator-supplied ICRC-3 block.
/// Backend validation remains authoritative before cancellation.
#[update]
async fn admin_reconcile_return_block(vault_id: u64, block_index: u64) -> Result<(), String> {
    require_admin();
    let _guard = ProcessingGuard::acquire()
        .map_err(|_| "Another operation is in progress".to_string())?;
    let config = state::read_state(|s| s.config.clone())
        .ok_or_else(|| "Config not set".to_string())?;
    process::admin_reconcile_return_block(&config, vault_id, block_index).await
}
