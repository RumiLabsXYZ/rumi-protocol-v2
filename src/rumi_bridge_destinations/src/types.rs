//! Typed destination-profile schema (design section 9.1 to 9.3).
//!
//! Every struct rejects unknown fields. Enum vocabularies are wider than what
//! groundwork accepts (for example `Status` has `Staged` and `Enabled`) so that
//! the validator, not the parser, is the place that says "not in this version".

use serde::{Deserialize, Serialize};

/// The only schema version this crate understands.
pub const SCHEMA_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// 9.1 Fact and Decision wrappers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Primary,
    Secondary,
    Repo,
    Derived,
}

/// A sourced fact. `source_url` is an `https` URL, or a repo-relative path
/// when `source_kind` is `repo`. `verified` is `YYYY-MM-DD`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fact<T> {
    pub value: T,
    pub source_url: String,
    pub verified: String,
    pub source_kind: SourceKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionStatus {
    /// A recommendation that nobody has settled yet.
    Recommendation,
    /// A settled planning assumption (design section 12). It never activates anything.
    PlanningDecision,
}

/// A design choice. Neither status activates anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision<T> {
    pub value: T,
    pub rationale: String,
    pub status: DecisionStatus,
}

// ---------------------------------------------------------------------------
// 9.2 Profile
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    Evm,
    Solana,
    Sui,
    Xrpl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TopologyKind {
    BridgeInstance,
    BackendModule,
}

