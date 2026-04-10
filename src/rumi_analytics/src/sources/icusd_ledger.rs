//! Typed wrapper around icusd_ledger ICRC-1 queries.

use candid::{CandidType, Nat, Principal};
use serde::Deserialize;

pub async fn icrc1_total_supply(ledger: Principal) -> Result<u128, String> {
    let res: Result<(Nat,), _> = ic_cdk::api::call::call(ledger, "icrc1_total_supply", ()).await;
    match res {
        Ok((nat,)) => nat
            .0
            .try_into()
            .map_err(|e| format!("icrc1_total_supply nat -> u128: {}", e)),
        Err((code, msg)) => Err(format!("icrc1_total_supply: {:?} {}", code, msg)),
    }
}

// --- ICRC-3 block tailing (Phase 4) ---

pub use icrc_ledger_types::icrc::generic_value::ICRC3Value;

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct GetBlocksArg {
    pub start: Nat,
    pub length: Nat,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct BlockWithId {
    pub id: Nat,
    pub block: ICRC3Value,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct GetBlocksResult {
    pub log_length: Nat,
    pub blocks: Vec<BlockWithId>,
    // archived_blocks omitted for Phase 4
}

pub async fn icrc3_get_blocks(
    ledger: Principal,
    start: u64,
    length: u64,
) -> Result<GetBlocksResult, String> {
    let args = vec![GetBlocksArg {
        start: Nat::from(start),
        length: Nat::from(length),
    }];
    let (result,): (GetBlocksResult,) = ic_cdk::call(ledger, "icrc3_get_blocks", (args,))
        .await
        .map_err(|(code, msg)| format!("icrc3_get_blocks: {:?} {}", code, msg))?;
    Ok(result)
}
