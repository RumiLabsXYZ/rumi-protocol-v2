//! Monad adapter (Phase 1b). First real chain integration.
//!
//! Implements the Phase 1a `ChainAdapter` trait against Monad testnet
//! (chain id 10143) using tECDSA for signing and the EVM RPC canister for
//! reads and `eth_sendRawTransaction`. Supply accounting is confirmed-supply
//! (Design B): `chain_supplies` and vault debt move only when an on-chain
//! mint/burn is observed at finality.

pub mod chain_vault;
pub mod config;

// Generic EVM logic now lives in chains::evm; re-export so existing
// crate::chains::monad::{tx,tecdsa,...} paths keep resolving (mirrors the
// chains/vault.rs <- chains/monad/chain_vault.rs hoist).
pub use crate::chains::evm::{adapter, burn_proof, deposit_watch, evm_rpc, hardening, settlement, tecdsa, tx};

pub use adapter::MonadAdapter;
pub use chain_vault::{ChainVaultStatus, ChainVaultV1};
pub use config::{monad_default_register_arg, monad_ecdsa_key_name, MONAD_CHAIN_ID, MONAD_ICUSD_DECIMALS};

#[cfg(test)]
mod tests_config;

#[cfg(test)]
mod tests_chain_vault;

#[cfg(test)]
mod tests_open_vault;

#[cfg(test)]
mod tests_withdraw;
