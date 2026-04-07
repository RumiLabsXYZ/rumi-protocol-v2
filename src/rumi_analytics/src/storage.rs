//! Stable-memory storage backend for rumi_analytics.
//!
//! This module owns the `MemoryManager` and every `StableCell`, `StableLog`,
//! and `StableBTreeMap` the canister persists across upgrades.
//!
//! The MemoryId map is reserved in full from day one (even slots not yet used)
//! so that future phases never have to renumber. See
//! `docs/plans/2026-04-07-rumi-analytics-design.md` for the full layout.
//!
//! Invariant: no module other than `storage` imports `ic_stable_structures`.
//! All persistent reads and writes go through accessor functions defined here.

use candid::{CandidType, Decode, Encode, Principal};
use ic_stable_structures::{
    memory_manager::{MemoryId, MemoryManager, VirtualMemory},
    storable::{Bound, Storable},
    DefaultMemoryImpl, StableCell,
};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::cell::RefCell;

pub type Memory = VirtualMemory<DefaultMemoryImpl>;

// --- Memory IDs ---
// Slot reservation map (must match docs/plans/2026-04-07-rumi-analytics-design.md):
//   0       SlimState (StableCell)
//   1-7     cursor cells (StableCell<u64>) - one per source stream
//   8-9     reserved
//   10-25   daily snapshot logs (paired idx/data, StableLog)
//   26-29   reserved
//   30-33   fast (5min) snapshot logs
//   34-37   reserved
//   38-41   hourly snapshot logs
//   42-43   reserved
//   44-51   per-event mirror logs
//   52-55   reserved
//   56-57   BalanceTracker maps (StableBTreeMap)
//   58-59   FirstSeen maps (StableBTreeMap)
//   60-63   reserved

pub const MEM_SLIM_STATE: MemoryId = MemoryId::new(0);

// Cursors (Phase 4+)
pub const MEM_CURSOR_BACKEND_EVENTS: MemoryId = MemoryId::new(1);
pub const MEM_CURSOR_3POOL_SWAPS: MemoryId = MemoryId::new(2);
pub const MEM_CURSOR_3POOL_LIQUIDITY: MemoryId = MemoryId::new(3);
pub const MEM_CURSOR_3POOL_BLOCKS: MemoryId = MemoryId::new(4);
pub const MEM_CURSOR_AMM_SWAPS: MemoryId = MemoryId::new(5);
pub const MEM_CURSOR_STABILITY_EVENTS: MemoryId = MemoryId::new(6);
pub const MEM_CURSOR_ICUSD_BLOCKS: MemoryId = MemoryId::new(7);

// Daily snapshot logs
pub const MEM_DAILY_TVL_IDX: MemoryId = MemoryId::new(10);
pub const MEM_DAILY_TVL_DATA: MemoryId = MemoryId::new(11);
pub const MEM_DAILY_VAULTS_IDX: MemoryId = MemoryId::new(12);
pub const MEM_DAILY_VAULTS_DATA: MemoryId = MemoryId::new(13);
pub const MEM_DAILY_HOLDERS_ICUSD_IDX: MemoryId = MemoryId::new(14);
pub const MEM_DAILY_HOLDERS_ICUSD_DATA: MemoryId = MemoryId::new(15);
pub const MEM_DAILY_HOLDERS_3USD_IDX: MemoryId = MemoryId::new(16);
pub const MEM_DAILY_HOLDERS_3USD_DATA: MemoryId = MemoryId::new(17);
pub const MEM_DAILY_LIQUIDATIONS_IDX: MemoryId = MemoryId::new(18);
pub const MEM_DAILY_LIQUIDATIONS_DATA: MemoryId = MemoryId::new(19);
pub const MEM_DAILY_SWAPS_IDX: MemoryId = MemoryId::new(20);
pub const MEM_DAILY_SWAPS_DATA: MemoryId = MemoryId::new(21);
pub const MEM_DAILY_FEES_IDX: MemoryId = MemoryId::new(22);
pub const MEM_DAILY_FEES_DATA: MemoryId = MemoryId::new(23);
pub const MEM_DAILY_STABILITY_IDX: MemoryId = MemoryId::new(24);
pub const MEM_DAILY_STABILITY_DATA: MemoryId = MemoryId::new(25);

