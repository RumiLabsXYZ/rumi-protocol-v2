//! Placeholder for `ChainConfig`, `ChainId`, `ChainStatus`. Real types in Task 3.

use candid::{CandidType, Deserialize};
use serde::Serialize;

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ChainId(pub u32);

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChainStatus { Registered, Disabled }

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct ChainConfig {
    pub chain_id: ChainId,
    pub display_name: String,
}
