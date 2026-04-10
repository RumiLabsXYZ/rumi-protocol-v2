//! Typed wrapper around rumi_stability_pool queries.
//!
//! Every function here returns Result<T, String> and never panics. Errors
//! propagate up to the caller (collectors), which increment the per-source
//! error counter and skip the snapshot for this tick.

use candid::{CandidType, Deserialize, Principal};

/// Subset of `StabilityPoolStatus` that analytics needs. We deliberately decode
/// only the fields we use; this insulates us from upstream additions to the type.
///
/// IMPORTANT: candid decodes record types structurally by field name, so this
/// minimal struct works as long as the source canister's StabilityPoolStatus
/// includes these fields. Additional fields on the source side are ignored.
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct StabilityPoolStatusSubset {
    pub total_deposits_e8s: u64,
    pub total_depositors: u64,
    pub total_liquidations_executed: u64,
    pub total_interest_received_e8s: u64,
    pub stablecoin_balances: Vec<(Principal, u64)>,
    pub collateral_gains: Vec<(Principal, u64)>,
}

pub async fn get_pool_status(
    stability_pool: Principal,
) -> Result<StabilityPoolStatusSubset, String> {
    let res: Result<(StabilityPoolStatusSubset,), _> =
        ic_cdk::api::call::call(stability_pool, "get_pool_status", ()).await;
    match res {
        Ok((status,)) => Ok(status),
        Err((code, msg)) => Err(format!("get_pool_status: {:?} {}", code, msg)),
    }
}
