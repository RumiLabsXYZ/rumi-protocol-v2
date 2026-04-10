//! Daily rollup row types and StableLogs for liquidations, swaps, and fees.

use candid::{CandidType, Decode, Encode, Principal};
use ic_stable_structures::storable::{Bound, Storable};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::cell::RefCell;
use super::{Memory, get_memory};
use super::{
    MEM_DAILY_LIQUIDATIONS_IDX, MEM_DAILY_LIQUIDATIONS_DATA,
    MEM_DAILY_SWAPS_IDX, MEM_DAILY_SWAPS_DATA,
    MEM_DAILY_FEES_IDX, MEM_DAILY_FEES_DATA,
};

// --- Row types ---

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct DailyLiquidationRollup {
    pub timestamp_ns: u64,
    pub full_count: u32,
    pub partial_count: u32,
    pub redistribution_count: u32,
    pub total_collateral_seized_e8s: u64,
    pub total_debt_covered_e8s: u64,
    pub by_collateral: Vec<(Principal, u64)>,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct DailySwapRollup {
    pub timestamp_ns: u64,
    pub three_pool_swap_count: u32,
    pub amm_swap_count: u32,
    pub three_pool_volume_e8s: u64,
    pub amm_volume_e8s: u64,
    pub three_pool_fees_e8s: u64,
    pub amm_fees_e8s: u64,
    pub unique_swappers: u32,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct DailyFeeRollup {
    pub timestamp_ns: u64,
    /// None until EVT_VAULTS captures fee_amount for borrow events.
    pub borrowing_fees_e8s: Option<u64>,
    pub borrow_count: u32,
    pub swap_fees_e8s: u64,
    /// None until EVT_VAULTS captures fee_amount for redemption events.
    pub redemption_fees_e8s: Option<u64>,
    pub redemption_count: u32,
}

// --- Storable impls ---

macro_rules! storable_candid {
    ($t:ty) => {
        impl Storable for $t {
            fn to_bytes(&self) -> Cow<[u8]> {
                Cow::Owned(Encode!(self).expect(concat!(stringify!($t), " encode")))
            }
            fn from_bytes(bytes: Cow<[u8]>) -> Self {
                Decode!(bytes.as_ref(), Self).expect(concat!(stringify!($t), " decode"))
            }
            const BOUND: Bound = Bound::Unbounded;
        }
    };
}

storable_candid!(DailyLiquidationRollup);
storable_candid!(DailySwapRollup);
storable_candid!(DailyFeeRollup);

// --- StableLog instances ---

thread_local! {
    static DAILY_LIQUIDATIONS_LOG: RefCell<ic_stable_structures::StableLog<DailyLiquidationRollup, Memory, Memory>> =
        RefCell::new({
            ic_stable_structures::StableLog::init(
                get_memory(MEM_DAILY_LIQUIDATIONS_IDX),
                get_memory(MEM_DAILY_LIQUIDATIONS_DATA),
            ).expect("init DAILY_LIQUIDATIONS log")
        });

    static DAILY_SWAPS_LOG: RefCell<ic_stable_structures::StableLog<DailySwapRollup, Memory, Memory>> =
        RefCell::new({
            ic_stable_structures::StableLog::init(
                get_memory(MEM_DAILY_SWAPS_IDX),
                get_memory(MEM_DAILY_SWAPS_DATA),
            ).expect("init DAILY_SWAPS log")
        });

    static DAILY_FEES_LOG: RefCell<ic_stable_structures::StableLog<DailyFeeRollup, Memory, Memory>> =
        RefCell::new({
            ic_stable_structures::StableLog::init(
                get_memory(MEM_DAILY_FEES_IDX),
                get_memory(MEM_DAILY_FEES_DATA),
            ).expect("init DAILY_FEES log")
        });
}

// --- Accessor modules ---

macro_rules! rollup_accessors {
    ($mod_name:ident, $log:ident, $row_type:ty) => {
        pub mod $mod_name {
            use super::*;

            pub fn push(row: $row_type) {
                $log.with(|log| {
                    log.borrow_mut().append(&row).expect(concat!("append ", stringify!($mod_name)));
                });
            }

            pub fn len() -> u64 {
                $log.with(|log| log.borrow().len())
            }

            pub fn get(index: u64) -> Option<$row_type> {
                $log.with(|log| log.borrow().get(index))
            }

            pub fn range(from_ts: u64, to_ts: u64, limit: usize) -> Vec<$row_type> {
                let mut out = Vec::new();
                $log.with(|log| {
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
    };
}

rollup_accessors!(daily_liquidations, DAILY_LIQUIDATIONS_LOG, DailyLiquidationRollup);
rollup_accessors!(daily_swaps, DAILY_SWAPS_LOG, DailySwapRollup);
rollup_accessors!(daily_fees, DAILY_FEES_LOG, DailyFeeRollup);
