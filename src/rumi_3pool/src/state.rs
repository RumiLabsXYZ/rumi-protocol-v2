use std::collections::BTreeMap;
use std::cell::RefCell;
use candid::{CandidType, Principal, Decode, Encode};
use serde::{Serialize, Deserialize};

use crate::types::*;

// ─── State ───

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct ThreePoolState {
    /// Pool configuration (tokens, A parameter, fees, admin).
    pub config: PoolConfig,
    /// Current balances of each coin in the pool (in native token units).
    pub balances: [u128; 3],
    /// LP token balances per user.
    pub lp_balances: BTreeMap<Principal, u128>,
    /// Total LP tokens in circulation.
    pub lp_total_supply: u128,
    /// Accumulated admin fees per coin (claimable by admin).
    pub admin_fees: [u128; 3],
    /// Whether the pool is paused (no swaps/deposits/withdrawals).
    pub is_paused: bool,
    /// Whether the pool has been initialized via `init`.
    pub is_initialized: bool,
}

impl Default for ThreePoolState {
    fn default() -> Self {
        let default_token = TokenConfig {
            ledger_id: Principal::anonymous(),
            symbol: String::new(),
            decimals: 0,
            precision_mul: 1,
        };
        Self {
            config: PoolConfig {
                tokens: [default_token.clone(), default_token.clone(), default_token],
                initial_a: 100,
                future_a: 100,
                initial_a_time: 0,
                future_a_time: 0,
                swap_fee_bps: 4,       // 0.04%
                admin_fee_bps: 5000,   // 50% of swap fee
                admin: Principal::anonymous(),
            },
            balances: [0; 3],
            lp_balances: BTreeMap::new(),
            lp_total_supply: 0,
            admin_fees: [0; 3],
            is_paused: false,
            is_initialized: false,
        }
    }
}

impl ThreePoolState {
    /// Initialize pool state from deploy args.
    pub fn initialize(&mut self, args: ThreePoolInitArgs) {
        self.config = PoolConfig {
            tokens: args.tokens,
            initial_a: args.initial_a,
            future_a: args.initial_a,
            initial_a_time: 0,
            future_a_time: 0,
            swap_fee_bps: args.swap_fee_bps,
            admin_fee_bps: args.admin_fee_bps,
            admin: args.admin,
        };
        self.is_initialized = true;
    }
}

// ─── Thread-local state ───

thread_local! {
    static STATE: RefCell<ThreePoolState> = RefCell::new(ThreePoolState::default());
}

pub fn mutate_state<F, R>(f: F) -> R
where
    F: FnOnce(&mut ThreePoolState) -> R,
{
    STATE.with(|s| f(&mut s.borrow_mut()))
}

pub fn read_state<F, R>(f: F) -> R
where
    F: FnOnce(&ThreePoolState) -> R,
{
    STATE.with(|s| f(&s.borrow()))
}

pub fn replace_state(state: ThreePoolState) {
    STATE.with(|s| {
        *s.borrow_mut() = state;
    });
}

// ─── Stable memory persistence ───

/// Serialize state to stable memory (called from pre_upgrade).
pub fn save_to_stable_memory() {
    STATE.with(|s| {
        let state = s.borrow();
        let bytes = Encode!(&*state).expect("Failed to encode 3pool state");
        let len = bytes.len() as u64;

        // Only grow if current stable memory is insufficient.
        // Pages are 64 KiB each and never shrink, so avoid redundant grows.
        let needed_pages = (len + 8 + 65535) / 65536;
        let current_pages = ic_cdk::api::stable::stable64_size();
        if needed_pages > current_pages {
            ic_cdk::api::stable::stable64_grow(needed_pages - current_pages)
                .expect("Failed to grow stable memory");
        }

        // Write length prefix (8 bytes) then data
        ic_cdk::api::stable::stable64_write(0, &len.to_le_bytes());
        ic_cdk::api::stable::stable64_write(8, &bytes);
    });
}

/// Restore state from stable memory (called from post_upgrade).
pub fn load_from_stable_memory() {
    let mut len_bytes = [0u8; 8];
    ic_cdk::api::stable::stable64_read(0, &mut len_bytes);
    let len = u64::from_le_bytes(len_bytes) as usize;

    if len == 0 {
        return; // No saved state — fresh start
    }

    let mut bytes = vec![0u8; len];
    ic_cdk::api::stable::stable64_read(8, &mut bytes);

    let state: ThreePoolState = Decode!(&bytes, ThreePoolState)
        .expect("Failed to decode 3pool state from stable memory");
    replace_state(state);
}
