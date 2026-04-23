use candid::{CandidType, Deserialize, Principal};
use ic_stable_structures::writer::Writer;
use serde::Serialize;
use std::cell::RefCell;

use crate::memory;

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct BotConfig {
    pub backend_principal: Principal,
    pub treasury_principal: Principal,
    pub admin: Principal,
    pub max_slippage_bps: u16,
    pub icp_ledger: Principal,
    pub ckusdc_ledger: Principal,

    // ICPSwap config (replaces kong_swap + three_pool)
    pub icpswap_pool: Principal,

    // Cached after admin_resolve_pool_ordering (determines swap direction)
    #[serde(default)]
    pub icpswap_zero_for_one: Option<bool>,

    // Cached ledger fees (set by admin or auto-detected)
    #[serde(default)]
    pub icp_fee_e8s: Option<u64>,
    #[serde(default)]
    pub ckusdc_fee_e6: Option<u64>,

    // Legacy fields kept for deserialization compatibility (ignored at runtime).
    // Can be removed after one upgrade cycle.
    #[serde(default)]
    pub three_pool_principal: Option<Principal>,
    #[serde(default)]
    pub kong_swap_principal: Option<Principal>,
    #[serde(default)]
    pub ckusdt_ledger: Option<Principal>,
    #[serde(default)]
    pub icusd_ledger: Option<Principal>,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct BotLiquidationEvent {
    pub timestamp: u64,
    pub vault_id: u64,
    pub debt_covered_e8s: u64,
    pub collateral_received_e8s: u64,
    pub icusd_burned_e8s: u64,
    pub collateral_to_treasury_e8s: u64,
    pub swap_route: String,
    pub effective_price_e8s: u64,
    pub slippage_bps: i32,
    pub success: bool,
    pub error_message: Option<String>,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, Default)]
pub struct BotStats {
    pub total_debt_covered_e8s: u64,
    #[serde(default, alias = "total_icusd_burned_e8s")]
    pub total_ckusdc_deposited_e6: u64,
    pub total_collateral_received_e8s: u64,
    pub total_collateral_to_treasury_e8s: u64,
    pub events_count: u64,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct LiquidatableVaultInfo {
    pub vault_id: u64,
    pub collateral_type: Principal,
    pub debt_amount: u64,
    pub collateral_amount: u64,
    pub recommended_liquidation_amount: u64,
    pub collateral_price_e8s: u64,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct BotAdminEvent {
    pub timestamp: u64,
    pub caller: String,
    pub action: BotAdminAction,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub enum BotAdminAction {
    ConfigUpdated,
    VaultsNotified { count: u64 },
}

#[derive(Serialize, Deserialize, Default)]
pub struct BotState {
    pub config: Option<BotConfig>,
    pub stats: BotStats,
    pub liquidation_events: Vec<BotLiquidationEvent>,
    pub pending_vaults: Vec<LiquidatableVaultInfo>,
    #[serde(default)]
    pub admin_events: Vec<BotAdminEvent>,
    #[serde(default)]
    pub migrated_to_stable_structures: bool,
}

thread_local! {
    static STATE: RefCell<Option<BotState>> = RefCell::default();
}

pub fn mutate_state<F, R>(f: F) -> R
where
    F: FnOnce(&mut BotState) -> R,
{
    STATE.with(|s| f(s.borrow_mut().as_mut().expect("State not initialized")))
}

pub fn read_state<F, R>(f: F) -> R
where
    F: FnOnce(&BotState) -> R,
{
    STATE.with(|s| f(s.borrow().as_ref().expect("State not initialized")))
}

pub fn init_state(state: BotState) {
    STATE.with(|s| *s.borrow_mut() = Some(state));
}

// ---- Legacy stable memory (raw offset 0, used only for first migration) ----

pub fn save_to_stable_memory() {
    STATE.with(|s| {
        let state = s.borrow();
        let state = state.as_ref().expect("State not initialized");
        let bytes = serde_json::to_vec(state).expect("Failed to serialize state");
        let len = bytes.len() as u64;
        let pages_needed = (len + 8 + 65535) / 65536;
        let current_pages = ic_cdk::api::stable::stable64_size();
        if pages_needed > current_pages {
            ic_cdk::api::stable::stable64_grow(pages_needed - current_pages)
                .expect("Failed to grow stable memory");
        }
        ic_cdk::api::stable::stable64_write(0, &len.to_le_bytes());
        ic_cdk::api::stable::stable64_write(8, &bytes);
    });
}

pub fn load_from_stable_memory() {
    let size = ic_cdk::api::stable::stable64_size();
    if size == 0 {
        init_state(BotState::default());
        return;
    }
    let mut len_bytes = [0u8; 8];
    ic_cdk::api::stable::stable64_read(0, &mut len_bytes);
    let len = u64::from_le_bytes(len_bytes) as usize;
    if len == 0 {
        init_state(BotState::default());
        return;
    }
    let mut bytes = vec![0u8; len];
    ic_cdk::api::stable::stable64_read(8, &mut bytes);
    let state: BotState = serde_json::from_slice(&bytes).expect("Failed to deserialize state");
    init_state(state);
}

// ---- New stable memory (MemoryManager virtual region MEM_ID_CONFIG) ----

pub fn save_config_to_stable() {
    STATE.with(|s| {
        let state = s.borrow();
        let state = state.as_ref().expect("State not initialized");
        let bytes = serde_json::to_vec(state).expect("Failed to serialize state");
        let len = bytes.len() as u64;

        let mut mem = memory::get_memory(memory::MEM_ID_CONFIG);
        let mut writer = Writer::new(&mut mem, 0);
        writer
            .write(&len.to_le_bytes())
            .expect("Failed to write config length");
        writer
            .write(&bytes)
            .expect("Failed to write config bytes");
    });
}

pub fn load_config_from_stable() {
    let mem = memory::get_memory(memory::MEM_ID_CONFIG);

    let mut len_bytes = [0u8; 8];
    ic_stable_structures::Memory::read(&mem, 0, &mut len_bytes);
    let len = u64::from_le_bytes(len_bytes) as usize;

    if len == 0 || len > 10_000_000 {
        init_state(BotState::default());
        return;
    }

    let mut bytes = vec![0u8; len];
    ic_stable_structures::Memory::read(&mem, 8, &mut bytes);
    match serde_json::from_slice::<BotState>(&bytes) {
        Ok(state) => init_state(state),
        Err(e) => {
            ic_canister_log::log!(crate::INFO, "Failed to deserialize config from stable: {}", e);
            init_state(BotState::default());
        }
    }
}
