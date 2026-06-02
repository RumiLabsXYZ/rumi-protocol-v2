//! Solana integration (mirrors `chains::monad`). M1 ships config, Ed25519
//! address derivation, and read-only SOL RPC access. Signing, vaults, and
//! timers land in M2+.
//!
//! Seam approach: hand-rolled on ic-cdk 0.12 (the typed `sol_rpc_client` /
//! `sol_rpc_types` crates require ic-cdk 0.20, a hard conflict with this
//! project's pin, the same wall Monad hit with `evm_rpc_client`). See
//! `docs/superpowers/specs/2026-06-01-solana-integration-design.md`.

pub mod adapter;
pub mod config;
pub mod deposit_watch;
pub mod hardening;
pub mod settlement;
pub mod sol_rpc;
pub mod ted25519;
pub mod tx;

#[cfg(test)]
mod tests_adapter;
#[cfg(test)]
mod tests_config;
#[cfg(test)]
mod tests_deposit_watch;
#[cfg(test)]
mod tests_settlement;
#[cfg(test)]
mod tests_sol_rpc;
#[cfg(test)]
mod tests_ted25519;
#[cfg(test)]
mod tests_tx;
