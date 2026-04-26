use std::cell::RefCell;
use std::collections::BTreeMap;
use candid::{CandidType, Principal, Decode, Encode};
use ic_canister_log::log;
use serde::{Serialize, Deserialize};

use crate::types::*;
use crate::logs::INFO;

// ─── Event log caps ───
// Prevents unbounded heap growth that could brick the canister by causing
// pre_upgrade to trap when serializing too much data. Oldest events are
// dropped when the cap is reached (ring buffer behavior).

pub const MAX_SWAP_EVENTS: usize = 50_000;
pub const MAX_LIQUIDITY_EVENTS: usize = 50_000;
pub const MAX_ADMIN_EVENTS: usize = 10_000;
pub const MAX_HOLDER_SNAPSHOTS: usize = 1_000; // ~500 days at 2/day
pub const MAX_PENDING_CLAIMS: usize = 1_000;

// ─── State ───

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct AmmState {
    pub admin: Principal,
    pub pools: BTreeMap<PoolId, Pool>,
    #[serde(default)]
    pub pool_creation_open: bool,
    #[serde(default)]
    pub maintenance_mode: bool,
    #[serde(default)]
    pub pending_claims: Vec<PendingClaim>,
    #[serde(default)]
    pub next_claim_id: u64,
    #[serde(default)]
    pub swap_events: Vec<AmmSwapEvent>,
    #[serde(default)]
    pub next_swap_event_id: u64,
    #[serde(default)]
    pub liquidity_events: Vec<AmmLiquidityEvent>,
    #[serde(default)]
    pub next_liquidity_event_id: u64,
    #[serde(default)]
    pub admin_events: Vec<AmmAdminEvent>,
    #[serde(default)]
    pub next_admin_event_id: u64,
    #[serde(default)]
    pub holder_snapshots: Vec<HolderSnapshot>,
}

impl Default for AmmState {
    fn default() -> Self {
        Self {
            admin: Principal::anonymous(),
            pools: BTreeMap::new(),
            pool_creation_open: false,
            maintenance_mode: false,
            pending_claims: Vec::new(),
            next_claim_id: 0,
            swap_events: Vec::new(),
            next_swap_event_id: 0,
            liquidity_events: Vec::new(),
            next_liquidity_event_id: 0,
            admin_events: Vec::new(),
            next_admin_event_id: 0,
            holder_snapshots: Vec::new(),
        }
    }
}

impl AmmState {
    pub fn initialize(&mut self, args: AmmInitArgs) {
        self.admin = args.admin;
    }

    pub fn record_swap_event(&mut self, caller: Principal, pool_id: PoolId, token_in: Principal, amount_in: u128, token_out: Principal, amount_out: u128, fee: u128) {
        if self.swap_events.len() >= MAX_SWAP_EVENTS {
            self.swap_events.remove(0);
        }
        let event = AmmSwapEvent {
            id: self.next_swap_event_id,
            caller,
            pool_id,
            token_in,
            amount_in,
            token_out,
            amount_out,
            fee,
            timestamp: ic_cdk::api::time(),
        };
        self.swap_events.push(event);
        self.next_swap_event_id += 1;
    }

    pub fn record_liquidity_event(
        &mut self,
        caller: Principal,
        pool_id: PoolId,
        action: AmmLiquidityAction,
        token_a: Principal,
        amount_a: u128,
        token_b: Principal,
        amount_b: u128,
        lp_shares: u128,
    ) {
        if self.liquidity_events.len() >= MAX_LIQUIDITY_EVENTS {
            self.liquidity_events.remove(0);
        }
        let event = AmmLiquidityEvent {
            id: self.next_liquidity_event_id,
            caller,
            pool_id,
            action,
            token_a,
            amount_a,
            token_b,
            amount_b,
            lp_shares,
            timestamp: ic_cdk::api::time(),
        };
        self.liquidity_events.push(event);
        self.next_liquidity_event_id += 1;
    }

