//! Monad (and future foreign-chain) vault record.
//!
//! Lives in `MultiChainStateV2.chain_vaults`, keyed by the globally-unique
//! u64 vault_id. The core ICP-native `Vault` struct is untouched in Phase 1b;
//! unifying the two models is a deliberate Phase 2 task.
//!
//! Design B (confirmed-supply): `debt_e8s` is the CONFIRMED debt. While a mint
//! is in flight, the intended amount lives in `pending_mint_e8s` and does NOT
//! count toward `total_debt` or `chain_supplies` until the on-chain mint is
//! observed at finality (settlement worker, Task 10).

use crate::chains::config::ChainId;
use candid::{CandidType, Deserialize, Principal};
use serde::Serialize;

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub enum ChainVaultStatus {
    MintPending,
    Open,
    Closing,
    Closed,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct ChainVaultV1 {
    pub vault_id: u64,
    pub owner: Principal,
    pub collateral_chain: ChainId,
    pub custody_address: String,
    pub collateral_amount_e18: u128,
    pub debt_e8s: u128,
    pub mint_recipient: String,
    pub pending_mint_e8s: u128,
    pub status: ChainVaultStatus,
    pub opened_at_ns: u64,
}
