//! Shared setup and helpers for the ICP <-> Base icUSD bridge end to end scenarios
//! (BB-09b). `bridge_e2e_pic.rs` (BB-09b-1) and `bridge_faults_pic.rs` (BB-09b-2) are
//! both built on [`BridgeWorld`].
//!
//! # Build the wasms first (all local, offline)
//!
//! ```text
//! cargo build --offline --release --target wasm32-unknown-unknown \
//!     -p rumi_icusd_bridge -p evm_rpc_relay            # BB-09b-1
//! cargo build --offline --release --target wasm32-unknown-unknown -p flaky_ledger   # BB-09b-2 (extra)
//! (cd foundry && ~/.foundry/bin/forge build --offline)  # only if foundry/out is missing
//! ```
//!
//! Wasms are read from disk at runtime (never `include_bytes!`): `BRIDGE_WASM`, `RELAY_WASM`
//! and `FLAKY_LEDGER_WASM` override the defaults under
//! `<target>/wasm32-unknown-unknown/release/` (`<target>` is `$CARGO_TARGET_DIR` or
//! `<workspace>/target`). Also required: `POCKET_IC_BIN=<absolute path>` and anvil at
//! `$ANVIL_BIN` or `~/.foundry/bin/anvil`.
//!
//! Run the scenarios with
//! `POCKET_IC_BIN=$PWD/pocket-ic cargo test --offline -p rumi_icusd_bridge --test bridge_e2e_pic -- --test-threads=1`.
//!
//! # REAL versus MOCKED evidence
//!
//! * Everything routed through [`FaultAction::Pass`] (the default; no fault hook installed)
//!   is a passthrough to a local anvil node: Base transactions the bridge signs with its
//!   threshold-ECDSA minter key REALLY execute on a local EVM running the real
//!   `BridgedIcUSD` bytecode. Receipts, logs, balances and `finalized` heights read through
//!   the bridge's three fake providers are anvil's own.
//! * The ICP side is real too: the real `ic-icrc1-ledger` wasm (ICRC-2 on, fee 10_000 e8s),
//!   the real bridge wasm, and real threshold ECDSA from PocketIC's II subnet.
//! * Anything produced by [`FaultAction::ReplaceBody`], `HttpStatus`, `Reject`, `DelayOnce`
//!   (via `world.router.set_fault`) is MOCKED external-chain evidence. A test that installs
//!   such a hook must say so.
//! * [`BridgeWorld::impersonate_send`] (and the `contract_*_as_admin` helpers) simulate an
//!   ADMIN ACTION on the contract (the canister's admin address is impersonated by anvil so a
//!   key the test does not hold can act). They are test-only.
//!
//! # Time and ticks
//!
//! PocketIC time only moves when the test moves it. [`BridgeWorld::tick`] advances 5 s of IC
//! time and then runs `admin_run_tick` through the outcall pump. The canister also schedules
//! its own timer (1 s after install, then 30 s active / 300 s idle) which fires from the same
//! time advances, and a tick guard makes an overlapping `run_tick` return early, so a single
//! tick may do nothing. Use the `wait_*` / [`BridgeWorld::tick_until`] helpers, which loop for
//! a bounded number of ticks and dump diagnostics (bridge info, audit tail, provider log) on
//! timeout. Base finality is controlled by the test: anvil only mines when told to (and on
//! each transaction), `finalized = latest - 2`, so [`BridgeWorld::mine_finalize`] mines 2
//! blocks and makes everything mined so far final.
//!
//! # API summary (all `pub` on [`BridgeWorld`] unless noted)
//!
//! Setup: `BridgeWorld::setup(SetupOptions)`, `setup_with_ledger(opts, make_ledger)`,
//! `SetupOptions::default()`; free fns `bridge_wasm_bytes`, `relay_wasm_bytes`,
//! `flaky_ledger_wasm`, `default_limits`, `escrow_subaccount`, `hex0x`, `parse_hex32`.
//!
//! Raw calls: `update`, `query` (generic over Candid args/results), `admin_call`.
//!
//! Time: `tick`, `ticks(n)`, `tick_until(what, max, pred)`, `pump_idle()`, `advance_secs(s)`,
//! `dump()`.
//!
//! Waiters: `wait_deposit`, `wait_release`, `wait_mint_included`, `wait_delivered`,
//! `wait_released`.
//!
//! Flows: `approve_bridge`, `deposit_args`, `bridge_to_base`, `approve_and_deposit`,
//! `deposit_and_deliver`, `bridge_out_from`, `bridge_out_and_release`, `notify_hint`.
//!
//! Bridge views/admin: `bridge_info`, `supply_audit`, `reconcile`, `get_deposit`, `get_release`,
//! `list_my_deposits`, `list_releases_for_owner`, `audit_log_all`, `set_pauses`, `set_limits`,
//! `set_fees`, `request_refund`, `sweep_fees`, `upgrade_bridge_same_wasm`.
//!
//! Ledger: `ledger_balance`, `user_balance`, `escrow_balance`, `escrow_account`,
//! `ledger_total_supply`, `approve`, `ledger_transfer`.
//!
//! Base: `mine`, `mine_finalize`, `set_automine`, `mine_block`, `send_bridge_out_unmined`, `base_balance`, `base_total_supply`, `base_bridge_out_count`,
//! `base_mint_consumed`, `base_nonce`, `minter_nonce`, `receipt`, `holder(i)`,
//! `impersonate_send`, `contract_pause_mint_as_admin`, `contract_unpause_mint_as_admin`,
//! `contract_mint_paused`.

use super::anvil::{parse_hex_u64, Anvil, ANVIL_CHAIN_ID};
use super::contract::{
    decode_bridge_mint, decode_bridge_out, logs_with_topic, BridgeOutEvent, BridgedIcUsd, DeployArgs,
    TOPIC_BRIDGE_MINT, TOPIC_BRIDGE_OUT,
};
use super::ledger::{self, account, deploy_icrc_ledger, LedgerConfig, TRANSFER_FEE_E8S};
use super::outcalls::{
    await_with_outcalls, pump_until_idle, submit, ProviderRouter, PROVIDER_A, PROVIDER_B, PROVIDER_C,
};
use super::pic::{self, build_pic, create_funded_canister, deploy_relay, test_principal};
use candid::utils::ArgumentEncoder;
use candid::{decode_one, encode_args, CandidType, Deserialize, Nat, Principal};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc2::approve::ApproveArgs;
use num_traits::ToPrimitive;
use pocket_ic::PocketIc;
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};
use sha3::Keccak256;
use std::time::Duration;

