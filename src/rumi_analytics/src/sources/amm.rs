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

// --- AMM1 (3USD/ICP) APY inputs ---

/// One day's icUSD reward stream for an AMM pool (e8s). `amount` is the running
/// total distributed so far for that day (the analytics side stores it
/// last-write-wins, so it is a total, not an increment).
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct DailyRewardPoint {
    pub day_start_ns: u64,
    pub amount: candid::Nat,
}

/// A TVL snapshot for an AMM pool, as returned by `get_amm_tvl_series`.
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct TvlSample {
    pub pool_id: String,
    pub timestamp: u64,
    pub reserve_a: Nat,
    pub reserve_b: Nat,
    pub price_a_e8s: Nat,
    pub price_b_e8s: Nat,
    pub tvl_usd_e8s: Nat,
}

/// Recent TVL series for a pool (one sample per snapshot over `window_days`).
pub async fn get_amm_tvl_series(
    amm: Principal,
    pool_id: String,
    window_days: u32,
) -> Result<Vec<TvlSample>, String> {
    let (series,): (Vec<TvlSample>,) =
        ic_cdk::call(amm, "get_amm_tvl_series", (pool_id, window_days))
            .await
            .map_err(|(code, msg)| format!("get_amm_tvl_series: {:?} {}", code, msg))?;
    Ok(series)
}

/// Recent daily icUSD-reward series for a pool over `window_days`.
pub async fn get_amm_reward_series(
    amm: Principal,
    pool_id: String,
    window_days: u32,
) -> Result<Vec<DailyRewardPoint>, String> {
    let (series,): (Vec<DailyRewardPoint>,) =
        ic_cdk::call(amm, "get_amm_reward_series", (pool_id, window_days))
            .await
            .map_err(|(code, msg)| format!("get_amm_reward_series: {:?} {}", code, msg))?;
    Ok(series)
}

/// Resolve the pool_id of the AMM1 (3USD/ICP) pool by matching the 3USD principal
/// (which IS the rumi_3pool canister) against each pool's token_a / token_b.
///
/// We do NOT hardcode a pool_id string (e.g. "3USD_ICP"): the AMM derives pool
/// ids from token principals, so a literal won't match.
pub async fn resolve_amm1_pool_id(amm: Principal, threeusd: Principal) -> Result<String, String> {
    let pools = get_pools(amm).await?;
    pools
        .into_iter()
        .find(|p| p.token_a == threeusd || p.token_b == threeusd)
        .map(|p| p.pool_id)
        .ok_or_else(|| format!("no AMM pool found for 3USD principal {}", threeusd))
}