    pub fn record_admin_event(&mut self, caller: Principal, action: AmmAdminAction) {
        if self.admin_events.len() >= MAX_ADMIN_EVENTS {
            self.admin_events.remove(0);
        }
        let event = AmmAdminEvent {
            id: self.next_admin_event_id,
            caller,
            action,
            timestamp: ic_cdk::api::time(),
        };
        self.admin_events.push(event);
        self.next_admin_event_id += 1;
    }
}

// ─── Thread-local state ───

thread_local! {
    static STATE: RefCell<AmmState> = RefCell::new(AmmState::default());
}

pub fn mutate_state<F, R>(f: F) -> R
where
    F: FnOnce(&mut AmmState) -> R,
{
    STATE.with(|s| f(&mut s.borrow_mut()))
}

pub fn read_state<F, R>(f: F) -> R
where
    F: FnOnce(&AmmState) -> R,
{
    STATE.with(|s| f(&s.borrow()))
}

pub fn replace_state(new_state: AmmState) {
    STATE.with(|s| {
        *s.borrow_mut() = new_state;
    });
}

// ─── Stable memory persistence ───

// SAFETY (UPG-004): this writes the encoded state at raw stable-memory offset 0
// using `stable64_write`, with a leading 8-byte length prefix. It does NOT use
// `ic_stable_structures::MemoryManager`. A future migration that introduces
// MemoryManager MUST first read the legacy blob into RAM via the same raw
// `stable64_read(0, ...)` path before calling `MemoryManager::init`, because
// `MemoryManager::init` unconditionally writes its 'MGR' magic header at
// physical offset 0 and would destructively overwrite the legacy state. See
// `liquidation_bot::post_upgrade` for the canonical "rescue legacy blob first,
// then init MemoryManager" pattern.
pub fn save_to_stable_memory() {
    STATE.with(|s| {
        let state = s.borrow();
        let bytes = Encode!(&*state).expect("Failed to encode AMM state");
        let len = bytes.len() as u64;

        let needed_pages = (len + 8 + 65535) / 65536;
        let current_pages = ic_cdk::api::stable::stable64_size();
        if needed_pages > current_pages {
            ic_cdk::api::stable::stable64_grow(needed_pages - current_pages)
                .expect("Failed to grow stable memory");
        }

        ic_cdk::api::stable::stable64_write(0, &len.to_le_bytes());
        ic_cdk::api::stable::stable64_write(8, &bytes);
    });
}

/// V4 state shape (current on-chain: has pending_claims but no swap_events).
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
struct AmmStateV4 {
    pub admin: Principal,
    pub pools: BTreeMap<PoolId, Pool>,
    pub pool_creation_open: bool,
    pub maintenance_mode: bool,
    pub pending_claims: Vec<PendingClaim>,
    pub next_claim_id: u64,
}

/// V3 state shape (has pool_creation_open + maintenance_mode, but no pending_claims).
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
struct AmmStateV3 {
    pub admin: Principal,
    pub pools: BTreeMap<PoolId, Pool>,
    pub pool_creation_open: bool,
    pub maintenance_mode: bool,
}

/// V2 state shape (has pool_creation_open but not maintenance_mode).
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
struct AmmStateV2 {
    pub admin: Principal,
    pub pools: BTreeMap<PoolId, Pool>,
    pub pool_creation_open: bool,
}

/// V1 state shape (before pool_creation_open was added).
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
struct AmmStateV1 {
    pub admin: Principal,
    pub pools: BTreeMap<PoolId, Pool>,
}

