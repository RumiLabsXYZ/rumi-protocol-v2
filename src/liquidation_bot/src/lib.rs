use candid::{CandidType, Deserialize, Nat, Principal};
use ic_canister_log::{declare_log_buffer, log};
use ic_cdk_macros::{init, post_upgrade, pre_upgrade, query, update};
use icrc_ledger_types::icrc1::account::Account;
use std::cell::RefCell;

mod history;
mod icpswap;
mod memory;
mod process;
mod state;
mod swap;

use state::{BotAdminAction, BotAdminEvent, BotConfig, BotState, LiquidatableVaultInfo};

declare_log_buffer!(name = INFO, capacity = 1000);

thread_local! {
    /// Reentrancy guard: true while process_pending or an admin async endpoint is running.
    static PROCESSING: RefCell<bool> = RefCell::new(false);
    /// Vault currently being processed, used to keep duplicate backend
    /// notifications from enqueueing a second claim while an await is open.
    static PROCESSING_VAULT_ID: RefCell<Option<u64>> = RefCell::new(None);
}

pub(crate) struct ProcessingGuard;

impl ProcessingGuard {
    pub(crate) fn acquire() -> Result<Self, &'static str> {
        PROCESSING.with(|p| {
            let mut flag = p.borrow_mut();
            if *flag {
                return Err("Already processing");
            }
            *flag = true;
            PROCESSING_VAULT_ID.with(|id| *id.borrow_mut() = None);
            Ok(Self)
        })
    }

    pub(crate) fn set_vault_id(vault_id: u64) {
        PROCESSING_VAULT_ID.with(|id| *id.borrow_mut() = Some(vault_id));
        state::mutate_state(|s| {
            s.pending_vaults
                .retain(|queued| queued.vault_id != vault_id)
        });
    }
}

#[cfg(test)]
#[test]
fn exported_rust_service_matches_checked_in_did() {
    use candid_parser::utils::{service_equal, CandidSource};

    let rust_service = __export_service();
    let did_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("liquidation_bot.did");

    if let Err(error) = service_equal(
        CandidSource::Text(&rust_service),
        CandidSource::File(did_path.as_path()),
    ) {
        let checked_in = std::fs::read_to_string(&did_path)
            .unwrap_or_else(|read_error| format!("<could not read DID: {read_error}>"));
        panic!(
            "Rust-exported liquidation_bot Candid differs from {}: {error:?}\n\nRust:\n{rust_service}\n\nChecked-in DID:\n{checked_in}",
            did_path.display()
        );
    }
}

