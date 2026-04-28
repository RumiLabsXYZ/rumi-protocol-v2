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

// --- Event tailing (Phase 4) ---

/// Shadow type for `PoolEventType` from rumi_stability_pool.did.
/// We enumerate every variant so Candid deserialization succeeds for all
/// event types. Variants we process have their fields decoded; others use
/// empty structs (Candid skips extra record fields on the source side).
#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum PoolEventType {
    #[serde(rename = "Deposit")]
    Deposit { token_ledger: Principal, amount: u64 },
    #[serde(rename = "Withdraw")]
    Withdraw { token_ledger: Principal, amount: u64 },
    #[serde(rename = "ClaimCollateral")]
    ClaimCollateral { collateral_ledger: Principal, amount: u64 },
    #[serde(rename = "DepositAs3USD")]
    DepositAs3USD { token_ledger: Principal, amount_in: u64, lp_minted: u64 },
    #[serde(rename = "InterestReceived")]
    InterestReceived { token_ledger: Principal, amount: u64 },
}

/// Shadow type for `PoolEvent` from rumi_stability_pool.did.
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct PoolEvent {
    pub id: u64,
    pub timestamp: u64,
    pub caller: Principal,
    pub event_type: PoolEventType,
}

pub async fn get_pool_event_count(sp: Principal) -> Result<u64, String> {
    let (count,): (u64,) = ic_cdk::call(sp, "get_pool_event_count", ())
        .await
        .map_err(|(code, msg)| format!("get_pool_event_count: {:?} {}", code, msg))?;
    Ok(count)
}

pub async fn get_pool_events(sp: Principal, start: u64, length: u64) -> Result<Vec<PoolEvent>, String> {
    let (events,): (Vec<PoolEvent>,) = ic_cdk::call(sp, "get_pool_events", (start, length))
        .await
        .map_err(|(code, msg)| format!("get_pool_events: {:?} {}", code, msg))?;
    Ok(events)
}
