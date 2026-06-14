//! Native XRP Ledger rail (mirrors `chains::solana`).
//!
//! XRP is closest to Solana: threshold Ed25519 (the same Schnorr key, a distinct
//! derivation path) plus RAW HTTPS outcalls to a public `rippled` JSON-RPC node,
//! with a hand-rolled binary codec locked by golden vectors (`testdata/
//! xrp_kat.json`). There is no XRP RPC canister on the IC, so the canister builds
//! and signs the XRPL transaction itself and talks to rippled directly.
//!
//! Scope (field guide "Phase 1"): derive an address, read balance/sequence/
//! reserve, send native XRP (exact + max), with an optional destination tag.
//! Out of scope until later (wiring): issued tokens / trust lines, the XRPL DEX,
//! dynamic fees, multi-node consensus, the observer/settlement timers, and the
//! `main.rs` `#[query]` transform shims that activate the outcalls.
//!
//! Like the rest of `chains::`, this rail is EXPERIMENTAL and dormant: it compiles
//! into the backend and is fully unit-tested, but nothing calls into it from the
//! production endpoints yet. See the banner in `chains::mod`.

pub mod address;
pub mod adapter;
pub mod codec;
pub mod config;
pub mod sign;
pub mod ted25519;
pub mod xrp_rpc;

pub use address::{
    account_id_from_classic_address, classic_address_from_ed25519_pubkey, is_valid_classic_address,
};
pub use adapter::XrpAdapter;
pub use codec::{serialize_signed, serialize_unsigned, Payment};
pub use config::{XRP_CHAIN_ID, XRP_NATIVE_DECIMALS};
pub use sign::{ed25519_signing_pubkey, signing_message, tx_hash};
