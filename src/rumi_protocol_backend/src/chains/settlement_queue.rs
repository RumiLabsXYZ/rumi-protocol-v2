//! Placeholder for the per-chain settlement queue. Real queue in Task 5.

use candid::{CandidType, Deserialize};
use serde::Serialize;

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, Default)]
pub struct SettlementQueueV1 {
    pub head: u64,
    pub tail: u64,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct SettlementOp {
    pub idempotency_key: String,
}
