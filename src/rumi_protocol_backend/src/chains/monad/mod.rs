//! Monad adapter (Phase 1b). First real chain integration.
//!
//! Implements the Phase 1a `ChainAdapter` trait against Monad testnet
//! (chain id 10143) using tECDSA for signing and the EVM RPC canister for
//! reads and `eth_sendRawTransaction`. Supply accounting is confirmed-supply
//! (Design B): `chain_supplies` and vault debt move only when an on-chain
//! mint/burn is observed at finality.

pub mod adapter;
pub mod chain_vault;
pub mod config;
pub mod deposit_watch;
pub mod evm_rpc;
pub mod settlement;
pub mod tecdsa;
pub mod tx;

pub use adapter::MonadAdapter;
pub use chain_vault::{ChainVaultStatus, ChainVaultV1};
pub use config::{monad_default_register_arg, monad_ecdsa_key_name, MONAD_CHAIN_ID, MONAD_ICUSD_DECIMALS};

#[cfg(test)]
mod tests_config;

#[cfg(test)]
mod tests_chain_vault;
