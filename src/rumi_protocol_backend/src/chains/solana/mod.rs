//! Solana integration (mirrors `chains::monad`). M1 ships config, Ed25519
//! address derivation, and read-only SOL RPC access. Signing, vaults, and
//! timers land in M2+.
//!
//! Seam approach: hand-rolled on ic-cdk 0.12 (the typed `sol_rpc_client` /
//! `sol_rpc_types` crates require ic-cdk 0.20, a hard conflict with this
//! project's pin, the same wall Monad hit with `evm_rpc_client`). See
//! `docs/superpowers/specs/2026-06-01-solana-integration-design.md`.

pub mod config;
pub mod sol_rpc;
pub mod ted25519;

#[cfg(test)]
mod tests_config;
#[cfg(test)]
mod tests_sol_rpc;
#[cfg(test)]
mod tests_ted25519;