impl Drop for ProcessingGuard {
    fn drop(&mut self) {
        PROCESSING_VAULT_ID.with(|id| *id.borrow_mut() = None);
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
    // Classify raw memory before interpreting it as a legacy length prefix.
    let size = ic_cdk::api::stable::stable64_size();
    let mut len_bytes = [0u8; 8];
    if size > 0 {
        ic_cdk::api::stable::stable64_read(0, &mut len_bytes);
    }
    let legacy_state: Option<BotState> = match state::legacy_snapshot_body_len(size, &len_bytes) {
        Ok(Some(len)) => {
            let mut bytes = vec![0u8; len];
            ic_cdk::api::stable::stable64_read(8, &mut bytes);
            state::decode_legacy_state(size, &len_bytes, &bytes).unwrap_or_else(|error| {
                ic_cdk::trap(&format!(
                    "liquidation_bot UPG-001: legacy snapshot rescue failed before \
                     MemoryManager initialization: {error}"
                ))
            })
        }
        Ok(None) => None,
        Err(error) => ic_cdk::trap(&format!(
            "liquidation_bot UPG-001: refusing to overwrite unknown raw stable memory: {error}"
        )),
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
            log!(
                INFO,
                "Migrating {} legacy events to stable map",
                state.liquidation_events.len()
            );
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

    setup_timer();
}

fn setup_timer() {
    ic_cdk_timers::set_timer_interval(std::time::Duration::from_secs(30), || {
        ic_cdk::spawn(process::process_pending())
    });
}

/// Test-only seam that runs the production claim worker with a one-shot fake
/// pool quote and output credit. The compiled production Wasm has no such
/// method and keeps `AUTOMATIC_CLAIM_SWAP_ENABLED` false.
#[cfg(feature = "bot_driver_test")]
#[update]
async fn test_drive_liquidation_worker(
    vault: LiquidatableVaultInfo,
    quoted_output_e6: u64,
    delivered_output_e6: u64,
) -> Result<(), String> {
    require_admin();
    let guard =
        ProcessingGuard::acquire().map_err(|_| "Another operation is in progress".to_string())?;
    ProcessingGuard::set_vault_id(vault.vault_id);
    process::process_specific_vault_for_test(vault, guard, quoted_output_e6, delivered_output_e6)
        .await;
    Ok(())
}

// ---- Inspect message (cycle optimization, NOT a security boundary) ----

#[ic_cdk_macros::inspect_message]
fn inspect_message() {
    let method = ic_cdk::api::call::method_name();
    let caller = ic_cdk::api::caller();
    let admin = state::read_state(|s| s.config.as_ref().map(|c| c.admin));
    if accept_ingress(&method, caller, admin) {
        ic_cdk::api::call::accept_message();
    }
}

fn accept_ingress(method: &str, caller: Principal, admin: Option<Principal>) -> bool {
    // This method is called by the backend as an inter-canister update. It
    // must never be accepted as user ingress; inter-canister calls bypass
    // inspect_message.
    if method == "notify_liquidatable_vaults" {
        return false;
    }
    match method {
        "set_config"
        | "admin_resolve_pool_ordering"
        | "admin_approve_pool"
        | "admin_sweep_ckusdc"
        | "admin_retry_stuck_claim"
        | "admin_recover_ckusdc_shortfall"
        | "admin_submit_paused_claim_ckusdc_payment"
        | "admin_requeue_pending_bot_claim"
        | "admin_retry_claim_return"
        | "admin_retry_icp_treasury_bonus"
        | "admin_reconcile_icp_treasury_bonus"
        | "admin_quarantine_icp_treasury_bonus"
        | "admin_refresh_fees"
        | "admin_test_swap" => admin == Some(caller),
        #[cfg(feature = "bot_driver_test")]
        "test_drive_liquidation_worker" => admin == Some(caller),
        #[cfg(feature = "test_endpoints")]
        "test_seed_ckusdc_payment_recovery"
        | "test_submit_native_icp_transfer"
        | "test_seed_native_icp_bonus_recovery" => admin == Some(caller),
        // Unknown and future methods stay closed to ingress until explicitly
        // reviewed and added to this allowlist. Inter-canister calls do not
        // pass through inspect_message, so backend notifications still work.
        _ => false,
    }
}

// ---- Core endpoints ----

#[update]
fn notify_liquidatable_vaults(vaults: Vec<LiquidatableVaultInfo>) {
    let caller = ic_cdk::api::caller();
    let backend = state::read_state(|s| s.config.as_ref().map(|c| c.backend_principal));
    if Some(caller) != backend {
        log!(
            INFO,
            "Rejected notification from unauthorized caller: {}",
            caller
        );
        return;
    }
    let count = vaults.len();
    let processing = PROCESSING.with(|p| *p.borrow());
    let active_vault_id = PROCESSING_VAULT_ID.with(|id| *id.borrow());
    let mut seen = std::collections::BTreeSet::new();
    let incoming: Vec<_> = vaults
        .into_iter()
        .filter(|vault| {
            let intent = history::get_claim_intent(vault.vault_id);
            if !notification_may_enqueue(
                vault.vault_id,
                intent.as_ref().map(|intent| intent.phase.clone()),
                intent
                    .as_ref()
                    .is_some_and(|intent| intent.return_transfer.is_some()),
                processing,
                active_vault_id,
            ) || !seen.insert(vault.vault_id)
            {
                return false;
            }
            true
        })
        .collect();
    state::mutate_state(|s| {
        // Keep durable receipt recoveries that were already queued even when
        // the backend's latest full-set notification omits them. Drop copies
        // of the active vault and merge incoming IDs without duplicates.
        let incoming_ids: std::collections::BTreeSet<_> =
            incoming.iter().map(|vault| vault.vault_id).collect();
        let mut merged = Vec::new();
        for queued in s.pending_vaults.drain(..) {
            if Some(queued.vault_id) == active_vault_id || incoming_ids.contains(&queued.vault_id) {
                continue;
            }
            let recoverable = history::get_claim_intent(queued.vault_id).is_some_and(|intent| {
                matches!(
                    intent.phase,
                    history::BotClaimIntentPhase::ClaimRequested
                        | history::BotClaimIntentPhase::AwaitingReceipt
                        | history::BotClaimIntentPhase::NoEffectAcknowledgementPending { .. }
                        | history::BotClaimIntentPhase::Claimed(_)
                )
            });
            if recoverable
                && !merged
                    .iter()
                    .any(|item: &LiquidatableVaultInfo| item.vault_id == queued.vault_id)
            {
                merged.push(queued);
            }
        }
        merged.extend(incoming);
        s.pending_vaults = merged;
        s.admin_events.push(BotAdminEvent {
            timestamp: ic_cdk::api::time(),
            caller: caller.to_text(),
            action: BotAdminAction::VaultsNotified {
                count: count as u64,
            },
        });
        trim_admin_events(&mut s.admin_events);
    });
    log!(INFO, "Received {} liquidatable vaults from backend", count);
}

fn notification_may_enqueue(
    vault_id: u64,
    phase: Option<history::BotClaimIntentPhase>,
    has_return_transfer: bool,
    processing: bool,
    active_vault_id: Option<u64>,
) -> bool {
    if Some(vault_id) == active_vault_id {
        return false;
    }
    match phase {
        Some(
            history::BotClaimIntentPhase::SwapStarted(_)
            | history::BotClaimIntentPhase::Acquired { .. },
        ) => false,
        Some(history::BotClaimIntentPhase::Claimed(_)) if has_return_transfer => false,
        Some(
            history::BotClaimIntentPhase::ClaimRequested
            | history::BotClaimIntentPhase::AwaitingReceipt
            | history::BotClaimIntentPhase::NoEffectAcknowledgementPending { .. }
            | history::BotClaimIntentPhase::Claimed(_),
        ) => !processing,
        None => true,
    }
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
    let _guard = ProcessingGuard::acquire()
        .unwrap_or_else(|_| ic_cdk::trap("Another operation is in progress"));
    let current_backend =
        state::read_state(|s| s.config.as_ref().map(|current| current.backend_principal));
    if !backend_identity_change_allowed(
        current_backend,
        config.backend_principal,
        history::has_claim_intents(),
    ) {
        ic_cdk::trap("Cannot change backend principal while a durable bot claim intent remains");
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

#[update]
fn backend_claim_request_id_floor() -> Result<u64, String> {
    let caller = ic_cdk::api::caller();
    let backend = state::read_state(|s| s.config.as_ref().map(|config| config.backend_principal));
    let processing = PROCESSING.with(|p| *p.borrow());
    let has_claim_intents = history::has_claim_intents();
    backend_claim_request_id_floor_result(
        caller,
        backend,
        processing,
        has_claim_intents,
        history::record_count(),
    )
    .map_err(str::to_string)
}

fn backend_claim_request_id_floor_result(
    caller: Principal,
    configured_backend: Option<Principal>,
    processing: bool,
    has_claim_intents: bool,
    next_unused_id: u64,
) -> Result<u64, &'static str> {
    if caller == Principal::anonymous() || configured_backend != Some(caller) {
        return Err("Only the configured backend can read the claim request ID floor");
    }
    if processing {
        return Err("Claim request ID floor is unavailable while processing is active");
    }
    if has_claim_intents {
        return Err("Claim request ID floor is unavailable while a durable claim intent exists");
    }
    Ok(next_unused_id)
}

#[query]
fn get_stuck_liquidations() -> Vec<history::LiquidationRecordVersioned> {
    history::get_stuck_records()
}

// ---- Legacy query (backward compat, delegates to new history) ----

#[query]
fn get_paused_claim_ckusdc_payment_account(vault_id: u64) -> Option<Account> {
    require_admin();
    let config = state::read_state(|s| s.config.clone())?;
    let record = history::get_latest_record_for_vault(vault_id)?;
    let intent = history::get_claim_intent(vault_id)?;
    let (receipt, minimum, is_initial) =
        paused_claim_payment_context(&config, vault_id, &record, &intent).ok()?;
    if !is_initial || intent.ckusdc_payment_transfer.is_some() {
        return None;
    }
    let _ = minimum;
    Some(Account {
        owner: ic_cdk::id(),
        subaccount: Some(process::claim_payment_subaccount(
            vault_id,
            receipt.claim_timestamp,
        )),
    })
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
        s.config
            .as_ref()
            .map(|c| c.admin == caller)
            .unwrap_or(false)
    });
    if !is_admin {
        ic_cdk::trap("Unauthorized: only admin can call this function");
    }
}

fn shortfall_payment_tuple_matches(
    transfer: &history::BotCkUsdcPaymentTransfer,
    ledger: Principal,
    backend: Principal,
    vault_id: u64,
    claim_timestamp: u64,
    payment_memo: &[u8],
    minimum_payment_e6: u64,
) -> bool {
    let amount = transfer.args.amount.0.to_string().parse::<u64>().ok();
    let fee = transfer
        .args
        .fee
        .as_ref()
        .and_then(|value| value.0.to_string().parse::<u64>().ok());
    transfer.ledger == ledger
        && transfer.args.to.owner == backend
        && transfer.args.to.subaccount.is_none()
        && transfer.args.from_subaccount
            == Some(process::claim_payment_subaccount(vault_id, claim_timestamp))
        && amount == Some(minimum_payment_e6)
        && fee.is_some()
        && transfer.args.memo.as_ref().map(|memo| memo.0.as_ref()) == Some(payment_memo)
        && transfer
            .args
            .created_at_time
            .is_some_and(|time| time >= claim_timestamp)
}

fn paused_claim_payment_context(
    config: &BotConfig,
    vault_id: u64,
    record: &history::LiquidationRecordVersioned,
    intent: &history::BotClaimIntent,
) -> Result<(history::BotClaimReceipt, u64, bool), String> {
    let history::LiquidationRecordVersioned::V1(record) = record;
    if record.vault_id != vault_id
        || !matches!(
            &record.status,
            history::LiquidationStatus::SwapFailed | history::LiquidationStatus::ConfirmFailed
        )
        || record.collateral_claimed_e8s == 0
        || record.debt_to_cover_e8s == 0
        || record.icp_swapped_e8s != 0
        || record.ckusdc_received_e6 != 0
        || record.icp_to_treasury_e8s != 0
    {
        return Err("latest record is not an unpaid paused-swap claim".into());
    }
    let receipt = match &intent.phase {
        history::BotClaimIntentPhase::Claimed(receipt) => receipt.clone(),
        _ => return Err("claim intent is not in the pre-swap Claimed phase".into()),
    };
    let claim_timestamp = record
        .claim_timestamp
        .ok_or("claim generation timestamp is missing")?;
    let payment_memo = record
        .payment_memo
        .as_deref()
        .filter(|memo| !memo.is_empty())
        .ok_or("claim payment memo is missing")?;
    if intent.vault_id != vault_id
        || intent.backend_principal != Some(config.backend_principal)
        || receipt.claim_timestamp != claim_timestamp
        || receipt.payment_memo != payment_memo
        || receipt.debt_covered != record.debt_to_cover_e8s
        || receipt.collateral_amount != record.collateral_claimed_e8s
        || receipt.collateral_price_e8s != record.oracle_price_e8s
        || !receipt.claim_transfer.as_ref().is_some_and(|proof| {
            process::claim_transfer_receipt_is_complete(
                config.backend_principal,
                ic_cdk::id(),
                config.icp_ledger,
                vault_id,
                claim_timestamp,
                record.collateral_claimed_e8s,
                proof,
            )
        })
        || intent.return_transfer.is_some()
        || intent.ckusdc_top_up_transfer.is_some()
        || intent.shortfall_eligibility.is_some()
    {
        return Err("claim record, generation, or exact collateral receipt does not match".into());
    }
    let minimum = process::ckusdc_minimum_payment_e6(record.debt_to_cover_e8s);
    let is_initial = matches!(&record.status, history::LiquidationStatus::SwapFailed)
        && record.ckusdc_transferred_e6 == 0
        && record.ckusdc_payment_block_index.is_none()
        && record.ckusdc_payment_amount_e6.is_none()
        && intent.ckusdc_payment_transfer.is_none();
    let is_confirmation_retry = matches!(&record.status, history::LiquidationStatus::ConfirmFailed)
        && record.ckusdc_transferred_e6 == minimum
        && record.ckusdc_payment_block_index.is_some()
        && record.ckusdc_payment_amount_e6 == Some(minimum)
        && intent
            .ckusdc_payment_transfer
            .as_ref()
            .is_some_and(|transfer| transfer.block_index == record.ckusdc_payment_block_index);
    let is_exact_transfer_replay = matches!(&record.status, history::LiquidationStatus::SwapFailed)
        && record.ckusdc_transferred_e6 == 0
        && record.ckusdc_payment_block_index.is_none()
        && record.ckusdc_payment_amount_e6.is_none()
        && intent.ckusdc_payment_transfer.is_some();
    if !is_initial && !is_confirmation_retry && !is_exact_transfer_replay {
        return Err(
            "claim payment was already attempted or is not in a recoverable confirmation state"
                .into(),
        );
    }
    Ok((receipt, minimum, is_initial))
}

fn paused_claim_payment_tuple_matches(
    transfer: &history::BotCkUsdcPaymentTransfer,
    config: &BotConfig,
    vault_id: u64,
    receipt: &history::BotClaimReceipt,
    minimum_payment_e6: u64,
) -> bool {
    let amount = transfer.args.amount.0.to_string().parse::<u64>().ok();
    let fee = transfer
        .args
        .fee
        .as_ref()
        .and_then(|value| value.0.to_string().parse::<u64>().ok());
    transfer.ledger == config.ckusdc_ledger
        && transfer.args.from_subaccount
            == Some(process::claim_payment_subaccount(
                vault_id,
                receipt.claim_timestamp,
            ))
        && transfer.args.to
            == Account {
                owner: config.backend_principal,
                subaccount: None,
            }
        && amount == Some(minimum_payment_e6)
        && fee.is_some_and(|fee| fee > 0)
        && transfer.args.memo.as_ref().map(|memo| memo.0.as_ref())
            == Some(receipt.payment_memo.as_slice())
        && transfer
            .args
            .created_at_time
            .is_some_and(|time| time >= receipt.claim_timestamp)
}

async fn backend_claim_is_active(config: &BotConfig, vault_id: u64) -> Result<bool, String> {
    let result: Result<(Vec<u64>,), _> =
        ic_cdk::call(config.backend_principal, "get_bot_claim_vault_ids", ()).await;
    let (active_claims,) = result.map_err(|(code, message)| {
        format!("unable to verify active backend claim: {code:?}: {message}")
    })?;
    Ok(active_claims.contains(&vault_id))
}

/// Admin-assisted payment for a swap that is paused before dispatch. Only the
/// deterministic claim-generation account can fund it; the bot's shared
/// ckUSDC account is never read or spent by this route.
#[update]
async fn admin_submit_paused_claim_ckusdc_payment(vault_id: u64) -> Result<(), String> {
    require_admin();
    let _guard =
        ProcessingGuard::acquire().map_err(|_| "Another operation is in progress".to_string())?;
    ProcessingGuard::set_vault_id(vault_id);
    let config = state::read_state(|s| s.config.clone()).ok_or("Config not set".to_string())?;
    let record = history::get_latest_record_for_vault(vault_id)
        .ok_or("No liquidation history for this vault".to_string())?;
    let intent = history::get_claim_intent(vault_id)
        .ok_or("Paused claim has no durable generation intent".to_string())?;
    let (receipt, minimum_payment_e6, _) =
        paused_claim_payment_context(&config, vault_id, &record, &intent)?;

    if !backend_claim_is_active(&config, vault_id).await? {
        return Err("backend claim is no longer active; no payment was dispatched".into());
    }

    let mut transfer = match intent.ckusdc_payment_transfer.clone() {
        Some(saved) => {
            if !paused_claim_payment_tuple_matches(
                &saved,
                &config,
                vault_id,
                &receipt,
                minimum_payment_e6,
            ) {
                return Err("persisted payment tuple is not bound to this claim generation".into());
            }
            reprice_first_typed_claim_payment_rejection(
                &config,
                vault_id,
                receipt.claim_timestamp,
                &receipt.payment_memo,
                minimum_payment_e6,
                &intent,
                record_id(&record),
                &saved,
            )
            .await?
            .unwrap_or(saved)
        }
        None => {
            let history::LiquidationRecordVersioned::V1(record) = &record;
            if !matches!(&record.status, history::LiquidationStatus::SwapFailed) {
                return Err(
                    "new payment tuple can only be prepared for an unpaid paused-swap record"
                        .into(),
                );
            }
            let fee_e6 = swap::fetch_ledger_fee(config.ckusdc_ledger).await?;
            let source = Account {
                owner: ic_cdk::id(),
                subaccount: Some(process::claim_payment_subaccount(
                    vault_id,
                    receipt.claim_timestamp,
                )),
            };
            let required_balance = minimum_payment_e6
                .checked_add(fee_e6)
                .ok_or("claim payment plus live fee overflows".to_string())?;
            let balance = swap::balance_of_ckusdc_account(config.ckusdc_ledger, source).await?;
            if balance < required_balance {
                return Err(format!(
                    "claim-specific ckUSDC account has {balance} e6; requires {minimum_payment_e6} plus fee {fee_e6}"
                ));
            }

            // Revalidate the exact active generation after both ledger awaits.
            let current_intent = history::get_claim_intent(vault_id)
                .ok_or("claim intent disappeared during payment preflight".to_string())?;
            let current_record = history::get_latest_record_for_vault(vault_id)
                .ok_or("claim record disappeared during payment preflight".to_string())?;
            let (current_receipt, current_minimum, current_initial) =
                paused_claim_payment_context(&config, vault_id, &current_record, &current_intent)?;
            if current_intent != intent
                || current_receipt != receipt
                || current_minimum != minimum_payment_e6
                || !current_initial
                || !matches!(&current_record, history::LiquidationRecordVersioned::V1(current)
                    if current.id == record.id)
                || !backend_claim_is_active(&config, vault_id).await?
            {
                return Err("paused claim changed during isolated-account preflight".into());
            }

            let gross_required = minimum_payment_e6
                .checked_add(fee_e6)
                .ok_or("claim payment plus live fee overflows".to_string())?;
            let prepared = swap::prepare_ckusdc_payment_transfer_from_subaccount(
                &config,
                gross_required,
                &receipt.payment_memo,
                fee_e6,
                ic_cdk::api::time().max(receipt.claim_timestamp),
                Some(process::claim_payment_subaccount(
                    vault_id,
                    receipt.claim_timestamp,
                )),
            )?;
            if !paused_claim_payment_tuple_matches(
                &prepared,
                &config,
                vault_id,
                &receipt,
                minimum_payment_e6,
            ) || !history::update_ckusdc_payment_transfer(vault_id, prepared.clone())
            {
                return Err("could not persist exact generation-bound ckUSDC payment tuple; no transfer dispatched".into());
            }
            prepared
        }
    };

    if !backend_claim_is_active(&config, vault_id).await? {
        return Err(
            "backend claim is no longer active; persisted payment tuple remains held".into(),
        );
    }
    let paid = process::dispatch_and_verify_ckusdc_payment(
        &config,
        vault_id,
        receipt.claim_timestamp,
        &receipt.payment_memo,
        &mut transfer,
    )
    .await?;
    if paid.amount_e6 != minimum_payment_e6 {
        return Err("exact ckUSDC block does not pay the full claim minimum".into());
    }
    if !backend_claim_is_active(&config, vault_id).await? {
        return Err(
            "payment is proven but backend claim is no longer active; retain for reconciliation"
                .into(),
        );
    }

    // Save a replayable backend-confirmation record before the inter-canister
    // call. If its reply is lost, this same block can be submitted again.
    let history::LiquidationRecordVersioned::V1(mut paid_record) = record.clone();
    if matches!(&paid_record.status, history::LiquidationStatus::SwapFailed) {
        paid_record.id = history::next_id();
        paid_record.timestamp = ic_cdk::api::time();
    }
    paid_record.status = history::LiquidationStatus::ConfirmFailed;
    paid_record.ckusdc_transferred_e6 = paid.amount_e6;
    paid_record.ckusdc_payment_block_index = Some(paid.block_index);
    paid_record.ckusdc_payment_amount_e6 = Some(paid.amount_e6);
    paid_record.error_message =
        Some("claim-bound ckUSDC payment proven; backend confirmation pending".into());
    history::insert_record(history::LiquidationRecordVersioned::V1(paid_record.clone()));

    process::call_bot_confirm_liquidation(
        &config,
        vault_id,
        receipt.claim_timestamp,
        paid.block_index,
    )
    .await?;

    if !history::update_record_status_and_error(
        paid_record.id,
        history::LiquidationStatus::AdminResolved,
        Some("backend debt confirmed from claim-bound ckUSDC; collateral remains held for operator disposition".into()),
    ) {
        return Err("backend confirmed payment but liquidation record could not be marked admin-resolved".into());
    }
    state::mutate_state(|s| {
        s.stats.total_debt_covered_e8s = s
            .stats
            .total_debt_covered_e8s
            .saturating_add(receipt.debt_covered);
        s.stats.total_collateral_received_e8s = s
            .stats
            .total_collateral_received_e8s
            .saturating_add(receipt.collateral_amount);
        s.stats.total_ckusdc_deposited_e6 = s
            .stats
            .total_ckusdc_deposited_e6
            .saturating_add(paid.amount_e6);
        s.stats.events_count = s.stats.events_count.saturating_add(1);
    });
    log!(INFO, "admin_submit_paused_claim_ckusdc_payment: vault #{} paid from its generation account at ckUSDC block {}; collateral remains held", vault_id, paid.block_index);
    Ok(())
}

fn record_id(record: &history::LiquidationRecordVersioned) -> u64 {
    match record {
        history::LiquidationRecordVersioned::V1(record) => record.id,
    }
}

/// A typed first-dispatch BadFee or InsufficientFunds is direct no-effect
/// evidence. Reprice only that exact generation-bound tuple after refreshing
/// both the live fee and the isolated account balance. Any later attempt,
/// ambiguity, or TooOld remains on the fixed-history reconciliation path.
async fn reprice_first_typed_claim_payment_rejection(
    config: &BotConfig,
    vault_id: u64,
    claim_timestamp: u64,
    payment_memo: &[u8],
    minimum_payment_e6: u64,
    expected_intent: &history::BotClaimIntent,
    expected_record_id: u64,
    saved: &history::BotCkUsdcPaymentTransfer,
) -> Result<Option<history::BotCkUsdcPaymentTransfer>, String> {
    if saved.dispatch_attempt_count != Some(1)
        || saved.block_index.is_some()
        || saved.history_scan.is_some()
        || !matches!(
            saved.dispatch_observation,
            Some(
                history::BotCkUsdcPaymentDispatchObservation::BadFee { .. }
                    | history::BotCkUsdcPaymentDispatchObservation::InsufficientFunds { .. }
            )
        )
    {
        return Ok(None);
    }
    let source_subaccount = process::claim_payment_subaccount(vault_id, claim_timestamp);
    if saved.ledger != config.ckusdc_ledger
        || saved.args.from_subaccount != Some(source_subaccount)
        || saved.args.to
            != (Account {
                owner: config.backend_principal,
                subaccount: None,
            })
        || saved.args.memo.as_ref().map(|memo| memo.0.as_ref()) != Some(payment_memo)
        || saved.args.amount.0.to_string().parse::<u64>().ok() != Some(minimum_payment_e6)
    {
        return Err(
            "typed no-effect payment is not bound to the exact claim generation and full principal"
                .into(),
        );
    }

    let fee_e6 = swap::fetch_ledger_fee(config.ckusdc_ledger).await?;
    let gross_required = minimum_payment_e6
        .checked_add(fee_e6)
        .ok_or("claim payment plus refreshed fee overflows".to_string())?;
    let balance = swap::balance_of_ckusdc_account(
        config.ckusdc_ledger,
        Account {
            owner: ic_cdk::id(),
            subaccount: Some(source_subaccount),
        },
    )
    .await?;
    if balance < gross_required {
        return Err(format!(
            "generation account has {balance} e6; refreshed full-principal payment requires {gross_required} e6"
        ));
    }

    let current_intent = history::get_claim_intent(vault_id)
        .ok_or("claim intent disappeared during typed no-effect recovery".to_string())?;
    let current_record = history::get_latest_record_for_vault(vault_id)
        .ok_or("claim record disappeared during typed no-effect recovery".to_string())?;
    let exact_generation = current_intent == *expected_intent
        && matches!(&current_intent.phase,
            history::BotClaimIntentPhase::Claimed(receipt)
                | history::BotClaimIntentPhase::SwapStarted(receipt)
                if receipt.claim_timestamp == claim_timestamp
                    && receipt.payment_memo.as_slice() == payment_memo)
        && matches!(&current_record,
            history::LiquidationRecordVersioned::V1(record)
                if record.id == expected_record_id);
    if !exact_generation || !backend_claim_is_active(config, vault_id).await? {
        return Err("claim generation, record, or backend active claim changed during typed no-effect fee/balance preflight".into());
    }

    let old_created_at = saved
        .args
        .created_at_time
        .ok_or("rejected payment tuple lacks created_at_time".to_string())?;
    let next_created_at = old_created_at
        .checked_add(1)
        .ok_or("payment timestamp is exhausted; typed no-effect tuple remains held".to_string())?
        .max(ic_cdk::api::time())
        .max(claim_timestamp);
    let replacement = swap::prepare_ckusdc_payment_transfer_from_subaccount(
        config,
        gross_required,
        payment_memo,
        fee_e6,
        next_created_at,
        Some(source_subaccount),
    )?;
    if !paused_claim_payment_tuple_matches(
        &replacement,
        config,
        vault_id,
        &match &current_intent.phase {
            history::BotClaimIntentPhase::Claimed(receipt)
            | history::BotClaimIntentPhase::SwapStarted(receipt) => receipt.clone(),
            _ => return Err("claim phase changed during typed no-effect preflight".into()),
        },
        minimum_payment_e6,
    ) || !history::replace_ckusdc_payment_after_first_no_effect(
        vault_id,
        saved,
        replacement.clone(),
    ) {
        return Err("could not persist the replacement tuple and prior typed no-effect tombstone; no transfer dispatched".into());
    }
    Ok(Some(replacement))
}

/// A retry may use only the fee pinned before this exact claim's pool call.
/// Absence is a legacy unknown, never a reason to use today's configured fee.
fn pinned_pool_input_fee_for_retry(
    intent: &history::BotClaimIntent,
    record: &history::LiquidationRecordV1,
    backend: Principal,
) -> Option<u64> {
    let history::BotClaimIntentPhase::SwapStarted(receipt) = &intent.phase else {
        return None;
    };
    if intent.vault_id != record.vault_id
        || intent.backend_principal != Some(backend)
        || record.claim_timestamp != Some(receipt.claim_timestamp)
        || record.payment_memo.as_deref() != Some(receipt.payment_memo.as_slice())
        || record.debt_to_cover_e8s != receipt.debt_covered
        || record.collateral_claimed_e8s != receipt.collateral_amount
        || record.oracle_price_e8s != receipt.collateral_price_e8s
    {
        return None;
    }
    intent.icp_pool_input_fee_e8s
}

/// Seed one exact interrupted payment attempt for PocketIC recovery tests.
/// This endpoint is absent from the default Wasm and Candid interface.
#[cfg(feature = "test_endpoints")]
#[update]
fn test_seed_ckusdc_payment_recovery(
    intent: history::BotClaimIntent,
    record: history::LiquidationRecordVersioned,
) -> Result<(), String> {
    require_admin();
    let config = state::read_state(|s| s.config.clone()).ok_or("Not configured".to_string())?;
    let history::LiquidationRecordVersioned::V1(record) = record;
    if record.status != history::LiquidationStatus::TransferFailed
        || record.vault_id != intent.vault_id
        || record.claim_timestamp.is_none()
        || record
            .payment_memo
            .as_ref()
            .is_none_or(|memo| memo.is_empty())
    {
        return Err("fixture record is not a matching unresolved payment attempt".into());
    }
    let receipt = match &intent.phase {
        history::BotClaimIntentPhase::SwapStarted(receipt) => receipt,
        _ => return Err("fixture intent must be in SwapStarted phase".into()),
    };
    let transfer = intent
        .ckusdc_payment_transfer
        .as_ref()
        .ok_or("fixture intent lacks the original payment tuple")?;
    let scan = transfer
        .history_scan
        .as_ref()
        .ok_or("fixture tuple must begin in history reconciliation")?;
    let invalid_binding = if intent.backend_principal != Some(config.backend_principal) {
        Some("backend principal mismatch")
    } else if receipt.claim_timestamp != record.claim_timestamp.unwrap() {
        Some("claim timestamp mismatch")
    } else if receipt.debt_covered != record.debt_to_cover_e8s {
        Some("claim debt mismatch")
    } else if Some(&receipt.payment_memo) != record.payment_memo.as_ref() {
        Some("claim memo mismatch")
    } else if transfer.ledger != config.ckusdc_ledger {
        Some("ledger mismatch")
    } else if transfer.block_index.is_some() {
        Some("block index must be unresolved")
    } else if transfer.args.from_subaccount
        != Some(process::claim_payment_subaccount(
            intent.vault_id,
            receipt.claim_timestamp,
        ))
    {
        Some("from subaccount must match the exact claim generation")
    } else if transfer.args.to.owner != config.backend_principal {
        Some("payment recipient mismatch")
    } else if transfer.args.to.subaccount.is_some() {
        Some("recipient subaccount must be absent")
    } else if transfer.args.fee.is_none() {
        Some("payment fee must be explicit")
    } else if transfer.args.memo.as_ref().map(|memo| memo.0.as_ref())
        != record.payment_memo.as_deref()
    {
        Some("payment tuple memo mismatch")
    } else if transfer
        .args
        .created_at_time
        .is_none_or(|created| created < receipt.claim_timestamp)
    {
        Some("payment tuple timestamp predates claim")
    } else if scan.next_index > scan.snapshot_log_length {
        Some("history cursor exceeds fixed snapshot")
    } else if scan.candidate_block_index.is_some() {
        Some("fixture scan already has a candidate")
    } else if scan.multiple_candidates {
        Some("fixture scan has conflicting candidates")
    } else {
        None
    };
    if let Some(reason) = invalid_binding {
        return Err(format!("fixture journal binding failed: {reason}"));
    }
    if history::next_id() != record.id {
        return Err("fixture record id is not the next unused history id".into());
    }
    history::put_claim_intent(intent);
    history::insert_record(history::LiquidationRecordVersioned::V1(record));
    Ok(())
}

/// Submit a native ICP transfer from the bot principal for local PocketIC
/// receipt-decoder tests. This endpoint is absent from the default Wasm.
fn enqueue_claim_receipt_retry(s: &mut BotState, vault_id: u64) -> bool {
    // Move any existing copy to the back because process_pending pops from
    // the back. This both deduplicates and ensures the admin request selects
    // this vault before any concurrent notification can replace the queue.
    let inserted = !s.pending_vaults.iter().any(|v| v.vault_id == vault_id);
    s.pending_vaults.retain(|v| v.vault_id != vault_id);
    s.pending_vaults.push(LiquidatableVaultInfo {
        vault_id,
        collateral_type: Principal::anonymous(),
        debt_amount: 0,
        collateral_amount: 0,
        recommended_liquidation_amount: 0,
        collateral_price_e8s: 0,
    });
    inserted
}

fn take_admin_requeued_vault(s: &mut BotState, vault_id: u64) -> Option<LiquidatableVaultInfo> {
    enqueue_claim_receipt_retry(s, vault_id);
    if !process::AUTOMATIC_CLAIM_SWAP_ENABLED {
        // Keep the durable no-effect ACK discoverable by the timer if an
        // upgrade interrupts the spawned worker before it clears the intent.
        s.pending_vaults.last().cloned()
    } else {
        s.pending_vaults.pop()
    }
}

fn paused_claim_requeue_allowed(intent: Option<&history::BotClaimIntent>) -> bool {
    intent.is_some_and(|intent| {
        matches!(
            intent.phase,
            history::BotClaimIntentPhase::NoEffectAcknowledgementPending { .. }
        )
    })
}

fn is_pending_receipt_intent(
    intent: Option<history::BotClaimIntent>,
    vault_id: u64,
    backend_principal: Principal,
) -> bool {
    intent.is_some_and(|intent| {
        intent.vault_id == vault_id
            && intent.backend_principal.as_ref() == Some(&backend_principal)
            && !(intent.return_transfer.is_some()
                && matches!(intent.phase, history::BotClaimIntentPhase::Claimed(_)))
            && matches!(
                intent.phase,
                history::BotClaimIntentPhase::AwaitingReceipt
                    | history::BotClaimIntentPhase::ClaimRequested
                    | history::BotClaimIntentPhase::NoEffectAcknowledgementPending { .. }
                    | history::BotClaimIntentPhase::Claimed(_)
            )
    })
}

fn backend_identity_change_allowed(
    current: Option<Principal>,
    requested: Principal,
    has_claim_intents: bool,
) -> bool {
    match current {
        Some(current) => current == requested || !has_claim_intents,
        None => !has_claim_intents,
    }
}

fn admin_claim_resolution_is_terminal(status: &history::LiquidationStatus) -> bool {
    matches!(status, history::LiquidationStatus::Completed)
}

fn clear_claim_intent_if_terminal_admin_status(vault_id: u64, status: &history::LiquidationStatus) {
    if admin_claim_resolution_is_terminal(status) {
        history::remove_claim_intent(vault_id);
    }
}

/// Schedule recovery of a durable exact-ID claim intent. While automatic
/// swaps are paused, only a pending no-effect ACK may be requeued here: the
/// backend claim method could otherwise create a fresh collateral claim.
/// Ok means the worker was scheduled, not that recovery completed.
#[update]
fn admin_requeue_pending_bot_claim(vault_id: u64) -> Result<(), String> {
    require_admin();
    let _guard =
        ProcessingGuard::acquire().map_err(|_| "Another operation is in progress".to_string())?;
    let backend_principal =
        state::read_state(|s| s.config.as_ref().map(|config| config.backend_principal))
            .ok_or_else(|| "Config not set".to_string())?;
    let intent = history::get_claim_intent(vault_id);
    // While swap output cannot be attributed to a claim, this admin recovery
    // endpoint must not turn an uncertain or pre-dispatch request into a fresh
    // collateral claim. Exact no-effect ACK remains callable here; existing
    // claimed collateral has a separate explicit return/payment recovery.
    if !process::AUTOMATIC_CLAIM_SWAP_ENABLED && !paused_claim_requeue_allowed(intent.as_ref()) {
        return Err(
            "Bot claim dispatch is paused; only an exact pending no-effect ACK can be requeued"
                .into(),
        );
    }
    if !is_pending_receipt_intent(intent, vault_id, backend_principal) {
        return Err(
            "No resumable claim-requested or pre-swap claimed receipt matches this backend".into(),
        );
    }

    ProcessingGuard::set_vault_id(vault_id);
    let vault = state::mutate_state(|s| take_admin_requeued_vault(s, vault_id))
        .ok_or_else(|| "Unable to enqueue exact claim receipt retry".to_string())?;
    // Transfer the guard into the spawned worker. It processes this captured
    // vault directly rather than rereading the replaceable notification queue;
    // retaining the guard prevents overlapping timer or admin processing.
    ic_cdk::spawn(process::process_specific_vault(vault, _guard));
    Ok(())
}

/// Retry a persisted claim-specific collateral return and complete the
/// backend's existing receipt-gated cancellation. This does not process swaps
/// or create a new transfer identity.
#[update]
async fn admin_retry_claim_return(vault_id: u64) -> Result<(), String> {
    require_admin();
    let _guard =
        ProcessingGuard::acquire().map_err(|_| "Another operation is in progress".to_string())?;
    ProcessingGuard::set_vault_id(vault_id);
    let config =
        state::read_state(|s| s.config.clone()).ok_or_else(|| "Config not set".to_string())?;
    process::retry_claim_collateral_return(&config, vault_id).await
}

/// One-time: fetch pool metadata to determine if ICP is token0 or token1.
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

