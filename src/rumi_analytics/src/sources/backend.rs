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

// --- Event tailing (Phase 4) ---

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct GetEventsArg {
    pub start: u64,
    pub length: u64,
}

/// Minimal subset of the backend Event variant.
/// Uses `#[serde(other)]` to catch variants we don't need to decode.
#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum BackendEvent {
    #[serde(rename = "open_vault")]
    OpenVault {
        block_index: u64,
        vault: BackendVaultRecord,
        timestamp: Option<u64>,
    },
    #[serde(rename = "borrow_from_vault")]
    BorrowFromVault {
        block_index: u64,
        vault_id: u64,
        fee_amount: u64,
        borrowed_amount: u64,
        caller: Option<Principal>,
        timestamp: Option<u64>,
    },
    #[serde(rename = "repay_to_vault")]
    RepayToVault {
        block_index: u64,
        vault_id: u64,
        repayed_amount: u64,
        caller: Option<Principal>,
        timestamp: Option<u64>,
    },
    #[serde(rename = "collateral_withdrawn")]
    CollateralWithdrawn {
        block_index: u64,
        vault_id: u64,
        amount: u64,
        caller: Option<Principal>,
        timestamp: Option<u64>,
    },
    #[serde(rename = "partial_collateral_withdrawn")]
    PartialCollateralWithdrawn {
        block_index: u64,
        vault_id: u64,
        amount: u64,
        caller: Option<Principal>,
        timestamp: Option<u64>,
    },
    #[serde(rename = "withdraw_and_close_vault")]
    WithdrawAndCloseVault {
        block_index: Option<u64>,
        vault_id: u64,
        amount: u64,
        caller: Option<Principal>,
        timestamp: Option<u64>,
    },
    VaultWithdrawnAndClosed {
        vault_id: u64,
        timestamp: u64,
        caller: Principal,
        amount: u64,
    },
    #[serde(rename = "dust_forgiven")]
    DustForgiven {
        vault_id: u64,
        amount: u64,
        timestamp: Option<u64>,
    },
    #[serde(rename = "redemption_on_vaults")]
    RedemptionOnVaults {
        icusd_amount: u64,
        icusd_block_index: u64,
        owner: Principal,
        fee_amount: u64,
        collateral_type: Option<Principal>,
        timestamp: Option<u64>,
    },
    #[serde(rename = "liquidate_vault")]
    LiquidateVault {
        vault_id: u64,
        liquidator: Option<Principal>,
        timestamp: Option<u64>,
    },
    #[serde(rename = "partial_liquidate_vault")]
    PartialLiquidateVault {
        protocol_fee_collateral: Option<u64>,
        liquidator_payment: u64,
        vault_id: u64,
        liquidator: Option<Principal>,
        icp_to_liquidator: u64,
        timestamp: Option<u64>,
    },
    #[serde(rename = "redistribute_vault")]
    RedistributeVault {
        vault_id: u64,
        timestamp: Option<u64>,
    },
    #[serde(other)]
    Unknown,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct BackendVaultRecord {
    pub vault_id: u64,
    pub owner: Principal,
    pub collateral_type: Principal,
    pub collateral_amount: u64,
    pub borrowed_icusd_amount: u64,
}

pub async fn get_events(
    backend: Principal,
    start: u64,
    length: u64,
) -> Result<Vec<BackendEvent>, String> {
    let arg = GetEventsArg { start, length };
    let (events,): (Vec<BackendEvent>,) = ic_cdk::call(backend, "get_events", (arg,))
        .await
        .map_err(|(code, msg)| format!("get_events: {:?} {}", code, msg))?;
    Ok(events)
}

pub async fn get_event_count(backend: Principal) -> Result<u64, String> {
    let (count,): (u64,) = ic_cdk::call(backend, "get_event_count", ())
        .await
        .map_err(|(code, msg)| format!("get_event_count: {:?} {}", code, msg))?;
    Ok(count)
}
