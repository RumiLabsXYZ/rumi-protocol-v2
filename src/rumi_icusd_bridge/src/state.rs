//! Stable state. Every record is written directly into `ic-stable-structures`, so an
//! upgrade needs no `pre_upgrade` snapshot and cannot lose operations.
//!
//! Records are CBOR (`ciborium`). CBOR maps fields by name, so a field added later with
//! `#[serde(default)]` decodes from older bytes. (That is not true of Candid `Decode!`,
//! which is why Candid is never used for stored records here.)
//!
//! The `MemoryId` layout is append-only. `memory_layout_is_unique` pins it.

use crate::accounting::{AccountingError, Buckets, Transition};
use crate::ids::{self, LaneKind};
use crate::types::{
    BridgeFees, BridgeLimits, ContractAdminAction, InitArgs, Pauses, RpcServiceConfig,
};
use candid::Principal;
use ic_stable_structures::memory_manager::{MemoryId, MemoryManager, VirtualMemory};
use ic_stable_structures::storable::Bound;
use ic_stable_structures::{DefaultMemoryImpl, StableBTreeMap, StableCell, StableLog, Storable};
use icrc_ledger_types::icrc1::account::Account;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::BTreeSet;

pub type Mem = VirtualMemory<DefaultMemoryImpl>;

pub const MEM_CORE: u8 = 0;
pub const MEM_DEPOSITS: u8 = 1;
pub const MEM_RELEASES: u8 = 2;
pub const MEM_TXS: u8 = 3;
pub const MEM_OWNER_DEPOSITS: u8 = 4;
pub const MEM_REQUEST_IDS: u8 = 5;
pub const MEM_OWNER_RELEASES: u8 = 6;
pub const MEM_RELEASE_BY_SEQ: u8 = 7;
pub const MEM_AUDIT_INDEX: u8 = 8;
pub const MEM_AUDIT_DATA: u8 = 9;

/// Append-only. Never reuse or reorder an id.
pub const MEMORY_LAYOUT: &[(u8, &str)] = &[
    (MEM_CORE, "core"),
    (MEM_DEPOSITS, "deposits"),
    (MEM_RELEASES, "releases"),
    (MEM_TXS, "evm_txs"),
    (MEM_OWNER_DEPOSITS, "owner_deposit_index"),
    (MEM_REQUEST_IDS, "client_request_index"),
    (MEM_OWNER_RELEASES, "owner_release_index"),
    (MEM_RELEASE_BY_SEQ, "release_by_sequence"),
    (MEM_AUDIT_INDEX, "audit_index"),
    (MEM_AUDIT_DATA, "audit_data"),
];

pub const CORE_SCHEMA_VERSION: u32 = 1;
/// Bounded number of retained Base tx hash hints.
pub const MAX_HINTS: usize = 256;
/// Bounded number of retained sweep records in the core cell.
pub const MAX_SWEEPS: usize = 64;
/// Maximum page size for list queries.
pub const MAX_PAGE: u32 = 100;

// ---------------------------------------------------------------------------
// CBOR storable helper
// ---------------------------------------------------------------------------

pub fn cbor_encode<T: Serialize>(v: &T) -> Vec<u8> {
    let mut buf = Vec::new();
    ciborium::ser::into_writer(v, &mut buf).expect("CBOR encode of bridge state");
    buf
}

pub fn cbor_decode<T: DeserializeOwned>(bytes: &[u8]) -> T {
    ciborium::de::from_reader(bytes).expect("CBOR decode of bridge state")
}

macro_rules! cbor_storable {
    ($t:ty) => {
        impl Storable for $t {
            fn to_bytes(&self) -> Cow<'_, [u8]> {
                Cow::Owned(cbor_encode(self))
            }
            fn from_bytes(bytes: Cow<[u8]>) -> Self {
                cbor_decode(bytes.as_ref())
            }
            const BOUND: Bound = Bound::Unbounded;
        }
    };
}

// ---------------------------------------------------------------------------
// Configuration records
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct GasSettings {
    pub max_fee_per_gas_wei: u128,
    pub max_priority_fee_per_gas_wei: u128,
    pub mint_gas_limit: u64,
    pub admin_gas_limit: u64,
    pub min_gas_reserve_wei: u128,
    pub replace_after_secs: u64,
}

