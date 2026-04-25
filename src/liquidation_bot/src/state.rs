use candid::{CandidType, Deserialize, Principal};
use ic_stable_structures::writer::Writer;
use serde::Serialize;
use std::cell::RefCell;

use crate::memory;

/// Sentinel used by `BotConfig.icpswap_pool` when an old serialized state has
/// no `icpswap_pool` field. `swap.rs` checks for this and refuses to swap
/// until admin sets a real principal via `set_config`.
fn default_icpswap_pool() -> Principal {
    Principal::anonymous()
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct BotConfig {
    pub backend_principal: Principal,
    pub treasury_principal: Principal,
    pub admin: Principal,
    pub max_slippage_bps: u16,
    pub icp_ledger: Principal,
    pub ckusdc_ledger: Principal,

    // ICPSwap config (replaces kong_swap + three_pool).
    //
    // Defaults to the anonymous principal so that pre-Apr-9 BotConfig blobs
    // (which have no `icpswap_pool` field at all) still deserialize cleanly
    // through the post_upgrade legacy-rescue path. Without this default the
    // rescue's `serde_json::from_slice::<BotState>` returns Err, the post
    // upgrade falls through to `load_config_from_stable`, and that traps on
    // an empty MEM_ID_CONFIG region (rolling the upgrade back).
    //
    // Runtime guard: `swap.rs` rejects swaps when this is anonymous so the
    // bot fails loudly instead of routing trades to nowhere. Admin must set
    // a real pool via `set_config` before liquidations resume.
    #[serde(default = "default_icpswap_pool")]
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression fence: a `BotState` JSON blob written by the pre-Apr-9 wasm
    /// (no `icpswap_pool` field, with the legacy `kong_swap_principal` /
    /// `three_pool_principal` / `ckusdt_ledger` / `icusd_ledger` fields, no
    /// `migrated_to_stable_structures`) MUST deserialize cleanly into the
    /// current `BotState`. Without `#[serde(default = "default_icpswap_pool")]`
    /// on `BotConfig.icpswap_pool`, the deserialization fails, the post
    /// upgrade legacy-rescue returns `None`, and the entire upgrade traps.
    #[test]
    fn legacy_pre_icpswap_state_deserializes() {
        let legacy_blob = serde_json::json!({
            "config": {
                "backend_principal": "tfesu-vyaaa-aaaap-qrd7a-cai",
                "three_pool_principal": "fohh4-yyaaa-aaaap-qtkpa-cai",
                "kong_swap_principal": "2ipq2-uqaaa-aaaar-qailq-cai",
                "treasury_principal": "tlg74-oiaaa-aaaap-qrd6a-cai",
                "admin": "fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae",
                "max_slippage_bps": 200,
                "icp_ledger": "ryjl3-tyaaa-aaaaa-aaaba-cai",
                "ckusdc_ledger": "xevnm-gaaaa-aaaar-qafnq-cai",
                "ckusdt_ledger": "cngnf-vqaaa-aaaar-qag4q-cai",
                "icusd_ledger": "t6bor-paaaa-aaaap-qrd5q-cai"
            },
            "stats": {
                "total_debt_covered_e8s": 0,
                "total_icusd_burned_e8s": 0,
                "total_collateral_received_e8s": 0,
                "total_collateral_to_treasury_e8s": 0,
                "events_count": 99
            },
            "liquidation_events": [],
            "pending_vaults": [],
            "admin_events": []
        });

        let bytes = serde_json::to_vec(&legacy_blob).expect("encode legacy blob");
        let state: BotState =
            serde_json::from_slice(&bytes).expect("legacy state must deserialize cleanly");

        let config = state.config.expect("config preserved");
        assert_eq!(
            config.icpswap_pool,
            Principal::anonymous(),
            "icpswap_pool must default to the anonymous-principal sentinel"
        );
        assert_eq!(
            config.three_pool_principal,
            Some(Principal::from_text("fohh4-yyaaa-aaaap-qtkpa-cai").unwrap()),
            "legacy three_pool_principal must round-trip via Option"
        );
        assert_eq!(state.stats.events_count, 99, "stats preserved");
        assert!(
            !state.migrated_to_stable_structures,
            "legacy blob has no migration marker, must default to false so post_upgrade runs the StableBTreeMap migration"
        );
    }
}

pub fn load_config_from_stable() {
    let mem = memory::get_memory(memory::MEM_ID_CONFIG);

    // MemoryManager allocates virtual addresses lazily — on the first upgrade
    // after migration, MEM_ID_CONFIG has zero physical pages until a prior
    // pre_upgrade has called `save_config_to_stable`. Reading at offset 0
    // would panic with "out of bounds" and trap the entire upgrade. Bail out
    // to a default state in that case (post_upgrade's other branches handle
    // legacy rescue, so this is reached only when there genuinely is no data).
    if ic_stable_structures::Memory::size(&mem) == 0 {
        init_state(BotState::default());
        return;
    }

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