// Fast (5min) snapshot logs
pub const MEM_FAST_PRICES_IDX: MemoryId = MemoryId::new(30);
pub const MEM_FAST_PRICES_DATA: MemoryId = MemoryId::new(31);
pub const MEM_FAST_3POOL_IDX: MemoryId = MemoryId::new(32);
pub const MEM_FAST_3POOL_DATA: MemoryId = MemoryId::new(33);

// Hourly snapshot logs
pub const MEM_HOURLY_CYCLES_IDX: MemoryId = MemoryId::new(38);
pub const MEM_HOURLY_CYCLES_DATA: MemoryId = MemoryId::new(39);
pub const MEM_HOURLY_FEE_CURVE_IDX: MemoryId = MemoryId::new(40);
pub const MEM_HOURLY_FEE_CURVE_DATA: MemoryId = MemoryId::new(41);

// Per-event mirror logs
pub const MEM_EVT_LIQUIDATIONS_IDX: MemoryId = MemoryId::new(44);
pub const MEM_EVT_LIQUIDATIONS_DATA: MemoryId = MemoryId::new(45);
pub const MEM_EVT_SWAPS_IDX: MemoryId = MemoryId::new(46);
pub const MEM_EVT_SWAPS_DATA: MemoryId = MemoryId::new(47);
pub const MEM_EVT_LIQUIDITY_IDX: MemoryId = MemoryId::new(48);
pub const MEM_EVT_LIQUIDITY_DATA: MemoryId = MemoryId::new(49);
pub const MEM_EVT_VAULTS_IDX: MemoryId = MemoryId::new(50);
pub const MEM_EVT_VAULTS_DATA: MemoryId = MemoryId::new(51);

// BalanceTracker maps (Phase 4)
pub const MEM_BAL_ICUSD: MemoryId = MemoryId::new(56);
pub const MEM_BAL_3USD: MemoryId = MemoryId::new(57);
pub const MEM_FIRSTSEEN_ICUSD: MemoryId = MemoryId::new(58);
pub const MEM_FIRSTSEEN_3USD: MemoryId = MemoryId::new(59);

