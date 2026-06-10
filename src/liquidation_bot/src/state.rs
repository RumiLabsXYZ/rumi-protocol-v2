use candid::{CandidType, Deserialize, Principal};
use ic_stable_structures::writer::Writer;
use serde::Serialize;
use std::cell::RefCell;
use std::collections::BTreeMap;

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
    /// Per-vault claim-attempt counter. Incremented on each `ClaimFailed`
    /// returned by `bot_claim_liquidation`, cleared on success or after the
    /// retry ceiling is hit. Lives in heap state only (not persisted across
    /// upgrades), because the cascade re-notifies surviving liquidatable
    /// vaults on its next tick anyway.
    #[serde(default)]
    pub claim_retry_counts: BTreeMap<u64, u8>,
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

/// Trap wrapper: real `ic_cdk::trap` on-canister, plain panic off-canister so
/// unit tests can exercise trap paths via `std::panic::catch_unwind` (the ic0
/// host stub panics with a generic message that would swallow ours).
fn trap(msg: &str) -> ! {
    #[cfg(target_arch = "wasm32")]
    ic_cdk::trap(msg);
    #[cfg(not(target_arch = "wasm32"))]
    panic!("{}", msg);
}

/// Which save path `pre_upgrade` must take.
#[derive(Debug, PartialEq, Eq)]
pub enum PreUpgradeSavePath {
    Config,
    LegacyRaw,
}

/// UPG-001 fence: the legacy raw-offset-0 save is only legal before the first
/// MemoryManager migration. Once the MGR layout exists at offset 0, a raw
/// write there clobbers the header and corrupts every virtual region (config,
/// history, next-id), so the config path wins even if the in-heap migration
/// flag was somehow lost.
pub fn pre_upgrade_save_path(
    migrated: bool,
    memory_manager_layout_exists: bool,
) -> PreUpgradeSavePath {
    if migrated || memory_manager_layout_exists {
        PreUpgradeSavePath::Config
    } else {
        PreUpgradeSavePath::LegacyRaw
    }
}

// ---- Legacy stable memory (raw offset 0, used only for first migration) ----

