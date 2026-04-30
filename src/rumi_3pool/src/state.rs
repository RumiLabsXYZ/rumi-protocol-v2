use std::collections::{BTreeMap, BTreeSet};
use std::cell::RefCell;
use candid::{CandidType, Principal};
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
    // NOTE: swap_events_v2 and liquidity_events_v2 used to live here as
    // `Option<Vec<...>>` heap collections. Phase A moved them into
    // `storage::swap_v2` / `storage::liq_v2` (StableLog, MemoryIds 8-11).
    //
    // The legacy state layout is still decoded through a separate
    // `LegacyThreePoolState` shape in `storage::migration` during the
    // one-shot drain in `post_upgrade`. Live code reads/writes v2 events
    // exclusively through the `storage::*` API.
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
                fee_curve: Some(FeeCurveParams::default()),
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
    ///
    /// Block IDs are sequential starting from 0, matching `StableLog` index.
    /// The hash of each block is also pushed to `storage::block_hashes` so
    /// `icrc3_get_blocks` can fetch a parent hash in O(1) rather than
    /// recomputing the chain from block 0.
    ///
    /// Both writes happen inside this single message; IC message-level
    /// atomicity guarantees they cannot diverge for blocks logged after
    /// this code ships. Pre-existing blocks (logged before the cache was
    /// introduced) are populated by the post_upgrade backfill in
    /// `storage::migration::backfill_hash_chain`.
    pub fn log_block(&mut self, tx: crate::types::Icrc3Transaction) -> u64 {
        let id = crate::storage::blocks::len();
        let block = crate::types::Icrc3Block {
            id,
            timestamp: ic_cdk::api::time(),
            tx,
        };
        let prev_hash = self.last_block_hash;
        let encoded = crate::icrc3::encode_block_with_phash(&block, prev_hash.as_ref());
        let block_hash = crate::certification::hash_value(&encoded);
        crate::storage::blocks::push(block);
        crate::storage::block_hashes::push(crate::storage::StorableHash(block_hash));
        self.last_block_hash = Some(block_hash);
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

    /// Snapshot every v2 swap event currently stored in `storage::swap_v2`.
    ///
    /// Returns a freshly allocated `Vec` — reads go through the stable log,
    /// not a heap mirror, so this allocates on every call. Explorer
    /// endpoints that iterate many times per query should collect once and
    /// reuse the `Vec`. At current event volumes (<1k) this is still much
    /// cheaper than the Candid upgrade serialization it replaces.
    pub fn swap_events_v2(&self) -> Vec<SwapEventV2> {
        crate::storage::swap_v2::iter_all()
    }

    /// Snapshot every v2 liquidity event. See `swap_events_v2` for notes.
    pub fn liquidity_events_v2(&self) -> Vec<LiquidityEventV2> {
        crate::storage::liq_v2::iter_all()
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

    // NOTE: `migrate_events_to_v2` used to live here and did its work against
    // the heap `swap_events_v2` / `liquidity_events_v2` vecs. The v2 vecs
    // moved to stable logs in Phase A, so the backfill logic moved with them
    // into `storage::migration::drain_legacy_state`. That function is the
    // one-shot drain invoked from `post_upgrade` on the first upgrade after
    // Phase A ships; it takes the decoded legacy state as input and populates
    // the stable logs. After that flag flips, the drain is never run again.

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
            fee_curve: Some(FeeCurveParams::default()),
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

// ─── SlimState bridge ───

/// Populate the heap `ThreePoolState` from a `storage::SlimState`.
///
/// Used by `post_upgrade` on both the drain path and the normal path.
/// Collection fields on the heap state are left at their `Default`
/// (empty `Option::Some(...)`); live code reads collections through
/// `crate::storage::*`, not these stubs (A7 removes them).
pub fn hydrate_from_slim(slim: &crate::storage::SlimState) {
    let s = ThreePoolState {
        config: slim.config.clone(),
        balances: slim.balances,
        admin_fees: slim.admin_fees,
        lp_total_supply: slim.lp_total_supply,
        lp_tx_count: Some(slim.lp_tx_count),
        last_block_hash: slim.last_block_hash,
        is_paused: slim.is_paused,
        is_initialized: slim.is_initialized,
        ..ThreePoolState::default()
    };
    replace_state(s);
}

/// Build a `storage::SlimState` snapshot of the heap's bounded fields.
/// Called from `pre_upgrade` to flush into the stable cell, and from the
/// drain path in `post_upgrade`.
///
/// The `storage_migrated` flag is preserved from the current cell value
/// (true once the one-shot drain has run). Callers on the drain path
/// must explicitly overwrite it to `true` after calling this.
pub fn snapshot_slim() -> crate::storage::SlimState {
    let prior_migrated = crate::storage::get_slim().storage_migrated;
    read_state(|s| crate::storage::SlimState {
        config: s.config.clone(),
        balances: s.balances,
        admin_fees: s.admin_fees,
        lp_total_supply: s.lp_total_supply,
        lp_tx_count: s.lp_tx_count.unwrap_or(0),
        last_block_hash: s.last_block_hash,
        is_paused: s.is_paused,
        is_initialized: s.is_initialized,
        storage_migrated: prior_migrated,
    })
}