impl Default for GasSettings {
    fn default() -> Self {
        GasSettings {
            // 2 gwei cap; Base execution fees are typically far below this.
            max_fee_per_gas_wei: 2_000_000_000,
            // 0.1 gwei priority cap.
            max_priority_fee_per_gas_wei: 100_000_000,
            mint_gas_limit: 150_000,
            admin_gas_limit: 100_000,
            // 0.0005 ETH kept above gas_limit * max_fee for the L1 data fee.
            min_gas_reserve_wei: 500_000_000_000_000,
            replace_after_secs: 90,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ObserverSettings {
    pub logs_chunk_blocks: u64,
    pub logs_max_response_bytes: u64,
    pub default_max_response_bytes: u64,
    pub idle_interval_secs: u64,
    pub active_interval_secs: u64,
    pub cycles_per_rpc_call: u128,
}

impl Default for ObserverSettings {
    fn default() -> Self {
        ObserverSettings {
            logs_chunk_blocks: 2_000,
            logs_max_response_bytes: 65_536,
            default_max_response_bytes: 8_192,
            idle_interval_secs: 300,
            active_interval_secs: 30,
            cycles_per_rpc_call: 2_000_000_000,
        }
    }
}

pub fn default_limits() -> BridgeLimits {
    const E8: u64 = 100_000_000;
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

/// Hard upper bounds on admin-settable fees, so a misconfigured admin cannot take a
/// material share of a transfer.
pub const MAX_DEPOSIT_FEE_E8S: u64 = 10 * 100_000_000;
pub const MAX_RELEASE_FEE_E8S: u64 = 10 * 100_000_000;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ContractBinding {
    pub address: [u8; 20],
    pub bound_at_ns: u64,
    /// First block the observer scans (the finalized head when bound).
    pub start_block: u64,
    pub supply_cap: u128,
    pub mint_window_limit: u128,
    pub mint_window_seconds: u64,
    pub min_bridge_out: u128,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BridgeConfig {
    pub icusd_ledger: Principal,
    pub evm_rpc: Principal,
    pub ecdsa_key_name: String,
    pub chain_id: u64,
    pub admins: Vec<Principal>,
    pub fee_recipient: Option<Account>,
    pub rpc_services: Vec<RpcServiceConfig>,
    pub min_quorum: u8,
    pub limits: BridgeLimits,
    pub fees: BridgeFees,
    #[serde(default)]
    pub gas: GasSettings,
    #[serde(default)]
    pub pauses: Pauses,
    #[serde(default)]
    pub observer: ObserverSettings,
    #[serde(default)]
    pub contract: Option<ContractBinding>,
}

impl Default for BridgeConfig {
    fn default() -> Self {
        BridgeConfig {
            icusd_ledger: Principal::anonymous(),
            evm_rpc: Principal::anonymous(),
            ecdsa_key_name: String::new(),
            chain_id: 0,
            admins: vec![],
            fee_recipient: None,
            rpc_services: vec![],
            min_quorum: 1,
            limits: default_limits(),
            fees: BridgeFees { deposit_fee_e8s: 0, release_fee_e8s: 0 },
            gas: GasSettings::default(),
            // A fresh canister starts with deposits paused until a contract is bound and
            // an admin explicitly unpauses.
            pauses: Pauses { deposits: true, mints: false, releases: false, observer: false },
            observer: ObserverSettings::default(),
            contract: None,
        }
    }
}

pub const CHAIN_BASE_MAINNET: u64 = 8453;
pub const CHAIN_BASE_SEPOLIA: u64 = 84532;
pub const CHAIN_LOCAL_ANVIL: u64 = 31337;

/// Validate install arguments into a config. Pure, unit tested.
pub fn validate_init(args: &InitArgs) -> Result<BridgeConfig, String> {
    let key = args.ecdsa_key_name.as_str();
    match (args.base_chain_id, key) {
        (CHAIN_BASE_MAINNET, "key_1") => {}
        (CHAIN_BASE_MAINNET, other) => return Err(format!("Base mainnet requires key_1, got {other}")),
        (CHAIN_BASE_SEPOLIA, "key_1" | "test_key_1") => {}
        (CHAIN_LOCAL_ANVIL, "test_key_1" | "dfx_test_key") => {}
        (CHAIN_LOCAL_ANVIL, other) => return Err(format!("local chain id cannot use key {other}")),
        (chain, k) => return Err(format!("unsupported chain id {chain} / key {k}")),
    }
    if args.admins.is_empty() {
        return Err("at least one admin is required".into());
    }
    if args.admins.iter().any(|p| *p == Principal::anonymous()) {
        return Err("anonymous principal cannot be an admin".into());
    }
    if args.icusd_ledger == Principal::anonymous() || args.evm_rpc == Principal::anonymous() {
        return Err("ledger and EVM RPC principals are required".into());
    }
    validate_rpc_services(args.base_chain_id, &args.rpc_services, args.min_quorum)?;
    Ok(BridgeConfig {
        icusd_ledger: args.icusd_ledger,
        evm_rpc: args.evm_rpc,
        ecdsa_key_name: args.ecdsa_key_name.clone(),
        chain_id: args.base_chain_id,
        admins: dedup_principals(&args.admins),
        fee_recipient: args.fee_recipient,
        rpc_services: args.rpc_services.clone(),
        min_quorum: args.min_quorum,
        ..BridgeConfig::default()
    })
}

pub fn validate_rpc_services(chain_id: u64, services: &[RpcServiceConfig], min_quorum: u8) -> Result<(), String> {
    if services.is_empty() {
        return Err("at least one RPC service is required".into());
    }
    let mut seen = BTreeSet::new();
    for s in services {
        let key = match s {
            RpcServiceConfig::BaseMainnet(p) => {
                if chain_id != CHAIN_BASE_MAINNET {
                    return Err("BaseMainnet providers are only valid for chain 8453".into());
                }
                format!("base:{p:?}")
            }
            RpcServiceConfig::Custom { url } => {
                if !(url.starts_with("https://") || (chain_id == CHAIN_LOCAL_ANVIL && url.starts_with("http://"))) {
                    return Err(format!("RPC URL must be https: {url}"));
                }
                if url.len() > 256 {
                    return Err("RPC URL too long".into());
                }
                format!("url:{}", url.trim_end_matches('/'))
            }
        };
        if !seen.insert(key.clone()) {
            return Err(format!("duplicate RPC service {key}"));
        }
    }
    if min_quorum == 0 || min_quorum as usize > services.len() {
        return Err(format!("min_quorum {min_quorum} must be in 1..={}", services.len()));
    }
    if chain_id == CHAIN_BASE_MAINNET && min_quorum < 2 {
        return Err("Base mainnet requires min_quorum >= 2".into());
    }
    Ok(())
}

pub fn validate_limits(l: &BridgeLimits) -> Result<(), String> {
    if l.min_deposit_e8s == 0 {
        return Err("min_deposit_e8s must be > 0".into());
    }
    if l.min_deposit_e8s > l.max_deposit_e8s {
        return Err("min_deposit_e8s exceeds max_deposit_e8s".into());
    }
    if l.max_deposit_e8s > l.max_outstanding_e8s {
        return Err("max_deposit_e8s exceeds max_outstanding_e8s".into());
    }
    if l.window_seconds == 0 {
        return Err("window_seconds must be > 0".into());
    }
    if l.max_active_per_caller == 0 || l.max_active_global == 0 || l.max_active_global > 10_000 {
        return Err("active operation limits must be in 1..=10000".into());
    }
    Ok(())
}

pub fn validate_fees(f: &BridgeFees, limits: &BridgeLimits) -> Result<(), String> {
    if f.deposit_fee_e8s > MAX_DEPOSIT_FEE_E8S || f.release_fee_e8s > MAX_RELEASE_FEE_E8S {
        return Err("fee above hard cap".into());
    }
    if f.deposit_fee_e8s >= limits.min_deposit_e8s {
        return Err("deposit fee must be below the minimum deposit".into());
    }
    Ok(())
}

fn dedup_principals(ps: &[Principal]) -> Vec<Principal> {
    let mut seen = BTreeSet::new();
    ps.iter().filter(|p| seen.insert(**p)).copied().collect()
}

// ---------------------------------------------------------------------------
// Core singleton
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Windows {
    pub window_start_ns: u64,
    pub admitted_in_window: u128,
    pub released_in_window: u128,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct LaneState {
    pub address: Option<[u8; 20]>,
    pub public_key: Option<Vec<u8>>,
    /// Next nonce to assign. `None` until synced once from the chain.
    pub next_nonce: Option<u64>,
    /// The single tx without a receipt yet, if any.
    pub unincluded_tx: Option<u64>,
    /// Txs with a receipt that are not finalized yet.
    #[serde(default)]
    pub unfinalized_txs: Vec<u64>,
    #[serde(default)]
    pub awaiting_gas: bool,
    #[serde(default)]
    pub last_gas_balance_wei: Option<u128>,
    #[serde(default)]
    pub halted: Option<String>,
    /// Txs created but not yet given a nonce (admin lane queue), oldest first.
    #[serde(default)]
    pub queued_txs: Vec<u64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct ObserverState {
    /// Last block whose `BridgeOut` logs are fully processed.
    pub cursor_block: u64,
    /// Number of `BridgeOut` events processed (equals the contract's `bridgeOutCount` at
    /// `cursor_block`).
    pub processed_count: u64,
    pub last_finalized: Option<(u64, [u8; 32])>,
    pub last_error: Option<String>,
    pub last_run_ns: Option<u64>,
    #[serde(default)]
    pub consecutive_failures: u32,
    /// Highest Base block at which this canister settled a mint (delivery receipt).
    /// The exact supply match (C3) is only evaluated at a finalized head at or above it.
    #[serde(default)]
    pub max_settled_block: u64,
    /// Last successful full pass (finalized head read). Between full passes an idle
    /// observer only probes `bridgeOutCount()` at the finalized tag.
    #[serde(default)]
    pub last_full_pass_ns: Option<u64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Hint {
    pub tx_hash: [u8; 32],
    pub submitted_by: Principal,
    pub submitted_at_ns: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct HaltRecord {
    pub reason: String,
    pub at_ns: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BalanceSnapshot {
    pub balance: u128,
    pub at_ns: u64,
    /// Coverage result at that time.
    pub coverage_ok: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BaseSupplySnapshot {
    pub total_supply: u128,
    pub block_number: u64,
    pub inflight_mint: u128,
    pub quiescent: bool,
    pub within_bound: bool,
    pub exact_match: Option<bool>,
    pub at_ns: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SweepRecord {
    pub id: u64,
    pub amount_e8s: u64,
    pub op: LedgerOp,
    pub created_at_ns: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BridgeCore {
    pub schema_version: u32,
    pub initialized: bool,
    pub config: BridgeConfig,
    pub instance_salt: [u8; 32],
    #[serde(default)]
    pub next_deposit_id: u64,
    #[serde(default)]
    pub next_tx_id: u64,
    #[serde(default)]
    pub next_sweep_id: u64,
    #[serde(default)]
    pub buckets: Buckets,
    #[serde(default)]
    pub windows: Windows,
    #[serde(default)]
    pub minter_lane: LaneState,
    #[serde(default)]
    pub admin_lane: LaneState,
    #[serde(default)]
    pub observer: ObserverState,
    /// Deposits not in a terminal state (`Delivered`, `PullFailed`, `Refunded`).
    #[serde(default)]
    pub active_deposits: BTreeSet<u64>,
    /// Releases not in a terminal state (`Released`, `Reminted`).
    #[serde(default)]
    pub active_releases: BTreeSet<[u8; 32]>,
    #[serde(default)]
    pub hints: Vec<Hint>,
    #[serde(default)]
    pub halted: Option<HaltRecord>,
    #[serde(default)]
    pub escrow_snapshot: Option<BalanceSnapshot>,
    #[serde(default)]
    pub base_supply_snapshot: Option<BaseSupplySnapshot>,
    #[serde(default)]
    pub ledger_fee_e8s: Option<u64>,
    /// The icUSD ledger's minting account. A transfer to it is a burn, so releases
    /// addressed to it are held.
    #[serde(default)]
    pub ledger_minting_account: Option<Account>,
    /// Lower bound on the ledger log length, refreshed by the timer. Used as the start
    /// of an ICRC-3 search for an operation admitted later.
    #[serde(default)]
    pub ledger_tip_lower_bound: u64,
    #[serde(default)]
    pub sweeps: Vec<SweepRecord>,
}

impl Default for BridgeCore {
    fn default() -> Self {
        BridgeCore {
            schema_version: CORE_SCHEMA_VERSION,
            initialized: false,
            config: BridgeConfig::default(),
            instance_salt: [0u8; 32],
            next_deposit_id: 0,
            next_tx_id: 0,
            next_sweep_id: 0,
            buckets: Buckets::default(),
            windows: Windows::default(),
            minter_lane: LaneState::default(),
            admin_lane: LaneState::default(),
            observer: ObserverState::default(),
            active_deposits: BTreeSet::new(),
            active_releases: BTreeSet::new(),
            hints: vec![],
            halted: None,
            escrow_snapshot: None,
            base_supply_snapshot: None,
            ledger_fee_e8s: None,
            ledger_minting_account: None,
            ledger_tip_lower_bound: 0,
            sweeps: vec![],
        }
    }
}

impl BridgeCore {
    pub fn lane(&self, lane: LaneKind) -> &LaneState {
        match lane {
            LaneKind::Minter => &self.minter_lane,
            LaneKind::Admin => &self.admin_lane,
        }
    }

    pub fn lane_mut(&mut self, lane: LaneKind) -> &mut LaneState {
        match lane {
            LaneKind::Minter => &mut self.minter_lane,
            LaneKind::Admin => &mut self.admin_lane,
        }
    }

    pub fn is_admin(&self, p: &Principal) -> bool {
        self.config.admins.contains(p)
    }

    pub fn escrow_account(&self, canister: Principal) -> Account {
        Account { owner: canister, subaccount: Some(ids::escrow_subaccount()) }
    }

    /// Roll the shared admission/release window forward if it has elapsed.
    pub fn roll_window(&mut self, now_ns: u64) {
        let len = self.config.limits.window_seconds.saturating_mul(1_000_000_000);
        if now_ns >= self.windows.window_start_ns.saturating_add(len) {
            self.windows = Windows { window_start_ns: now_ns, admitted_in_window: 0, released_in_window: 0 };
        }
    }

    pub fn window_resets_at_ns(&self) -> u64 {
        self.windows
            .window_start_ns
            .saturating_add(self.config.limits.window_seconds.saturating_mul(1_000_000_000))
    }
}

cbor_storable!(BridgeCore);

// ---------------------------------------------------------------------------
// Ledger operations
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedgerOpKind {
    /// `icrc2_transfer_from(depositor -> escrow)`.
    Pull,
    /// `icrc1_transfer(escrow -> bridge-out destination)`.
    Release,
    /// `icrc1_transfer(escrow -> deposit source account)`.
    Refund,
    /// `icrc1_transfer(escrow -> fee recipient)`.
    Sweep,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum LedgerOpStatus {
    /// Persisted, not yet sent (or sent with no reply observed yet in this message).
    Pending,
    /// Sent at least once with an ambiguous outcome. Only `Ok`/`Duplicate` or a complete
    /// ICRC-3 search may resolve it.
    Unknown { last_error: String },
    Succeeded { block_index: u64 },
    Failed { reason: String },
}

/// One persisted ledger call tuple. The tuple is fixed before the first await and
/// reused unchanged on every retry, so the ledger's deduplication applies.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct LedgerOp {
    pub kind: LedgerOpKind,
    pub from: Account,
    pub to: Account,
    /// `Some(bridge canister default account)` for pulls.
    pub spender: Option<Account>,
    pub amount_e8s: u64,
    /// Explicit fee for escrow transfers (needed to compute the exact escrow outflow);
    /// `None` for pulls (the ledger charges the depositor).
    pub fee_e8s: Option<u64>,
    pub memo: Vec<u8>,
    pub created_at_time: u64,
    #[serde(default)]
    pub attempts: u32,
    /// True once any attempt had an ambiguous outcome.
    #[serde(default)]
    pub ambiguous_seen: bool,
    pub status: LedgerOpStatus,
    /// ICRC-3 search lower bound (ledger log length known before the first attempt).
    #[serde(default)]
    pub search_from_block: u64,
    /// Next block to inspect in an ongoing search.
    #[serde(default)]
    pub search_next_block: Option<u64>,
    #[serde(default)]
    pub last_attempt_ns: Option<u64>,
    /// True while this op's `escrow_outflow()` is counted in `unknown_outflow`. Set
    /// before the first send of a tuple, cleared when its outcome is proven.
    #[serde(default)]
    pub outflow_reserved: bool,
}

impl LedgerOp {
    /// Escrow decrease if this transfer executes: amount plus fee (zero for pulls).
    pub fn escrow_outflow(&self) -> u64 {
        match self.kind {
            LedgerOpKind::Pull => 0,
            _ => self.amount_e8s.saturating_add(self.fee_e8s.unwrap_or(0)),
        }
    }
}

// ---------------------------------------------------------------------------
// Deposits (ICP -> Base, and held-claim re-mints)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum DepositKind {
    UserDeposit,
    Remint { transfer_id: [u8; 32] },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum DepositState {
    PullPending,
    PullUnknown,
    PullFailed { reason: String },
    /// Backing is in `pending_mint`; waiting for the minter lane.
    Escrowed,
    /// A Base tx exists (see `active_tx`).
    MintInFlight,
    Delivered,
    /// Backing still in `pending_mint`; needs retry or (user deposits only) refund.
    MintHeld { reason: String },
    Refunding,
    Refunded,
}

impl DepositState {
    pub fn is_terminal(&self) -> bool {
        matches!(self, DepositState::Delivered | DepositState::PullFailed { .. } | DepositState::Refunded)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Inclusion {
    pub tx_hash: [u8; 32],
    pub block_number: u64,
    pub block_hash: [u8; 32],
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct DepositRecord {
    pub id: u64,
    pub kind: DepositKind,
    /// Depositor (user deposits) or the bridge canister itself (re-mints).
    pub owner: Principal,
    /// Pull source; `None` for re-mints.
    pub from: Option<Account>,
    pub client_request_id: Vec<u8>,
    pub base_recipient: [u8; 20],
    /// `A`: escrowed amount (pull amount, or held claim amount for a re-mint).
    pub amount_e8s: u64,
    pub deposit_fee_e8s: u64,
    /// `M = A - deposit_fee`.
    pub mint_amount_e8s: u64,
    pub mint_id: [u8; 32],
    pub state: DepositState,
    #[serde(default)]
    pub pull: Option<LedgerOp>,
    #[serde(default)]
    pub refund: Option<LedgerOp>,
    /// Every Base tx ever created for this deposit, oldest first.
    #[serde(default)]
    pub tx_ids: Vec<u64>,
    #[serde(default)]
    pub active_tx: Option<u64>,
    #[serde(default)]
    pub delivered: Option<Inclusion>,
    /// For holds caused by preflight conditions: when to return to `Escrowed`.
    #[serde(default)]
    pub hold_retry_at_ns: Option<u64>,
    pub created_at_ns: u64,
    pub updated_at_ns: u64,
}

cbor_storable!(DepositRecord);

// ---------------------------------------------------------------------------
// Releases (Base -> ICP)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ReleaseState {
    /// In `pending_release`; transfer pending, unknown, or not yet started.
    Pending,
    Released,
    /// In `held`; awaiting re-mint to the Base sender.
    Held { reason: String },
    /// Moved to a re-mint deposit.
    Reminting { deposit_id: u64 },
    Reminted { deposit_id: u64 },
}

impl ReleaseState {
    pub fn is_terminal(&self) -> bool {
        matches!(self, ReleaseState::Released | ReleaseState::Reminted { .. })
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ReleaseRecord {
    pub transfer_id: [u8; 32],
    pub sequence: u64,
    pub base_sender: [u8; 20],
    /// `B`: burned amount on Base.
    pub amount_e8s: u64,
    pub raw_icp_principal: Vec<u8>,
    pub icp_subaccount: [u8; 32],
    /// Parsed destination when valid.
    pub destination: Option<Account>,
    pub base_tx_hash: [u8; 32],
    pub base_block_number: u64,
    pub base_block_hash: [u8; 32],
    pub log_index: u64,
    /// Fee snapshot at observation time.
    pub release_fee_e8s: u64,
    pub state: ReleaseState,
    #[serde(default)]
    pub transfer: Option<LedgerOp>,
    pub created_at_ns: u64,
    pub updated_at_ns: u64,
}

cbor_storable!(ReleaseRecord);

// ---------------------------------------------------------------------------
// Base transactions
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum TxPurpose {
    /// Mint for a deposit (user deposit or re-mint).
    Mint { deposit_id: u64 },
    /// Canister-signed contract admin call.
    Admin { action: ContractAdminAction },
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttemptKind {
    Original,
    /// Same call, higher fees, same nonce.
    Replacement,
    /// Zero-value self-transfer at the same nonce (admin recovery).
    Cancel,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct TxAttempt {
    pub kind: AttemptKind,
    pub to: [u8; 20],
    pub value_wei: u128,
    pub data: Vec<u8>,
    pub max_fee_per_gas: u128,
    pub max_priority_fee_per_gas: u128,
    pub gas_limit: u64,
    /// Signed EIP-1559 bytes (`0x02 || rlp(...)`).
    pub raw: Vec<u8>,
    /// keccak256 of `raw`, computed locally before broadcast.
    pub tx_hash: [u8; 32],
    pub created_at_ns: u64,
    #[serde(default)]
    pub broadcast_count: u32,
    #[serde(default)]
    pub last_broadcast_ns: Option<u64>,
    #[serde(default)]
    pub last_broadcast_note: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum TxStatus {
    /// Nonce assigned; no signed attempt yet.
    Unsigned,
    /// At least one signed attempt; no receipt for any attempt yet.
    Pending,
    /// A receipt exists for attempt `attempt` (not final).
    Included { attempt: u32, block_number: u64, block_hash: [u8; 32], success: bool },
    /// The receipt block is at or below the quorum finalized head and canonical.
    Finalized { attempt: u32, block_number: u64, block_hash: [u8; 32], success: bool },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct EvmTxRecord {
    pub id: u64,
    pub lane: LaneKind,
    pub nonce: u64,
    pub purpose: TxPurpose,
    pub to: [u8; 20],
    pub value_wei: u128,
    pub data: Vec<u8>,
    pub gas_limit: u64,
    #[serde(default)]
    pub attempts: Vec<TxAttempt>,
    pub status: TxStatus,
    #[serde(default)]
    pub cancel_requested: bool,
    /// Outcome already applied to the purpose (deposit delivery etc.).
    #[serde(default)]
    pub outcome_applied: bool,
    pub created_at_ns: u64,
    pub updated_at_ns: u64,
    #[serde(default)]
    pub last_checked_ns: Option<u64>,
    #[serde(default)]
    pub note: Option<String>,
}

cbor_storable!(EvmTxRecord);

// ---------------------------------------------------------------------------
// Audit log
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct AuditRecord {
    pub seq: u64,
    pub at_ns: u64,
    pub transition: Option<Transition>,
    pub deposit_id: Option<u64>,
    pub transfer_id: Option<[u8; 32]>,
    pub tx_id: Option<u64>,
    pub note: String,
}

cbor_storable!(AuditRecord);

#[derive(Clone, Debug, Default)]
pub struct AuditCtx {
    pub deposit_id: Option<u64>,
    pub transfer_id: Option<[u8; 32]>,
    pub tx_id: Option<u64>,
    pub note: String,
}

impl AuditCtx {
    pub fn deposit(id: u64, note: impl Into<String>) -> Self {
        AuditCtx { deposit_id: Some(id), note: note.into(), ..Default::default() }
    }
    pub fn release(tid: [u8; 32], note: impl Into<String>) -> Self {
        AuditCtx { transfer_id: Some(tid), note: note.into(), ..Default::default() }
    }
    pub fn tx(id: u64, note: impl Into<String>) -> Self {
        AuditCtx { tx_id: Some(id), note: note.into(), ..Default::default() }
    }
    pub fn note(note: impl Into<String>) -> Self {
        AuditCtx { note: note.into(), ..Default::default() }
    }
}

// ---------------------------------------------------------------------------
// Index keys
// ---------------------------------------------------------------------------

/// `[len, principal bytes padded to 29, id BE]`.
pub type OwnerKey = [u8; 38];
/// `[principal len, principal padded to 29, request id len, request id padded to 32]`.
pub type RequestKey = [u8; 63];

fn principal_prefix(p: &Principal) -> [u8; 30] {
    let bytes = p.as_slice();
    let mut out = [0u8; 30];
    out[0] = bytes.len() as u8;
    out[1..1 + bytes.len()].copy_from_slice(bytes);
    out
}

pub fn owner_key(p: &Principal, id: u64) -> OwnerKey {
    let mut k = [0u8; 38];
    k[..30].copy_from_slice(&principal_prefix(p));
    k[30..].copy_from_slice(&id.to_be_bytes());
    k
}

pub fn request_key(p: &Principal, request_id: &[u8]) -> Result<RequestKey, String> {
    if request_id.len() < 16 || request_id.len() > 32 {
        return Err("client_request_id must be 16 to 32 bytes".into());
    }
    let mut k = [0u8; 63];
    k[..30].copy_from_slice(&principal_prefix(p));
    k[30] = request_id.len() as u8;
    k[31..31 + request_id.len()].copy_from_slice(request_id);
    Ok(k)
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

thread_local! {
    static MEMORY_MANAGER: RefCell<MemoryManager<DefaultMemoryImpl>> =
        RefCell::new(MemoryManager::init(DefaultMemoryImpl::default()));

    static CORE: RefCell<StableCell<BridgeCore, Mem>> = RefCell::new(
        StableCell::init(mem(MEM_CORE), BridgeCore::default()).expect("init core cell"));

    static DEPOSITS: RefCell<StableBTreeMap<u64, DepositRecord, Mem>> =
        RefCell::new(StableBTreeMap::init(mem(MEM_DEPOSITS)));

    static RELEASES: RefCell<StableBTreeMap<[u8; 32], ReleaseRecord, Mem>> =
        RefCell::new(StableBTreeMap::init(mem(MEM_RELEASES)));

    static TXS: RefCell<StableBTreeMap<u64, EvmTxRecord, Mem>> =
        RefCell::new(StableBTreeMap::init(mem(MEM_TXS)));

    static OWNER_DEPOSITS: RefCell<StableBTreeMap<OwnerKey, (), Mem>> =
        RefCell::new(StableBTreeMap::init(mem(MEM_OWNER_DEPOSITS)));

    static REQUEST_IDS: RefCell<StableBTreeMap<RequestKey, u64, Mem>> =
        RefCell::new(StableBTreeMap::init(mem(MEM_REQUEST_IDS)));

    static OWNER_RELEASES: RefCell<StableBTreeMap<OwnerKey, [u8; 32], Mem>> =
        RefCell::new(StableBTreeMap::init(mem(MEM_OWNER_RELEASES)));

    static RELEASE_BY_SEQ: RefCell<StableBTreeMap<u64, [u8; 32], Mem>> =
        RefCell::new(StableBTreeMap::init(mem(MEM_RELEASE_BY_SEQ)));

    static AUDIT: RefCell<StableLog<AuditRecord, Mem, Mem>> = RefCell::new(
        StableLog::init(mem(MEM_AUDIT_INDEX), mem(MEM_AUDIT_DATA)).expect("init audit log"));
}

fn mem(id: u8) -> Mem {
    MEMORY_MANAGER.with(|m| m.borrow().get(MemoryId::new(id)))
}

pub fn read_core<R>(f: impl FnOnce(&BridgeCore) -> R) -> R {
    CORE.with(|c| f(c.borrow().get()))
}

/// Read-modify-write of the core cell. The closure runs on a copy; the copy is written
/// back only after the closure returns.
pub fn mutate_core<R>(f: impl FnOnce(&mut BridgeCore) -> R) -> R {
    CORE.with(|c| {
        let mut cell = c.borrow_mut();
        let mut core = cell.get().clone();
        let r = f(&mut core);
        cell.set(core).expect("write core cell");
        r
    })
}

/// Apply an accounting transition and append an audit record. On error nothing changes.
pub fn apply_transition(t: Transition, ctx: AuditCtx) -> Result<(), AccountingError> {
    let applied = mutate_core(|c| {
        let next = c.buckets.apply(&t)?;
        c.buckets = next;
        Ok::<(), AccountingError>(())
    });
    if applied.is_ok() {
        append_audit(Some(t), ctx);
    }
    applied
}

/// Apply several transitions as one unit: all or nothing.
pub fn apply_transitions(ts: &[Transition], ctx: AuditCtx) -> Result<(), AccountingError> {
    let applied = mutate_core(|c| {
        let next = ts.iter().try_fold(c.buckets, |b, t| b.apply(t))?;
        c.buckets = next;
        Ok::<(), AccountingError>(())
    });
    if applied.is_ok() {
        for t in ts {
            append_audit(Some(t.clone()), ctx.clone());
        }
    }
    applied
}

pub fn append_audit(transition: Option<Transition>, ctx: AuditCtx) {
    let at_ns = crate::env::now_ns();
    AUDIT.with(|a| {
        let log = a.borrow_mut();
        let seq = log.len();
        let rec = AuditRecord {
            seq,
            at_ns,
            transition,
            deposit_id: ctx.deposit_id,
            transfer_id: ctx.transfer_id,
            tx_id: ctx.tx_id,
            note: ctx.note,
        };
        log.append(&rec).expect("append audit record");
    });
}

pub fn audit_len() -> u64 {
    AUDIT.with(|a| a.borrow().len())
}

pub fn audit_page(start: u64, limit: u32) -> Vec<AuditRecord> {
    AUDIT.with(|a| {
        let log = a.borrow();
        let end = start.saturating_add(limit.min(MAX_PAGE) as u64).min(log.len());
        (start..end).filter_map(|i| log.get(i)).collect()
    })
}

/// Record the halt reason (first reason wins) and log it.
pub fn halt(reason: impl Into<String>) {
    let reason = reason.into();
    let now = crate::env::now_ns();
    let newly = mutate_core(|c| {
        if c.halted.is_none() {
            c.halted = Some(HaltRecord { reason: reason.clone(), at_ns: now });
            true
        } else {
            false
        }
    });
    if newly {
        append_audit(None, AuditCtx::note(format!("HALT: {reason}")));
    }
}

pub fn is_halted() -> bool {
    read_core(|c| c.halted.is_some())
}

// Deposits ------------------------------------------------------------------

pub fn get_deposit(id: u64) -> Option<DepositRecord> {
    DEPOSITS.with(|m| m.borrow().get(&id))
}

/// Insert or update a deposit and keep the active set in sync.
pub fn put_deposit(d: &DepositRecord) {
    let terminal = d.state.is_terminal();
    DEPOSITS.with(|m| m.borrow_mut().insert(d.id, d.clone()));
    mutate_core(|c| {
        if terminal {
            c.active_deposits.remove(&d.id);
        } else {
            c.active_deposits.insert(d.id);
        }
    });
}

pub fn index_owner_deposit(owner: &Principal, id: u64) {
    OWNER_DEPOSITS.with(|m| m.borrow_mut().insert(owner_key(owner, id), ()));
}

pub fn list_owner_deposits(owner: &Principal, start_after: Option<u64>, limit: u32) -> Vec<DepositRecord> {
    let limit = limit.clamp(1, MAX_PAGE) as usize;
    // Newest first.
    let upper = match start_after {
        Some(0) => return vec![],
        Some(id) => owner_key(owner, id - 1),
        None => owner_key(owner, u64::MAX),
    };
    let lower = owner_key(owner, 0);
    let ids: Vec<u64> = OWNER_DEPOSITS.with(|m| {
        m.borrow()
            .range(lower..=upper)
            .rev()
            .take(limit)
            .map(|(k, _)| u64::from_be_bytes(k[30..].try_into().unwrap()))
            .collect()
    });
    ids.into_iter().filter_map(get_deposit).collect()
}

pub fn lookup_request(key: &RequestKey) -> Option<u64> {
    REQUEST_IDS.with(|m| m.borrow().get(key))
}

pub fn insert_request(key: RequestKey, deposit_id: u64) {
    REQUEST_IDS.with(|m| m.borrow_mut().insert(key, deposit_id));
}

pub fn deposits_in_state(pred: impl Fn(&DepositRecord) -> bool, limit: usize) -> Vec<DepositRecord> {
    let ids: Vec<u64> = read_core(|c| c.active_deposits.iter().copied().collect());
    ids.into_iter().filter_map(get_deposit).filter(|d| pred(d)).take(limit).collect()
}

// Releases ------------------------------------------------------------------

pub fn get_release(tid: &[u8; 32]) -> Option<ReleaseRecord> {
    RELEASES.with(|m| m.borrow().get(tid))
}

pub fn put_release(r: &ReleaseRecord) {
    let terminal = r.state.is_terminal();
    RELEASES.with(|m| m.borrow_mut().insert(r.transfer_id, r.clone()));
    mutate_core(|c| {
        if terminal {
            c.active_releases.remove(&r.transfer_id);
        } else {
            c.active_releases.insert(r.transfer_id);
        }
    });
}

pub fn release_by_sequence(seq: u64) -> Option<[u8; 32]> {
    RELEASE_BY_SEQ.with(|m| m.borrow().get(&seq))
}

pub fn index_release(r: &ReleaseRecord) {
    RELEASE_BY_SEQ.with(|m| m.borrow_mut().insert(r.sequence, r.transfer_id));
    if let Some(dest) = &r.destination {
        OWNER_RELEASES.with(|m| m.borrow_mut().insert(owner_key(&dest.owner, r.sequence), r.transfer_id));
    }
}

pub fn list_owner_releases(owner: &Principal, start_after_sequence: Option<u64>, limit: u32) -> Vec<ReleaseRecord> {
    let limit = limit.clamp(1, MAX_PAGE) as usize;
    let upper = match start_after_sequence {
        Some(0) => return vec![],
        Some(s) => owner_key(owner, s - 1),
        None => owner_key(owner, u64::MAX),
    };
    let lower = owner_key(owner, 0);
    let tids: Vec<[u8; 32]> =
        OWNER_RELEASES.with(|m| m.borrow().range(lower..=upper).rev().take(limit).map(|(_, v)| v).collect());
    tids.iter().filter_map(get_release).collect()
}

pub fn releases_in_state(pred: impl Fn(&ReleaseRecord) -> bool, limit: usize) -> Vec<ReleaseRecord> {
    let tids: Vec<[u8; 32]> = read_core(|c| c.active_releases.iter().copied().collect());
    tids.iter().filter_map(get_release).filter(|r| pred(r)).take(limit).collect()
}

// Txs -----------------------------------------------------------------------

pub fn get_tx(id: u64) -> Option<EvmTxRecord> {
    TXS.with(|m| m.borrow().get(&id))
}

pub fn put_tx(t: &EvmTxRecord) {
    TXS.with(|m| m.borrow_mut().insert(t.id, t.clone()));
}

pub fn next_tx_id() -> u64 {
    mutate_core(|c| {
        let id = c.next_tx_id;
        c.next_tx_id += 1;
        id
    })
}

/// Sum of mint amounts of deposits with at least one signed attempt that is not
/// finalized yet. Those mints may already exist on Base (invariant C2).
pub fn inflight_mint_e8s() -> u128 {
    deposits_in_state(|d| matches!(d.state, DepositState::MintInFlight), usize::MAX)
        .iter()
        .filter(|d| {
            d.active_tx
                .and_then(get_tx)
                .map(|t| !t.attempts.is_empty() && !matches!(t.status, TxStatus::Finalized { .. }))
                .unwrap_or(false)
        })
        .map(|d| d.mint_amount_e8s as u128)
        .sum()
}

/// Test-only reset of every structure (host unit tests share thread-locals).
#[cfg(test)]
pub fn reset_for_tests() {
    CORE.with(|c| {
        c.borrow_mut().set(BridgeCore::default()).unwrap();
    });
    DEPOSITS.with(|m| {
        let keys: Vec<u64> = m.borrow().iter().map(|(k, _)| k).collect();
        for k in keys {
            m.borrow_mut().remove(&k);
        }
    });
    RELEASES.with(|m| {
        let keys: Vec<[u8; 32]> = m.borrow().iter().map(|(k, _)| k).collect();
        for k in keys {
            m.borrow_mut().remove(&k);
        }
    });
    TXS.with(|m| {
        let keys: Vec<u64> = m.borrow().iter().map(|(k, _)| k).collect();
        for k in keys {
            m.borrow_mut().remove(&k);
        }
    });
    OWNER_DEPOSITS.with(|m| {
        let keys: Vec<OwnerKey> = m.borrow().iter().map(|(k, _)| k).collect();
        for k in keys {
            m.borrow_mut().remove(&k);
        }
    });
    REQUEST_IDS.with(|m| {
        let keys: Vec<RequestKey> = m.borrow().iter().map(|(k, _)| k).collect();
        for k in keys {
            m.borrow_mut().remove(&k);
        }
    });
    OWNER_RELEASES.with(|m| {
        let keys: Vec<OwnerKey> = m.borrow().iter().map(|(k, _)| k).collect();
        for k in keys {
            m.borrow_mut().remove(&k);
        }
    });
    RELEASE_BY_SEQ.with(|m| {
        let keys: Vec<u64> = m.borrow().iter().map(|(k, _)| k).collect();
        for k in keys {
            m.borrow_mut().remove(&k);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::L2MainnetService;

    fn p(n: u8) -> Principal {
        Principal::from_slice(&[n; 10])
    }

    fn init_args(chain: u64, key: &str) -> InitArgs {
        InitArgs {
            icusd_ledger: p(1),
            evm_rpc: p(2),
            ecdsa_key_name: key.into(),
            base_chain_id: chain,
            admins: vec![p(3)],
            fee_recipient: None,
            rpc_services: vec![
                RpcServiceConfig::Custom { url: "https://a.example".into() },
                RpcServiceConfig::Custom { url: "https://b.example".into() },
                RpcServiceConfig::Custom { url: "https://c.example".into() },
            ],
            min_quorum: 2,
        }
    }

    #[test]
    fn memory_layout_is_unique() {
        let mut ids = BTreeSet::new();
        let mut names = BTreeSet::new();
        for (id, name) in MEMORY_LAYOUT {
            assert!(ids.insert(*id), "duplicate memory id {id}");
            assert!(names.insert(*name), "duplicate memory name {name}");
        }
        assert_eq!(ids.len(), 10);
        assert_eq!(*ids.iter().next_back().unwrap(), MEM_AUDIT_DATA);
    }

    #[test]
    fn init_validation_binds_key_to_network() {
        assert!(validate_init(&init_args(CHAIN_BASE_MAINNET, "key_1")).is_ok());
        assert!(validate_init(&init_args(CHAIN_BASE_MAINNET, "test_key_1")).is_err());
        assert!(validate_init(&init_args(CHAIN_BASE_SEPOLIA, "test_key_1")).is_ok());
        assert!(validate_init(&init_args(CHAIN_LOCAL_ANVIL, "key_1")).is_err());
        assert!(validate_init(&init_args(CHAIN_LOCAL_ANVIL, "dfx_test_key")).is_ok());
        assert!(validate_init(&init_args(1, "key_1")).is_err());
        assert!(validate_init(&init_args(1030, "key_1")).is_err());
    }

    #[test]
    fn init_validation_rejects_bad_admins_and_rpc() {
        let mut a = init_args(CHAIN_BASE_SEPOLIA, "test_key_1");
        a.admins = vec![];
        assert!(validate_init(&a).is_err());
        let mut a = init_args(CHAIN_BASE_SEPOLIA, "test_key_1");
        a.admins = vec![Principal::anonymous()];
        assert!(validate_init(&a).is_err());
        let mut a = init_args(CHAIN_BASE_SEPOLIA, "test_key_1");
        a.min_quorum = 4;
        assert!(validate_init(&a).is_err());
        let mut a = init_args(CHAIN_BASE_SEPOLIA, "test_key_1");
        a.rpc_services.push(RpcServiceConfig::Custom { url: "https://a.example/".into() });
        assert!(validate_init(&a).is_err(), "duplicate URL modulo trailing slash");
        let mut a = init_args(CHAIN_BASE_SEPOLIA, "test_key_1");
        a.rpc_services[0] = RpcServiceConfig::Custom { url: "http://a.example".into() };
        assert!(validate_init(&a).is_err(), "plain http only for local anvil");
        let mut a = init_args(CHAIN_BASE_SEPOLIA, "test_key_1");
        a.rpc_services[0] = RpcServiceConfig::BaseMainnet(L2MainnetService::Alchemy);
        assert!(validate_init(&a).is_err(), "built-in mainnet provider on testnet");
        let mut a = init_args(CHAIN_BASE_MAINNET, "key_1");
        a.min_quorum = 1;
        assert!(validate_init(&a).is_err(), "mainnet needs quorum >= 2");
    }

    #[test]
    fn fresh_config_starts_with_deposits_paused() {
        let c = validate_init(&init_args(CHAIN_BASE_SEPOLIA, "test_key_1")).unwrap();
        assert!(c.pauses.deposits);
        assert!(c.contract.is_none());
    }

    #[test]
    fn limit_and_fee_validation() {
        let l = default_limits();
        assert!(validate_limits(&l).is_ok());
        let mut bad = l.clone();
        bad.min_deposit_e8s = 0;
        assert!(validate_limits(&bad).is_err());
        let mut bad = l.clone();
        bad.max_deposit_e8s = bad.max_outstanding_e8s + 1;
        assert!(validate_limits(&bad).is_err());
        assert!(validate_fees(&BridgeFees { deposit_fee_e8s: l.min_deposit_e8s, release_fee_e8s: 0 }, &l).is_err());
        assert!(validate_fees(&BridgeFees { deposit_fee_e8s: 0, release_fee_e8s: MAX_RELEASE_FEE_E8S + 1 }, &l).is_err());
        assert!(validate_fees(&BridgeFees { deposit_fee_e8s: 1, release_fee_e8s: 1 }, &l).is_ok());
    }

    #[test]
    fn core_round_trips_and_tolerates_missing_fields() {
        let mut core = BridgeCore::default();
        core.initialized = true;
        core.buckets.outstanding_base = u128::from(u64::MAX) * 3;
        core.active_releases.insert([7u8; 32]);
        core.minter_lane.next_nonce = Some(42);
        let bytes = cbor_encode(&core);
        let back: BridgeCore = cbor_decode(&bytes);
        assert_eq!(back, core);

        // An older encoding without the later `#[serde(default)]` fields still decodes.
        #[derive(Serialize)]
        struct OldCore {
            schema_version: u32,
            initialized: bool,
            config: BridgeConfig,
            instance_salt: [u8; 32],
        }
        let old = OldCore {
            schema_version: 1,
            initialized: true,
            config: BridgeConfig::default(),
            instance_salt: [5u8; 32],
        };
        let decoded: BridgeCore = cbor_decode(&cbor_encode(&old));
        assert!(decoded.initialized);
        assert_eq!(decoded.instance_salt, [5u8; 32]);
        assert_eq!(decoded.buckets, Buckets::default());
        assert!(decoded.hints.is_empty());
    }

    #[test]
    fn request_key_bounds() {
        assert!(request_key(&p(1), &[0u8; 15]).is_err());
        assert!(request_key(&p(1), &[0u8; 33]).is_err());
        let a = request_key(&p(1), &[1u8; 16]).unwrap();
        let b = request_key(&p(1), &[1u8; 17]).unwrap();
        let c = request_key(&p(2), &[1u8; 16]).unwrap();
        assert_ne!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn owner_index_pages_newest_first_and_isolates_owners() {
        reset_for_tests();
        let owner = p(9);
        let other = p(8);
        for id in 0..5u64 {
            let d = sample_deposit(id, owner);
            put_deposit(&d);
            index_owner_deposit(&owner, id);
        }
        let d = sample_deposit(5, other);
        put_deposit(&d);
        index_owner_deposit(&other, 5);

        let page = list_owner_deposits(&owner, None, 2);
        assert_eq!(page.iter().map(|d| d.id).collect::<Vec<_>>(), vec![4, 3]);
        let page = list_owner_deposits(&owner, Some(3), 10);
        assert_eq!(page.iter().map(|d| d.id).collect::<Vec<_>>(), vec![2, 1, 0]);
        assert!(list_owner_deposits(&owner, Some(0), 10).is_empty());
        assert_eq!(list_owner_deposits(&other, None, 10).len(), 1);
    }

    #[test]
    fn active_sets_follow_terminal_states() {
        reset_for_tests();
        let mut d = sample_deposit(1, p(1));
        put_deposit(&d);
        assert!(read_core(|c| c.active_deposits.contains(&1)));
        d.state = DepositState::Delivered;
        put_deposit(&d);
        assert!(!read_core(|c| c.active_deposits.contains(&1)));
    }

    #[test]
    fn transitions_are_atomic_and_audited() {
        reset_for_tests();
        let before = audit_len();
        apply_transition(Transition::AdmitDeposit { amount: 10 }, AuditCtx::deposit(1, "admit")).unwrap();
        assert_eq!(read_core(|c| c.buckets.reserved_pulls), 10);
        assert_eq!(audit_len(), before + 1);
        // A failing batch changes nothing.
        let err = apply_transitions(
            &[Transition::PullSucceeded { amount: 10 }, Transition::PullSucceeded { amount: 1 }],
            AuditCtx::deposit(1, "bad"),
        );
        assert!(err.is_err());
        assert_eq!(read_core(|c| c.buckets.reserved_pulls), 10);
        assert_eq!(read_core(|c| c.buckets.pending_mint), 0);
        assert_eq!(audit_len(), before + 1);
    }

    #[test]
    fn halt_keeps_first_reason() {
        reset_for_tests();
        halt("first");
        halt("second");
        assert_eq!(read_core(|c| c.halted.clone()).unwrap().reason, "first");
    }

    pub(crate) fn sample_deposit(id: u64, owner: Principal) -> DepositRecord {
        DepositRecord {
            id,
            kind: DepositKind::UserDeposit,
            owner,
            from: Some(Account { owner, subaccount: None }),
            client_request_id: vec![id as u8; 16],
            base_recipient: [1u8; 20],
            amount_e8s: 100,
            deposit_fee_e8s: 0,
            mint_amount_e8s: 100,
            mint_id: [2u8; 32],
            state: DepositState::Escrowed,
            pull: None,
            refund: None,
            tx_ids: vec![],
            active_tx: None,
            delivered: None,
            hold_retry_at_ns: None,
            created_at_ns: 0,
            updated_at_ns: 0,
        }
    }
}
