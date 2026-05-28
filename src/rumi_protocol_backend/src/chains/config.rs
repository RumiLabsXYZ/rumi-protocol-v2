//! Per-chain configuration record.
//!
//! Versioned-snapshot pattern (see spec Section 3): the active shape is
//! `ChainConfigV1`. Adding a field = bump to `ChainConfigV2`, register a
//! migration in `crate::chains::supply::migrate_multi_chain_state`, and
//! rebind `pub type ChainConfig = ChainConfigV2;`. Never modify V1 in
//! place once it has shipped.

use candid::{CandidType, Deserialize};
use serde::Serialize;

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ChainId(pub u32);

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChainStatus {
    Registered,
    Disabled,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub enum GasStrategy {
    /// EIP-1559 EVM chains (Monad, Ethereum, L2s).
    EvmEip1559 {
        max_priority_fee_gwei: u64,
        max_fee_gwei_ceiling: u64,
    },
    /// Pre-EIP-1559 EVM (rare).
    EvmLegacy { gas_price_gwei_ceiling: u64 },
    /// Solana priority fee bidding.
    SolanaPriorityFee { lamports_per_cu_ceiling: u64 },
    /// No fee model needed (read-only adapters; dev placeholders).
    NotApplicable,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct ChainConfigV1 {
    pub chain_id: ChainId,
    pub display_name: String,
    pub rpc_endpoints: Vec<String>,
    /// Blocks past head before a deposit/event is treated as committed.
    pub finality_depth: u32,
    pub gas_strategy: GasStrategy,
    /// Decimals of the chain-native gas asset (18 for EVM, 9 for Solana SOL).
    pub chain_native_decimals: u8,
    /// `ic_cdk::api::time()` nanoseconds when this config was first registered.
    pub registered_at_ns: u64,
    pub status: ChainStatus,
}

/// Active alias. Rebind to a later version when a field is added.
pub type ChainConfig = ChainConfigV1;

/// Caller-supplied registration payload. Distinct from the persisted
/// `ChainConfigV1` so the admin endpoint can fill `registered_at_ns` and
/// `status` server-side without trusting the caller.
#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct RegisterChainArg {
    pub chain_id: ChainId,
    pub display_name: String,
    pub rpc_endpoints: Vec<String>,
    pub finality_depth: u32,
    pub gas_strategy: GasStrategy,
    pub chain_native_decimals: u8,
}

/// Operator-supplied update payload. Every field is optional; omitted
/// fields are left unchanged.
#[derive(CandidType, Deserialize, Serialize, Clone, Debug, Default)]
pub struct UpdateChainConfigArg {
    pub display_name: Option<String>,
    pub rpc_endpoints: Option<Vec<String>>,
    pub finality_depth: Option<u32>,
    pub gas_strategy: Option<GasStrategy>,
}

/// Reasons a `register_chain`/`set_chain_config` call can be rejected.
#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub enum ChainAdminError {
    NotDeveloper,
    ChainAlreadyRegistered(ChainId),
    ChainNotRegistered(ChainId),
    InvalidConfig(String),
}