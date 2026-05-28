//! Chain-agnostic adapter trait. Every foreign chain implements this trait
//! in its own module (`chains::monad`, `chains::solana`, ...). Phase 1a
//! ships the trait only; Phase 1b adds the first impl (Monad).

use super::config::ChainId;
use async_trait::async_trait;
use candid::{CandidType, Deserialize, Principal};
use serde::Serialize;

/// Per-chain operations the protocol relies on.
///
/// `?Send` because canister code is single-threaded; relaxing the bound
/// keeps adapters free to hold ICP-side stable-memory handles without
/// going through `Arc` indirection.
#[async_trait(?Send)]
pub trait ChainAdapter {
    fn chain_id(&self) -> ChainId;

    async fn verify_deposit(&self, tx_hash: &str) -> Result<DepositRecord, ChainAdapterError>;

    async fn sign_withdrawal(&self, req: WithdrawalRequest) -> Result<SignedWithdrawal, ChainAdapterError>;

    async fn sign_mint(&self, instr: MintInstruction) -> Result<SignedMint, ChainAdapterError>;

    async fn sign_burn(&self, amount_e8s: u128, burner: Principal) -> Result<SignedBurn, ChainAdapterError>;

    async fn fetch_finality(&self) -> Result<FinalitySnapshot, ChainAdapterError>;

    async fn observe_event(&self, from_block: u64) -> Result<Vec<DepositRecord>, ChainAdapterError>;
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct DepositRecord {
    pub depositor: String,
    pub amount_e8s: u128,
    pub block_number: u64,
    pub tx_hash: String,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct WithdrawalRequest {
    pub recipient: String,
    pub amount_e8s: u128,
    pub idempotency_key: String,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct SignedWithdrawal {
    pub raw_tx: Vec<u8>,
    pub tx_hash: String,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct MintInstruction {
    pub recipient: String,
    pub amount_e8s: u128,
    pub vault_id: u64,
    pub idempotency_key: String,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct SignedMint {
    pub raw_tx: Vec<u8>,
    pub tx_hash: String,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct SignedBurn {
    pub raw_tx: Vec<u8>,
    pub tx_hash: String,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct FinalitySnapshot {
    pub latest_block: u64,
    pub finalized_block: u64,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub enum ChainAdapterError {
    NotImplemented,
    RpcError { provider: String, message: String },
    SignatureFailed(String),
    InsufficientFinality { latest: u64, required: u64 },
    InvalidPayload(String),
}
