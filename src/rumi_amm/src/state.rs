use std::cell::RefCell;
use std::collections::BTreeMap;
use candid::{CandidType, Principal, Decode, Encode};
use serde::{Serialize, Deserialize};

use crate::types::*;

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
        }
    }
}

impl AmmState {
    pub fn initialize(&mut self, args: AmmInitArgs) {
        self.admin = args.admin;
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

/// V3 state shape (has pool_creation_open + maintenance_mode, but no pending_claims).
/// This is what's currently on-chain.
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

pub fn load_from_stable_memory() {
    let mut len_bytes = [0u8; 8];
    ic_cdk::api::stable::stable64_read(0, &mut len_bytes);
    let len = u64::from_le_bytes(len_bytes) as usize;

    if len == 0 {
        return;
    }

    let mut bytes = vec![0u8; len];
    ic_cdk::api::stable::stable64_read(8, &mut bytes);

    // Try current shape first, then V3 (on-chain), then V2, then V1 (original)
    if let Ok(state) = Decode!(&bytes, AmmState) {
        replace_state(state);
    } else if let Ok(v3) = Decode!(&bytes, AmmStateV3) {
        replace_state(AmmState {
            admin: v3.admin,
            pools: v3.pools,
            pool_creation_open: v3.pool_creation_open,
            maintenance_mode: v3.maintenance_mode,
            pending_claims: Vec::new(),
            next_claim_id: 0,
        });
    } else if let Ok(v2) = Decode!(&bytes, AmmStateV2) {
        replace_state(AmmState {
            admin: v2.admin,
            pools: v2.pools,
            pool_creation_open: v2.pool_creation_open,
            maintenance_mode: false,
            pending_claims: Vec::new(),
            next_claim_id: 0,
        });
    } else {
        let v1: AmmStateV1 = Decode!(&bytes, AmmStateV1)
            .expect("Failed to decode AMM state from stable memory (tried V4, V3, V2, V1)");
        replace_state(AmmState {
            admin: v1.admin,
            pools: v1.pools,
            pool_creation_open: false,
            maintenance_mode: false,
            pending_claims: Vec::new(),
            next_claim_id: 0,
        });
    }
}
