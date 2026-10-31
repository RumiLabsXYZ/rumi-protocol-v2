//! Candid-facing types of the Sui bridge instance (BB-52 skeleton).

use crate::graphql::MirrorSettings;
use crate::ids::{SuiNetwork, SuiRole};
use candid::{CandidType, Deserialize};

/// Install and upgrade argument.
#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum BridgeArg {
    Init(InitArgs),
    Upgrade(UpgradeArgs),
}

/// One canister serves one network. Both fields are required and must agree: the 64-hex
/// chain identifier must be the genesis checkpoint digest of the declared network (only Sui
/// mainnet and testnet are accepted; anything else is refused, never guessed).
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct InitArgs {
    pub network: SuiNetwork,
    /// 64 lowercase hex digits: the genesis checkpoint digest.
    pub chain_identifier: String,
    /// Provider set and quorum. Optional, and disabled by default.
    pub mirror: Option<MirrorSettings>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct UpgradeArgs {
    /// Replaces the provider set when present. Nothing else can change at upgrade: the network
    /// and the gate binding are fixed for the life of the instance.
    pub mirror: Option<MirrorSettings>,
}

/// The exact identities of the deployed gate (`move/rumi_icusd_gate`), as the operator states
/// them. They are configuration, not facts the canister has observed: the bind check that
/// reads the chain belongs to BB-53.
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct GateBindingArg {
    /// Each id is `0x` followed by 64 lowercase hex digits.
    pub package_id: String,
    pub gate_id: String,
    pub mint_cap_id: String,
    pub admin_cap_id: String,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct GateBindingView {
    pub package_id: String,
    pub gate_id: String,
    pub mint_cap_id: String,
    pub admin_cap_id: String,
    /// `<package>::rumi_icusd_gate::Gate`
    pub gate_type: String,
    /// `<package>::rumi_icusd_gate::RUMI_ICUSD_GATE`
    pub coin_type: String,
    pub decimals: u8,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct RoleView {
    pub role: SuiRole,
    /// The threshold-Schnorr derivation path, each element in hex.
    pub derivation_path: Vec<String>,
    /// 32-byte Ed25519 public key in hex, once derived.
    pub public_key: Option<String>,
    /// The Sui address (`0x` plus 64 hex), once derived.
    pub address: Option<String>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ConfigView {
    pub initialized: bool,
    pub network: SuiNetwork,
    pub chain_identifier: String,
    /// The RPC form: the first four bytes of the chain identifier.
    pub short_chain_id: String,
    pub schnorr_key_name: String,
    pub instance_salt: String,
    pub binding: Option<GateBindingView>,
    pub mirror: MirrorSettings,
    pub bridge_version: String,
    pub schema_version: u32,
    /// Always false in the foundation: no endpoint signs or broadcasts anything.
    pub signing_enabled: bool,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum SuiBridgeError {
    Anonymous,
    Unauthorized,
    InvalidArgument(String),
    /// A set-once value is already set (to something else).
    Conflict(String),
    NotConfigured(String),
    SignerFailed(String),
}

pub type SuiResult<T> = Result<T, SuiBridgeError>;
