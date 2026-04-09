//! Typed wrapper around rumi_3pool queries.
//!
//! Every function here returns Result<T, String> and never panics. Errors
//! propagate up to the caller (collectors), which increment the per-source
//! error counter and skip the snapshot for this tick.
//!
//! The 3pool .did uses `nat` (arbitrary precision) for balances, lp_total_supply,
//! and virtual_price. We decode those as `candid::Nat` then convert to u128.

use candid::{CandidType, Deserialize, Nat, Principal};

/// Raw decoded form of `PoolStatus` using `candid::Nat` for arbitrary-precision
/// fields. Never stored directly; converted to `ThreePoolStatusSubset` immediately.
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct PoolStatusRaw {
    pub balances: Vec<Nat>,
    pub lp_total_supply: Nat,
    pub current_a: u64,
    pub virtual_price: Nat,
    pub swap_fee_bps: u64,
    pub admin_fee_bps: u64,
}

/// Converted form with all `nat` fields represented as `u128`.
#[derive(Clone, Debug)]
pub struct ThreePoolStatusSubset {
    pub balances: Vec<u128>,
    pub lp_total_supply: u128,
    pub current_a: u64,
    pub virtual_price: u128,
    pub swap_fee_bps: u64,
    pub admin_fee_bps: u64,
}

fn nat_to_u128(nat: Nat, field: &str) -> Result<u128, String> {
    nat.0
        .try_into()
        .map_err(|e| format!("3pool {} nat -> u128: {}", field, e))
}

pub async fn get_pool_status(three_pool: Principal) -> Result<ThreePoolStatusSubset, String> {
    let res: Result<(PoolStatusRaw,), _> =
        ic_cdk::api::call::call(three_pool, "get_pool_status", ()).await;
    match res {
        Ok((raw,)) => {
            let balances = raw
                .balances
                .into_iter()
                .enumerate()
                .map(|(i, n)| nat_to_u128(n, &format!("balances[{}]", i)))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ThreePoolStatusSubset {
                balances,
                lp_total_supply: nat_to_u128(raw.lp_total_supply, "lp_total_supply")?,
                current_a: raw.current_a,
                virtual_price: nat_to_u128(raw.virtual_price, "virtual_price")?,
                swap_fee_bps: raw.swap_fee_bps,
                admin_fee_bps: raw.admin_fee_bps,
            })
        }
        Err((code, msg)) => Err(format!("get_pool_status: {:?} {}", code, msg)),
    }
}
