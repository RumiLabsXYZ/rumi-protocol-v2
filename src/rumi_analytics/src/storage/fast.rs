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
    /// Per-token decimal scale. `None` on legacy rows that pre-date this field.
    /// Readers fall back to `[8; N]` (3pool's standard precision).
    pub decimals: Option<Vec<u8>>,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Schema used before the `decimals` field was introduced. Rows written on
    /// mainnet under this shape must still decode under the current type.
    #[derive(CandidType, Serialize, Deserialize)]
    struct Fast3PoolSnapshotV0 {
        timestamp_ns: u64,
        balances: Vec<u128>,
        virtual_price: u128,
        lp_total_supply: u128,
    }

    #[test]
    fn decodes_legacy_row_missing_decimals_field() {
        let v0 = Fast3PoolSnapshotV0 {
            timestamp_ns: 1_700_000_000_000_000_000,
            balances: vec![1_234, 2_345, 3_456],
            virtual_price: 1_000_000_000_000_000_000,
            lp_total_supply: 7_035,
        };
        let bytes = Encode!(&v0).expect("encode legacy row");
        let decoded: Fast3PoolSnapshot =
            Decode!(&bytes, Fast3PoolSnapshot).expect("decode legacy row as current schema");
        assert_eq!(decoded.timestamp_ns, 1_700_000_000_000_000_000);
        assert_eq!(decoded.balances, vec![1_234, 2_345, 3_456]);
        assert_eq!(decoded.virtual_price, 1_000_000_000_000_000_000);
        assert_eq!(decoded.lp_total_supply, 7_035);
        assert_eq!(decoded.decimals, None);
    }

    #[test]
    fn current_row_round_trips_via_storable() {
        let current = Fast3PoolSnapshot {
            timestamp_ns: 1_800_000_000_000_000_000,
            balances: vec![5, 10, 15],
            virtual_price: 1_020_000_000_000_000_000,
            lp_total_supply: 30,
            decimals: Some(vec![8, 8, 8]),
        };
        let bytes = <Fast3PoolSnapshot as Storable>::to_bytes(&current);
        let decoded = <Fast3PoolSnapshot as Storable>::from_bytes(bytes);
        assert_eq!(decoded.timestamp_ns, current.timestamp_ns);
        assert_eq!(decoded.balances, current.balances);
        assert_eq!(decoded.virtual_price, current.virtual_price);
        assert_eq!(decoded.lp_total_supply, current.lp_total_supply);
        assert_eq!(decoded.decimals, Some(vec![8, 8, 8]));
    }

    #[test]
    fn mixed_log_decodes_legacy_and_current_rows() {
        // A "mixed" log on mainnet has older rows without `decimals` and newer
        // rows with `Some(vec![...])`. The Storable::from_bytes path has to
        // tolerate both shapes so that range() can iterate the full log.
        let legacy = Fast3PoolSnapshotV0 {
            timestamp_ns: 100,
            balances: vec![1, 2, 3],
            virtual_price: 1_000_000_000_000_000_000,
            lp_total_supply: 6,
        };
        let current = Fast3PoolSnapshot {
            timestamp_ns: 200,
            balances: vec![4, 5, 6],
            virtual_price: 1_010_000_000_000_000_000,
            lp_total_supply: 15,
            decimals: Some(vec![8, 8, 8]),
        };

        let legacy_bytes = Encode!(&legacy).expect("encode legacy");
        let current_bytes = <Fast3PoolSnapshot as Storable>::to_bytes(&current);

        let decoded_legacy: Fast3PoolSnapshot =
            Decode!(&legacy_bytes, Fast3PoolSnapshot).expect("decode legacy");
        let decoded_current =
            <Fast3PoolSnapshot as Storable>::from_bytes(current_bytes);

        let rows = vec![decoded_legacy, decoded_current];
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|r| r.timestamp_ns == 100 && r.decimals.is_none()));
        assert!(rows
            .iter()
            .any(|r| r.timestamp_ns == 200 && r.decimals.as_deref() == Some(&[8, 8, 8][..])));
    }
}
