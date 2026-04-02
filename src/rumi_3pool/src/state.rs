use std::collections::{BTreeMap, BTreeSet};
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
    /// ICRC-2 LP token allowances: (owner, spender) -> allowance.
    /// Option for upgrade compatibility — old state won't have this field.
    pub lp_allowances: Option<BTreeMap<(Principal, Principal), crate::types::LpAllowance>>,
    /// Transaction counter for ICRC-1/2 block index.
    /// Option for upgrade compatibility — old state won't have this field.
    pub lp_tx_count: Option<u64>,
    /// Virtual price snapshots for APY calculation (taken every 6 hours).
    /// Option for upgrade compatibility — old state won't have this field.
    pub vp_snapshots: Option<Vec<crate::types::VirtualPriceSnapshot>>,
    /// ICRC-3 transaction log for index canister support.
    /// Option for upgrade compatibility — old state won't have this field.
    pub blocks: Option<Vec<crate::types::Icrc3Block>>,
    /// Hash of the last ICRC-3 block (for hash-chain certification).
    /// Option for upgrade compatibility — recomputed from blocks on upgrade.
    pub last_block_hash: Option<[u8; 32]>,
    /// Accumulated admin fees per coin (claimable by admin).
    pub admin_fees: [u128; 3],
    /// Whether the pool is paused (no swaps/deposits/withdrawals).
    pub is_paused: bool,
    /// Whether the pool has been initialized via `init`.
    pub is_initialized: bool,
    /// Canisters authorized to call `authorized_redeem_and_burn`.
    /// Option for upgrade compatibility — old state won't have this field.
    #[serde(default)]
    pub authorized_burn_callers: Option<BTreeSet<Principal>>,
    /// Swap event log for explorer/analytics queries.
    /// Option for upgrade compatibility — old state won't have this field.
    #[serde(default)]
    pub swap_events: Option<Vec<SwapEvent>>,
    /// Liquidity event log for explorer.
    #[serde(default)]
    pub liquidity_events: Option<Vec<LiquidityEvent>>,
    /// Admin event log for explorer.
    #[serde(default)]
    pub admin_events: Option<Vec<ThreePoolAdminEvent>>,
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
            lp_allowances: Some(BTreeMap::new()),
            lp_tx_count: Some(0),
            blocks: Some(Vec::new()),
            last_block_hash: None,
            vp_snapshots: Some(Vec::new()),
            admin_fees: [0; 3],
            is_paused: false,
            is_initialized: false,
            authorized_burn_callers: Some(BTreeSet::new()),
            swap_events: Some(Vec::new()),
            liquidity_events: Some(Vec::new()),
            admin_events: Some(Vec::new()),
        }
    }
}

impl ThreePoolState {
    /// Get LP allowances map (initializes if None for upgrade compat).
    pub fn allowances(&self) -> &BTreeMap<(Principal, Principal), crate::types::LpAllowance> {
        // SAFETY: Default impl always sets to Some; only None from old state that was never mutated.
        // In that case the caller should use allowances_mut() first.
        static EMPTY: std::sync::LazyLock<BTreeMap<(Principal, Principal), crate::types::LpAllowance>> =
            std::sync::LazyLock::new(BTreeMap::new);
        self.lp_allowances.as_ref().unwrap_or(&EMPTY)
    }

    /// Get mutable LP allowances map (initializes if None for upgrade compat).
    pub fn allowances_mut(&mut self) -> &mut BTreeMap<(Principal, Principal), crate::types::LpAllowance> {
        self.lp_allowances.get_or_insert_with(BTreeMap::new)
    }

    /// Get current tx count.
    pub fn tx_count(&self) -> u64 {
        self.lp_tx_count.unwrap_or(0)
    }

    /// Increment and return new tx count.
    pub fn next_tx_count(&mut self) -> u64 {
        let count = self.lp_tx_count.get_or_insert(0);
        *count += 1;
        *count
    }

    /// Get blocks vec (empty if None for upgrade compat).
    pub fn blocks(&self) -> &Vec<crate::types::Icrc3Block> {
        static EMPTY: std::sync::LazyLock<Vec<crate::types::Icrc3Block>> =
            std::sync::LazyLock::new(Vec::new);
        self.blocks.as_ref().unwrap_or(&EMPTY)
    }