pub fn save_to_stable_memory() {
    // UPG-001 fence (defense in depth behind pre_upgrade_save_path): never
    // raw-write offset 0 once the MemoryManager layout exists. Trapping here
    // aborts the upgrade with the old wasm and stable memory intact.
    if memory::memory_manager_layout_exists() {
        trap(
            "liquidation_bot UPG-001: legacy raw-offset-0 save blocked, MemoryManager \
             layout already exists; config must be saved via save_config_to_stable",
        );
    }
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

    fn test_config() -> BotConfig {
        BotConfig {
            backend_principal: Principal::from_text("tfesu-vyaaa-aaaap-qrd7a-cai").unwrap(),
            treasury_principal: Principal::from_text("tlg74-oiaaa-aaaap-qrd6a-cai").unwrap(),
            admin: Principal::anonymous(),
            max_slippage_bps: 200,
            icp_ledger: Principal::from_text("ryjl3-tyaaa-aaaaa-aaaba-cai").unwrap(),
            ckusdc_ledger: Principal::from_text("xevnm-gaaaa-aaaar-qafnq-cai").unwrap(),
            icpswap_pool: Principal::anonymous(),
            icpswap_zero_for_one: None,
            icp_fee_e8s: None,
            ckusdc_fee_e6: None,
            three_pool_principal: None,
            kong_swap_principal: None,
            ckusdt_ledger: None,
            icusd_ledger: None,
        }
    }

    fn write_config_region(payload: &[u8]) {
        let mut mem = memory::get_memory(memory::MEM_ID_CONFIG);
        let mut writer = Writer::new(&mut mem, 0);
        writer
            .write(&(payload.len() as u64).to_le_bytes())
            .expect("write len");
        writer.write(payload).expect("write payload");
    }

    fn catch_panic_message<F: FnOnce() + std::panic::UnwindSafe>(f: F) -> Option<String> {
        std::panic::catch_unwind(f).err().map(|payload| {
            payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default()
        })
    }

    /// UPG-001(a): a genuine decode failure of the MEM_ID_CONFIG snapshot must
    /// trap (upgrade rolls back, old wasm + stable memory intact), NOT silently
    /// wipe heap state to `BotState::default()`.
    #[test]
    fn upg_001_decode_failure_traps_instead_of_wiping() {
        memory::init_memory_manager();
        write_config_region(b"definitely not json");

        let msg = catch_panic_message(load_config_from_stable)
            .expect("decode failure must trap, not silently wipe state to default");
        assert!(
            msg.contains("UPG-001") && msg.contains("failed to decode"),
            "trap message must name the finding and the cause, got: {}",
            msg
        );
        STATE.with(|s| {
            assert!(
                s.borrow().is_none(),
                "state must stay uninitialized after the trap (no default wipe)"
            )
        });
    }

    /// UPG-001(a): an implausible snapshot length is corruption, not "no
    /// data", and must also trap instead of wiping.
    #[test]
    fn upg_001_corrupt_length_traps_instead_of_wiping() {
        memory::init_memory_manager();
        let mut mem = memory::get_memory(memory::MEM_ID_CONFIG);
        let mut writer = Writer::new(&mut mem, 0);
        writer.write(&u64::MAX.to_le_bytes()).expect("write len");

        let msg = catch_panic_message(load_config_from_stable)
            .expect("corrupt length must trap, not silently wipe state to default");
        assert!(
            msg.contains("UPG-001") && msg.contains("implausible"),
            "trap message must name the finding, got: {}",
            msg
        );
        STATE.with(|s| assert!(s.borrow().is_none(), "state must stay uninitialized"));
    }

    /// Fresh install / genuinely empty MEM_ID_CONFIG must still default (no
    /// trap), and the default must keep the legacy raw-offset-0 save disarmed
    /// (UPG-001(b)): the MemoryManager layout exists by the time this runs.
    #[test]
    fn upg_001_no_data_defaults_with_migration_flag_set() {
        memory::init_memory_manager();
        load_config_from_stable();
        read_state(|s| {
            assert!(s.config.is_none(), "no data means default config");
            assert!(
                s.migrated_to_stable_structures,
                "no-data default must mark migrated so pre_upgrade never re-arms the legacy save"
            );
        });
    }

    /// Sanity: the happy path (valid snapshot written by save_config_to_stable)
    /// must keep round-tripping, so the new traps are not over-eager.
    #[test]
    fn upg_001_valid_snapshot_roundtrips() {
        memory::init_memory_manager();
        init_state(BotState {
            config: Some(test_config()),
            migrated_to_stable_structures: true,
            ..Default::default()
        });
        save_config_to_stable();
        STATE.with(|s| *s.borrow_mut() = None);

        load_config_from_stable();
        read_state(|s| {
            assert_eq!(
                s.config.as_ref().expect("config preserved").backend_principal,
                test_config().backend_principal
            );
            assert!(s.migrated_to_stable_structures);
        });
    }

    /// UPG-001(b) fence: once the MemoryManager layout exists, the legacy
    /// raw-offset-0 save must trap instead of clobbering the MGR header, even
    /// if heap state was somehow reset to default (migrated == false).
    #[test]
    fn upg_001_legacy_save_fence_blocks_raw_write_when_layout_exists() {
        memory::init_memory_manager();
        init_state(BotState::default()); // migrated_to_stable_structures == false

        let msg = catch_panic_message(save_to_stable_memory)
            .expect("legacy save must trap once the MemoryManager layout exists");
        assert!(
            msg.contains("UPG-001") && msg.contains("raw-offset-0 save blocked"),
            "fence trap must name the finding, got: {}",
            msg
        );
    }

    /// UPG-001(b): pre_upgrade path selection can only reach the legacy save
    /// when BOTH the migration flag is unset AND no MemoryManager layout
    /// exists at offset 0 (a pre-migration canister).
    #[test]
    fn upg_001_pre_upgrade_never_takes_legacy_path_once_layout_exists() {
        assert_eq!(pre_upgrade_save_path(true, true), PreUpgradeSavePath::Config);
        assert_eq!(pre_upgrade_save_path(true, false), PreUpgradeSavePath::Config);
        assert_eq!(pre_upgrade_save_path(false, true), PreUpgradeSavePath::Config);
        assert_eq!(
            pre_upgrade_save_path(false, false),
            PreUpgradeSavePath::LegacyRaw
        );
    }

    /// UPG-001(b): validate the magic constant against the real crate. A fresh
    /// MemoryManager must write a header our fence recognizes.
    #[test]
    fn upg_001_memory_manager_magic_matches_crate() {
        use ic_stable_structures::memory_manager::MemoryManager;
        use ic_stable_structures::{DefaultMemoryImpl, Memory};

        let raw = DefaultMemoryImpl::default();
        let _mm = MemoryManager::init(raw.clone());
        assert!(raw.size() > 0, "MemoryManager::init must write its header");
        let mut first = [0u8; 3];
        raw.read(0, &mut first);
        assert!(
            memory::is_memory_manager_header(&first),
            "fence must recognize the header MemoryManager actually writes"
        );
        assert!(!memory::is_memory_manager_header(&[0u8; 3]));
        assert!(!memory::is_memory_manager_header(b"MG"));
        assert!(!memory::is_memory_manager_header(b""));
    }
}

