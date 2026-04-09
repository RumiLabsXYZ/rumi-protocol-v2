use candid::{CandidType, Deserialize, Nat, Principal};
use ic_cdk_macros::{init, post_upgrade, pre_upgrade, query, update};
use ic_canister_log::{declare_log_buffer, log};
use icrc_ledger_types::icrc1::account::Account;

mod history;
mod icpswap;
mod memory;
mod process;
mod state;
mod swap;

use state::{BotAdminAction, BotAdminEvent, BotConfig, BotState, LiquidatableVaultInfo};

declare_log_buffer!(name = INFO, capacity = 1000);

#[derive(CandidType, Deserialize)]
pub struct BotInitArgs {
    pub config: BotConfig,
}

#[init]
fn init(args: BotInitArgs) {
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
    let migrated = state::read_state(|s| s.migrated_to_stable_structures);
    if migrated {
        state::save_config_to_stable();
    } else {
        state::save_to_stable_memory();
    }
}

#[post_upgrade]
fn post_upgrade() {
    // STEP 1: Rescue legacy JSON blob BEFORE MemoryManager::init.
    // Raw stable64_read is safe here because MemoryManager hasn't been initialized yet.
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

    setup_timer();
}

fn setup_timer() {
    ic_cdk_timers::set_timer_interval(
        std::time::Duration::from_secs(30),
        || ic_cdk::spawn(process::process_pending()),
    );
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
        s.pending_vaults = vaults;
        s.admin_events.push(BotAdminEvent {
            timestamp: ic_cdk::api::time(),
            caller: caller.to_text(),
            action: BotAdminAction::VaultsNotified { count: count as u64 },
        });
    });
    log!(INFO, "Received {} liquidatable vaults from backend", count);
}

#[query]
fn get_bot_stats() -> state::BotStats {
    state::read_state(|s| s.stats.clone())
}

#[query]
fn get_admin_events(offset: u64, limit: u64) -> Vec<state::BotAdminEvent> {
    state::read_state(|s| {
        let len = s.admin_events.len();
        let start = (len as u64).saturating_sub(offset + limit) as usize;
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

#[update]
fn set_config(config: BotConfig) {
    require_admin();
    state::mutate_state(|s| {
        s.config = Some(config);
        s.admin_events.push(BotAdminEvent {
            timestamp: ic_cdk::api::time(),
            caller: ic_cdk::api::caller().to_text(),
            action: BotAdminAction::ConfigUpdated,
        });
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
    let (pool, icp_ledger) = state::read_state(|s| {
        let c = s.config.as_ref().unwrap();
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
    let (icp_ledger, pool) = state::read_state(|s| {
        let c = s.config.as_ref().unwrap();
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
    let ckusdc_ledger = state::read_state(|s| s.config.as_ref().unwrap().ckusdc_ledger);

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
            let val: u64 = b.0.to_string().parse().unwrap_or(0);
            if val == 0 {
                ic_cdk::trap("Bot has zero ckUSDC balance");
            }
            val
        }
        Err((code, msg)) => ic_cdk::trap(&format!("Balance query failed: {:?} {}", code, msg)),
    };

    let fee = state::read_state(|s| s.config.as_ref().unwrap().ckusdc_fee_e6.unwrap_or(10));
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

/// Retry confirm for a stuck claim.
#[update]
async fn admin_retry_stuck_claim(vault_id: u64) {
    require_admin();
    let config = state::read_state(|s| s.config.clone()).expect("Not configured");

    match process::call_bot_confirm_liquidation(&config, vault_id).await {
        Ok(()) => {
            log!(INFO, "admin_retry_stuck_claim: confirmed vault #{}", vault_id);
        }
        Err(e) => {
            ic_cdk::trap(&format!(
                "Confirm still failing for vault #{}: {}",
                vault_id, e
            ));
        }
    }
}