pub const E8: u64 = 100_000_000;
/// The harness ledger fee (0.0001 icUSD).
pub const LEDGER_FEE: u64 = TRANSFER_FEE_E8S;
/// IC time advanced by every [`BridgeWorld::tick`].
pub const TICK_ADVANCE_SECS: u64 = 5;
/// Default bound for the `wait_*` helpers.
pub const DEFAULT_MAX_TICKS: u32 = 60;
pub const ONE_ETH_WEI: u128 = 1_000_000_000_000_000_000;

// ===========================================================================
// Candid mirrors of rumi_icusd_bridge.did (kept local on purpose: this harness does
// not link the bridge library, it exercises the wasm through its public interface)
// ===========================================================================

#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum L2MainnetService {
    Alchemy,
    Ankr,
    BlockPi,
    PublicNode,
    Llama,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum RpcServiceConfig {
    BaseMainnet(L2MainnetService),
    Custom { url: String },
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct InitArgs {
    pub icusd_ledger: Principal,
    pub evm_rpc: Principal,
    pub ecdsa_key_name: String,
    pub base_chain_id: u64,
    pub admins: Vec<Principal>,
    pub fee_recipient: Option<Account>,
    pub rpc_services: Vec<RpcServiceConfig>,
    pub min_quorum: u8,
}

#[derive(CandidType, Deserialize, Clone, Debug, Default)]
pub struct UpgradeArgs {
    pub admins: Option<Vec<Principal>>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum BridgeArg {
    Init(InitArgs),
    Upgrade(UpgradeArgs),
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
    TooManyActiveOperations,
    RequestIdConflict,
    NotFound,
    NotOwner,
    InvalidState(String),
    Ledger(String),
    Rpc(String),
    Internal(String),
    /// Newer canisters pre-check the caller's allowance before creating a deposit. Mirrored
    /// here so decoding works against both the old and the new wasm.
    InsufficientAllowance { allowance_e8s: u64, required_e8s: u64 },
    /// Newer canisters pre-check the caller's balance before creating a deposit.
    InsufficientFunds { balance_e8s: u64, required_e8s: u64 },
}

pub type BridgeResult<T> = Result<T, BridgeError>;

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BridgeLimits {
    pub min_deposit_e8s: u64,
    pub max_deposit_e8s: u64,
    pub max_outstanding_e8s: u64,
    pub mint_window_limit_e8s: u64,
    pub release_window_limit_e8s: u64,
    pub window_seconds: u64,
    pub max_active_per_caller: u32,
    pub max_active_global: u32,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BridgeFees {
    pub deposit_fee_e8s: u64,
    pub release_fee_e8s: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct GasConfig {
    pub max_fee_per_gas_wei: Nat,
    pub max_priority_fee_per_gas_wei: Nat,
    pub mint_gas_limit: u64,
    pub admin_gas_limit: u64,
    pub min_gas_reserve_wei: Nat,
    pub replace_after_secs: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Pauses {
    pub deposits: bool,
    pub mints: bool,
    pub releases: bool,
    pub observer: bool,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ObserverConfig {
    pub logs_chunk_blocks: u64,
    pub logs_max_response_bytes: u64,
    pub default_max_response_bytes: u64,
    pub idle_interval_secs: u64,
    pub active_interval_secs: u64,
    pub cycles_per_rpc_call: Nat,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BridgeToBaseArgs {
    pub amount_e8s: u64,
    pub base_recipient: String,
    pub from_subaccount: Option<serde_bytes::ByteBuf>,
    pub client_request_id: serde_bytes::ByteBuf,
    pub expected_deposit_fee_e8s: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ListDepositsArgs {
    pub start_after: Option<u64>,
    pub limit: u32,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ListReleasesArgs {
    pub owner: Principal,
    pub start_after_sequence: Option<u64>,
    pub limit: u32,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
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
pub enum DepositKindView {
    UserDeposit,
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
    Queued,
    Submitted,
    Included { block_number: u64 },
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
}

#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaneKind {
    Minter,
    Admin,
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

impl BridgeInfo {
    pub fn lane(&self, kind: LaneKind) -> &LaneStatus {
        self.lanes.iter().find(|l| l.lane == kind).expect("lane status")
    }
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SupplyAudit {
    pub escrow_account: Account,
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
    pub base_supply_within_bound: Option<bool>,
    pub base_supply_exact_match: Option<bool>,
    pub reporting_note: String,
}

impl SupplyAudit {
    /// `(outstanding, pending_mint, pending_release, pending_refund, held, fees, reserved_pulls)`
    /// as u128, the buckets in the order of design section 7.2.
    pub fn buckets(&self) -> [u128; 7] {
        [
            nat_u128(&self.outstanding_base_e8s),
            nat_u128(&self.pending_mint_e8s),
            nat_u128(&self.pending_release_e8s),
            nat_u128(&self.pending_refund_e8s),
            nat_u128(&self.held_e8s),
            nat_u128(&self.fees_e8s),
            nat_u128(&self.reserved_pulls_e8s),
        ]
    }

    pub fn outstanding(&self) -> u128 {
        nat_u128(&self.outstanding_base_e8s)
    }

    pub fn fees(&self) -> u128 {
        nat_u128(&self.fees_e8s)
    }

    pub fn escrow(&self) -> Option<u128> {
        self.escrow_balance_e8s.as_ref().map(nat_u128)
    }
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BaseAddresses {
    pub minter: String,
    pub admin: String,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum Transition {
    AdmitDeposit { amount: u64 },
    PullSucceeded { amount: u64 },
    PullFailed { amount: u64 },
    MintDelivered { amount: u64, fee: u64 },
    RefundStarted { amount: u64 },
    RefundCompleted { amount: u64 },
    BridgeOutAccepted { amount: u64 },
    BridgeOutHeld { amount: u64 },
    ReleaseCompleted { amount: u64, release_fee: u64 },
    ReleaseHeld { amount: u64 },
    RemintStarted { amount: u64 },
    RemintDelivered { amount: u64 },
    SweepCompleted { amount: u64 },
    OutflowUnknown { amount: u64 },
    OutflowResolved { amount: u64 },
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

// ===========================================================================
// Small helpers
// ===========================================================================

pub fn nat_u128(n: &Nat) -> u128 {
    n.0.to_u128().unwrap_or_else(|| panic!("nat {n} does not fit u128"))
}

pub fn hex0x(b: &[u8]) -> String {
    format!("0x{}", hex::encode(b))
}

pub fn parse_hex32(s: &str) -> [u8; 32] {
    hex::decode(s.trim_start_matches("0x"))
        .unwrap_or_else(|e| panic!("bad hex {s}: {e}"))
        .try_into()
        .unwrap_or_else(|_| panic!("{s} is not 32 bytes"))
}

pub fn keccak(data: &[u8]) -> [u8; 32] {
    Keccak256::digest(data).into()
}

/// 4-byte selector of a Solidity signature, `0x`-prefixed hex (also the form the canister
/// puts in a mint-simulation hold reason).
pub fn selector_hex(sig: &str) -> String {
    hex0x(&keccak(sig.as_bytes())[..4])
}

/// sha256("rumi-icusd-base-bridge:escrow:v1"), the bridge's escrow subaccount.
pub fn escrow_subaccount() -> [u8; 32] {
    Sha256::digest(b"rumi-icusd-base-bridge:escrow:v1").into()
}

/// The default limits the harness installs (all inside the contract's limits): 1 icUSD
/// minimum, 1000 icUSD maximum per deposit, 10,000 icUSD outstanding, 5,000 icUSD per day,
/// a 24 h window.
pub fn default_limits() -> BridgeLimits {
    BridgeLimits {
        min_deposit_e8s: E8,
        max_deposit_e8s: 1_000 * E8,
        max_outstanding_e8s: 10_000 * E8,
        mint_window_limit_e8s: 5_000 * E8,
        release_window_limit_e8s: 5_000 * E8,
        window_seconds: 86_400,
        max_active_per_caller: 5,
        max_active_global: 500,
    }
}

fn target_dir() -> std::path::PathBuf {
    std::env::var("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| pic::workspace_root().join("target"))
}

pub fn bridge_wasm_bytes() -> Vec<u8> {
    pic::bridge_wasm()
}

pub fn relay_wasm_bytes() -> Vec<u8> {
    pic::relay_wasm()
}

/// The `flaky_ledger` test wasm (BB-09b-2): `FLAKY_LEDGER_WASM` or
/// `<target>/wasm32-unknown-unknown/release/flaky_ledger.wasm`.
pub fn flaky_ledger_wasm() -> Vec<u8> {
    pic::read_wasm(
        "FLAKY_LEDGER_WASM",
        target_dir().join("wasm32-unknown-unknown/release/flaky_ledger.wasm"),
    )
}

// ===========================================================================
// Setup
// ===========================================================================

#[derive(Clone, Debug)]
pub struct SetupOptions {
    /// Initial ICP-side icUSD balance of `user` and `user2` (each).
    pub user_balance_e8s: u128,
    /// Limits installed before binding (must fit the contract's limits).
    pub limits: BridgeLimits,
    /// Fees installed after binding when not zero.
    pub fees: BridgeFees,
    /// Bind the deployed contract (needs outcalls). When false the bridge stays unbound and
    /// deposits stay paused.
    pub bind_contract: bool,
    /// Unpause deposits/mints/releases/observer after binding.
    pub unpause: bool,
    /// Ledger archive knobs (the harness default makes archives appear after ~50 blocks).
    pub archive_trigger_threshold: u64,
    pub num_blocks_to_archive: u64,
    /// Contract constructor limits (e8s).
    pub contract_supply_cap_e8s: u128,
    pub contract_mint_window_limit_e8s: u128,
    pub contract_min_bridge_out_e8s: u128,
    /// Quorum of identical provider answers (3 providers are configured).
    pub min_quorum: u8,
    /// Cycles for the bridge canister.
    pub bridge_cycles: u128,
    /// Extra cycles for the relay (it plays the EVM RPC canister and pays the outcalls).
    pub relay_extra_cycles: u128,
}

impl Default for SetupOptions {
    fn default() -> Self {
        SetupOptions {
            user_balance_e8s: 1_000 * E8 as u128,
            limits: default_limits(),
            fees: BridgeFees { deposit_fee_e8s: 0, release_fee_e8s: 0 },
            bind_contract: true,
            unpause: true,
            archive_trigger_threshold: 50,
            num_blocks_to_archive: 20,
            contract_supply_cap_e8s: 1_000_000_000 * E8 as u128,
            contract_mint_window_limit_e8s: 1_000_000_000 * E8 as u128,
            contract_min_bridge_out_e8s: E8 as u128,
            min_quorum: 2,
            bridge_cycles: 500_000_000_000_000,
            relay_extra_cycles: 900_000_000_000_000,
        }
    }
}

/// What a custom ledger installer is told about the accounts the scenario needs.
#[derive(Clone, Debug)]
pub struct LedgerPlan {
    pub minting: Principal,
    pub balances: Vec<(Principal, u128)>,
    pub controller: Principal,
    pub archive_trigger_threshold: u64,
    pub num_blocks_to_archive: u64,
}

/// A complete local world: PocketIC (II + application subnet), anvil, the outcall router,
/// the real ICRC ledger, the relay (EVM RPC canister), the bridge canister and the deployed,
/// bound `BridgedIcUSD` contract.
pub struct BridgeWorld {
    pub pic: PocketIc,
    pub anvil: Anvil,
    /// Three fake providers (`provider-a/b/c.test`) in front of ONE anvil. Install a fault
    /// hook with `world.router.set_fault(..)`; the default is a pure passthrough.
    pub router: ProviderRouter,
    pub relay: Principal,
    pub ledger: Principal,
    pub bridge: Principal,
    /// Bridge admin principal (also the sender of every `tick`).
    pub admin: Principal,
    /// Funded ICP-side users (`user_balance_e8s` each).
    pub user: Principal,
    pub user2: Principal,
    /// The icUSD ledger minting account owner (NOT the bridge).
    pub minting: Principal,
    pub fee_recipient: Account,
    pub token: BridgedIcUsd,
    /// The bridge's tECDSA minter and admin addresses on Base.
    pub base_addrs: BaseAddresses,
    /// anvil's first unlocked account; deployed the contract and holds no authority.
    pub deployer: String,
    /// Bound at setup time (`None` if `bind_contract` was false).
    pub bound: bool,
    /// Bound for the `wait_*` helpers.
    pub max_ticks: u32,
    /// The exact wasm the bridge was installed with (used by `upgrade_bridge_same_wasm`, so a
    /// rebuild of the wasm on disk during a run cannot turn it into a different-wasm upgrade).
    bridge_wasm: Vec<u8>,
    rid_counter: u64,
    nonce_counter: u64,
}

fn real_ledger(pic: &PocketIc, plan: &LedgerPlan) -> Principal {
    let mut cfg = LedgerConfig::new(
        account(plan.minting),
        plan.controller,
        plan.balances.iter().map(|(p, v)| (account(*p), *v)).collect(),
    );
    cfg.archive_trigger_threshold = plan.archive_trigger_threshold;
    cfg.num_blocks_to_archive = plan.num_blocks_to_archive;
    deploy_icrc_ledger(pic, &cfg)
}

impl BridgeWorld {
    /// The standard world on the real ICRC ledger.
    pub fn setup(opts: SetupOptions) -> BridgeWorld {
        BridgeWorld::setup_with_ledger(opts, real_ledger)
    }

    /// Like [`BridgeWorld::setup`] but the ICP ledger is installed by `make_ledger` (for
    /// example `flaky_ledger`). The closure must install a ledger whose minting account is
    /// `plan.minting` and which credits `plan.balances`.
    pub fn setup_with_ledger(
        opts: SetupOptions,
        make_ledger: impl FnOnce(&PocketIc, &LedgerPlan) -> Principal,
    ) -> BridgeWorld {
        let anvil = Anvil::start();
        assert!(anvil.accounts.len() >= 8, "anvil should expose >= 8 default accounts");
        let pic = build_pic();

        let admin = test_principal(0xA1);
        let user = test_principal(0xB1);
        let user2 = test_principal(0xB2);
        let minting = test_principal(0xC1);
        let fee_recipient = Account { owner: test_principal(0xF1), subaccount: None };

        // 1. ledger (real by default): minting account is NOT the bridge.
        let plan = LedgerPlan {
            minting,
            balances: vec![(user, opts.user_balance_e8s), (user2, opts.user_balance_e8s)],
            controller: minting,
            archive_trigger_threshold: opts.archive_trigger_threshold,
            num_blocks_to_archive: opts.num_blocks_to_archive,
        };
        let ledger = make_ledger(&pic, &plan);

        // 2. relay (plays the EVM RPC canister) with plenty of cycles.
        let relay = deploy_relay(&pic);
        pic.add_cycles(relay, opts.relay_extra_cycles);

        // 3. router: three fake providers, all in front of the one anvil.
        let router = ProviderRouter::new(anvil.endpoint());

        // 4. the bridge canister.
        let init = InitArgs {
            icusd_ledger: ledger,
            evm_rpc: relay,
            ecdsa_key_name: "test_key_1".to_string(),
            base_chain_id: ANVIL_CHAIN_ID,
            admins: vec![admin],
            fee_recipient: Some(fee_recipient),
            rpc_services: vec![
                RpcServiceConfig::Custom { url: PROVIDER_A.to_string() },
                RpcServiceConfig::Custom { url: PROVIDER_B.to_string() },
                RpcServiceConfig::Custom { url: PROVIDER_C.to_string() },
            ],
            min_quorum: opts.min_quorum,
        };
        let bridge = create_funded_canister(&pic, opts.bridge_cycles);
        let bridge_wasm = bridge_wasm_bytes();
        pic.install_canister(
            bridge,
            bridge_wasm.clone(),
            encode_args((BridgeArg::Init(init),)).expect("encode init"),
            None,
        );

        let deployer = anvil.accounts[0].clone();
        let placeholder = BaseAddresses { minter: String::new(), admin: String::new() };
        let token_placeholder = BridgedIcUsd { address: String::new(), deploy_tx: String::new() };
        let mut w = BridgeWorld {
            pic,
            anvil,
            router,
            relay,
            ledger,
            bridge,
            admin,
            user,
            user2,
            minting,
            fee_recipient,
            token: token_placeholder,
            base_addrs: placeholder,
            deployer,
            bound: false,
            max_ticks: DEFAULT_MAX_TICKS,
            bridge_wasm,
            rid_counter: 0,
            nonce_counter: 0,
        };

        // 5. the bridge's real tECDSA addresses; fund both with test ETH.
        let addrs: BridgeResult<BaseAddresses> = w.update(Principal::anonymous(), "get_base_addresses", ());
        w.base_addrs = addrs.expect("get_base_addresses");
        for a in [&w.base_addrs.minter, &w.base_addrs.admin] {
            w.anvil.set_balance(a, 10 * ONE_ETH_WEI);
        }

        // 6. deploy BridgedIcUSD with constructor admin/minter = the bridge's addresses.
        let args = DeployArgs {
            admin: w.base_addrs.admin.clone(),
            initial_admin_delay: 24 * 3600,
            minter: w.base_addrs.minter.clone(),
            guardian: None,
            supply_cap: opts.contract_supply_cap_e8s,
            mint_window_limit: opts.contract_mint_window_limit_e8s,
            mint_window_seconds: 24 * 3600,
            min_bridge_out: opts.contract_min_bridge_out_e8s,
        };
        w.token = BridgedIcUsd::deploy(&w.anvil, &w.deployer.clone(), &args);
        // The deployment block must be at or below the finalized head before binding.
        w.anvil.mine(2);

        // 7. limits (inside the contract's), bind, then open the bridge.
        w.set_limits(opts.limits.clone()).expect("admin_set_limits");
        if opts.bind_contract {
            // Binding compares the contract's minBridgeOut with the release cost, which needs
            // the ledger fee; the first tick learns it.
            w.tick();
            let address = w.token.address.clone();
            let bound: BridgeResult<ContractBindingView> = w.admin_call("admin_bind_contract", (address,));
            let bound = bound.expect("admin_bind_contract");
            assert_eq!(bound.address, w.token.address);
            w.bound = true;
            if opts.fees != (BridgeFees { deposit_fee_e8s: 0, release_fee_e8s: 0 }) {
                w.set_fees(opts.fees.clone()).expect("admin_set_fees");
            }
            if opts.unpause {
                w.set_pauses(Pauses { deposits: false, mints: false, releases: false, observer: false })
                    .expect("admin_set_pauses");
            }
        }
        w
    }

    // -----------------------------------------------------------------------
    // Raw calls
    // -----------------------------------------------------------------------

    /// Update call through the outcall pump. Panics on a reject or a trap.
    pub fn update<A: ArgumentEncoder, R: CandidType + for<'de> Deserialize<'de>>(
        &mut self,
        sender: Principal,
        method: &str,
        args: A,
    ) -> R {
        let arg = encode_args(args).expect("encode args");
        let id = submit(&self.pic, self.bridge, sender, method, arg).expect("submit");
        let bytes = await_with_outcalls(&self.pic, id, &mut self.router)
            .unwrap_or_else(|e| panic!("update {method} failed: {e}\n{}", self.dump()));
        decode_one(&bytes).unwrap_or_else(|e| panic!("decode {method} reply: {e}"))
    }

    /// Query call as `sender`.
    pub fn query<A: ArgumentEncoder, R: CandidType + for<'de> Deserialize<'de>>(
        &self,
        sender: Principal,
        method: &str,
        args: A,
    ) -> R {
        let arg = encode_args(args).expect("encode args");
        match self
            .pic
            .query_call(self.bridge, sender, method, arg)
            .unwrap_or_else(|e| panic!("query {method}: {e}"))
        {
            pocket_ic::WasmResult::Reply(b) => decode_one(&b).unwrap_or_else(|e| panic!("decode {method}: {e}")),
            pocket_ic::WasmResult::Reject(m) => panic!("query {method} rejected: {m}"),
        }
    }

    /// Update call as the bridge admin.
    pub fn admin_call<A: ArgumentEncoder, R: CandidType + for<'de> Deserialize<'de>>(
        &mut self,
        method: &str,
        args: A,
    ) -> R {
        let admin = self.admin;
        self.update(admin, method, args)
    }

    // -----------------------------------------------------------------------
    // Time
    // -----------------------------------------------------------------------

    pub fn advance_secs(&self, secs: u64) {
        self.pic.advance_time(Duration::from_secs(secs));
    }

    /// Advance 5 s of IC time, then run `admin_run_tick` (outcalls are pumped to anvil).
    /// The canister's tick guard may make this a no-op when its own timer is mid-run.
    pub fn tick(&mut self) {
        self.advance_secs(TICK_ADVANCE_SECS);
        let r: BridgeResult<()> = self.admin_call("admin_run_tick", ());
        r.expect("admin_run_tick");
    }

    pub fn ticks(&mut self, n: u32) {
        for _ in 0..n {
            self.tick();
        }
    }

    /// Tick until `pred` holds (checked once before the first tick, then after every tick).
    /// Returns the ticks used. Panics with diagnostics after `max` ticks.
    pub fn tick_until(&mut self, what: &str, max: u32, mut pred: impl FnMut(&mut BridgeWorld) -> bool) -> u32 {
        if pred(self) {
            return 0;
        }
        for i in 1..=max {
            self.tick();
            if pred(self) {
                return i;
            }
        }
        panic!("timed out after {max} ticks waiting for: {what}\n{}", self.dump());
    }

    /// Answer outstanding outcalls without advancing time (lets timer-spawned work finish).
    pub fn pump_idle(&mut self) {
        pump_until_idle(&self.pic, &mut self.router, 200);
    }

    /// Human-readable diagnostics: bridge info highlights, audit tail, last provider calls.
    pub fn dump(&self) -> String {
        let mut s = String::new();
        let info: Result<BridgeInfo, String> = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.query(Principal::anonymous(), "get_bridge_info", ())
        }))
        .map_err(|_| "get_bridge_info panicked".to_string());
        match info {
            Ok(i) => {
                s.push_str(&format!(
                    "--- bridge info: halted={:?} pauses={:?} active_deposits={} active_releases={}\n    observer: cursor={} processed={} last_finalized={:?} last_error={:?}\n    lanes: {:?}\n",
                    i.halted, i.pauses, i.active_deposits, i.active_releases, i.observer.cursor_block,
                    i.observer.processed_count, i.observer.last_finalized_block, i.observer.last_error, i.lanes
                ));
            }
            Err(e) => s.push_str(&format!("--- {e}\n")),
        }
        let log: Result<Vec<AuditEntry>, String> = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.audit_log_all()
        }))
        .map_err(|_| "audit log panicked".to_string());
        if let Ok(log) = log {
            s.push_str("--- audit tail:\n");
            for e in log.iter().rev().take(15).rev() {
                s.push_str(&format!("    #{} {:?} dep={:?} tx={:?} {}\n", e.seq, e.transition, e.deposit_id, e.tx_id, e.note));
            }
        }
        s.push_str("--- last provider calls:\n");
        for r in self.router.log().iter().rev().take(8).rev() {
            s.push_str(&format!("    {} {:?} {:?}\n", r.provider, r.rpc_methods, r.served));
        }
        s
    }

    // -----------------------------------------------------------------------
    // Bridge views and admin
    // -----------------------------------------------------------------------

    pub fn bridge_info(&self) -> BridgeInfo {
        self.query(Principal::anonymous(), "get_bridge_info", ())
    }

    /// The stored snapshot (the escrow balance and Base supply are as of the last reconcile).
    pub fn supply_audit(&self) -> SupplyAudit {
        self.query(Principal::anonymous(), "get_supply_audit", ())
    }

    /// `admin_reconcile`: re-read the escrow balance and the finalized Base supply NOW and
    /// return the refreshed audit.
    pub fn reconcile(&mut self) -> SupplyAudit {
        let r: BridgeResult<SupplyAudit> = self.admin_call("admin_reconcile", ());
        r.expect("admin_reconcile")
    }

    pub fn get_deposit(&self, id: u64) -> Option<DepositView> {
        self.query(Principal::anonymous(), "get_deposit", (id,))
    }

    pub fn get_release(&self, transfer_id: &str) -> Option<ReleaseView> {
        self.query(Principal::anonymous(), "get_release", (transfer_id.to_string(),))
    }

    pub fn list_my_deposits(&self, caller: Principal, start_after: Option<u64>, limit: u32) -> Vec<DepositView> {
        self.query(caller, "list_my_deposits", (ListDepositsArgs { start_after, limit },))
    }

    pub fn list_releases_for_owner(
        &self,
        owner: Principal,
        start_after_sequence: Option<u64>,
        limit: u32,
    ) -> Vec<ReleaseView> {
        self.query(
            Principal::anonymous(),
            "list_releases_for_owner",
            (ListReleasesArgs { owner, start_after_sequence, limit },),
        )
    }

    /// The whole audit log (paged in 100s).
    pub fn audit_log_all(&self) -> Vec<AuditEntry> {
        let mut out = Vec::new();
        loop {
            let page: Vec<AuditEntry> =
                self.query(Principal::anonymous(), "get_audit_log", (out.len() as u64, 100u32));
            if page.is_empty() {
                return out;
            }
            out.extend(page);
        }
    }

    pub fn set_pauses(&mut self, p: Pauses) -> BridgeResult<()> {
        self.admin_call("admin_set_pauses", (p,))
    }

    pub fn set_limits(&mut self, l: BridgeLimits) -> BridgeResult<()> {
        self.admin_call("admin_set_limits", (l,))
    }

    pub fn set_fees(&mut self, f: BridgeFees) -> BridgeResult<()> {
        self.admin_call("admin_set_fees", (f,))
    }

    pub fn request_refund(&mut self, caller: Principal, deposit_id: u64) -> BridgeResult<DepositView> {
        self.update(caller, "request_deposit_refund", (deposit_id,))
    }

    /// `admin_sweep_fees(amount)`; returns the sweep id.
    pub fn sweep_fees(&mut self, amount_e8s: u64) -> BridgeResult<u64> {
        self.admin_call("admin_sweep_fees", (amount_e8s,))
    }

    /// `notify_base_bridge_out(tx_hash)` as `caller` (a hint only; it never settles).
    pub fn notify_hint(&mut self, caller: Principal, tx_hash: &str) -> BridgeResult<()> {
        self.update(caller, "notify_base_bridge_out", (tx_hash.to_string(),))
    }

    /// Stop, upgrade with the same wasm and `post_upgrade(null)`, start. Outcalls in flight
    /// are drained first (no time advance) so the stop is clean.
    pub fn upgrade_bridge_same_wasm(&mut self) {
        self.pump_idle();
        self.pic.stop_canister(self.bridge, None).expect("stop bridge");
        self.pic
            .upgrade_canister(
                self.bridge,
                self.bridge_wasm.clone(),
                encode_args((None::<BridgeArg>,)).expect("encode upgrade arg"),
                None,
            )
            .expect("upgrade bridge");
        self.pic.start_canister(self.bridge, None).expect("start bridge");
    }

    // -----------------------------------------------------------------------
    // Waiters
    // -----------------------------------------------------------------------

    /// Tick until deposit `id` satisfies `pred`; returns that view.
    pub fn wait_deposit(&mut self, id: u64, what: &str, pred: impl Fn(&DepositView) -> bool) -> DepositView {
        let max = self.max_ticks;
        self.tick_until(&format!("deposit {id}: {what}"), max, |w| {
            w.get_deposit(id).map(|d| pred(&d)).unwrap_or(false)
        });
        self.get_deposit(id).expect("deposit")
    }

    /// Tick until the release `transfer_id` exists and satisfies `pred`.
    pub fn wait_release(&mut self, transfer_id: &str, what: &str, pred: impl Fn(&ReleaseView) -> bool) -> ReleaseView {
        let max = self.max_ticks;
        let tid = transfer_id.to_string();
        self.tick_until(&format!("release {tid}: {what}"), max, |w| {
            w.get_release(&tid).map(|r| pred(&r)).unwrap_or(false)
        });
        self.get_release(transfer_id).expect("release")
    }

    /// Tick until the mint transaction is mined on Base but not yet final.
    pub fn wait_mint_included(&mut self, id: u64) -> DepositView {
        self.wait_deposit(id, "mint included", |d| matches!(d.mint_stage, Some(BaseTxStage::Included { .. })))
    }

    /// Tick until the deposit is `Delivered` (the caller must have made the mint final).
    pub fn wait_delivered(&mut self, id: u64) -> DepositView {
        self.wait_deposit(id, "Delivered", |d| d.status == DepositStatus::Delivered)
    }

    pub fn wait_released(&mut self, transfer_id: &str) -> ReleaseView {
        self.wait_release(transfer_id, "Released", |r| r.status == ReleaseStatus::Released)
    }

    // -----------------------------------------------------------------------
    // ICP ledger
    // -----------------------------------------------------------------------

    pub fn ledger_balance(&self, who: &Account) -> u128 {
        nat_u128(&ledger::icrc1_balance_of(&self.pic, self.ledger, who))
    }

    /// Default-subaccount balance of a principal.
    pub fn user_balance(&self, who: Principal) -> u128 {
        self.ledger_balance(&account(who))
    }

    pub fn escrow_account(&self) -> Account {
        Account { owner: self.bridge, subaccount: Some(escrow_subaccount()) }
    }

    pub fn escrow_balance(&self) -> u128 {
        self.ledger_balance(&self.escrow_account())
    }

    pub fn ledger_total_supply(&self) -> u128 {
        nat_u128(&ledger::icrc1_total_supply(&self.pic, self.ledger))
    }

    /// `icrc2_approve(spender, amount)` from `owner` (fee 10_000 charged to `owner`).
    pub fn approve(&self, owner: Principal, spender: Principal, amount: u128) {
        ledger::icrc2_approve(
            &self.pic,
            self.ledger,
            owner,
            ApproveArgs {
                from_subaccount: None,
                spender: account(spender),
                amount: Nat::from(amount),
                expected_allowance: None,
                expires_at: None,
                fee: Some(Nat::from(LEDGER_FEE)),
                memo: None,
                created_at_time: None,
            },
        )
        .unwrap_or_else(|e| panic!("icrc2_approve failed: {e:?}"));
    }

    /// Plain `icrc1_transfer` of `amount` from `from` to `to` (fee on top).
    pub fn ledger_transfer(&self, from: Principal, to: Account, amount: u128) {
        ledger::transfer(&self.pic, self.ledger, from, to, amount)
            .unwrap_or_else(|e| panic!("icrc1_transfer failed: {e:?}"));
    }

    /// Approve the bridge for `amount + ledger fee` (what one `bridge_to_base(amount)` pulls).
    pub fn approve_bridge(&self, owner: Principal, amount_e8s: u64) {
        self.approve(owner, self.bridge, amount_e8s as u128 + LEDGER_FEE as u128);
    }

    // -----------------------------------------------------------------------
    // ICP -> Base flows
    // -----------------------------------------------------------------------

    /// A fresh unique 16-byte request id.
    pub fn fresh_request_id(&mut self) -> Vec<u8> {
        self.rid_counter += 1;
        let mut v = vec![0xD0u8; 8];
        v.extend_from_slice(&self.rid_counter.to_be_bytes());
        v
    }

    /// A fresh unique `bridgeOut` client nonce.
    pub fn fresh_nonce(&mut self) -> [u8; 32] {
        self.nonce_counter += 1;
        super::contract::bytes32_from_u64(0x1000 + self.nonce_counter)
    }

    /// Args with a fresh request id and the CURRENT deposit fee as `expected_deposit_fee_e8s`.
    pub fn deposit_args(&mut self, amount_e8s: u64, base_recipient: &str) -> BridgeToBaseArgs {
        let fee = self.bridge_info().fees.deposit_fee_e8s;
        BridgeToBaseArgs {
            amount_e8s,
            base_recipient: base_recipient.to_string(),
            from_subaccount: None,
            client_request_id: serde_bytes::ByteBuf::from(self.fresh_request_id()),
            expected_deposit_fee_e8s: fee,
        }
    }

    pub fn bridge_to_base(&mut self, caller: Principal, args: BridgeToBaseArgs) -> BridgeResult<DepositView> {
        self.update(caller, "bridge_to_base", (args,))
    }

    /// Approve `amount + fee`, call `bridge_to_base`, and require success (any status).
    pub fn approve_and_deposit(&mut self, caller: Principal, amount_e8s: u64, base_recipient: &str) -> DepositView {
        self.approve_bridge(caller, amount_e8s);
        let args = self.deposit_args(amount_e8s, base_recipient);
        self.bridge_to_base(caller, args).unwrap_or_else(|e| panic!("bridge_to_base failed: {e:?}"))
    }

    /// Approve, deposit, wait for the mint to be mined, make it final, wait for `Delivered`.
    pub fn deposit_and_deliver(&mut self, caller: Principal, amount_e8s: u64, base_recipient: &str) -> DepositView {
        let d = self.approve_and_deposit(caller, amount_e8s, base_recipient);
        assert_eq!(d.status, DepositStatus::Escrowed, "pull should have succeeded: {d:?}");
        self.wait_mint_included(d.id);
        self.mine_finalize();
        self.wait_delivered(d.id)
    }

    /// A holder's `bridgeOut` on anvil (an unlocked anvil account). A reverting call is
    /// mined as a failed receipt (`success == false`, no event).
    pub fn bridge_out_from(
        &self,
        base_account: &str,
        amount: u128,
        principal_bytes: &[u8],
        subaccount: [u8; 32],
        nonce: [u8; 32],
    ) -> BridgeOutTx {
        let tx_hash = self
            .token
            .try_bridge_out(&self.anvil, base_account, amount, principal_bytes, subaccount, nonce)
            .unwrap_or_else(|e| panic!("bridgeOut from {base_account} was refused by the node: {e}"));
        let rc = self.anvil.receipt(&tx_hash).expect("bridgeOut receipt");
        let success = rc["status"] == "0x1";
        let event = if success {
            let logs = logs_with_topic(&rc, &self.token.address, TOPIC_BRIDGE_OUT);
            assert_eq!(logs.len(), 1, "a successful bridgeOut emits exactly one BridgeOut log");
            Some(decode_bridge_out(logs[0]))
        } else {
            None
        };
        BridgeOutTx {
            transfer_id: hex0x(&self.token.compute_transfer_id(&self.anvil, base_account, nonce)),
            block_number: parse_hex_u64(&rc["blockNumber"]),
            tx_hash,
            success,
            event,
        }
    }

    /// bridgeOut to `dest` (default subaccount), make it final and wait for the release.
    pub fn bridge_out_and_release(&mut self, base_account: &str, amount: u128, dest: Principal) -> (BridgeOutTx, ReleaseView) {
        let nonce = self.fresh_nonce();
        let tx = self.bridge_out_from(base_account, amount, dest.as_slice(), [0u8; 32], nonce);
        assert!(tx.success, "bridgeOut must succeed");
        self.mine_finalize();
        let r = self.wait_released(&tx.transfer_id);
        (tx, r)
    }

    // -----------------------------------------------------------------------
    // Base (anvil) side
    // -----------------------------------------------------------------------

    /// anvil's i-th unlocked account that is not used as deployer/admin/minter (`accounts[4 + i]`),
    /// lowercase `0x` address. Usable as a Base holder (it can send `bridgeOut` directly).
    pub fn holder(&self, i: usize) -> String {
        self.anvil.accounts[4 + i].clone()
    }

    pub fn mine(&self, n: u64) {
        self.anvil.mine(n);
    }

    /// Mine 2 blocks: with `--slots-in-an-epoch 1` that makes everything mined so far final.
    pub fn mine_finalize(&self) {
        self.anvil.mine(2);
    }

    /// `evm_setAutomine`: with automine off, transactions wait in the mempool until
    /// [`BridgeWorld::mine_block`]. The bridge's own mint transactions then also wait, so turn it
    /// back on before ticking.
    pub fn set_automine(&self, on: bool) {
        self.anvil.rpc("evm_setAutomine", json!([on]));
    }

    /// `evm_mine`: mine ONE block (containing every pending transaction).
    pub fn mine_block(&self) {
        self.anvil.rpc("evm_mine", json!([]));
    }

    /// Submit a holder's `bridgeOut` WITHOUT waiting for it to be mined (use with
    /// `set_automine(false)` to put several burns into one block). Returns the tx hash.
    pub fn send_bridge_out_unmined(
        &self,
        from: &str,
        amount: u128,
        principal_bytes: &[u8],
        subaccount: [u8; 32],
        nonce: [u8; 32],
    ) -> String {
        let data = super::contract::encode_bridge_out(amount, principal_bytes, subaccount, nonce);
        self.anvil
            .rpc(
                "eth_sendTransaction",
                json!([{
                    "from": from,
                    "to": self.token.address,
                    "data": format!("0x{}", hex::encode(data)),
                    "gas": "0x7a120",
                    "value": "0x0",
                }]),
            )
            .as_str()
            .expect("tx hash")
            .to_string()
    }

    pub fn base_balance(&self, who: &str) -> u128 {
        self.token.balance_of(&self.anvil, who)
    }

    pub fn base_total_supply(&self) -> u128 {
        self.token.total_supply(&self.anvil)
    }

    pub fn base_bridge_out_count(&self) -> u64 {
        self.token.bridge_out_count(&self.anvil)
    }

    pub fn base_mint_consumed(&self, mint_id_hex: &str) -> bool {
        self.token.mint_consumed(&self.anvil, parse_hex32(mint_id_hex))
    }

    /// `eth_getTransactionCount(addr, "latest")`.
    pub fn base_nonce(&self, addr: &str) -> u64 {
        parse_hex_u64(&self.anvil.rpc("eth_getTransactionCount", json!([addr, "latest"])))
    }

    /// Transactions the bridge's minter address has sent (one per mint).
    pub fn minter_nonce(&self) -> u64 {
        self.base_nonce(&self.base_addrs.minter)
    }

    pub fn receipt(&self, tx_hash: &str) -> Option<Value> {
        self.anvil.receipt(tx_hash)
    }

    /// The decoded `BridgeMint` of a mint receipt: `(mintId, to, amount)`. Panics unless the
    /// receipt has exactly one `BridgeMint` from the bound contract.
    pub fn bridge_mint_of(&self, tx_hash: &str) -> ([u8; 32], String, u128) {
        let rc = self.receipt(tx_hash).unwrap_or_else(|| panic!("no receipt for {tx_hash}"));
        let logs = logs_with_topic(&rc, &self.token.address, TOPIC_BRIDGE_MINT);
        assert_eq!(logs.len(), 1, "exactly one BridgeMint expected in {tx_hash}");
        decode_bridge_mint(logs[0])
    }

    /// TEST-ONLY admin simulation: have anvil impersonate `from` (an address whose key the
    /// test does not hold, such as the bridge's tECDSA admin address) and send `data` to `to`.
    /// Returns the (mined) transaction hash. Never used to mint or to touch the bridge itself.
    pub fn impersonate_send(&self, from: &str, to: &str, data: &[u8]) -> String {
        self.anvil.rpc("anvil_impersonateAccount", json!([from]));
        let h = self.anvil.send_tx_from_unlocked(from, Some(to), data, 0);
        self.anvil.rpc("anvil_stopImpersonatingAccount", json!([from]));
        h
    }

    fn contract_call_as_admin(&self, sig: &str) -> String {
        let data = hex::decode(selector_hex(sig).trim_start_matches("0x")).unwrap();
        let admin = self.base_addrs.admin.clone();
        let h = self.impersonate_send(&admin, &self.token.address.clone(), &data);
        assert!(self.anvil.tx_succeeded(&h), "{sig} as admin reverted");
        h
    }

    /// TEST-ONLY: `pauseMint()` on the contract as the (impersonated) admin address.
    pub fn contract_pause_mint_as_admin(&self) -> String {
        self.contract_call_as_admin("pauseMint()")
    }

    /// TEST-ONLY: `unpauseMint()` on the contract as the (impersonated) admin address.
    pub fn contract_unpause_mint_as_admin(&self) -> String {
        self.contract_call_as_admin("unpauseMint()")
    }

    /// `mintPaused()` on the contract.
    pub fn contract_mint_paused(&self) -> bool {
        let data = hex::decode(selector_hex("mintPaused()").trim_start_matches("0x")).unwrap();
        let ret = self.anvil.eth_call(&self.token.address, &data);
        super::contract::decode_u128(&ret) != 0
    }
}

/// Result of [`BridgeWorld::bridge_out_from`].
#[derive(Clone, Debug)]
pub struct BridgeOutTx {
    pub tx_hash: String,
    pub success: bool,
    pub block_number: u64,
    /// The decoded `BridgeOut` log (only when the call succeeded).
    pub event: Option<BridgeOutEvent>,
    /// `0x` hex of `computeTransferId(from, nonce)`: the key of `get_release`.
    pub transfer_id: String,
}