    /// Get mutable blocks vec (initializes if None for upgrade compat).
    pub fn blocks_mut(&mut self) -> &mut Vec<crate::types::Icrc3Block> {
        self.blocks.get_or_insert_with(Vec::new)
    }

    /// Log a transaction block, compute its hash, update certified data,
    /// and return its index.
    /// Block IDs are sequential starting from 0, matching Vec position,
    /// so that ICRC-3 `log_length` == `blocks.len()` and `start` indexing works.
    pub fn log_block(&mut self, tx: crate::types::Icrc3Transaction) -> u64 {
        let id = self.blocks().len() as u64;
        let block = crate::types::Icrc3Block {
            id,
            timestamp: ic_cdk::api::time(),
            tx,
        };
        // Compute hash: encode block with parent hash, then hash the value.
        // Copy last_block_hash before mutating to avoid borrow conflict.
        let prev_hash = self.last_block_hash;
        let encoded = crate::icrc3::encode_block_with_phash(&block, prev_hash.as_ref());
        let block_hash = crate::certification::hash_value(&encoded);
        self.blocks_mut().push(block);
        self.last_block_hash = Some(block_hash);
        // Update IC certified data so index-ng can verify the chain tip
        crate::certification::set_certified_tip(id, &block_hash);
        id
    }

    /// Get VP snapshots vec (empty if None for upgrade compat).
    pub fn snapshots(&self) -> &Vec<crate::types::VirtualPriceSnapshot> {
        static EMPTY: std::sync::LazyLock<Vec<crate::types::VirtualPriceSnapshot>> =
            std::sync::LazyLock::new(Vec::new);
        self.vp_snapshots.as_ref().unwrap_or(&EMPTY)
    }

    /// Get mutable VP snapshots vec (initializes if None for upgrade compat).
    pub fn snapshots_mut(&mut self) -> &mut Vec<crate::types::VirtualPriceSnapshot> {
        self.vp_snapshots.get_or_insert_with(Vec::new)
    }

    /// Get authorized burn callers (empty if None for upgrade compat).
    pub fn burn_callers(&self) -> &BTreeSet<Principal> {
        static EMPTY: std::sync::LazyLock<BTreeSet<Principal>> =
            std::sync::LazyLock::new(BTreeSet::new);
        self.authorized_burn_callers.as_ref().unwrap_or(&EMPTY)
    }

    /// Get mutable burn callers set (initializes if None for upgrade compat).
    pub fn burn_callers_mut(&mut self) -> &mut BTreeSet<Principal> {
        self.authorized_burn_callers.get_or_insert_with(BTreeSet::new)
    }

    /// Get swap events vec (empty if None for upgrade compat).
    pub fn swap_events(&self) -> &Vec<SwapEvent> {
        static EMPTY: std::sync::LazyLock<Vec<SwapEvent>> =
            std::sync::LazyLock::new(Vec::new);
        self.swap_events.as_ref().unwrap_or(&EMPTY)
    }

    /// Get mutable swap events vec (initializes if None for upgrade compat).
    pub fn swap_events_mut(&mut self) -> &mut Vec<SwapEvent> {
        self.swap_events.get_or_insert_with(Vec::new)
    }

    /// Get liquidity events vec (empty if None for upgrade compat).
    pub fn liquidity_events(&self) -> &Vec<LiquidityEvent> {
        static EMPTY: std::sync::LazyLock<Vec<LiquidityEvent>> =
            std::sync::LazyLock::new(Vec::new);
        self.liquidity_events.as_ref().unwrap_or(&EMPTY)
    }

    /// Get mutable liquidity events vec (initializes if None for upgrade compat).
    pub fn liquidity_events_mut(&mut self) -> &mut Vec<LiquidityEvent> {
        self.liquidity_events.get_or_insert_with(Vec::new)
    }

    /// Get admin events vec (empty if None for upgrade compat).
    pub fn admin_events(&self) -> &Vec<ThreePoolAdminEvent> {
        static EMPTY: std::sync::LazyLock<Vec<ThreePoolAdminEvent>> =
            std::sync::LazyLock::new(Vec::new);
        self.admin_events.as_ref().unwrap_or(&EMPTY)
    }

    /// Get mutable admin events vec (initializes if None for upgrade compat).
    pub fn admin_events_mut(&mut self) -> &mut Vec<ThreePoolAdminEvent> {
        self.admin_events.get_or_insert_with(Vec::new)
    }

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