// --- SlimState ---
// Bounded residual heap state. Written to MemoryId 0 via StableCell. Holds
// only small fixed-size values; never any unbounded collections.
//
// Cursors are deliberately NOT in here: they live in their own StableCells
// (MemoryIds 1-7) so cursor advancement is atomic with the StableLog write
// that uses it.

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct SlimState {
    /// Admin principal authorized to call mutating endpoints.
    pub admin: Principal,
    /// Source canister IDs (configurable so we can wire test fixtures).
    pub sources: SourceCanisterIds,
    /// Cached circulating supply for /api/supply, refreshed by the 60s pull cycle.
    /// `None` until the first successful refresh after canister start.
    pub circulating_supply_icusd_e8s: Option<u128>,
    pub circulating_supply_3usd_e8s: Option<u128>,
    /// Last successful daily snapshot timestamp (ns).
    pub last_daily_snapshot_ns: u64,
    /// Per-source error counters incremented on inter-canister call failures.
    pub error_counters: ErrorCounters,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct SourceCanisterIds {
    pub backend: Principal,
    pub icusd_ledger: Principal,
    pub three_pool: Principal,
    pub stability_pool: Principal,
    pub amm: Principal,
}

#[derive(CandidType, Clone, Debug, Default, Serialize, Deserialize)]
pub struct ErrorCounters {
    pub backend: u64,
    pub icusd_ledger: u64,
    pub three_pool: u64,
    pub stability_pool: u64,
    pub amm: u64,
}

impl Default for SlimState {
    fn default() -> Self {
        Self {
            admin: Principal::anonymous(),
            sources: SourceCanisterIds {
                backend: Principal::anonymous(),
                icusd_ledger: Principal::anonymous(),
                three_pool: Principal::anonymous(),
                stability_pool: Principal::anonymous(),
                amm: Principal::anonymous(),
            },
            circulating_supply_icusd_e8s: None,
            circulating_supply_3usd_e8s: None,
            last_daily_snapshot_ns: 0,
            error_counters: ErrorCounters::default(),
        }
    }
}

impl Storable for SlimState {
    fn to_bytes(&self) -> Cow<[u8]> {
        Cow::Owned(Encode!(self).expect("SlimState encode"))
    }
    fn from_bytes(bytes: Cow<[u8]>) -> Self {
        Decode!(bytes.as_ref(), Self).expect("SlimState decode")
    }
    const BOUND: Bound = Bound::Unbounded;
}

// --- DailyTvlRow ---
// First snapshot row type. Subsequent phases add more row types in their own
// modules.

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct DailyTvlRow {
    pub timestamp_ns: u64,
    pub total_icp_collateral_e8s: u128,
    pub total_icusd_supply_e8s: u128,
    pub system_collateral_ratio_bps: u32,
}

impl Storable for DailyTvlRow {
    fn to_bytes(&self) -> Cow<[u8]> {
        Cow::Owned(Encode!(self).expect("DailyTvlRow encode"))
    }
    fn from_bytes(bytes: Cow<[u8]>) -> Self {
        Decode!(bytes.as_ref(), Self).expect("DailyTvlRow decode")
    }
    const BOUND: Bound = Bound::Unbounded;
}

// --- Memory manager and instantiated structures ---

thread_local! {
    static MEMORY_MANAGER: RefCell<MemoryManager<DefaultMemoryImpl>> =
        RefCell::new(MemoryManager::init(DefaultMemoryImpl::default()));

    static SLIM_CELL: RefCell<StableCell<SlimState, Memory>> = RefCell::new({
        let mem = MEMORY_MANAGER.with(|m| m.borrow().get(MEM_SLIM_STATE));
        StableCell::init(mem, SlimState::default())
            .expect("init SlimState cell")
    });

    static DAILY_TVL_LOG: RefCell<ic_stable_structures::StableLog<DailyTvlRow, Memory, Memory>> =
        RefCell::new({
            let idx = MEMORY_MANAGER.with(|m| m.borrow().get(MEM_DAILY_TVL_IDX));
            let data = MEMORY_MANAGER.with(|m| m.borrow().get(MEM_DAILY_TVL_DATA));
            ic_stable_structures::StableLog::init(idx, data)
                .expect("init DAILY_TVL log")
        });
}

// --- Public accessors ---

pub fn get_slim() -> SlimState {
    SLIM_CELL.with(|c| c.borrow().get().clone())
}

pub fn set_slim(s: SlimState) {
    SLIM_CELL.with(|c| {
        c.borrow_mut().set(s).expect("set SlimState cell");
    });
}

pub fn mutate_slim<F: FnOnce(&mut SlimState)>(f: F) {
    let mut s = get_slim();
    f(&mut s);
    set_slim(s);
}

pub mod daily_tvl {
    use super::*;

    pub fn push(row: DailyTvlRow) {
        DAILY_TVL_LOG.with(|log| {
            log.borrow_mut().append(&row).expect("append DAILY_TVL");
        });
    }

    pub fn len() -> u64 {
        DAILY_TVL_LOG.with(|log| log.borrow().len())
    }

    pub fn get(index: u64) -> Option<DailyTvlRow> {
        DAILY_TVL_LOG.with(|log| log.borrow().get(index))
    }

    /// Iterate rows whose `timestamp_ns` falls in `[from_ts, to_ts)`, returning
    /// at most `limit` rows starting at the row whose timestamp is `>= from_ts`.
    ///
    /// **Phase 1 simplification (deviation from spec)**: this uses a linear
    /// scan instead of the binary search the spec calls for. Phase 1 ships
    /// with at most ~30 daily rows in the log; linear scan is O(n) over a
    /// trivially small n. Binary search is added in a later phase once any
    /// historical log grows past ~10k rows. Documenting the deviation here
    /// so the next phase has an obvious place to fix it.
    pub fn range(from_ts: u64, to_ts: u64, limit: usize) -> Vec<DailyTvlRow> {
        let mut out = Vec::new();
        DAILY_TVL_LOG.with(|log| {
            let log = log.borrow();
            let n = log.len();
            for i in 0..n {
                if let Some(row) = log.get(i) {
                    if row.timestamp_ns >= to_ts {
                        break;
                    }
                    if row.timestamp_ns >= from_ts {
                        out.push(row);
                        if out.len() >= limit {
                            break;
                        }
                    }
                }
            }
        });
        out
    }
}
