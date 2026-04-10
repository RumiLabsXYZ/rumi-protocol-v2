//! DailyHolderRow type and StableLog instances for icUSD and 3USD holder snapshots.

use candid::{CandidType, Decode, Encode, Principal};
use ic_stable_structures::storable::{Bound, Storable};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::cell::RefCell;
use super::{Memory, get_memory};
use super::{
    MEM_DAILY_HOLDERS_ICUSD_IDX, MEM_DAILY_HOLDERS_ICUSD_DATA,
    MEM_DAILY_HOLDERS_3USD_IDX, MEM_DAILY_HOLDERS_3USD_DATA,
};

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct DailyHolderRow {
    pub timestamp_ns: u64,
    pub token: Principal,
    pub total_holders: u32,
    pub total_supply_tracked_e8s: u64,
    pub median_balance_e8s: u64,
    pub top_50: Vec<(Principal, u64)>,
    pub top_10_pct_bps: u32,
    pub gini_bps: u32,
    pub new_holders_today: u32,
    pub distribution_buckets: Vec<u32>,
}

impl Storable for DailyHolderRow {
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        Cow::Owned(Encode!(self).expect("DailyHolderRow encode"))
    }
    fn from_bytes(bytes: Cow<'_, [u8]>) -> Self {
        Decode!(bytes.as_ref(), Self).expect("DailyHolderRow decode")
    }
    const BOUND: Bound = Bound::Unbounded;
}

thread_local! {
    static DAILY_HOLDERS_ICUSD_LOG: RefCell<ic_stable_structures::StableLog<DailyHolderRow, Memory, Memory>> =
        RefCell::new({
            ic_stable_structures::StableLog::init(
                get_memory(MEM_DAILY_HOLDERS_ICUSD_IDX),
                get_memory(MEM_DAILY_HOLDERS_ICUSD_DATA),
            ).expect("init DAILY_HOLDERS_ICUSD log")
        });

    static DAILY_HOLDERS_3USD_LOG: RefCell<ic_stable_structures::StableLog<DailyHolderRow, Memory, Memory>> =
        RefCell::new({
            ic_stable_structures::StableLog::init(
                get_memory(MEM_DAILY_HOLDERS_3USD_IDX),
                get_memory(MEM_DAILY_HOLDERS_3USD_DATA),
            ).expect("init DAILY_HOLDERS_3USD log")
        });
}

macro_rules! holder_accessors {
    ($mod_name:ident, $log:ident) => {
        #[allow(dead_code)]
        pub mod $mod_name {
            use super::*;

            pub fn push(row: DailyHolderRow) {
                $log.with(|log| {
                    log.borrow_mut().append(&row).expect(concat!("append ", stringify!($mod_name)));
                });
            }

            pub fn len() -> u64 {
                $log.with(|log| log.borrow().len())
            }

            pub fn get(index: u64) -> Option<DailyHolderRow> {
                $log.with(|log| log.borrow().get(index))
            }

            pub fn range(from_ts: u64, to_ts: u64, limit: usize) -> Vec<DailyHolderRow> {
                let mut out = Vec::new();
                $log.with(|log| {
                    let log = log.borrow();
                    let n = log.len();
                    for i in 0..n {
                        if let Some(row) = log.get(i) {
                            if row.timestamp_ns >= to_ts { break; }
                            if row.timestamp_ns >= from_ts {
                                out.push(row);
                                if out.len() >= limit { break; }
                            }
                        }
                    }
                });
                out
            }
        }
    };
}

holder_accessors!(daily_holders_icusd, DAILY_HOLDERS_ICUSD_LOG);
holder_accessors!(daily_holders_3usd, DAILY_HOLDERS_3USD_LOG);
