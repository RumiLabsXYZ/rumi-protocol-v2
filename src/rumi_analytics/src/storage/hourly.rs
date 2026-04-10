//! Hourly snapshot row types and StableLogs for cycle balances and fee curve state.

use candid::{CandidType, Decode, Encode, Principal};
use ic_stable_structures::storable::{Bound, Storable};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::cell::RefCell;
use super::{Memory, get_memory};
use super::{
    MEM_HOURLY_CYCLES_IDX, MEM_HOURLY_CYCLES_DATA,
    MEM_HOURLY_FEE_CURVE_IDX, MEM_HOURLY_FEE_CURVE_DATA,
};

// --- Row types ---

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct HourlyCycleSnapshot {
    pub timestamp_ns: u64,
    pub cycle_balance: u128,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct HourlyFeeCurveSnapshot {
    pub timestamp_ns: u64,
    pub system_cr_bps: u32,
    /// (collateral_principal, total_debt, total_collateral, price)
    pub collateral_stats: Vec<(Principal, u64, u64, f64)>,
}

// --- Storable impls ---

macro_rules! storable_candid {
    ($t:ty) => {
        impl Storable for $t {
            fn to_bytes(&self) -> Cow<'_, [u8]> {
                Cow::Owned(Encode!(self).expect(concat!(stringify!($t), " encode")))
            }
            fn from_bytes(bytes: Cow<'_, [u8]>) -> Self {
                Decode!(bytes.as_ref(), Self).expect(concat!(stringify!($t), " decode"))
            }
            const BOUND: Bound = Bound::Unbounded;
        }
    };
}

storable_candid!(HourlyCycleSnapshot);
storable_candid!(HourlyFeeCurveSnapshot);

// --- StableLog instances ---

thread_local! {
    static HOURLY_CYCLES_LOG: RefCell<ic_stable_structures::StableLog<HourlyCycleSnapshot, Memory, Memory>> =
        RefCell::new({
            ic_stable_structures::StableLog::init(
                get_memory(MEM_HOURLY_CYCLES_IDX),
                get_memory(MEM_HOURLY_CYCLES_DATA),
            ).expect("init HOURLY_CYCLES log")
        });

    static HOURLY_FEE_CURVE_LOG: RefCell<ic_stable_structures::StableLog<HourlyFeeCurveSnapshot, Memory, Memory>> =
        RefCell::new({
            ic_stable_structures::StableLog::init(
                get_memory(MEM_HOURLY_FEE_CURVE_IDX),
                get_memory(MEM_HOURLY_FEE_CURVE_DATA),
            ).expect("init HOURLY_FEE_CURVE log")
        });
}

// --- Accessor modules ---

macro_rules! hourly_accessors {
    ($mod_name:ident, $log:ident, $row_type:ty) => {
        #[allow(dead_code)]
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

hourly_accessors!(hourly_cycles, HOURLY_CYCLES_LOG, HourlyCycleSnapshot);
hourly_accessors!(hourly_fee_curve, HOURLY_FEE_CURVE_LOG, HourlyFeeCurveSnapshot);
