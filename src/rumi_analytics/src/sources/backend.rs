//! Typed wrapper around rumi_protocol_backend queries.
//!
//! Every function here returns Result<T, String> and never panics. Errors
//! propagate up to the caller (collectors), which increment the per-source
//! error counter and skip the snapshot for this tick.

use candid::{CandidType, Deserialize, Principal};

/// Subset of `ProtocolStatus` that Phase 1 needs. We deliberately decode only
/// the fields we use; this insulates us from upstream additions to the type.
///
/// IMPORTANT: candid decodes record types structurally by field name, so this
/// minimal struct works as long as the source canister's ProtocolStatus
/// includes these fields. Additional fields on the source side are ignored.
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ProtocolStatusSubset {
    pub total_icp_margin: u64,
    pub total_icusd_borrowed: u64,
    pub total_collateral_ratio: f64,
}

pub async fn get_protocol_status(backend: Principal) -> Result<ProtocolStatusSubset, String> {
    let res: Result<(ProtocolStatusSubset,), _> =
        ic_cdk::api::call::call(backend, "get_protocol_status", ()).await;
    match res {
        Ok((status,)) => Ok(status),
        Err((code, msg)) => Err(format!("get_protocol_status: {:?} {}", code, msg)),
    }
}

/// Subset of `CandidVault` that analytics needs. We deliberately decode only
/// the fields we use; this insulates us from upstream additions to the type.
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct CandidVaultSubset {
    pub owner: Principal,
    pub borrowed_icusd_amount: u64,
    pub icp_margin_amount: u64,
    pub vault_id: u64,
    pub collateral_amount: u64,
    pub collateral_type: Principal,
    pub accrued_interest: u64,
}

pub async fn get_all_vaults(backend: Principal) -> Result<Vec<CandidVaultSubset>, String> {
    let res: Result<(Vec<CandidVaultSubset>,), _> =
        ic_cdk::api::call::call(backend, "get_all_vaults", ()).await;
    match res {
        Ok((vaults,)) => Ok(vaults),
        Err((code, msg)) => Err(format!("get_all_vaults: {:?} {}", code, msg)),
    }
}

/// Subset of `CollateralTotals` that analytics needs.
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct CollateralTotalsSubset {
    pub decimals: u8,
    pub total_collateral: u64,
    pub total_debt: u64,
    pub collateral_type: Principal,
    pub price: f64,
    pub vault_count: u64,
    pub symbol: String,
}

pub async fn get_collateral_totals(
    backend: Principal,
) -> Result<Vec<CollateralTotalsSubset>, String> {
    let res: Result<(Vec<CollateralTotalsSubset>,), _> =
        ic_cdk::api::call::call(backend, "get_collateral_totals", ()).await;
    match res {
        Ok((totals,)) => Ok(totals),
        Err((code, msg)) => Err(format!("get_collateral_totals: {:?} {}", code, msg)),
    }
}
