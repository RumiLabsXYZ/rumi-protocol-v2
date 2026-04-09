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