/// Try to deserialize an AMM state snapshot, walking known schema versions
/// in order (current, V4, V3, V2, V1). Returns `None` if no version decodes.
pub fn try_decode_state(bytes: &[u8]) -> Option<AmmState> {
    if let Ok(state) = Decode!(bytes, AmmState) {
        return Some(state);
    }
    if let Ok(v4) = Decode!(bytes, AmmStateV4) {
        return Some(AmmState {
            admin: v4.admin,
            pools: v4.pools,
            pool_creation_open: v4.pool_creation_open,
            maintenance_mode: v4.maintenance_mode,
            pending_claims: v4.pending_claims,
            next_claim_id: v4.next_claim_id,
            swap_events: Vec::new(),
            next_swap_event_id: 0,
            liquidity_events: Vec::new(),
            next_liquidity_event_id: 0,
            admin_events: Vec::new(),
            next_admin_event_id: 0,
            holder_snapshots: Vec::new(),
        });
    }
    if let Ok(v3) = Decode!(bytes, AmmStateV3) {
        return Some(AmmState {
            admin: v3.admin,
            pools: v3.pools,
            pool_creation_open: v3.pool_creation_open,
            maintenance_mode: v3.maintenance_mode,
            pending_claims: Vec::new(),
            next_claim_id: 0,
            swap_events: Vec::new(),
            next_swap_event_id: 0,
            liquidity_events: Vec::new(),
            next_liquidity_event_id: 0,
            admin_events: Vec::new(),
            next_admin_event_id: 0,
            holder_snapshots: Vec::new(),
        });
    }
    if let Ok(v2) = Decode!(bytes, AmmStateV2) {
        return Some(AmmState {
            admin: v2.admin,
            pools: v2.pools,
            pool_creation_open: v2.pool_creation_open,
            maintenance_mode: false,
            pending_claims: Vec::new(),
            next_claim_id: 0,
            swap_events: Vec::new(),
            next_swap_event_id: 0,
            liquidity_events: Vec::new(),
            next_liquidity_event_id: 0,
            admin_events: Vec::new(),
            next_admin_event_id: 0,
            holder_snapshots: Vec::new(),
        });
    }
    if let Ok(v1) = Decode!(bytes, AmmStateV1) {
        return Some(AmmState {
            admin: v1.admin,
            pools: v1.pools,
            pool_creation_open: false,
            maintenance_mode: false,
            pending_claims: Vec::new(),
            next_claim_id: 0,
            swap_events: Vec::new(),
            next_swap_event_id: 0,
            liquidity_events: Vec::new(),
            next_liquidity_event_id: 0,
            admin_events: Vec::new(),
            next_admin_event_id: 0,
            holder_snapshots: Vec::new(),
        });
    }
    None
}

/// Restore state from stable memory (called from post_upgrade).
///
/// UPG-002 fix: rather than trapping on decode failure (which bricks the
/// canister until a hotfix wasm ships), walk the V-current..V1 fallback chain
/// via `try_decode_state`. If every known version fails, log a CRITICAL
/// diagnostic with the snapshot length and a short hex preview, then fall
/// back to empty state. AMM positions are reconstructable from underlying
/// ledger balances, so empty fallback is a defensible last resort here.
pub fn load_from_stable_memory() {
    let mut len_bytes = [0u8; 8];
    ic_cdk::api::stable::stable64_read(0, &mut len_bytes);
    let len = u64::from_le_bytes(len_bytes) as usize;

    if len == 0 {
        return;
    }

    let mut bytes = vec![0u8; len];
    ic_cdk::api::stable::stable64_read(8, &mut bytes);

    if let Some(state) = try_decode_state(&bytes) {
        replace_state(state);
        return;
    }

    let preview_len = bytes.len().min(64);
    let preview_hex: String = bytes[..preview_len]
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();
    log!(
        INFO,
        "CRITICAL UPG-002: AMM snapshot decode failed for all known schema versions \
         (current, V4, V3, V2, V1). snapshot_len={} bytes, first_{}_bytes_hex={}. \
         Falling back to empty state. AMM positions can be reconstructed from \
         underlying ledger balances; admin must re-set via admin endpoints.",
        bytes.len(),
        preview_len,
        preview_hex
    );
    replace_state(AmmState::default());
}