/// Default state for the genuine no-data branches of `load_config_from_stable`.
/// `migrated_to_stable_structures` starts true (not the derived false) because
/// this only runs after `init_memory_manager`, so the MemoryManager layout
/// exists and a later legacy raw-offset-0 save would corrupt it (UPG-001).
fn fresh_migrated_state() -> BotState {
    BotState {
        migrated_to_stable_structures: true,
        ..Default::default()
    }
}

pub fn load_config_from_stable() {
    let mem = memory::get_memory(memory::MEM_ID_CONFIG);

    // MemoryManager allocates virtual addresses lazily: on the first upgrade
    // after migration, MEM_ID_CONFIG has zero physical pages until a prior
    // pre_upgrade has called `save_config_to_stable`. Reading at offset 0
    // would panic with "out of bounds" and trap the entire upgrade. Bail out
    // to a default state in that case (post_upgrade's other branches handle
    // legacy rescue, so this is reached only when there genuinely is no data).
    if ic_stable_structures::Memory::size(&mem) == 0 {
        init_state(fresh_migrated_state());
        return;
    }

    let mut len_bytes = [0u8; 8];
    ic_stable_structures::Memory::read(&mem, 0, &mut len_bytes);
    let len = u64::from_le_bytes(len_bytes) as usize;

    // A zero length is treated as "no data" (region grown but never written).
    if len == 0 {
        init_state(fresh_migrated_state());
        return;
    }

    // save_config_to_stable always writes the real snapshot length first, so
    // an implausible length means the region is corrupt, not empty. UPG-001:
    // trap (upgrade rolls back, old wasm + state stay intact) instead of
    // silently wiping to default.
    if len > 10_000_000 {
        trap(&format!(
            "liquidation_bot UPG-001: implausible config snapshot length {} in MEM_ID_CONFIG; \
             refusing to wipe state, aborting upgrade",
            len
        ));
    }

    let mut bytes = vec![0u8; len];
    ic_stable_structures::Memory::read(&mem, 8, &mut bytes);
    match serde_json::from_slice::<BotState>(&bytes) {
        Ok(state) => init_state(state),
        Err(e) => {
            ic_canister_log::log!(
                crate::INFO,
                "CRITICAL UPG-001: config snapshot decode failed (len={} bytes): {}. \
                 Trapping to preserve on-chain state rather than wiping to default.",
                bytes.len(),
                e
            );
            trap(&format!(
                "liquidation_bot UPG-001: config snapshot in MEM_ID_CONFIG failed to decode ({}); \
                 trapping to preserve state (old wasm + stable memory stay intact) instead of \
                 wiping to default",
                e
            ));
        }
    }
}
