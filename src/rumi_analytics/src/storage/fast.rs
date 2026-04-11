//! Fast (5-minute) snapshot row types and StableLogs for prices and 3pool state.

use candid::{CandidType, Decode, Encode, Principal};
use ic_stable_structures::storable::{Bound, Storable};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::cell::RefCell;
use super::{Memory, get_memory};
use super::{
    MEM_FAST_PRICES_IDX, MEM_FAST_PRICES_DATA,
    MEM_FAST_3POOL_IDX, MEM_FAST_3POOL_DATA,
};

// --- Row types ---

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct FastPriceSnapshot {
    pub timestamp_ns: u64,
    /// (collateral_principal, price_usd, symbol)
    pub prices: Vec<(Principal, f64, String)>,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct Fast3PoolSnapshot {
    pub timestamp_ns: u64,
    pub balances: Vec<u128>,
    pub virtual_price: u128,
    pub lp_total_supply: u128,
    pub decimals: Vec<u8>,
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

storable_candid!(FastPriceSnapshot);
storable_candid!(Fast3PoolSnapshot);

// --- StableLog instances ---

thread_local! {
    static FAST_PRICES_LOG: RefCell<ic_stable_structures::StableLog<FastPriceSnapshot, Memory, Memory>> =
        RefCell::new({
            ic_stable_structures::StableLog::init(
                get_memory(MEM_FAST_PRICES_IDX),
                get_memory(MEM_FAST_PRICES_DATA),
            ).expect("init FAST_PRICES log")
        });

    static FAST_3POOL_LOG: RefCell<ic_stable_structures::StableLog<Fast3PoolSnapshot, Memory, Memory>> =
        RefCell::new({
            ic_stable_structures::StableLog::init(
                get_memory(MEM_FAST_3POOL_IDX),
                get_memory(MEM_FAST_3POOL_DATA),
            ).expect("init FAST_3POOL log")
        });
}

// --- Accessor modules ---

macro_rules! fast_accessors {
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

fast_accessors!(fast_prices, FAST_PRICES_LOG, FastPriceSnapshot);
fast_accessors!(fast_3pool, FAST_3POOL_LOG, Fast3PoolSnapshot);
