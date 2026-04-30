//! Source wrapper for rumi_amm event queries.

use candid::{CandidType, Nat, Principal};
use serde::Deserialize;

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct AmmSwapEvent {
    pub id: u64,
    pub caller: Principal,
    pub pool_id: String,
    pub token_in: Principal,
    pub amount_in: Nat,
    pub token_out: Principal,
    pub amount_out: Nat,
    pub fee: Nat,
    pub timestamp: u64,
}

pub async fn get_amm_swap_events(
    amm: Principal,
    start: u64,
    length: u64,
) -> Result<Vec<AmmSwapEvent>, String> {
    let (events,): (Vec<AmmSwapEvent>,) =
        ic_cdk::call(amm, "get_amm_swap_events", (start, length))
            .await
            .map_err(|(code, msg)| format!("get_amm_swap_events: {:?} {}", code, msg))?;
    Ok(events)
}

pub async fn get_amm_swap_event_count(amm: Principal) -> Result<u64, String> {
    let (count,): (u64,) = ic_cdk::call(amm, "get_amm_swap_event_count", ())
        .await
        .map_err(|(code, msg)| format!("get_amm_swap_event_count: {:?} {}", code, msg))?;
    Ok(count)
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum AmmLiquidityAction {
    AddLiquidity,
    RemoveLiquidity,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct AmmLiquidityEvent {
    pub id: u64,
    pub caller: Principal,
    pub pool_id: String,
    pub action: AmmLiquidityAction,
    pub token_a: Principal,
    pub amount_a: Nat,
    pub token_b: Principal,
    pub amount_b: Nat,
    pub lp_shares: Nat,
    pub timestamp: u64,
}

pub async fn get_amm_liquidity_events(
    amm: Principal,
    start: u64,
    length: u64,
) -> Result<Vec<AmmLiquidityEvent>, String> {
    let (events,): (Vec<AmmLiquidityEvent>,) =
        ic_cdk::call(amm, "get_amm_liquidity_events", (start, length))
            .await
            .map_err(|(code, msg)| format!("get_amm_liquidity_events: {:?} {}", code, msg))?;
    Ok(events)
}

pub async fn get_amm_liquidity_event_count(amm: Principal) -> Result<u64, String> {
    let (count,): (u64,) = ic_cdk::call(amm, "get_amm_liquidity_event_count", ())
        .await
        .map_err(|(code, msg)| format!("get_amm_liquidity_event_count: {:?} {}", code, msg))?;
    Ok(count)
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct AmmPoolInfo {
    pub pool_id: String,
    pub token_a: Principal,
    pub token_b: Principal,
    pub reserve_a: Nat,
    pub reserve_b: Nat,
    pub total_lp_shares: Nat,
    // Other fields (fee_bps, protocol_fee_bps, curve, paused) ignored —
    // pricing only needs reserves + supply. Optional means decoding skips
    // them; absence here doesn't break a candid extractor that orders
    // record fields.
}

pub async fn get_pools(amm: Principal) -> Result<Vec<AmmPoolInfo>, String> {
    let (pools,): (Vec<AmmPoolInfo>,) = ic_cdk::call(amm, "get_pools", ())
        .await
        .map_err(|(code, msg)| format!("get_pools: {:?} {}", code, msg))?;
    Ok(pools)
}