/// Existing work a profile builds on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExistingWork {
    /// Open PR #586 (XRPL icUSD lane in the backend).
    #[serde(rename = "pr586")]
    Pr586,
    /// The reviewed Base bridge canister (`src/rumi_icusd_bridge`).
    #[serde(rename = "rumi_icusd_bridge")]
    RumiIcusdBridge,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Topology {
    pub kind: Decision<TopologyKind>,
    pub wasm_family: Option<String>,
    pub existing_work: Option<Fact<ExistingWork>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Standard {
    #[serde(rename = "erc20")]
    Erc20,
    #[serde(rename = "spl_token_2022")]
    SplToken2022,
    #[serde(rename = "spl_token")]
    SplToken,
    #[serde(rename = "sui_currency")]
    SuiCurrency,
    #[serde(rename = "xrpl_iou")]
    XrplIou,
    #[serde(rename = "xrpl_mpt")]
    XrplMpt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backing {
    EscrowLockedIcusd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Encoding {
    #[serde(rename = "u64")]
    U64,
    #[serde(rename = "int63")]
    Int63,
    #[serde(rename = "xrpl_decimal_15")]
    XrplDecimal15,
}

/// Strict canonical decimal `u64`: ASCII digits only, no sign, no whitespace,
/// no leading zeros (except `"0"`), and no value above `u64::MAX`.
pub fn parse_u64_decimal(s: &str) -> Result<u64, String> {
    if s.is_empty() {
        return Err("empty amount string".to_string());
    }
    if !s.bytes().all(|c| c.is_ascii_digit()) {
        return Err(format!("amount {s:?} is not plain ASCII digits"));
    }
    if s.len() > 1 && s.starts_with('0') {
        return Err(format!("amount {s:?} has a leading zero"));
    }
    s.parse::<u64>()
        .map_err(|_| format!("amount {s:?} is above u64::MAX"))
}

/// Serde adapter: a `u64` that travels as a canonical decimal string.
mod u64_string {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &u64, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&v.to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
        let s = String::deserialize(d)?;
        super::parse_u64_decimal(&s).map_err(serde::de::Error::custom)
    }
}

/// Serde adapter: an `Option<u64>` that travels as a canonical decimal string or null.
mod opt_u64_string {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &Option<u64>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(x) => s.serialize_str(&x.to_string()),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
        match Option::<String>::deserialize(d)? {
            Some(s) => super::parse_u64_decimal(&s)
                .map(Some)
                .map_err(serde::de::Error::custom),
            None => Ok(None),
        }
    }
}

/// Amount limits in e8s. In JSON every amount is a canonical decimal STRING
/// (for example `"18446744073709551615"`), never a number, so a JavaScript
/// `JSON.parse` cannot round it. Rust parses the string into a checked `u64`
/// and rejects signs, leading zeros, empty or non-digit text and overflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AmountBounds {
    #[serde(with = "u64_string")]
    pub min_e8s: u64,
    /// Largest single operation.
    #[serde(with = "u64_string")]
    pub max_e8s: u64,
    /// Largest total outstanding. Required for `xrpl_decimal_15`; `None` where
    /// the per-instance supply cap is configuration, not a protocol limit.
    #[serde(with = "opt_u64_string")]
    pub max_outstanding_e8s: Option<u64>,
    pub encoding: Encoding,
    /// Why the bound is what it is, when the basis needs saying (for example
    /// Sui's conservative bound).
    pub basis: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Representation {
    pub standard: Decision<Standard>,
    pub backing: Backing,
    pub debt_creating: bool,
    pub decimals: u8,
    pub amount_bounds: Fact<AmountBounds>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scheme {
    EcdsaSecp256k1,
    SchnorrEd25519,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyNames {
    pub mainnet: String,
    pub testnet: String,
}

/// Chain-qualified domain strings (design section 5.1). `None` where the
/// identifier is computed elsewhere (for example inside the EVM contract).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Domains {
    pub mint_id: Option<String>,
    pub remint_id: Option<String>,
    pub return_id: Option<String>,
    /// Custodied-return families only (Solana); atomic burns have no burn id.
    pub burn_id: Option<String>,
    pub escrow_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Signing {
    pub scheme: Scheme,
    pub key_name: Fact<KeyNames>,
    /// Path template. It carries exactly one network element: `{network_identity}`
    /// (chain identity), or `{network_name}` for XRPL, whose PR #586 path uses
    /// the network names "mainnet" and "testnet". It carries one `{lane}` or
    /// `{role}` element when `roles` is not empty.
    pub derivation_domain: Vec<String>,
    /// Separate keys (least privilege) that fill the `{lane}` or `{role}` element.
    pub roles: Vec<String>,
    pub domains: Domains,
    pub signed_payload: Fact<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WalletStandard {
    #[serde(rename = "eip1193_eip6963")]
    Eip1193Eip6963,
    #[serde(rename = "solana_wallet_standard")]
    SolanaWalletStandard,
    #[serde(rename = "sui_wallet_standard")]
    SuiWalletStandard,
    #[serde(rename = "vendor_specific")]
    VendorSpecific,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wallet {
    pub standard: WalletStandard,
    pub named: Fact<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prerequisite {
    pub id: String,
    pub text: String,
    pub unavoidable: bool,
}

/// An open decision (design section 12) recorded as a recommendation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenDecision {
    /// `D1` to `D9`.
    pub id: String,
    pub topic: String,
    pub choice: Decision<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub schema_version: u32,
    pub id: String,
    pub display_name: String,
    pub family: Family,
    pub topology: Topology,
    pub representation: Representation,
    pub signing: Signing,
    pub wallet: Wallet,
    pub prerequisites: Vec<Fact<Prerequisite>>,
    pub open_decisions: Vec<OpenDecision>,
    pub networks: Vec<Network>,
}

// ---------------------------------------------------------------------------
// 9.3 Network record
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Mainnet,
    Testnet,
    Devnet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityKind {
    EvmChainId,
    SolanaGenesisHash,
    SuiChainIdentifier,
    XrplNetworkId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub kind: IdentityKind,
    pub value: String,
}

/// Activation state. Groundwork accepts `Disabled` only; the other variants
/// exist so the validator can reject them by name (rule V2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Disabled,
    Staged,
    Enabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockerKind {
    Config,
    Code,
    External,
    Decision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Blocker {
    pub id: String,
    pub text: String,
    pub kind: BlockerKind,
    pub source: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Deployment {
    None,
    DeployedUnbound,
    Bound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MintAuthority {
    None,
    CanisterOnly,
    Other,
}

/// Distinct claims (design section 8). Each needs its own evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Claims {
    pub designed: bool,
    pub deployment: Deployment,
    pub mint_authority: MintAuthority,
    pub mint_enabled: bool,
    pub finality_checked: bool,
    pub settlement_proven: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinalityRule {
    EvmTag,
    MonadVerified,
    SolanaCommitment,
    SuiCheckpoint,
    XrplValidated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FinalityParameters {
    /// EVM block tag the bridge reads at (`finalized`); also the base head for Monad.
    pub tag: Option<String>,
    /// Solana commitment level (`finalized`).
    pub commitment: Option<String>,
    /// Sui checkpoint state every read is pinned to (`finalized`).
    pub checkpoint: Option<String>,
    /// XRPL `ledger_index` used for reads (`validated`).
    pub ledger_index: Option<String>,
    /// XRPL transaction result required before crediting (`tesSUCCESS`).
    pub required_result: Option<String>,
    /// Monad only: blocks between Finalized and Verified.
    pub execution_delay_blocks: Option<u32>,
    pub typical_wait: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Finality {
    pub rule: FinalityRule,
    pub parameters: Fact<FinalityParameters>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RpcPath {
    EvmRpcCanister,
    SolRpcCanister,
    HttpsOutcallGraphql,
    HttpsOutcallJsonrpc,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rpc {
    pub path: RpcPath,
    /// EVM RPC canister built-in provider variant, when the chain has one.
    pub native_variant: Option<Fact<String>>,
    pub min_quorum: Decision<u8>,
    pub min_independent_providers: u8,
    pub historical_state_required: bool,
    /// Host names only: no scheme, port, path, query or key.
    pub endpoints: Fact<Vec<String>>,
    pub outcall_notes: Fact<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Network {
    pub network_id: String,
    pub role: Role,
    pub identity: Fact<Identity>,
    pub status: Status,
    /// `None` in groundwork; later the instance canister principal.
    pub instance_ref: Option<String>,
    pub activation_blockers: Vec<Blocker>,
    pub claims: Claims,
    pub finality: Finality,
    pub rpc: Rpc,
}
