//! Reusable test harness for the Rumi ICP icUSD <-> Base icUSD bridge.
//!
//! Include from an integration test with `mod support;`. Nothing here depends on
//! the `rumi_icusd_bridge` library: the bridge is exercised purely through its
//! `.wasm` (see `pic::bridge_wasm`).
//!
//! * [`anvil`]    : spawn a local anvil node, blocking JSON-RPC client.
//! * [`contract`] : deploy and drive foundry's `BridgedIcUSD` on anvil.
//! * [`outcalls`] : PocketIC HTTPS-outcall pump, provider router, fault injection.
//! * [`ledger`]   : the real ICRC ledger wasm with ICRC-2 and a small archive threshold.
//! * [`pic`]      : PocketIC construction and canister-install helpers.
//! * [`relay`]    : client types and helpers for the `evm_rpc_relay` test canister.

#![allow(dead_code)]

pub mod anvil;
pub mod bridge;
pub mod contract;
pub mod ledger;
pub mod outcalls;
pub mod pic;
pub mod relay;
