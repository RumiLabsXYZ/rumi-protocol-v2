//! Public Candid types. Internal stable records live in `state.rs`; views here are
//! projections of them.

use candid::{CandidType, Nat, Principal};
use icrc_ledger_types::icrc1::account::Account;
use serde::{Deserialize, Serialize};

pub use crate::accounting::Transition;
pub use crate::ids::LaneKind;

// ---------------------------------------------------------------------------
// Install arguments
// ---------------------------------------------------------------------------

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum BridgeArg {
    Init(InitArgs),
    Upgrade(UpgradeArgs),
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct InitArgs {
    /// Canonical icUSD ICRC-1/2/3 ledger.
    pub icusd_ledger: Principal,
    /// EVM RPC canister (`7hfb6-caaaa-aaaar-qadga-cai` on mainnet).
    pub evm_rpc: Principal,
    /// Threshold ECDSA key: `key_1` (production), `test_key_1`, or `dfx_test_key`.
    pub ecdsa_key_name: String,
    /// 8453 (Base), 84532 (Base Sepolia) or 31337 (local anvil, non-`key_1` only).
    pub base_chain_id: u64,
    pub admins: Vec<Principal>,
    /// Destination for swept fees. When absent, sweeps are refused.
    pub fee_recipient: Option<Account>,
    pub rpc_services: Vec<RpcServiceConfig>,
    /// Number of identical provider answers required (>= 1, <= number of services).
    pub min_quorum: u8,
}

#[derive(CandidType, Deserialize, Clone, Debug, Default)]
pub struct UpgradeArgs {
    /// Optional replacement admin list. Upgrades are controller-only, so this is a
    /// controller action.
    pub admins: Option<Vec<Principal>>,
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum L2MainnetService {
    Alchemy,
    Ankr,
    BlockPi,
    PublicNode,
    Llama,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub enum RpcServiceConfig {
    /// EVM RPC canister built-in Base mainnet provider.
    BaseMainnet(L2MainnetService),
    /// A custom JSON-RPC URL (Base Sepolia, local tests). No headers: secrets would be
    /// visible in canister state.
    Custom { url: String },
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct BridgeLimits {
    pub min_deposit_e8s: u64,
    pub max_deposit_e8s: u64,
    /// Cap on `O + PM + reserved pulls`.
    pub max_outstanding_e8s: u64,
    /// Maximum deposits admitted per window.
    pub mint_window_limit_e8s: u64,
    /// Maximum releases executed per window.
    pub release_window_limit_e8s: u64,
    pub window_seconds: u64,
    pub max_active_per_caller: u32,
    pub max_active_global: u32,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct BridgeFees {
    /// Deducted from the minted amount on ICP -> Base.
    pub deposit_fee_e8s: u64,
    /// Deducted from the released amount on Base -> ICP (in addition to the ledger fee).
    pub release_fee_e8s: u64,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct GasConfig {
    pub max_fee_per_gas_wei: Nat,
    pub max_priority_fee_per_gas_wei: Nat,
    pub mint_gas_limit: u64,
    pub admin_gas_limit: u64,
    /// Extra balance (wei) the signer must keep above `gas_limit * max_fee` (covers the
    /// Base L1 data fee).
    pub min_gas_reserve_wei: Nat,
    /// Age after which an unincluded attempt may be replaced (wall clock).
    pub replace_after_secs: u64,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Pauses {
    pub deposits: bool,
    pub mints: bool,
    pub releases: bool,
    pub observer: bool,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct ObserverConfig {
    /// Maximum block span per `eth_getLogs` request.
    pub logs_chunk_blocks: u64,
    pub logs_max_response_bytes: u64,
    pub default_max_response_bytes: u64,
    /// Timer interval while nothing is pending.
    pub idle_interval_secs: u64,
    /// Timer interval while deposits, releases, hints or transactions are pending.
    pub active_interval_secs: u64,
    /// Cycles attached to each EVM RPC canister `request` call.
    pub cycles_per_rpc_call: Nat,
}

// ---------------------------------------------------------------------------
// User calls
// ---------------------------------------------------------------------------

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BridgeToBaseArgs {
    /// Amount pulled into escrow, in e8s (`A`). The ledger fee is charged to the caller
    /// on top of this.
    pub amount_e8s: u64,
    /// `0x`-prefixed 20-byte Base address.
    pub base_recipient: String,
    pub from_subaccount: Option<serde_bytes::ByteBuf>,
    /// 16 to 32 bytes, unique per caller. Repeating it with identical arguments returns
    /// the existing operation.
    pub client_request_id: serde_bytes::ByteBuf,
    /// The deposit fee the caller was shown. Rejected if it no longer matches.
    pub expected_deposit_fee_e8s: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ListDepositsArgs {
    pub start_after: Option<u64>,
    pub limit: u32,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ListDepositsForOwnerArgs {
    pub owner: Principal,
    pub start_after: Option<u64>,
    pub limit: u32,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ListReleasesForSenderArgs {
    /// `0x`-prefixed 20-byte Base address that called `bridgeOut`.
    pub base_sender: String,
    pub start_after_sequence: Option<u64>,
    pub limit: u32,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ListReleasesArgs {
    pub owner: Principal,
    pub start_after_sequence: Option<u64>,
    pub limit: u32,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub enum ContractAdminAction {
    PauseMint,
    UnpauseMint,
    PauseBridgeOut,
    UnpauseBridgeOut,
    SetSupplyCap { cap_e8s: u64 },
    SetMintRateLimit { limit_e8s: u64, window_seconds: u64 },
    SetMinBridgeOut { minimum_e8s: u64 },
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum BridgeError {
    Anonymous,
    Unauthorized,
    NotConfigured(String),
    Paused(String),
    Halted(String),
    InvalidArgument(String),
    InvalidRecipient(String),
    AmountBelowMinimum { min_e8s: u64 },
    AmountAboveMaximum { max_e8s: u64 },
    CapacityExceeded { available_e8s: u64 },
    RateLimited { available_e8s: u64, window_resets_at_ns: u64 },
    FeeChanged { current_fee_e8s: u64 },
    /// The caller's ICRC-2 allowance to the bridge does not cover amount plus ledger fee.
    InsufficientAllowance { allowance_e8s: u64, required_e8s: u64 },
    /// The caller's icUSD balance does not cover amount plus ledger fee.
    InsufficientFunds { balance_e8s: u64, required_e8s: u64 },
    TooManyActiveOperations,
    RequestIdConflict,
    NotFound,
    NotOwner,
    InvalidState(String),
    Ledger(String),
    Rpc(String),
    Internal(String),
}

pub type BridgeResult<T> = Result<T, BridgeError>;

// ---------------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------------

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum DepositKindView {
    /// ICP -> Base deposit by `owner`.
    UserDeposit,
    /// Re-mint of a held bridge-out claim to its Base sender.
    Remint { transfer_id: String },
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum DepositStatus {
    PullPending,
    PullUnknown,
    PullFailed { reason: String },
    Escrowed,
    MintInFlight,
    Delivered,
    MintHeld { reason: String },
    Refunding,
    Refunded,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum BaseTxStage {
    /// Waiting for the lane (nonce not assigned yet).
    Queued,
    /// Signed and broadcast; no receipt yet.
    Submitted,
    /// A receipt exists at `latest` (sequencer inclusion).
    Included { block_number: u64 },
    /// The receipt block is at or below the quorum `finalized` head.
    Finalized { block_number: u64, success: bool },
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BaseInclusion {
    pub tx_hash: String,
    pub block_number: u64,
    pub block_hash: String,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct DepositView {
    pub id: u64,
    pub kind: DepositKindView,
    pub owner: Principal,
    pub from: Option<Account>,
    pub base_recipient: String,
    pub amount_e8s: u64,
    pub deposit_fee_e8s: u64,
    pub mint_amount_e8s: u64,
    pub mint_id: String,
    pub status: DepositStatus,
    pub client_request_id: serde_bytes::ByteBuf,
    pub pull_block_index: Option<u64>,
    pub refund_block_index: Option<u64>,
    pub refund_amount_e8s: Option<u64>,
    pub mint_tx_hashes: Vec<String>,
    pub mint_stage: Option<BaseTxStage>,
    pub delivered: Option<BaseInclusion>,
    pub created_at_ns: u64,
    pub updated_at_ns: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ReleaseStatus {
    Pending,
    TransferUnknown,
    Released,
    Held { reason: String },
    Reminting { deposit_id: u64 },
    Reminted { deposit_id: u64 },
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ReleaseView {
    pub transfer_id: String,
    pub sequence: u64,
    pub base_sender: String,
    pub amount_e8s: u64,
    pub release_fee_e8s: u64,
    pub ledger_fee_e8s: Option<u64>,
    pub amount_to_receive_e8s: Option<u64>,
    pub destination: Option<Account>,
    pub raw_icp_principal: serde_bytes::ByteBuf,
    pub icp_subaccount: serde_bytes::ByteBuf,
    pub base_tx_hash: String,
    pub base_block_number: u64,
    pub log_index: u64,
    pub status: ReleaseStatus,
    pub release_block_index: Option<u64>,
    pub created_at_ns: u64,
    pub updated_at_ns: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct HintView {
    pub tx_hash: String,
    pub submitted_by: Principal,
    pub submitted_at_ns: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ObserverStatus {
    pub cursor_block: u64,
    pub processed_count: u64,
    pub last_finalized_block: Option<u64>,
    pub last_error: Option<String>,
    pub last_run_ns: Option<u64>,
    pub pending_hints: Vec<HintView>,
    /// Set while the scan is waiting on a burn that in-flight mints explain (not a
    /// failure, so `last_error` stays empty): when the wait began. Cleared once the scan
    /// has moved past that burn.
    pub deferred_since_ns: Option<u64>,
    /// Why the scan is waiting, as of the latest pass.
    pub deferred_reason: Option<String>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct LaneStatus {
    pub lane: LaneKind,
    pub address: Option<String>,
    pub next_nonce: Option<u64>,
    pub unincluded_tx: Option<u64>,
    pub unfinalized_txs: u32,
    pub awaiting_gas: bool,
    pub halted: Option<String>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ContractBindingView {
    pub address: String,
    pub bound_at_ns: u64,
    pub start_block: u64,
    pub supply_cap_e8s: Nat,
    pub mint_window_limit_e8s: Nat,
    pub mint_window_seconds: u64,
    pub min_bridge_out_e8s: Nat,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BridgeInfo {
    pub version: String,
    pub chain_id: u64,
    pub icusd_ledger: Principal,
    pub escrow_account: Account,
    pub contract: Option<ContractBindingView>,
    pub limits: BridgeLimits,
    pub fees: BridgeFees,
    pub gas: GasConfig,
    pub observer_config: ObserverConfig,
    pub pauses: Pauses,
    pub halted: Option<String>,
    /// Human-readable settlement policy, e.g. "Base finalized (L1 batch finality), 2 of 3 providers".
    pub finality_policy: String,
    pub min_quorum: u8,
    pub rpc_service_count: u32,
    pub observer: ObserverStatus,
    pub lanes: Vec<LaneStatus>,
    pub ledger_fee_e8s: Option<u64>,
    pub active_deposits: u32,
    pub active_releases: u32,
    pub mint_window_used_e8s: u64,
    pub release_window_used_e8s: u64,
    pub window_resets_at_ns: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SupplyAudit {
    pub escrow_account: Account,
    /// icUSD ledger `icrc1_total_supply` at the last reconciliation (includes escrow).
    pub icp_ledger_total_supply_e8s: Option<Nat>,
    /// `icp_ledger_total_supply - escrow_balance`: ICP icUSD not locked as Base backing.
    pub circulating_on_icp_e8s: Option<Nat>,
    /// Base `totalSupply` at the last reconciliation (finalized head).
    pub circulating_on_base_e8s: Option<Nat>,
    pub escrow_balance_e8s: Option<Nat>,
    pub escrow_balance_checked_at_ns: Option<u64>,
    pub outstanding_base_e8s: Nat,
    pub pending_mint_e8s: Nat,
    pub pending_release_e8s: Nat,
    pub pending_refund_e8s: Nat,
    pub held_e8s: Nat,
    pub fees_e8s: Nat,
    pub reserved_pulls_e8s: Nat,
    pub unknown_outflow_e8s: Nat,
    pub liabilities_e8s: Nat,
    pub required_escrow_e8s: Nat,
    pub surplus_e8s: Option<Nat>,
    pub deficit_e8s: Option<Nat>,
    pub coverage_ok: Option<bool>,
    pub base_total_supply_e8s: Option<Nat>,
    pub base_supply_block: Option<u64>,
    pub inflight_mint_e8s: Nat,
    /// C2 and C4 together: Base `totalSupply` within outstanding plus in-flight mints, and the
    /// contract's cumulative `totalMinted` within the bridge's cumulative mints plus in flight.
    pub base_supply_within_bound: Option<bool>,
    pub base_supply_exact_match: Option<bool>,
    /// How to combine these figures with the ICP ledger and debt-backed chain supply
    /// without double counting.
    pub reporting_note: String,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BaseAddresses {
    pub minter: String,
    pub admin: String,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct AuditEntry {
    pub seq: u64,
    pub at_ns: u64,
    pub transition: Option<Transition>,
    pub deposit_id: Option<u64>,
    pub transfer_id: Option<String>,
    pub tx_id: Option<u64>,
    pub note: String,
}
