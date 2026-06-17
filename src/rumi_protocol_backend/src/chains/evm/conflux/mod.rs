//! Conflux eSpace rail. EVM-compatible, so it reuses the shared `chains::evm`
//! modules (tx, tecdsa, evm_rpc, settlement, deposit_watch) and the chain-aware
//! dev-gated vault endpoints. This module currently holds only the per-chain
//! config; there is no per-chain vault wrapper because the open/withdraw/close
//! endpoints read the price symbol and min-CR from the per-chain configs.

pub mod config;
