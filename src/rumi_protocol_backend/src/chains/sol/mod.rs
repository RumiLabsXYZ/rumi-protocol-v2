//! Native-SOL-COLLATERAL rail (mirrors `chains::xrp`).
//!
//! This module is the CDP-collateral integration: a `CollateralConfig` row
//! with `custody_kind = NativeSol`, keyed under a synthetic principal
//! (`state::sol_collateral_principal`), per-vault threshold-Ed25519 custody
//! addresses, open-then-verify deposits, and claim-based payouts settled via
//! a durable-nonce transfer — all plugged into the ordinary IC-native `Vault`
//! machinery. icUSD stays entirely IC-native on this rail; SOL never leaves
//! the protocol except through a settled `SolClaim`.
//!
//! ## NOT the same product as `chains::solana`
//! `chains::solana` is a DIFFERENT, dormant rail that mints icUSD AS AN SPL
//! TOKEN ON Solana via `ChainAdapter` / `MultiChainState` / `SettlementQueue`
//! (SPL `MintTo`, associated-token-account creation, a supply gate in
//! `deposit_watch.rs`). None of that SPL-mint machinery is in scope here or
//! used by this module. What `chains::sol` DOES reuse from `chains::solana`
//! is only its PURE, cluster-agnostic Solana primitives: the legacy-message /
//! instruction builders and wire-assembly helpers in `tx.rs`, the JSON-RPC
//! parsers and candid type mirrors in `sol_rpc.rs`, and the Schnorr candid
//! structs in `ted25519.rs`. See each submodule's doc comment for exactly
//! what is reused vs. written fresh.
//!
//! Like the rest of `chains::`, and like `chains::xrp`, this rail is
//! EXPERIMENTAL and dormant at this phase (Phase 1 / foundation): it compiles
//! into the backend and is fully unit-tested, but nothing in `main.rs` or
//! `vault.rs` calls into it yet — the endpoints, deposit/withdraw wiring, and
//! Stability Pool opt-in land in a later phase. See the banner in
//! `chains::mod`.

pub mod adapter;
pub mod address;
pub mod config;
pub mod rpc;
pub mod ted25519;

pub use address::{decode_sol_address, is_valid_sol_address};
pub use config::{SOL_CHAIN_ID, SOL_NATIVE_DECIMALS};