    log!(
        INFO,
        "Infinite approve set: ICP ledger {} -> pool {}",
        icp_ledger,
        pool
    );
}

/// Emergency: transfer all bot ckUSDC to a target principal.
/// Optionally mark an associated history record as AdminResolved.
#[update]
async fn admin_sweep_ckusdc(target: Principal, record_id: Option<u64>) {
    require_admin();
    let _guard = ProcessingGuard::acquire()
        .unwrap_or_else(|_| ic_cdk::trap("Another operation is in progress"));
    let ckusdc_ledger =
        state::read_state(|s| s.config.as_ref().expect("Config not set").ckusdc_ledger);

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

    let result: Result<(Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError>,), _> =
        ic_cdk::call(ckusdc_ledger, "icrc1_transfer", (transfer_args,)).await;

    match result {
        Ok((Ok(block),)) => {
            log!(
                INFO,
                "Swept {} ckUSDC e6 to {}, block {}",
                send_amount,
                target,
                block
            );
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
    let _guard =
        ProcessingGuard::acquire().map_err(|_| "Another operation is in progress".to_string())?;
    let config =
        state::read_state(|s| s.config.clone()).ok_or_else(|| "Config not set".to_string())?;

    log!(
        INFO,
        "admin_test_swap: attempting to swap {} ICP e8s",
        amount_e8s
    );
    let result = swap::swap_icp_for_ckusdc(&config, amount_e8s).await;
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

/// Retry confirm for a stuck claim.
#[update]
async fn admin_retry_stuck_claim(vault_id: u64) {
    require_admin();
    let _guard = ProcessingGuard::acquire()
        .unwrap_or_else(|_| ic_cdk::trap("Another operation is in progress"));
    ProcessingGuard::set_vault_id(vault_id);
    let config = state::read_state(|s| s.config.clone()).expect("Not configured");

    // Do not let an admin retry turn an unpaid stuck claim into a completed
    // liquidation. ConfirmFailed is also used when swap failure cleanup could
    // not cancel the claim, so require the latest attempt to prove that a
    // positive ckUSDC transfer succeeded before calling the backend.
    let mut record = history::get_latest_record_for_vault(vault_id)
        .unwrap_or_else(|| ic_cdk::trap("No liquidation history for this vault"));

    // This query is intentionally issued before confirmation: local history
    // can outlive the backend claim after auto-cancel or admin resolution.
    let active_claims: Result<(Vec<u64>,), _> =
        ic_cdk::call(config.backend_principal, "get_bot_claim_vault_ids", ()).await;
    let (active_claim_ids,) = match active_claims {
        Ok(ids) => ids,
        Err((code, msg)) => ic_cdk::trap(&format!(
            "Unable to verify active backend claim for vault #{}: {:?}: {}",
            vault_id, code, msg
        )),
    };

    // A TransferFailed row may represent a lost reply from the original
    // ckUSDC call. Recover only from the exact tuple durably attached to the
    // matching SwapStarted generation; never synthesize a new timestamp or
    // transfer identity here.
    let transfer_failed_record = match &record {
        history::LiquidationRecordVersioned::V1(r)
            if r.vault_id == vault_id && r.status == history::LiquidationStatus::TransferFailed =>
        {
            Some(r.clone())
        }
        _ => None,
    };
    if let Some(r) = transfer_failed_record {
        if active_claim_ids.contains(&vault_id) {
            let claim_timestamp = r.claim_timestamp.unwrap_or_else(|| {
                ic_cdk::trap("TransferFailed claim lacks a generation timestamp")
            });
            let payment_memo = r
                .payment_memo
                .clone()
                .filter(|memo| !memo.is_empty())
                .unwrap_or_else(|| ic_cdk::trap("TransferFailed claim lacks its payment memo"));
            let intent = history::get_claim_intent(vault_id).unwrap_or_else(|| {
                ic_cdk::trap("TransferFailed claim has no durable claim intent")
            });
            if intent.vault_id != vault_id
                || intent.backend_principal.as_ref() != Some(&config.backend_principal)
            {
                ic_cdk::trap(
                    "TransferFailed claim intent is bound to a different vault or backend",
                );
            }
            let receipt_matches = match &intent.phase {
                history::BotClaimIntentPhase::SwapStarted(receipt) => {
                    receipt.claim_timestamp == claim_timestamp
                        && receipt.payment_memo == payment_memo
                        && receipt.debt_covered == r.debt_to_cover_e8s
                }
                _ => false,
            };
            if !receipt_matches {
                ic_cdk::trap(
                    "TransferFailed record does not match the durable SwapStarted generation",
                );
            }
            let mut transfer = intent.ckusdc_payment_transfer.unwrap_or_else(|| {
                ic_cdk::trap("TransferFailed claim has no persisted original ckUSDC tuple")
            });
            let paid = match process::dispatch_and_verify_ckusdc_payment(
                &config,
                vault_id,
                claim_timestamp,
                &payment_memo,
                &mut transfer,
            )
            .await
            {
                Ok(paid) => paid,
                Err(error) => {
                    // Reconciliation may have durably advanced one bounded
                    // ICRC-3 page. Return normally so those stable cursor
                    // writes commit; trapping here would roll the page back.
                    log!(
                        INFO,
                        "Original ckUSDC transfer remains held for vault #{}: {}",
                        vault_id,
                        error
                    );
                    return;
                }
            };
            let mut recovered = r.clone();
            recovered.status = history::LiquidationStatus::ConfirmFailed;
            recovered.ckusdc_transferred_e6 = paid.amount_e6;
            recovered.ckusdc_payment_block_index = Some(paid.block_index);
            recovered.ckusdc_payment_amount_e6 = Some(paid.amount_e6);
            recovered.confirm_retry_count = 0;
            recovered.error_message =
                Some("original ckUSDC payment authenticated; backend confirmation pending".into());
            history::insert_record(history::LiquidationRecordVersioned::V1(recovered));
            record = history::get_record(r.id).unwrap_or_else(|| {
                ic_cdk::trap("Recovered liquidation history record disappeared")
            });
        }
    }
    let active_claim_or_committed_top_up = active_claim_ids.contains(&vault_id)
        || history::get_claim_intent(vault_id).is_some_and(|intent| {
            intent
                .ckusdc_top_up_transfer
                .as_ref()
                .is_some_and(|transfer| transfer.block_index.is_some())
                || intent
                    .ckusdc_payment_transfer
                    .as_ref()
                    .is_some_and(|transfer| transfer.block_index.is_some())
        });
    let (
        record_id,
        claim_timestamp,
        payment_block_index,
        debt_to_cover,
        expected_payment_amount,
        payment_memo,
    ) = match &record {
        history::LiquidationRecordVersioned::V1(r)
            if r.vault_id == vault_id
                && history::is_paid_confirm_failure(r)
                && active_claim_or_committed_top_up =>
        {
            (
                r.id,
                r.claim_timestamp.expect("validated claim timestamp"),
                r.ckusdc_payment_block_index
                    .expect("validated payment block"),
                r.debt_to_cover_e8s,
                r.ckusdc_payment_amount_e6
                    .expect("validated payment amount"),
                r.payment_memo.clone().expect("validated payment memo"),
            )
        }
        _ => ic_cdk::trap("No active paid ConfirmFailed claim for this vault"),
    };

    // Local history is only a candidate. The original payment must be the
    // exact journaled transfer from this claim generation's isolated account,
    // including fee and timestamp, before confirmation or top-up recovery.
    let pinned_payment = history::get_claim_intent(vault_id)
        .and_then(|intent| intent.ckusdc_payment_transfer)
        .unwrap_or_else(|| {
            ic_cdk::trap("Paid claim has no exact original ckUSDC transfer journal")
        });
    let expected_source = Account {
        owner: ic_cdk::id(),
        subaccount: Some(process::claim_payment_subaccount(vault_id, claim_timestamp)),
    };
    let expected_destination = Account {
        owner: config.backend_principal,
        subaccount: None,
    };
    let pinned_amount = pinned_payment.args.amount.0.to_string().parse::<u64>().ok();
    let pinned_fee = pinned_payment
        .args
        .fee
        .as_ref()
        .and_then(|fee| fee.0.to_string().parse::<u64>().ok());
    let pinned_time = pinned_payment.args.created_at_time;
    if pinned_payment.ledger != config.ckusdc_ledger
        || pinned_payment.block_index != Some(payment_block_index)
        || pinned_payment.args.from_subaccount != expected_source.subaccount
        || pinned_payment.args.to != expected_destination
        || pinned_payment
            .args
            .memo
            .as_ref()
            .map(|memo| memo.0.as_ref())
            != Some(payment_memo.as_slice())
        || pinned_amount != Some(expected_payment_amount)
        || pinned_fee.is_none()
        || pinned_time.is_none_or(|time| time < claim_timestamp)
    {
        ic_cdk::trap("Paid claim does not match its exact isolated-account ckUSDC journal");
    }
    let original_payment = swap::verify_ckusdc_transfer_block(
        config.ckusdc_ledger,
        payment_block_index,
        expected_source,
        expected_destination,
        &payment_memo,
    )
    .await
    .unwrap_or_else(|error| {
        ic_cdk::trap(&format!(
            "Cannot authenticate original ckUSDC block {} for vault #{}: {}",
            payment_block_index, vault_id, error
        ))
    });
    if original_payment.amount_e6 != expected_payment_amount
        || Some(original_payment.fee_e6) != pinned_fee
        || Some(original_payment.created_at_time) != pinned_time
    {
        ic_cdk::trap(
            "Original ckUSDC block does not match the exact persisted claim payment tuple",
        );
    }
    // This is the fee passed to the pool for this exact swap, not today's
    // configured ICP fee. Older intents lack the pin and cannot safely create
    // a new treasury obligation. Hold them before backend confirmation.
    let history::LiquidationRecordVersioned::V1(ref candidate) = record;
    let pool_input_fee_e8s = history::get_claim_intent(vault_id)
        .as_ref()
        .and_then(|intent| pinned_pool_input_fee_for_retry(intent, candidate, config.backend_principal))
        .unwrap_or_else(|| ic_cdk::trap("Claim has no exact pinned ICP pool input fee; backend confirmation and treasury preparation remain held"));
    let minimum_payment = process::ckusdc_minimum_payment_e6(debt_to_cover);
    if original_payment.amount_e6 < minimum_payment {
        let history::LiquidationRecordVersioned::V1(r) = &record;
        let recovered =
            match process::recover_short_ckusdc_payment(&config, r, &original_payment).await {
                Ok(recovered) => recovered,
                Err(error) => {
                    // Preserve any exact tuple or scan cursor written before a
                    // ledger ambiguity; trapping would roll that evidence back.
                    log!(
                        INFO,
                        "ckUSDC short-payment recovery held for vault #{}: {}",
                        vault_id,
                        error
                    );
                    return;
                }
            };
        if !recovered {
            log!(INFO, "ckUSDC short-payment recovery did not finish the required multi-block confirmation for vault #{}", vault_id);
            return;
        }
    }
    let confirm_result = if original_payment.amount_e6 < minimum_payment {
        // The recovery helper submitted the exact two-block confirmation.
        Ok(())
    } else {
        process::call_bot_confirm_liquidation(
            &config,
            vault_id,
            claim_timestamp,
            payment_block_index,
        )
        .await
    };

    match confirm_result {
        Ok(()) => {
            let current = history::get_record(record_id)
                .unwrap_or_else(|| ic_cdk::trap("Liquidation history record disappeared"));
            let history::LiquidationRecordVersioned::V1(mut r) = current;
            let obligation = process::icp_treasury_bonus_gross_after_swap(
                r.collateral_claimed_e8s,
                r.icp_swapped_e8s,
                pool_input_fee_e8s,
            );
            state::mutate_state(|s| {
                s.stats.total_debt_covered_e8s = s
                    .stats
                    .total_debt_covered_e8s
                    .saturating_add(r.debt_to_cover_e8s);
                s.stats.total_collateral_received_e8s = s
                    .stats
                    .total_collateral_received_e8s
                    .saturating_add(r.collateral_claimed_e8s);
                s.stats.events_count = s.stats.events_count.saturating_add(1);
            });
            if obligation == 0 {
                history::update_record_status(record_id, history::LiquidationStatus::Completed);
                log!(
                    INFO,
                    "admin_retry_stuck_claim: confirmed vault #{} with no ICP bonus",
                    vault_id
                );
                clear_claim_intent_if_terminal_admin_status(
                    vault_id,
                    &history::LiquidationStatus::Completed,
                );
                return;
            }

            match swap::prepare_icp_treasury_bonus_transfer(&config, obligation, record_id) {
                Ok(mut transfer) => {
                    r.icp_to_treasury_e8s = obligation;
                    r.icp_treasury_transfer = Some(transfer.clone());
                    r.icp_treasury_bonus_state = Some(history::IcpTreasuryBonusState::Prepared);
                    r.status = history::LiquidationStatus::TransferFailed;
                    r.error_message = Some(
                        "Backend claim confirmed; ICP treasury bonus transfer prepared and pending"
                            .into(),
                    );
                    history::insert_record(history::LiquidationRecordVersioned::V1(r));
                    match swap::transfer_icp_treasury_bonus_exact(&transfer).await {
                        Ok(block_index) => {
                            transfer.block_index = Some(block_index);
                            if !history::update_icp_treasury_bonus(
                                record_id,
                                Some(transfer),
                                history::IcpTreasuryBonusState::Paid,
                                history::LiquidationStatus::Completed,
                                None,
                            ) {
                                ic_cdk::trap("ICP treasury transfer accepted but its history record disappeared");
                            }
                            state::mutate_state(|s| {
                                s.stats.total_collateral_to_treasury_e8s = s
                                    .stats
                                    .total_collateral_to_treasury_e8s
                                    .saturating_add(obligation);
                            });
                            log!(
                                INFO,
                                "admin_retry_stuck_claim: vault #{} confirmed; ICP bonus block {}",
                                vault_id,
                                block_index
                            );
                            clear_claim_intent_if_terminal_admin_status(
                                vault_id,
                                &history::LiquidationStatus::Completed,
                            );
                        }
                        Err(error) => {
                            history::update_icp_treasury_bonus(
                                record_id,
                                Some(transfer),
                                history::IcpTreasuryBonusState::Prepared,
                                history::LiquidationStatus::TransferFailed,
                                Some(format!(
                                    "Backend claim confirmed; ICP treasury bonus pending: {error}"
                                )),
                            );
                            log!(INFO, "admin_retry_stuck_claim: vault #{} confirmed; ICP bonus remains pending: {}", vault_id, error);
                        }
                    }
                }
                Err(error) => {
                    r.icp_to_treasury_e8s = obligation;
                    r.icp_treasury_transfer = None;
                    r.icp_treasury_bonus_state = Some(history::IcpTreasuryBonusState::Quarantined);
                    r.status = history::LiquidationStatus::TransferFailed;
                    r.error_message = Some(format!(
                        "Backend claim confirmed; ICP treasury bonus quarantined because no transfer could be prepared: {error}"
                    ));
                    history::insert_record(history::LiquidationRecordVersioned::V1(r));
                    log!(
                        INFO,
                        "admin_retry_stuck_claim: vault #{} confirmed; ICP bonus quarantined: {}",
                        vault_id,
                        error
                    );
                }
            }
        }
        Err(e) => {
            ic_cdk::trap(&format!(
                "Confirm still failing for vault #{}: {}",
                vault_id, e
            ));
        }
    }
}

/// Admin-funded recovery for a claim whose completed swap was durably held
/// before any ckUSDC transfer because its measured proceeds were short.
/// `maximum_subsidy_e6` caps the amount needed to reach the claim minimum;
/// the exact full-minimum transfer identity is persisted before dispatch.
#[update]
async fn admin_recover_ckusdc_shortfall(
    vault_id: u64,
    maximum_subsidy_e6: u64,
) -> Result<(), String> {
    require_admin();
    let _guard =
        ProcessingGuard::acquire().map_err(|_| "Another operation is in progress".to_string())?;
    ProcessingGuard::set_vault_id(vault_id);
    let config = state::read_state(|s| s.config.clone()).ok_or("Config not set".to_string())?;

    let record = history::get_latest_record_for_vault(vault_id)
        .ok_or("No liquidation history for this vault".to_string())?;
    let r = match &record {
        history::LiquidationRecordVersioned::V1(r)
            if r.vault_id == vault_id && r.status == history::LiquidationStatus::TransferFailed =>
        {
            r
        }
        _ => return Err("latest vault record is not an eligible TransferFailed shortfall".into()),
    };
    let claim_timestamp = r
        .claim_timestamp
        .ok_or("shortfall record lacks claim timestamp".to_string())?;
    let payment_memo = r
        .payment_memo
        .clone()
        .filter(|memo| !memo.is_empty())
        .ok_or("shortfall record lacks payment memo".to_string())?;
    if r.ckusdc_transferred_e6 != 0
        || r.ckusdc_payment_block_index.is_some()
        || r.ckusdc_payment_amount_e6.is_some()
    {
        return Err("shortfall record already contains payment evidence".into());
    }
    let intent = history::get_claim_intent(vault_id)
        .ok_or("shortfall claim intent is missing".to_string())?;
    let receipt = match &intent.phase {
        history::BotClaimIntentPhase::SwapStarted(receipt) => receipt.clone(),
        _ => return Err("shortfall intent is not in SwapStarted phase".into()),
    };
    let marker = intent
        .shortfall_eligibility
        .clone()
        .ok_or("claim has no durable shortfall eligibility marker".to_string())?;
    if intent.vault_id != vault_id
        || intent.backend_principal != Some(config.backend_principal)
        || receipt.claim_timestamp != claim_timestamp
        || receipt.payment_memo != payment_memo
        || receipt.debt_covered != r.debt_to_cover_e8s
        || receipt.collateral_amount != r.collateral_claimed_e8s
        || receipt.collateral_price_e8s != r.oracle_price_e8s
        || marker.ledger != config.ckusdc_ledger
        || marker.measured_reserved_output_e6 == 0
        || marker.measured_reserved_output_e6 != r.ckusdc_received_e6
        || marker.minimum_payment_e6 != process::ckusdc_minimum_payment_e6(r.debt_to_cover_e8s)
        || intent.return_transfer.is_some()
        || intent.ckusdc_top_up_transfer.is_some()
    {
        return Err(
            "shortfall marker, record, claim generation, or configured ledger do not match".into(),
        );
    }
    let active_claims: Result<(Vec<u64>,), _> =
        ic_cdk::call(config.backend_principal, "get_bot_claim_vault_ids", ()).await;
    let (active_claim_ids,) = active_claims.map_err(|(code, msg)| {
        format!("unable to verify active backend claim: {:?}: {}", code, msg)
    })?;
    if !active_claim_ids.contains(&vault_id) {
        return Err("backend no longer has this claim active".into());
    }

    // Existing tuples represent an earlier authorization and possibly an
    // ambiguous or committed ledger call. Their stored fee/time/amount must
    // remain authoritative even if the live fee or wallet balance has moved.
    let needs_new_tuple = intent.ckusdc_payment_transfer.is_none();
    let mut live_fee_for_new_tuple = None;
    if needs_new_tuple {
        if maximum_subsidy_e6 == 0 {
            return Err("maximum subsidy must be positive".into());
        }
        let fee_e6 = swap::fetch_ledger_fee(config.ckusdc_ledger).await?;
        let available_net = marker.measured_reserved_output_e6.saturating_sub(fee_e6);
        let subsidy_e6 = marker.minimum_payment_e6.saturating_sub(available_net);
        if subsidy_e6 == 0 || subsidy_e6 > maximum_subsidy_e6 {
            return Err(format!(
                "required subsidy {} e6 is zero or exceeds the admin cap {} e6",
                subsidy_e6, maximum_subsidy_e6
            ));
        }
        let gross_required = marker
            .minimum_payment_e6
            .checked_add(fee_e6)
            .ok_or("full-minimum payment plus live fee overflows".to_string())?;
        let source_subaccount = process::claim_payment_subaccount(vault_id, claim_timestamp);
        let balance_e6 = swap::balance_of_ckusdc_account(
            config.ckusdc_ledger,
            Account {
                owner: ic_cdk::id(),
                subaccount: Some(source_subaccount),
            },
        )
        .await?;
        if balance_e6 < gross_required {
            return Err(format!(
                "bot ckUSDC balance {} e6 is below required gross payment {} e6",
                balance_e6, gross_required
            ));
        }
        live_fee_for_new_tuple = Some(fee_e6);
    }

    // Re-read after the ledger awaits. A prepared tuple can be resumed only
    // when the durable marker and exact SwapStarted receipt remain unchanged.
    let current_intent = history::get_claim_intent(vault_id)
        .ok_or("shortfall intent disappeared during preflight".to_string())?;
    let current_record = history::get_latest_record_for_vault(vault_id)
        .ok_or("shortfall record disappeared during preflight".to_string())?;
    if current_intent.shortfall_eligibility.as_ref() != Some(&marker)
        || current_intent.backend_principal != Some(config.backend_principal)
        || !matches!(&current_intent.phase, history::BotClaimIntentPhase::SwapStarted(saved) if saved == &receipt)
        || !matches!(&current_record, history::LiquidationRecordVersioned::V1(saved)
            if saved.id == r.id && saved.status == history::LiquidationStatus::TransferFailed
                && saved.claim_timestamp == Some(claim_timestamp) && saved.payment_memo.as_ref() == Some(&payment_memo)
                && saved.ckusdc_transferred_e6 == 0 && saved.ckusdc_payment_block_index.is_none()
                && saved.ckusdc_payment_amount_e6.is_none())
        || current_intent.return_transfer.is_some()
        || current_intent.ckusdc_top_up_transfer.is_some()
    {
        return Err("shortfall claim changed during ledger preflight".into());
    }

    let typed_no_effect_replacement = match current_intent.ckusdc_payment_transfer.as_ref() {
        Some(saved) => {
            reprice_first_typed_claim_payment_rejection(
                &config,
                vault_id,
                claim_timestamp,
                &payment_memo,
                marker.minimum_payment_e6,
                &current_intent,
                r.id,
                saved,
            )
            .await?
        }
        None => None,
    };

    let mut transfer = if let Some(replacement) = typed_no_effect_replacement {
        if !shortfall_payment_tuple_matches(
            &replacement,
            config.ckusdc_ledger,
            config.backend_principal,
            vault_id,
            claim_timestamp,
            &payment_memo,
            marker.minimum_payment_e6,
        ) {
            return Err(
                "replacement full-minimum tuple differs from this shortfall generation".into(),
            );
        }
        replacement
    } else if let Some(saved) = current_intent.ckusdc_payment_transfer.clone() {
        if !shortfall_payment_tuple_matches(
            &saved,
            config.ckusdc_ledger,
            config.backend_principal,
            vault_id,
            claim_timestamp,
            &payment_memo,
            marker.minimum_payment_e6,
        ) {
            return Err(
                "prepared full-minimum payment tuple does not match this shortfall generation"
                    .into(),
            );
        }
        saved
    } else {
        let fee_e6 = live_fee_for_new_tuple
            .ok_or("live ckUSDC fee was not verified for new payment tuple".to_string())?;
        let gross_required = marker
            .minimum_payment_e6
            .checked_add(fee_e6)
            .ok_or("full-minimum payment plus live fee overflows".to_string())?;
        let prepared = swap::prepare_ckusdc_payment_transfer_from_subaccount(
            &config,
            gross_required,
            &payment_memo,
            fee_e6,
            ic_cdk::api::time().max(claim_timestamp),
            Some(process::claim_payment_subaccount(vault_id, claim_timestamp)),
        )?;
        if !shortfall_payment_tuple_matches(
            &prepared,
            config.ckusdc_ledger,
            config.backend_principal,
            vault_id,
            claim_timestamp,
            &payment_memo,
            marker.minimum_payment_e6,
        ) {
            return Err(
                "prepared shortfall payment is not bound to the claim-specific account".into(),
            );
        }
        if !history::update_ckusdc_payment_transfer(vault_id, prepared.clone()) {
            return Err(
                "could not persist exact full-minimum ckUSDC payment tuple; no transfer attempted"
                    .into(),
            );
        }
        prepared
    };

    // One more backend check immediately before the payment call. From this
    // point onward every unresolved outcome returns normally so stable scan
    // cursor writes made by dispatch_and_verify are retained.
    let active_again: Result<(Vec<u64>,), _> =
        ic_cdk::call(config.backend_principal, "get_bot_claim_vault_ids", ()).await;
    let (active_again,) = active_again.map_err(|(code, msg)| {
        format!(
            "unable to revalidate active backend claim before payment: {:?}: {}",
            code, msg
        )
    })?;
    if !active_again.contains(&vault_id) {
        return Err("backend claim ceased to be active before payment dispatch".into());
    }
    let paid = match process::dispatch_and_verify_ckusdc_payment(
        &config,
        vault_id,
        claim_timestamp,
        &payment_memo,
        &mut transfer,
    )
    .await
    {
        Ok(paid) => paid,
        Err(error) => {
            return Err(format!(
                "full-minimum payment remains unresolved: {}",
                error
            ))
        }
    };

    // The payment is now proven. Persist a new paid ConfirmFailed record before
    // handing final confirmation to the existing admin_retry_stuck_claim path.
    let mut paid_record = match record {
        history::LiquidationRecordVersioned::V1(r) => r,
    };
    paid_record.id = history::next_id();
    paid_record.status = history::LiquidationStatus::ConfirmFailed;
    paid_record.ckusdc_transferred_e6 = paid.amount_e6;
    paid_record.ckusdc_payment_block_index = Some(paid.block_index);
    paid_record.ckusdc_payment_amount_e6 = Some(paid.amount_e6);
    paid_record.confirm_retry_count = 0;
    paid_record.error_message = Some(
        "admin shortfall recovery paid full claim minimum; backend confirmation pending".into(),
    );
    history::insert_record(history::LiquidationRecordVersioned::V1(paid_record));
    Ok(())
}

ic_cdk_macros::export_candid!();
