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

/// Shadow type for `PoolEventType` from rumi_stability_pool. The Candid
/// service .did file lists only 5 variants but the actual Rust enum (in
/// `src/stability_pool/src/types.rs`) has 16. The wire format includes all
/// 16, so this shadow MUST enumerate every one or `Vec<PoolEvent>` decoding
/// fails the moment any non-listed variant appears. We tail Deposit /
/// Withdraw / DepositAs3USD / ClaimCollateral; everything else gets matched
/// exhaustively but routed through a no-op skip (see route_sp_event).
#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum PoolEventType {
    Deposit { token_ledger: Principal, amount: u64 },
    Withdraw { token_ledger: Principal, amount: u64 },
    ClaimCollateral { collateral_ledger: Principal, amount: u64 },
    DepositAs3USD { token_ledger: Principal, amount_in: u64, lp_minted: u64 },
    InterestReceived { token_ledger: Principal, amount: u64 },
    OptOutCollateral { collateral_type: Principal },
    OptInCollateral { collateral_type: Principal },
    LiquidationNotification { vault_count: u64 },
    LiquidationExecuted {
        vault_id: u64,
        stables_consumed_e8s: u64,
        collateral_gained: u64,
        collateral_type: Principal,
        success: bool,
    },
    StablecoinRegistered { ledger: Principal, symbol: String },
    CollateralRegistered { ledger: Principal, symbol: String },
    ConfigurationUpdated,
    EmergencyPauseActivated,
    OperationsResumed,
    BalanceCorrected { user: Principal, token_ledger: Principal, new_amount: u64 },
    CollateralGainCorrected {
        user: Principal,
        collateral_ledger: Principal,
        new_amount: u64,
    },
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
