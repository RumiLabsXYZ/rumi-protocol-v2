//! Per-chain configuration record.
//!
//! Versioned-snapshot pattern (see spec Section 3): the active shape is
//! `ChainConfigV3`. Adding a field = bump to the next `ChainConfigV(N+1)`
//! (carry every prior field verbatim, decorate the new one with
//! `#[serde(default)]`), rebind `pub type ChainConfig = ...;`, and bump the
//! enclosing `MultiChainState` so its `chain_configs` value type follows. Never
//! modify a shipped version in place once it has been persisted.

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

/// Phase 1c snapshot. Carries every `ChainConfigV1` field verbatim (so a
/// V1-shaped CBOR sub-map maps each by name straight across) and adds the
/// notify-then-verify emergency poll flag.
///
/// `burn_watch_poll_enabled` carries `#[serde(default)]` so a `ChainConfigV1`
/// CBOR sub-map (which lacks this key entirely) decodes into `ChainConfigV2`
/// without error, defaulting the flag to `false`. The V1-carried fields are
/// NOT decorated because V1 always wrote them and they must be present in any
/// valid snapshot. State persists via ciborium (CBOR + serde, see
/// `storage.rs`), which decodes structs as field-name-keyed maps and fills a
/// missing key from `#[serde(default)]` rather than failing — this is the SAME
/// mechanism that makes the `MultiChainStateV1 -> V2` add-a-field decode safe
/// (proven by `tests_multi_chain_state_v2`). It is NOT a Candid `Decode!` of a
/// fixed record, so an added serde-default field cannot trip the AMM-style
/// state-wipe fallback (2026-05-18 incident).
///
/// Add the NEXT field by bumping to `ChainConfigV3` (keep V2 verbatim), adding
/// `#[serde(default)]` on the new field, and rebinding the alias below.
#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct ChainConfigV2 {
    // carried verbatim from V1 — always present in any valid snapshot
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
    /// Phase 1c: emergency continuous `eth_getLogs` burn-watch poll-scan toggle.
    /// `false` (default) = notify-then-verify only (the observer advances its
    /// cursor without scanning logs, and burns are applied via the pull-based
    /// `submit_burn_proof` endpoint). `true` = re-enable the legacy continuous
    /// scan for a targeted catch-up. Developer-gated via
    /// `set_burn_watch_poll_enabled`. New in V2 — `#[serde(default)]` lets a V1
    /// CBOR sub-map decode cleanly to `false`.
    #[serde(default)]
    pub burn_watch_poll_enabled: bool,
}

/// Phase 1d snapshot (audit M-05, QUORUM-2). Carries every `ChainConfigV2`
/// field verbatim (so a V2-shaped CBOR sub-map maps each by name straight
/// across) and adds the per-chain quorum-provider floor override.
///
/// `min_quorum_providers` carries `#[serde(default)]` so a `ChainConfigV2` CBOR
/// sub-map (which lacks this key entirely) decodes into `ChainConfigV3` without
/// error, defaulting the override to `None` (= use `DEFAULT_MIN_QUORUM_PROVIDERS`).
/// State persists via ciborium (CBOR + serde), which fills a missing key from
/// `#[serde(default)]` rather than failing — the SAME mechanism that made the
/// V1->V2 add-a-field decode safe (proven by `tests_config`). It is NOT a Candid
/// `Decode!` of a fixed record, so the added serde-default field cannot trip the
/// AMM-style state-wipe fallback (2026-05-18 incident).
///
/// Add the NEXT field by bumping to `ChainConfigV4` (keep V3 verbatim), adding
/// `#[serde(default)]` on the new field, and rebinding the alias below.
#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct ChainConfigV3 {
    // carried verbatim from V2 — always present in any valid snapshot
    pub chain_id: ChainId,
    pub display_name: String,
    pub rpc_endpoints: Vec<String>,
    pub finality_depth: u32,
    pub gas_strategy: GasStrategy,
    pub chain_native_decimals: u8,
    pub registered_at_ns: u64,
    pub status: ChainStatus,
    #[serde(default)]
    pub burn_watch_poll_enabled: bool,
    /// Audit M-05 (QUORUM-2): per-chain override for the minimum number of
    /// DISTINCT RPC providers that must agree before a financial read
    /// (balance / supply / block / receipt) is credited on an EVM chain.
    /// `None` (default) => use `DEFAULT_MIN_QUORUM_PROVIDERS` (3). A value of
    /// `Some(n)` lowers/raises the floor for a chain whose viable independent
    /// provider count differs (must be >= 1). Solana chains do NOT consult this
    /// (the SOL-RPC canister enforces its own consensus). New in V3 —
    /// `#[serde(default)]` lets a V2 CBOR sub-map decode cleanly to `None`.
    #[serde(default)]
    pub min_quorum_providers: Option<u32>,
}

/// Active alias. Rebind to a later version when a field is added.
pub type ChainConfig = ChainConfigV3;

/// Default minimum number of DISTINCT RPC providers that must agree on a
/// financial read before an EVM chain credits it (audit M-05 / QUORUM-2). A
/// deployment MUST configure at least this many independent endpoints to run the
/// observer / settlement workers (or perform a financial read); below the floor
/// the read FAILS CLOSED. Override per chain via
/// `ChainConfigV3::min_quorum_providers`.
pub const DEFAULT_MIN_QUORUM_PROVIDERS: u32 = 3;

/// The effective quorum-provider floor for a config: its override when set
/// (clamped to >= 1), else `DEFAULT_MIN_QUORUM_PROVIDERS`.
pub fn effective_min_quorum_providers(cfg: &ChainConfigV3) -> u32 {
    cfg.min_quorum_providers
        .map(|n| n.max(1))
        .unwrap_or(DEFAULT_MIN_QUORUM_PROVIDERS)
}

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
    /// Audit M-05 (QUORUM-2): optional per-chain quorum-provider floor override.
    /// `None`/omitted => use `DEFAULT_MIN_QUORUM_PROVIDERS` (3 distinct
    /// providers). Candid: `opt nat32`.
    #[serde(default)]
    pub min_quorum_providers: Option<u32>,
}

/// Operator-supplied update payload. Every field is optional; omitted
/// fields are left unchanged.
#[derive(CandidType, Deserialize, Serialize, Clone, Debug, Default)]
pub struct UpdateChainConfigArg {
    pub display_name: Option<String>,
    pub rpc_endpoints: Option<Vec<String>>,
    pub finality_depth: Option<u32>,
    pub gas_strategy: Option<GasStrategy>,
    /// Audit M-05 (QUORUM-2): set/clear the per-chain quorum-provider floor.
    /// Candid `opt opt nat32`: outer `None` (absent) => leave unchanged; outer
    /// `Some(None)` => clear back to the default; outer `Some(Some(n))` => set
    /// the floor to `n` (clamped to >= 1 at apply time).
    #[serde(default)]
    pub min_quorum_providers: Option<Option<u32>>,
}

/// Reasons a `register_chain`/`set_chain_config` call can be rejected.
#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub enum ChainAdminError {
    NotDeveloper,
    ChainAlreadyRegistered(ChainId),
    ChainNotRegistered(ChainId),
    InvalidConfig(String),
}