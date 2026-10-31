//! The stable runtime core of the ICP-side sagas (BB-53, 53a): configuration, the escrow buckets,
//! the persisted ledger operations, deposits, releases, the audit log and the bounded work queues.
//!
//! **Fresh install only.** There is no supported upgrade from a BB-52 image to this runtime:
//! `post_upgrade` of an image without the runtime record traps ("the saga runtime is not
//! initialized; reinstall") and the previous module stays. No migration endpoint exists and none is
//! planned: nothing is deployed, so the BB-53 instance is installed fresh with the saga install
//! arguments. Do not read the "preserved byte for byte" below as an upgrade promise.
//!
//! `state.rs` keeps the frozen identity of the instance (`SuiCore`: network, gate binding, role
//! keys, provider set, memory 0, schema 1) untouched. This module adds a SECOND record
//! ([`BridgeCore`], memory 1) plus the per-record maps, so the BB-52 identity fields stay byte for
//! byte what BB-52 wrote. `state` re-exports everything here, so the saga modules (copied from the
//! frozen Solana instance at 4394b472) keep their `state::...` paths; only the accessors are renamed
//! (`read_runtime`, `mutate_runtime`).
//!
//! Records are CBOR (`ciborium`); CBOR maps fields by name, so a field added later with
//! `#[serde(default)]` decodes from older bytes. The pinned images in the tests catch a rename or
//! retype before it can wipe state at an upgrade.
//!
//! Memory ids are permanent. 0 is `SuiCore`; the saga uses the ids below; later sub-milestones
//! take new ids and never reuse these ([`MEMORY_LAYOUT`] is checked for duplicates).

use crate::accounting::{AccountingError, Buckets, Transition};
use crate::ids::{self, Hash32};
use crate::state::{cbor_encode, mem, try_cbor_decode, Mem};
use crate::types::*;
use candid::Principal;
use ic_stable_structures::storable::Bound;
use ic_stable_structures::{StableBTreeMap, StableCell, StableLog, Storable};
use icrc_ledger_types::icrc1::account::Account;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::cell::RefCell;

pub const MEM_RUNTIME: u8 = 1;
pub const MEM_AUDIT_INDEX: u8 = 2;
pub const MEM_AUDIT_DATA: u8 = 3;
pub const MEM_DEPOSITS: u8 = 4;
pub const MEM_RELEASES: u8 = 5;
pub const MEM_OWNER_DEPOSITS: u8 = 6;
pub const MEM_REQUEST_IDS: u8 = 7;
pub const MEM_ACTIVE_DEPOSITS: u8 = 8;
pub const MEM_ACTIVE_REMINTS: u8 = 9;
pub const MEM_ACTIVE_RELEASES: u8 = 10;
pub const MEM_OWNER_RELEASES: u8 = 11;
pub const MEM_SENDER_RELEASES: u8 = 12;
pub const MEM_RELEASE_BY_SEQUENCE: u8 = 13;
pub const MEM_MINT_OPS: u8 = 14;
pub const MEM_MINT_OPS_BY_DEPOSIT: u8 = 15;

/// Every memory id this crate uses, for the duplicate check and as documentation of the layout.
pub const MEMORY_LAYOUT: &[(u8, &str)] = &[
    (0, "SuiCore (identity: network, binding, roles, providers) [BB-52]"),
    (MEM_RUNTIME, "BridgeCore (config, buckets, counters)"),
    (MEM_AUDIT_INDEX, "audit log index"),
    (MEM_AUDIT_DATA, "audit log data"),
    (MEM_DEPOSITS, "deposits by id"),
    (MEM_RELEASES, "releases by transfer id"),
    (MEM_OWNER_DEPOSITS, "deposits by owner"),
    (MEM_REQUEST_IDS, "(caller, client request id) to deposit id"),
    (MEM_ACTIVE_DEPOSITS, "non-terminal deposits"),
    (MEM_ACTIVE_REMINTS, "non-terminal re-mint deposits"),
    (MEM_ACTIVE_RELEASES, "non-terminal releases by creation sequence"),
    (MEM_OWNER_RELEASES, "releases by ICP destination owner"),
    (MEM_SENDER_RELEASES, "releases by Sui sender"),
    (MEM_RELEASE_BY_SEQUENCE, "releases by Gate bridge_out sequence"),
    (MEM_MINT_OPS, "mint operations by id [53b]"),
    (MEM_MINT_OPS_BY_DEPOSIT, "mint operations by (deposit id, operation id) [53b]"),
];

/// Version of the runtime record. 1 = BB-53 53a.
pub const RUNTIME_SCHEMA_VERSION: u32 = 1;
/// Maximum page size for list queries.
pub const MAX_PAGE: u32 = 100;

macro_rules! cbor_storable {
    ($t:ty) => {
        impl Storable for $t {
            fn to_bytes(&self) -> Cow<'_, [u8]> {
                Cow::Owned(cbor_encode(self))
            }
            // `Storable::from_bytes` has no error channel. A record that does not decode is corrupt
            // state, and trapping (which rolls the message or upgrade back) is the only safe response.
            fn from_bytes(bytes: Cow<[u8]>) -> Self {
                match try_cbor_decode(bytes.as_ref()) {
                    Ok(v) => v,
                    Err(e) => panic!("stored bridge record does not decode: {e}"),
                }
            }
            const BOUND: Bound = Bound::Unbounded;
        }
    };
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BridgeConfig {
    pub icusd_ledger: Principal,
    pub admins: Vec<Principal>,
    pub fee_recipient: Option<Account>,
    #[serde(default)]
    pub fees: BridgeFees,
    #[serde(default)]
    pub pauses: Pauses,
    /// Deposit, capacity and rate limits.
    #[serde(default = "default_limits")]
    pub limits: BridgeLimits,
    /// The mint lane's gas settings (53b).
    #[serde(default)]
    pub lane: LaneConfig,
}

/// The mint lane's settings. The gas coin is an identity (controller-set, like the gate binding); the
/// budget and the attempt limit are admin-tunable within hard bounds.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct LaneConfig {
    /// The one SUI coin object, owned by the `gas` role address, that pays for every mint. A mint
    /// reserves it for the life of its attempt; nothing funds it automatically.
    #[serde(default)]
    pub gas_coin: Option<Hash32>,
    /// `GasData.budget` in MIST. Unused gas is refunded; the coin must hold at least this much.
    #[serde(default = "default_gas_budget")]
    pub gas_budget: u64,
    /// How many signed attempts one operation may use (each after the previous one's finalized
    /// failure receipt) before the operation fails to a held deposit.
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
}

pub const DEFAULT_GAS_BUDGET: u64 = 50_000_000;
pub const MAX_ATTEMPTS_LIMIT: u32 = 5;

fn default_gas_budget() -> u64 {
    DEFAULT_GAS_BUDGET
}

fn default_max_attempts() -> u32 {
    3
}

impl Default for LaneConfig {
    fn default() -> Self {
        LaneConfig { gas_coin: None, gas_budget: DEFAULT_GAS_BUDGET, max_attempts: 3 }
    }
}

pub fn validate_lane(l: &LaneConfig) -> Result<(), String> {
    if !(crate::tx::MIN_GAS_BUDGET..=crate::tx::MAX_GAS_BUDGET).contains(&l.gas_budget) {
        return Err("gas_budget must be between 2,000 MIST and 50 SUI".into());
    }
    if l.max_attempts == 0 || l.max_attempts > MAX_ATTEMPTS_LIMIT {
        return Err(format!("max_attempts must be 1 to {MAX_ATTEMPTS_LIMIT}"));
    }
    if l.gas_coin == Some([0u8; 32]) {
        return Err("the gas coin id is zero".into());
    }
    Ok(())
}

/// Starting limits, the Base instance's numbers: 1 to 1,000 icUSD per deposit, 10,000 outstanding,
/// 5,000 per day in each direction, 5 active deposits per caller and 500 in all.
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

/// Hard upper bounds on admin-settable fees, so a misconfigured admin cannot take a material share
/// of a transfer.
pub const MAX_DEPOSIT_FEE_E8S: u64 = 10 * 100_000_000;
pub const MAX_RELEASE_FEE_E8S: u64 = 10 * 100_000_000;

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

impl Default for BridgeConfig {
    fn default() -> Self {
        BridgeConfig {
            icusd_ledger: Principal::anonymous(),
            admins: vec![],
            fee_recipient: None,
            fees: BridgeFees::default(),
            // A fresh canister starts with deposits paused until the gate binding and role keys
            // are set and an admin explicitly unpauses.
            pauses: Pauses { deposits: true, mints: false, releases: false, observer: false },
            limits: default_limits(),
            lane: LaneConfig::default(),
        }
    }
}

fn dedup_principals(ps: &[Principal]) -> Vec<Principal> {
    let mut out: Vec<Principal> = vec![];
    for p in ps {
        if !out.contains(p) {
            out.push(*p);
        }
    }
    out
}

/// Validate the saga part of the install arguments into a runtime config. Pure, unit tested.
pub fn validate_runtime_init(args: &InitArgs) -> Result<BridgeConfig, String> {
    if args.admins.is_empty() {
        return Err("at least one admin is required".into());
    }
    if args.admins.iter().any(|p| *p == Principal::anonymous()) {
        return Err("anonymous principal cannot be an admin".into());
    }
    if args.icusd_ledger == Principal::anonymous() {
        return Err("the icUSD ledger principal is required".into());
    }
    let limits = args.limits.clone().unwrap_or_else(default_limits);
    validate_limits(&limits)?;
    let fees = args.fees.clone().unwrap_or_default();
    validate_fees(&fees, &limits)?;
    Ok(BridgeConfig {
        icusd_ledger: args.icusd_ledger,
        admins: dedup_principals(&args.admins),
        fee_recipient: args.fee_recipient,
        fees,
        limits,
        ..BridgeConfig::default()
    })
}

/// The stored runtime config must still satisfy the install-time rules (re-checked at upgrade).
pub fn validate_stored_runtime(c: &BridgeConfig) -> Result<(), String> {
    if c.icusd_ledger == Principal::anonymous() {
        return Err("the stored icUSD ledger is anonymous".into());
    }
    if c.admins.is_empty() || c.admins.iter().any(|p| *p == Principal::anonymous()) {
        return Err("the stored admin list is empty or contains the anonymous principal".into());
    }
    validate_limits(&c.limits)?;
    validate_lane(&c.lane)?;
    validate_fees(&c.fees, &c.limits)
}

// ---------------------------------------------------------------------------
// Core singleton
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct HaltRecord {
    pub reason: String,
    pub at_ns: u64,
}

/// The shared admission and release window: what was admitted for minting and what was released to
/// ICP since `window_start_ns`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Windows {
    pub window_start_ns: u64,
    pub admitted_in_window: u128,
    pub released_in_window: u128,
}

/// "Look at this Sui transaction sooner" (`notify_sui_return`, 53c). A hint never settles anything:
/// the return observer reads it only to fetch the digest early, and a return is credited by the same
/// strict verification whether or not it was hinted.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Hint {
    pub digest: Hash32,
    pub submitted_by: Principal,
    pub submitted_at_ns: u64,
}

/// One fee sweep: escrow to the fee recipient, with its own persisted ledger tuple.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SweepRecord {
    pub id: u64,
    /// The fee bucket reduction on success (`transfer amount + ledger fee`).
    pub amount_e8s: u64,
    pub op: LedgerOp,
    pub created_at_ns: u64,
}

pub const MAX_HINTS: usize = 256;
pub const MAX_SWEEPS: usize = 64;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BridgeCore {
    pub schema_version: u32,
    pub initialized: bool,
    pub config: BridgeConfig,
    #[serde(default)]
    pub buckets: Buckets,
    #[serde(default)]
    pub halted: Option<HaltRecord>,
    #[serde(default)]
    pub next_deposit_id: u64,
    #[serde(default)]
    pub next_sweep_id: u64,
    /// The next release sequence (the creation order of release records).
    #[serde(default)]
    pub next_release_seq: u64,
    #[serde(default)]
    pub windows: Windows,
    /// Pending return hints (bounded).
    #[serde(default)]
    pub hints: Vec<Hint>,
    /// The icUSD ledger's transfer fee, refreshed by the timer.
    #[serde(default)]
    pub ledger_fee_e8s: Option<u64>,
    /// The icUSD ledger's minting account. A transfer to it is a burn, so releases addressed to it
    /// are held.
    #[serde(default)]
    pub ledger_minting_account: Option<Account>,
    /// A lower bound on the ledger log length, refreshed by the timer: where the ICRC-3 search of an
    /// operation created later starts.
    #[serde(default)]
    pub ledger_tip_lower_bound: u64,
    /// The most recent fee sweeps (bounded; terminal ones are dropped first).
    #[serde(default)]
    pub sweeps: Vec<SweepRecord>,
    /// What the last successful bind observation saw (53b). `None` until `observe_deployment` agrees.
    #[serde(default)]
    pub observed: Option<ObservedDeployment>,
    /// The next mint operation id.
    #[serde(default)]
    pub next_mint_op_id: u64,
    /// The one mint operation in flight. One at a time: the `MintCap` and the gas coin are owned
    /// objects whose version a transaction consumes, so a second transaction built while the first is
    /// unresolved could only equivocate.
    #[serde(default)]
    pub active_mint_op: Option<u64>,
    /// The mint lane's own halt (a foreign consumer of the `MintCap`, a mismatched consumed mint
    /// record). It stops minting only; releases and refunds continue.
    #[serde(default)]
    pub mint_lane_halted: Option<HaltRecord>,
    /// The highest checkpoint at which one of this canister's own transactions finalized. A new
    /// attempt reads its owned inputs at a checkpoint at or after it, so it never builds on a
    /// reference its own earlier transaction already spent.
    #[serde(default)]
    pub last_final_checkpoint: u64,
    /// Why the lane last waited without changing anything (providers unavailable or disagreeing, a
    /// stale head); shown to operators, never an authorization.
    #[serde(default)]
    pub lane_wait: Option<String>,
    /// The lane does not ask the providers for fresh facts before this time (set after an unavailable
    /// or disagreeing answer, so a persistent outage does not burn cycles every pass).
    #[serde(default)]
    pub gather_retry_at_ns: u64,
    /// The return observer's cursor, candidates and sequence progress (53c).
    #[serde(default)]
    pub observer: ObserverState,
    /// Mints this canister has settled as delivered: how many, and their total (e8s). The Gate's own
    /// counters must agree (C4).
    #[serde(default)]
    pub minted_count: u64,
    #[serde(default)]
    pub minted_e8s: u128,
    /// The total (e8s) of the releases opened from verified returns; the Gate's `burned_total` must agree
    /// when the observer is caught up (C4).
    #[serde(default)]
    pub returned_e8s: u128,
    /// The last monitor run.
    #[serde(default)]
    pub monitor: Option<MonitorReport>,
}

impl Default for BridgeCore {
    fn default() -> Self {
        BridgeCore {
            schema_version: RUNTIME_SCHEMA_VERSION,
            initialized: false,
            config: BridgeConfig::default(),
            buckets: Buckets::default(),
            halted: None,
            next_deposit_id: 0,
            next_sweep_id: 0,
            next_release_seq: 0,
            windows: Windows::default(),
            hints: vec![],
            ledger_fee_e8s: None,
            ledger_minting_account: None,
            ledger_tip_lower_bound: 0,
            sweeps: vec![],
            observed: None,
            next_mint_op_id: 0,
            active_mint_op: None,
            mint_lane_halted: None,
            last_final_checkpoint: 0,
            lane_wait: None,
            gather_retry_at_ns: 0,
            observer: ObserverState::default(),
            minted_count: 0,
            minted_e8s: 0,
            returned_e8s: 0,
            monitor: None,
        }
    }
}

impl BridgeCore {
    pub fn is_admin(&self, p: &Principal) -> bool {
        self.config.admins.contains(p)
    }

    pub fn escrow_account(&self, canister: Principal) -> Account {
        Account { owner: canister, subaccount: Some(ids::escrow_subaccount()) }
    }

    /// Roll the shared admission and release window forward if it has elapsed.
    pub fn roll_window(&mut self, now_ns: u64) {
        let len = self.config.limits.window_seconds.saturating_mul(1_000_000_000);
        if now_ns >= self.windows.window_start_ns.saturating_add(len) {
            self.windows = Windows { window_start_ns: now_ns, admitted_in_window: 0, released_in_window: 0 };
        }
    }

    pub fn window_resets_at_ns(&self) -> u64 {
        self.windows.window_start_ns.saturating_add(self.config.limits.window_seconds.saturating_mul(1_000_000_000))
    }
}

cbor_storable!(BridgeCore);

// ---------------------------------------------------------------------------
// Ledger operations (the record shape `ledger.rs` classifies and searches; byte-compatible with the
// Base and Solana instances). The deposit, release and sweep records below embed it
// (`flows::ledger_ops` is the one place that sends, resolves and searches).
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
    /// Sent at least once with an ambiguous outcome. Only `Ok`/`Duplicate` or a complete ICRC-3
    /// search may resolve it.
    Unknown { last_error: String },
    Succeeded { block_index: u64 },
    Failed { reason: String },
}

/// One persisted ledger call tuple. The tuple is fixed before the first await and reused unchanged
/// on every retry, so the ledger's deduplication applies.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct LedgerOp {
    pub kind: LedgerOpKind,
    pub from: Account,
    pub to: Account,
    /// `Some(bridge canister default account)` for pulls.
    pub spender: Option<Account>,
    pub amount_e8s: u64,
    /// Explicit fee for escrow transfers (needed to compute the exact escrow outflow); `None` for
    /// pulls (the ledger charges the depositor).
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
    /// True while this op's `escrow_outflow()` is counted in `unknown_outflow`. Set before the first
    /// send of a tuple, cleared when its outcome is proven.
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
// Deposits (ICP -> Sui, and held-claim re-mints)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum DepositKind {
    UserDeposit,
    /// Re-mint of a held release claim (a return the ICP release could not pay) to its Sui sender.
    /// Created by the mint saga (53b); the shape is fixed here.
    Remint { return_id: Hash32 },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum DepositState {
    PullPending,
    PullUnknown,
    PullFailed { reason: String },
    /// Backing is in `pending_mint`; waiting for the mint saga.
    Escrowed,
    /// A mint operation exists (see `mint_op`).
    MintInFlight,
    Delivered,
    /// Backing still in `pending_mint`; needs a retry or, for user deposits whose mint was never
    /// built or was abandoned by an admin, a refund.
    MintHeld { reason: String },
    Refunding,
    Refunded,
}

impl DepositState {
    pub fn is_terminal(&self) -> bool {
        matches!(self, DepositState::Delivered | DepositState::PullFailed { .. } | DepositState::Refunded)
    }
}

/// The finalized mint transaction that delivered a deposit.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct MintDelivery {
    /// The typed transaction digest (the one `transaction(digest:)` looks up). `None` when delivery
    /// was proven by the consumed mint-id record alone, which does not name the transaction.
    pub tx_digest: Option<Hash32>,
    pub checkpoint: u64,
    /// The Gate's `MintRecord.sequence` of this mint.
    #[serde(default)]
    pub sequence: u64,
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
    /// The 32-byte Sui address that receives the coin.
    pub sui_recipient: [u8; 32],
    /// `A`: escrowed amount (pull amount, or the held claim amount for a re-mint).
    pub amount_e8s: u64,
    pub deposit_fee_e8s: u64,
    /// `M = A - deposit_fee`.
    pub mint_amount_e8s: u64,
    /// The mint id (`ids::mint_id`). It contains the ICP escrow-pull block index, so it exists only
    /// once the pull is proven; `None` before that.
    #[serde(default)]
    pub mint_id: Option<Hash32>,
    pub state: DepositState,
    #[serde(default)]
    pub pull: Option<LedgerOp>,
    #[serde(default)]
    pub refund: Option<LedgerOp>,
    /// The current mint operation (an id in the mint lane, 53b), once the mint saga created it.
    #[serde(default)]
    pub mint_op: Option<u64>,
    /// Mint operations an admin abandoned after the lane's own records proved they can never land.
    /// A deposit with any abandoned operation is never minted again, only refunded.
    #[serde(default)]
    pub abandoned_mint_ops: Vec<u64>,
    #[serde(default)]
    pub delivered: Option<MintDelivery>,
    /// For holds caused by preflight conditions: when to return to `Escrowed`.
    #[serde(default)]
    pub hold_retry_at_ns: Option<u64>,
    pub created_at_ns: u64,
    pub updated_at_ns: u64,
}

cbor_storable!(DepositRecord);

// ---------------------------------------------------------------------------
// Releases (Sui -> ICP)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ReleaseState {
    /// In `pending_release`; the transfer is pending, unknown or not yet started.
    Pending,
    Released,
    /// In `held`; awaiting a re-mint to the Sui sender.
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

/// One ICP release owed for a return that is final on Sui. It is also the durable identity of that
/// return: the only constructor is `flows::release::open_release`, which takes a [`VerifiedReturn`]
/// built from the Gate's `ReturnRecord` at a finalized checkpoint, applies `ReturnFinalized`
/// (`O -> PR`) in the same synchronous step, and refuses a second record for the same `transfer_id`.
/// The destination is the ICP account the Gate saved on chain and never changes afterwards.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ReleaseRecord {
    /// The Gate's `transfer_id`: the primary key, and the memo of the ICP transfer.
    pub return_id: Hash32,
    /// Creation order (`BridgeCore::next_release_seq`).
    pub seq: u64,
    /// The Gate's `bridge_out` sequence for this return (first is 1).
    pub gate_sequence: u64,
    /// The Sui sender of the return (the `bridge_out` transaction's sender).
    pub sui_sender: [u8; 32],
    pub client_nonce: u64,
    /// `B`: the amount burned.
    pub amount_e8s: u64,
    /// The ICP account the release is paid to, parsed from the saved principal and subaccount. When
    /// the saved bytes are not a usable ICP account the release is held and this is a placeholder
    /// (the anonymous principal); `icp_principal` keeps the exact bytes.
    pub destination: Account,
    /// The exact principal bytes the Gate saved.
    pub icp_principal: Vec<u8>,
    /// The exact 32-byte subaccount the Gate saved.
    pub icp_subaccount: Vec<u8>,
    /// The typed transaction digest of the `bridge_out` transaction, and the finalized checkpoint it
    /// was verified at.
    pub tx_digest: Hash32,
    pub checkpoint: u64,
    /// Fee snapshot at creation.
    pub release_fee_e8s: u64,
    pub state: ReleaseState,
    #[serde(default)]
    pub transfer: Option<LedgerOp>,
    pub created_at_ns: u64,
    pub updated_at_ns: u64,
}

cbor_storable!(ReleaseRecord);

// ---------------------------------------------------------------------------
// Observed deployment (53b)
// ---------------------------------------------------------------------------

/// An object reference as observed from a provider quorum: the exact id, version and digest.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoredRef {
    pub id: Hash32,
    pub version: u64,
    pub digest: Hash32,
}

impl From<crate::reads::ObservedRef> for StoredRef {
    fn from(r: crate::reads::ObservedRef) -> Self {
        StoredRef { id: r.id, version: r.version, digest: r.digest }
    }
}

impl From<StoredRef> for crate::sui::ObjectRef {
    fn from(r: StoredRef) -> Self {
        crate::sui::ObjectRef { id: r.id, version: r.version, digest: r.digest }
    }
}

/// What a quorum of providers reported about the configured deployment at one finalized checkpoint,
/// kept distinct from the configured identities (`SuiCore.binding`). Nothing here is a claim about
/// who controls the package or its `UpgradeCap`; the package module bytes were NOT observed (no
/// `sui move build` digest exists), so `module_digest_observed` is false and says so.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ObservedDeployment {
    pub checkpoint: u64,
    pub checkpoint_digest: Hash32,
    pub epoch: u64,
    pub observed_at_ns: u64,
    pub gate_version: u64,
    pub gate_object_version: u64,
    pub initial_shared_version: u64,
    pub supply_cap: u64,
    pub min_bridge_out: u64,
    pub total_supply: u64,
    pub minted_total: u128,
    pub burned_total: u128,
    pub mint_count: u64,
    pub bridge_out_count: u64,
    pub mints_table_size: u64,
    pub returns_table_size: u64,
    pub decimals: u8,
    pub symbol: String,
    pub mint_cap: StoredRef,
    pub mint_cap_owner: Hash32,
    pub admin_cap: StoredRef,
    pub admin_cap_owner: Hash32,
    /// The role whose address owns the `AdminCap`, if it is one of ours (disclosed, not required).
    pub admin_cap_role: Option<ids::SuiRole>,
    /// Always false in 53b: the published module bytes were not compared with a build.
    pub module_digest_observed: bool,
}

// ---------------------------------------------------------------------------
// Mint operations (53b)
// ---------------------------------------------------------------------------

/// How one signed-or-not attempt ended, as far as the lane's own records can prove.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum AttemptDisposition {
    /// Built; its outcome is not established (signed or not, sent or not). It owns the operation.
    Open,
    /// Proven unable to execute: a role's signature was never produced and none may exist (every
    /// signing call for that role was a definite refusal), so no complete signed transaction exists.
    NeverSigned { reason: String, at_ns: u64 },
    /// The transaction executed in a finalized checkpoint and FAILED (the receipt). The mint did not happen.
    FailedFinal { checkpoint: u64, error: String },
    /// The transaction executed in a finalized checkpoint and succeeded.
    LandedFinal { checkpoint: u64 },
}

/// One transaction built for a mint operation, persisted before the first signature. Every field the
/// transaction was built from is kept, so the same bytes are re-signed and rebroadcast, never rebuilt.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct MintAttempt {
    pub generation: u32,
    /// The exact unsigned `TransactionData` BCS.
    pub tx_bcs: Vec<u8>,
    pub signing_digest: Hash32,
    /// The typed transaction digest (what `transaction(digest:)` takes).
    pub tx_digest: Hash32,
    /// The owned inputs this attempt consumes (their versions are its sequencing token).
    pub mint_cap: StoredRef,
    pub gas_coin: StoredRef,
    pub gate_initial_shared_version: u64,
    pub gas_price: u64,
    pub gas_budget: u64,
    /// `TransactionExpiration::Epoch(epoch)`: bounds when validators sign it, not proof of death.
    pub epoch: u64,
    pub built_checkpoint: u64,
    pub built_at_ns: u64,
    /// Raw 64-byte Ed25519 signatures (treasury as sender, gas as gas owner), verified against the
    /// role keys before they were stored.
    #[serde(default)]
    pub treasury_sig: Option<Vec<u8>>,
    #[serde(default)]
    pub gas_sig: Option<Vec<u8>>,
    /// A signing reply for that role was lost or unusable: a signature may exist that we never saw.
    #[serde(default)]
    pub treasury_sig_unknown: bool,
    #[serde(default)]
    pub gas_sig_unknown: bool,
    /// How many signing calls were begun for each role on this attempt. A call that finds an earlier
    /// uncertainty already recorded, or that is no longer the latest call, can never clear the flag
    /// with a definite refusal: only a verified stored signature resolves the uncertainty.
    #[serde(default)]
    pub treasury_sign_calls: u32,
    #[serde(default)]
    pub gas_sign_calls: u32,
    #[serde(default)]
    pub sends: u32,
    #[serde(default)]
    pub last_send_ns: Option<u64>,
    #[serde(default)]
    pub last_send_note: Option<String>,
    /// When the `MintCap` or gas coin version was first seen changed with no receipt for this attempt
    /// (the foreign-consumer suspicion starts its grace period here).
    #[serde(default)]
    pub versions_changed_since_ns: Option<u64>,
    /// When the lane last asked the providers about this attempt (outcalls are paid in cycles, so an
    /// unresolved attempt is checked on a decaying schedule, see `flows::mint::check_due`).
    #[serde(default)]
    pub last_check_ns: Option<u64>,
    pub disposition: AttemptDisposition,
}

impl MintAttempt {
    /// Whether a complete signed transaction may exist outside this record.
    pub fn may_be_complete(&self) -> bool {
        (self.treasury_sig.is_some() || self.treasury_sig_unknown) && (self.gas_sig.is_some() || self.gas_sig_unknown)
    }

    pub fn is_open(&self) -> bool {
        matches!(self.disposition, AttemptDisposition::Open)
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryVia {
    /// The transaction's own finalized receipt (and the consumed record agreed with it).
    Receipt,
    /// The consumed mint-id record alone, matching the id, recipient and amount.
    ConsumedRecord,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum MintOpState {
    /// Being driven: a current attempt is open, or the next one is due.
    Active,
    /// The mint landed. `sequence` is the Gate's `MintRecord.sequence` for this mint id.
    Delivered { tx_digest: Option<Hash32>, checkpoint: u64, sequence: u64, via: DeliveryVia },
    /// Every attempt is a finalized failure or proven never signed and attempts are exhausted: the
    /// deposit returns to `MintHeld`, refundable.
    Failed { reason: String, at_ns: u64 },
}

/// One mint of one deposit. Created in the same synchronous step that links the deposit to it
/// (`deposit.mint_op`), before any signing.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct MintOp {
    pub id: u64,
    pub deposit_id: u64,
    /// Copied from the deposit at creation; checked against it at every step.
    pub mint_id: Hash32,
    pub recipient: Hash32,
    pub amount_e8s: u64,
    pub attempts: Vec<MintAttempt>,
    pub state: MintOpState,
    /// Why an active operation is waiting (shown to operators; never an authorization).
    #[serde(default)]
    pub hold_reason: Option<String>,
    pub created_at_ns: u64,
    pub updated_at_ns: u64,
}

impl MintOp {
    pub fn is_terminal(&self) -> bool {
        !matches!(self.state, MintOpState::Active)
    }

    pub fn current(&self) -> Option<&MintAttempt> {
        self.attempts.last()
    }
}

cbor_storable!(MintOp);

// ---------------------------------------------------------------------------
// Return observer and monitors (53c)
// ---------------------------------------------------------------------------

pub const MAX_CANDIDATES: usize = 64;
pub const MAX_PROBLEMS: usize = 32;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateSource {
    /// Found in a `BridgeOut` event scan.
    Event,
    /// Named by a user's `notify_sui_return` hint.
    Hint,
}

/// What a `BridgeOut` event CLAIMED. Never a basis for payment: it is compared with the durable record.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Claim {
    pub transfer_id: Hash32,
    pub amount_e8s: u64,
    pub sequence: u64,
    pub icp_principal: Vec<u8>,
    pub icp_subaccount: Vec<u8>,
}

/// A return verified against the Gate's durable `ReturnRecord` at a finalized checkpoint, waiting for its
/// turn in Gate sequence order. The fields are the record's, not the event's.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ReadyReturn {
    pub transfer_id: Hash32,
    pub gate_sequence: u64,
    pub sui_sender: Hash32,
    pub client_nonce: u64,
    pub amount_e8s: u64,
    pub icp_principal: Vec<u8>,
    pub icp_subaccount: Vec<u8>,
    pub tx_digest: Hash32,
    pub checkpoint: u64,
}

/// A return the observer has heard of and not yet settled. Keyed by `(sender, nonce)`, the only things
/// the transfer id is computed from besides the network and the Gate.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub sender: Hash32,
    pub nonce: u64,
    pub tx_digest: Hash32,
    pub source: CandidateSource,
    pub claim: Claim,
    pub found_at_ns: u64,
    #[serde(default)]
    pub tries: u32,
    #[serde(default)]
    pub ready: Option<ReadyReturn>,
    /// Why it is not opened yet (for example the mint it returns is not applied to the books).
    #[serde(default)]
    pub waiting: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ObserverProblem {
    pub at_ns: u64,
    pub note: String,
    #[serde(default)]
    pub tx_digest: Option<Hash32>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct ObserverState {
    /// `scanned_through` is meaningful only once true.
    #[serde(default)]
    pub started: bool,
    /// Events of every checkpoint up to this one have been listed, and the Gate's own return counter at
    /// that checkpoint is accounted for by returns this canister has verified or holds as candidates.
    #[serde(default)]
    pub scanned_through: u64,
    /// The highest Gate `bridge_out` sequence whose return has been opened as a release (releases are
    /// opened strictly in sequence order; the next one is this plus one).
    #[serde(default)]
    pub verified_through: u64,
    #[serde(default)]
    pub candidates: Vec<Candidate>,
    #[serde(default)]
    pub problems: Vec<ObserverProblem>,
    #[serde(default)]
    pub next_run_ns: u64,
    /// When the last pass finished its reads and scheduled the next (0: never). A new hint may pull
    /// `next_run_ns` forward only to this plus one busy interval.
    #[serde(default)]
    pub last_run_ns: u64,
    /// The highest rounded head the observer has worked at.
    #[serde(default)]
    pub last_head: u64,
    /// Observer fully caught up at `last_head`: scanned through it, the Gate's return counter equals
    /// `verified_through`, and nothing is pending.
    #[serde(default)]
    pub caught_up: bool,
    /// An unexplained gap: the Gate reports returns that no event or hint has accounted for.
    #[serde(default)]
    pub gap: Option<String>,
    /// Why the last pass stopped early (providers unavailable or disagreeing, a stale head), if it did.
    #[serde(default)]
    pub last_wait: Option<String>,
    /// How many checkpoints the next event window covers (0 means the maximum). It shrinks when a window
    /// needs more queries than a pass may make, and doubles back after a window completes, so a busy
    /// stretch is always reached by a pass that fits its budget.
    #[serde(default)]
    pub window: u64,
}

/// What a breached invariant stops.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreachScope {
    /// The whole bridge halts (releases, refunds and sweeps stop): escrow may be short, or supply may exceed
    /// what the books can explain.
    Bridge,
    /// Only the mint lane halts: the books claim more than the chain shows.
    MintLane,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum CheckOutcome {
    Ok,
    /// A violated invariant (what was found).
    Breach { detail: String, scope: BreachScope },
    /// Could not be evaluated now (why): never a pass.
    NotEvaluated { reason: String },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct CheckResult {
    pub name: String,
    pub outcome: CheckOutcome,
}

/// What the providers reported about the Gate at one finalized checkpoint (INDEPENDENT observation).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ChainObservation {
    pub total_supply: u64,
    pub supply_cap: u64,
    pub minted_total: u128,
    pub burned_total: u128,
    pub mint_count: u64,
    pub bridge_out_count: u64,
    pub gate_version: u64,
}

/// What this canister's own books said when the checks ran (SELF-REPORT).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct OwnBooks {
    pub buckets: Buckets,
    pub minted_count: u64,
    pub minted_e8s: u128,
    pub verified_through: u64,
    pub returned_e8s: u128,
    pub inflight_mint_e8s: u128,
    pub inflight_ops: u64,
}

/// The last monitor run: independent observations kept apart from the self-reported books, with the
/// finalized checkpoint they were made at.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct MonitorReport {
    pub at_ns: u64,
    pub checkpoint: u64,
    pub checkpoint_digest: Hash32,
    pub epoch: u64,
    pub chain: ChainObservation,
    /// The icUSD ledger balance of the escrow account, read from the ledger at `at_ns` (independent of
    /// the books), if the read succeeded.
    pub escrow_balance: Option<u128>,
    pub books: OwnBooks,
    pub caught_up: bool,
    pub checks: Vec<CheckResult>,
}

// ---------------------------------------------------------------------------
// Audit log
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct AuditRecord {
    pub seq: u64,
    pub at_ns: u64,
    pub transition: Option<Transition>,
    pub note: String,
    #[serde(default)]
    pub deposit_id: Option<u64>,
    #[serde(default)]
    pub return_id: Option<Hash32>,
    #[serde(default)]
    pub op_id: Option<u64>,
}

cbor_storable!(AuditRecord);

#[derive(Clone, Debug, Default)]
pub struct AuditCtx {
    pub note: String,
    pub deposit_id: Option<u64>,
    pub return_id: Option<Hash32>,
    pub op_id: Option<u64>,
}

impl AuditCtx {
    pub fn note(note: impl Into<String>) -> Self {
        AuditCtx { note: note.into(), ..AuditCtx::default() }
    }

    pub fn deposit(id: u64, note: impl Into<String>) -> Self {
        AuditCtx { note: note.into(), deposit_id: Some(id), ..AuditCtx::default() }
    }

    pub fn release(return_id: Hash32, note: impl Into<String>) -> Self {
        AuditCtx { note: note.into(), return_id: Some(return_id), ..AuditCtx::default() }
    }

    pub fn op(op_id: u64, note: impl Into<String>) -> Self {
        AuditCtx { note: note.into(), op_id: Some(op_id), ..AuditCtx::default() }
    }
}

// ---------------------------------------------------------------------------
// Index keys
// ---------------------------------------------------------------------------

/// `[len, principal bytes padded to 29, id BE]`.
pub type OwnerKey = [u8; 38];
/// `[principal len, principal padded to 29, request id len, request id padded to 32]`.
pub type RequestKey = [u8; 63];
/// `[Sui sender address, release seq BE]`.
pub type SenderKey = [u8; 40];

pub fn sender_key(sender: &[u8; 32], seq: u64) -> SenderKey {
    let mut k = [0u8; 40];
    k[..32].copy_from_slice(sender);
    k[32..].copy_from_slice(&seq.to_be_bytes());
    k
}

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
    static RUNTIME: RefCell<StableCell<BridgeCore, Mem>> = RefCell::new(
        StableCell::init(mem(MEM_RUNTIME), BridgeCore::default()).expect("init runtime cell"));

    static AUDIT: RefCell<StableLog<AuditRecord, Mem, Mem>> = RefCell::new(
        StableLog::init(mem(MEM_AUDIT_INDEX), mem(MEM_AUDIT_DATA)).expect("init audit log"));

    static DEPOSITS: RefCell<StableBTreeMap<u64, DepositRecord, Mem>> =
        RefCell::new(StableBTreeMap::init(mem(MEM_DEPOSITS)));

    static RELEASES: RefCell<StableBTreeMap<Hash32, ReleaseRecord, Mem>> =
        RefCell::new(StableBTreeMap::init(mem(MEM_RELEASES)));

    static OWNER_DEPOSITS: RefCell<StableBTreeMap<OwnerKey, (), Mem>> =
        RefCell::new(StableBTreeMap::init(mem(MEM_OWNER_DEPOSITS)));

    static REQUEST_IDS: RefCell<StableBTreeMap<RequestKey, u64, Mem>> =
        RefCell::new(StableBTreeMap::init(mem(MEM_REQUEST_IDS)));

    /// Non-terminal deposits, by id (oldest first).
    static ACTIVE_DEPOSITS: RefCell<StableBTreeMap<u64, (), Mem>> =
        RefCell::new(StableBTreeMap::init(mem(MEM_ACTIVE_DEPOSITS)));

    /// The subset of `ACTIVE_DEPOSITS` that are re-mints, so admission can count active user
    /// deposits in constant time. Maintained only by `put_deposit`.
    static ACTIVE_REMINTS: RefCell<StableBTreeMap<u64, (), Mem>> =
        RefCell::new(StableBTreeMap::init(mem(MEM_ACTIVE_REMINTS)));

    /// Non-terminal releases, by creation sequence (oldest first).
    static ACTIVE_RELEASES: RefCell<StableBTreeMap<u64, Hash32, Mem>> =
        RefCell::new(StableBTreeMap::init(mem(MEM_ACTIVE_RELEASES)));

    static OWNER_RELEASES: RefCell<StableBTreeMap<OwnerKey, Hash32, Mem>> =
        RefCell::new(StableBTreeMap::init(mem(MEM_OWNER_RELEASES)));

    static SENDER_RELEASES: RefCell<StableBTreeMap<SenderKey, Hash32, Mem>> =
        RefCell::new(StableBTreeMap::init(mem(MEM_SENDER_RELEASES)));

    /// Gate `bridge_out` sequence to the release it opened: one release per sequence, forever.
    static RELEASE_BY_SEQUENCE: RefCell<StableBTreeMap<u64, Hash32, Mem>> =
        RefCell::new(StableBTreeMap::init(mem(MEM_RELEASE_BY_SEQUENCE)));

    static MINT_OPS: RefCell<StableBTreeMap<u64, MintOp, Mem>> =
        RefCell::new(StableBTreeMap::init(mem(MEM_MINT_OPS)));

    /// `[deposit id BE, operation id BE]`: every operation a deposit ever had, the back-index the
    /// refund guard checks against the deposit's own lists.
    static MINT_OPS_BY_DEPOSIT: RefCell<StableBTreeMap<[u8; 16], (), Mem>> =
        RefCell::new(StableBTreeMap::init(mem(MEM_MINT_OPS_BY_DEPOSIT)));
}

pub fn read_runtime<R>(f: impl FnOnce(&BridgeCore) -> R) -> R {
    RUNTIME.with(|c| f(c.borrow().get()))
}

/// Read-modify-write of the runtime cell. The closure runs on a copy; the copy is written back only
/// after the closure returns.
pub fn mutate_runtime<R>(f: impl FnOnce(&mut BridgeCore) -> R) -> R {
    RUNTIME.with(|c| {
        let mut cell = c.borrow_mut();
        let mut core = cell.get().clone();
        let r = f(&mut core);
        cell.set(core).expect("write runtime cell");
        r
    })
}

/// Upgrade step: bring a stored runtime record to the current schema. Returns whether it changed.
pub fn migrate_runtime() -> bool {
    mutate_runtime(|c| {
        if c.schema_version < RUNTIME_SCHEMA_VERSION {
            c.schema_version = RUNTIME_SCHEMA_VERSION;
            true
        } else {
            false
        }
    })
}

// Deposits ------------------------------------------------------------------

pub fn get_deposit(id: u64) -> Option<DepositRecord> {
    DEPOSITS.with(|m| m.borrow().get(&id))
}

/// Insert or update a deposit and keep the active sets in sync.
pub fn put_deposit(d: &DepositRecord) {
    DEPOSITS.with(|m| m.borrow_mut().insert(d.id, d.clone()));
    let active = !d.state.is_terminal();
    ACTIVE_DEPOSITS.with(|m| {
        if active {
            m.borrow_mut().insert(d.id, ());
        } else {
            m.borrow_mut().remove(&d.id);
        }
    });
    // A record's kind never changes, but keep the index exact whatever is written.
    ACTIVE_REMINTS.with(|m| {
        if active && matches!(d.kind, DepositKind::Remint { .. }) {
            m.borrow_mut().insert(d.id, ());
        } else {
            m.borrow_mut().remove(&d.id);
        }
    });
}

pub fn active_deposit_count() -> u64 {
    ACTIVE_DEPOSITS.with(|m| m.borrow().len())
}

/// Active re-mint deposits (a subset of the active deposits).
pub fn active_remint_count() -> u64 {
    ACTIVE_REMINTS.with(|m| m.borrow().len())
}

/// Active user deposits, in constant time (admission's global limit counts only these).
pub fn active_user_deposit_count() -> u64 {
    active_deposit_count().saturating_sub(active_remint_count())
}

pub fn is_deposit_active(id: u64) -> bool {
    ACTIVE_DEPOSITS.with(|m| m.borrow().contains_key(&id))
}

/// Count active deposits owned by `owner`, walking only this owner's slice of the owner index and
/// checking each id against the active set (no record is decoded). Re-mint deposits are owned by the
/// canister itself and are deliberately not indexed: the canister is never the caller of an
/// admission.
pub fn active_deposits_owned_by(owner: &Principal) -> usize {
    let lower = owner_key(owner, 0);
    let upper = owner_key(owner, u64::MAX);
    ACTIVE_DEPOSITS.with(|active| {
        let active = active.borrow();
        OWNER_DEPOSITS.with(|index| {
            index
                .borrow()
                .range(lower..=upper)
                .filter(|(k, _)| active.contains_key(&u64::from_be_bytes(k[30..].try_into().unwrap_or([0; 8]))))
                .count()
        })
    })
}

/// Coin that exists on Sui, or may, without having been applied to the books: the mint amount of
/// every active deposit that has a live mint operation. `check_destination_supply` adds it to the
/// bound (C2).
pub fn inflight_mint_e8s() -> u128 {
    let mut total = 0u128;
    ACTIVE_DEPOSITS.with(|m| {
        for (id, _) in m.borrow().iter() {
            if let Some(d) = get_deposit(id) {
                if d.mint_op.is_some() {
                    total += d.mint_amount_e8s as u128;
                }
            }
        }
    });
    total
}

pub fn index_owner_deposit(owner: &Principal, id: u64) {
    OWNER_DEPOSITS.with(|m| m.borrow_mut().insert(owner_key(owner, id), ()));
}

/// Deposits of `owner`, newest first, after `start_after` (exclusive).
pub fn list_owner_deposits(owner: &Principal, start_after: Option<u64>, limit: u32) -> Vec<DepositRecord> {
    let limit = limit.clamp(1, MAX_PAGE) as usize;
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
            .map(|(k, _)| u64::from_be_bytes(k[30..].try_into().unwrap_or([0; 8])))
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

/// Active deposits matching `pred`, oldest first, at most `limit`. Walks the index lazily and stops
/// at `limit`; it is for admin lookups and tests. Background queues use [`next_deposits`], which is
/// fair across passes.
pub fn deposits_in_state(pred: impl Fn(&DepositRecord) -> bool, limit: usize) -> Vec<DepositRecord> {
    let mut out = Vec::new();
    ACTIVE_DEPOSITS.with(|m| {
        for (id, _) in m.borrow().iter() {
            if let Some(d) = get_deposit(id) {
                if pred(&d) {
                    out.push(d);
                    if out.len() >= limit {
                        break;
                    }
                }
            }
        }
    });
    out
}

// Fair paging of the background queues ---------------------------------------

/// The background queues of the ICP-side sagas. Each walks one active index with its own cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Queue {
    Pulls,
    Refunds,
    Holds,
    Releases,
    /// Deposits whose mint operation may have delivered (53b).
    MintDeliveries,
    /// Escrowed deposits waiting for a mint operation (53b).
    MintSubmits,
    /// Held releases waiting to be re-minted (53b).
    Remints,
}

impl Queue {
    fn slot(self) -> usize {
        match self {
            Queue::Pulls => 0,
            Queue::Refunds => 1,
            Queue::Holds => 2,
            Queue::Releases => 3,
            Queue::MintDeliveries => 4,
            Queue::MintSubmits => 5,
            Queue::Remints => 6,
        }
    }
}

/// Active ids examined per pass per queue at most: bounds both the index walk and the records
/// decoded, so a small pass never allocates or decodes in proportion to everything that is active.
pub const QUEUE_SCAN: usize = 64;

thread_local! {
    /// Where each queue's next window starts (the last id its previous pass examined). Heap only, on
    /// purpose: after an upgrade the walk restarts at the lowest id, and the active indexes it walks
    /// are stable, so there is nothing to rebuild.
    static QUEUE_CURSORS: RefCell<[Option<u64>; 8]> = const { RefCell::new([None; 8]) };
}

pub fn queue_cursor(q: Queue) -> Option<u64> {
    QUEUE_CURSORS.with(|c| c.borrow()[q.slot()])
}

pub fn set_queue_cursor(q: Queue, cursor: Option<u64>) {
    QUEUE_CURSORS.with(|c| c.borrow_mut()[q.slot()] = cursor);
}

thread_local! {
    /// The `(submitted_at_ns, digest)` of the last hint the return observer read: the next window starts
    /// after it, so unreadable hints cannot occupy every slot of a pass. Heap only, like the queue cursors.
    static HINT_CURSOR: RefCell<Option<(u64, Hash32)>> = const { RefCell::new(None) };
}

pub fn hint_cursor() -> Option<(u64, Hash32)> {
    HINT_CURSOR.with(|c| *c.borrow())
}

pub fn set_hint_cursor(cursor: Option<(u64, Hash32)>) {
    HINT_CURSOR.with(|c| *c.borrow_mut() = cursor);
}

/// The next window of an active index: up to `limit` entries after `after` (exclusive), wrapping to
/// the lowest keys when the end is reached (the wrapped keys are all `<= after`, so none repeats).
fn index_window<V: Storable + Clone>(m: &StableBTreeMap<u64, V, Mem>, after: Option<u64>, limit: usize) -> Vec<(u64, V)> {
    use std::ops::Bound;
    let mut page: Vec<(u64, V)> = match after {
        Some(c) => m.range((Bound::Excluded(c), Bound::Unbounded)).take(limit).collect(),
        None => m.iter().take(limit).collect(),
    };
    if let Some(c) = after {
        if page.len() < limit {
            let room = limit - page.len();
            page.extend(m.range(..=c).take(room));
        }
    }
    page
}

/// Take up to `limit` active deposits matching `pred` from the next [`QUEUE_SCAN`] ids of `queue`'s
/// window, and advance the queue's cursor to the last id examined (stopping at the last id taken when
/// `limit` is reached, so no match is skipped). Entries that match but cannot be settled (an archive
/// that is unavailable, an effect still unresolved) therefore delay the entries behind them by at
/// most one rotation of the index, never forever. With a nonzero `limit` every active deposit is
/// examined within `ceil(active / min(limit, QUEUE_SCAN))` passes, and nothing is reset or marked
/// settled by looking at it.
pub fn next_deposits(queue: Queue, limit: usize, pred: impl Fn(&DepositRecord) -> bool) -> Vec<DepositRecord> {
    let window = ACTIVE_DEPOSITS.with(|m| index_window(&m.borrow(), queue_cursor(queue), QUEUE_SCAN));
    let mut out = Vec::new();
    let mut last = None;
    for (id, _) in window {
        last = Some(id);
        if let Some(d) = get_deposit(id) {
            if pred(&d) {
                out.push(d);
                if out.len() >= limit {
                    break;
                }
            }
        }
    }
    set_queue_cursor(queue, last);
    out
}

// Releases ------------------------------------------------------------------

pub fn get_release(return_id: &Hash32) -> Option<ReleaseRecord> {
    RELEASES.with(|m| m.borrow().get(return_id))
}

/// Insert or update a release and keep the active set in sync. Indexes are written by
/// [`index_release`], once, when the record is created.
pub fn put_release(r: &ReleaseRecord) {
    RELEASES.with(|m| m.borrow_mut().insert(r.return_id, r.clone()));
    ACTIVE_RELEASES.with(|m| {
        if r.state.is_terminal() {
            m.borrow_mut().remove(&r.seq);
        } else {
            m.borrow_mut().insert(r.seq, r.return_id);
        }
    });
}

pub fn active_release_count() -> u64 {
    ACTIVE_RELEASES.with(|m| m.borrow().len())
}

/// The release a Gate `bridge_out` sequence opened, if any.
pub fn release_by_sequence(gate_sequence: u64) -> Option<Hash32> {
    RELEASE_BY_SEQUENCE.with(|m| m.borrow().get(&gate_sequence))
}

pub fn index_release(r: &ReleaseRecord) {
    RELEASE_BY_SEQUENCE.with(|m| m.borrow_mut().insert(r.gate_sequence, r.return_id));
    SENDER_RELEASES.with(|m| m.borrow_mut().insert(sender_key(&r.sui_sender, r.seq), r.return_id));
    OWNER_RELEASES.with(|m| m.borrow_mut().insert(owner_key(&r.destination.owner, r.seq), r.return_id));
}

/// Releases paid (or payable) to `owner` on ICP, newest first, after `start_after_seq`.
pub fn list_owner_releases(owner: &Principal, start_after_seq: Option<u64>, limit: u32) -> Vec<ReleaseRecord> {
    let limit = limit.clamp(1, MAX_PAGE) as usize;
    let upper = match start_after_seq {
        Some(0) => return vec![],
        Some(s) => owner_key(owner, s - 1),
        None => owner_key(owner, u64::MAX),
    };
    let lower = owner_key(owner, 0);
    let ids: Vec<Hash32> = OWNER_RELEASES.with(|m| m.borrow().range(lower..=upper).rev().take(limit).map(|(_, v)| v).collect());
    ids.iter().filter_map(get_release).collect()
}

/// Releases created by returns from the Sui address `sender`, newest first. Lets a sender find held
/// claims (for example an unpayable destination) without local storage.
pub fn list_sender_releases(sender: &[u8; 32], start_after_seq: Option<u64>, limit: u32) -> Vec<ReleaseRecord> {
    let limit = limit.clamp(1, MAX_PAGE) as usize;
    let upper = match start_after_seq {
        Some(0) => return vec![],
        Some(s) => sender_key(sender, s - 1),
        None => sender_key(sender, u64::MAX),
    };
    let lower = sender_key(sender, 0);
    let ids: Vec<Hash32> = SENDER_RELEASES.with(|m| m.borrow().range(lower..=upper).rev().take(limit).map(|(_, v)| v).collect());
    ids.iter().filter_map(get_release).collect()
}

/// Active releases matching `pred`, oldest first, at most `limit`. Lazy, for admin lookups and
/// tests; the background queue uses [`next_releases`].
pub fn releases_in_state(pred: impl Fn(&ReleaseRecord) -> bool, limit: usize) -> Vec<ReleaseRecord> {
    let mut out = Vec::new();
    ACTIVE_RELEASES.with(|m| {
        for (_, rid) in m.borrow().iter() {
            if let Some(r) = get_release(&rid) {
                if pred(&r) {
                    out.push(r);
                    if out.len() >= limit {
                        break;
                    }
                }
            }
        }
    });
    out
}

/// The release counterpart of [`next_deposits`], over the active releases by creation sequence, with
/// the same bound.
pub fn next_releases(queue: Queue, limit: usize, pred: impl Fn(&ReleaseRecord) -> bool) -> Vec<ReleaseRecord> {
    let window = ACTIVE_RELEASES.with(|m| index_window(&m.borrow(), queue_cursor(queue), QUEUE_SCAN));
    let mut out = Vec::new();
    let mut last = None;
    for (seq, rid) in window {
        last = Some(seq);
        if let Some(r) = get_release(&rid) {
            if pred(&r) {
                out.push(r);
                if out.len() >= limit {
                    break;
                }
            }
        }
    }
    set_queue_cursor(queue, last);
    out
}

/// Sum of the amounts of releases in `Pending` (the part of `pending_release` that is spoken for).
pub fn pending_release_total() -> u128 {
    let mut total = 0u128;
    ACTIVE_RELEASES.with(|m| {
        for (_, rid) in m.borrow().iter() {
            if let Some(r) = get_release(&rid) {
                if matches!(r.state, ReleaseState::Pending) {
                    total += r.amount_e8s as u128;
                }
            }
        }
    });
    total
}

// Mint operations -----------------------------------------------------------

pub fn get_mint_op(id: u64) -> Option<MintOp> {
    MINT_OPS.with(|m| m.borrow().get(&id))
}

fn deposit_op_key(deposit_id: u64, op_id: u64) -> [u8; 16] {
    let mut k = [0u8; 16];
    k[..8].copy_from_slice(&deposit_id.to_be_bytes());
    k[8..].copy_from_slice(&op_id.to_be_bytes());
    k
}

/// Insert or update a mint operation and keep the by-deposit back-index exact.
pub fn put_mint_op(op: &MintOp) {
    MINT_OPS.with(|m| m.borrow_mut().insert(op.id, op.clone()));
    MINT_OPS_BY_DEPOSIT.with(|m| m.borrow_mut().insert(deposit_op_key(op.deposit_id, op.id), ()));
}

/// The ids of every mint operation `deposit_id` ever had, from the back-index (oldest first).
pub fn mint_op_ids_for_deposit(deposit_id: u64) -> Vec<u64> {
    let lower = deposit_op_key(deposit_id, 0);
    let upper = deposit_op_key(deposit_id, u64::MAX);
    MINT_OPS_BY_DEPOSIT.with(|m| m.borrow().range(lower..=upper).map(|(k, _)| u64::from_be_bytes(k[8..].try_into().unwrap_or([0; 8]))).collect())
}

/// Mint operations after `start_after`, oldest first.
pub fn list_mint_ops(start_after: Option<u64>, limit: u32) -> Vec<MintOp> {
    use std::ops::Bound;
    let limit = limit.clamp(1, MAX_PAGE) as usize;
    MINT_OPS.with(|m| {
        let m = m.borrow();
        match start_after {
            Some(c) => m.range((Bound::Excluded(c), Bound::Unbounded)).take(limit).map(|(_, v)| v).collect(),
            None => m.iter().take(limit).map(|(_, v)| v).collect(),
        }
    })
}

pub fn mint_op_count() -> u64 {
    MINT_OPS.with(|m| m.borrow().len())
}

/// The mint operation in flight, if any.
pub fn active_mint_op() -> Option<MintOp> {
    read_runtime(|c| c.active_mint_op).and_then(get_mint_op)
}

/// Halt only the mint lane (first reason wins) and log it.
pub fn halt_mint_lane(reason: impl Into<String>) {
    let reason = reason.into();
    let now = crate::env::now_ns();
    let newly = mutate_runtime(|c| {
        if c.mint_lane_halted.is_none() {
            c.mint_lane_halted = Some(HaltRecord { reason: reason.clone(), at_ns: now });
            true
        } else {
            false
        }
    });
    if newly {
        append_audit(None, AuditCtx::note(format!("MINT LANE HALT: {reason}")));
    }
}

pub fn is_mint_lane_halted() -> bool {
    read_runtime(|c| c.mint_lane_halted.is_some())
}

// Accounting and audit --------------------------------------------------------

/// Apply an accounting transition and append an audit record. On error nothing changes.
pub fn apply_transition(t: Transition, ctx: AuditCtx) -> Result<(), AccountingError> {
    let applied = mutate_runtime(|c| {
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
    let applied = mutate_runtime(|c| {
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
        let rec = AuditRecord { seq, at_ns, transition, note: ctx.note, deposit_id: ctx.deposit_id, return_id: ctx.return_id, op_id: ctx.op_id };
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
    let newly = mutate_runtime(|c| {
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
    read_runtime(|c| c.halted.is_some())
}

/// Clear the bridge halt after a human resolved its cause. Returns whether it was halted.
pub fn clear_halt() -> bool {
    let cleared = mutate_runtime(|c| c.halted.take().is_some());
    if cleared {
        append_audit(None, AuditCtx::note("halt cleared"));
    }
    cleared
}

/// Test-only reset of every runtime structure (host unit tests share thread-locals).
#[cfg(test)]
pub fn reset_runtime_for_tests() {
    RUNTIME.with(|c| {
        c.borrow_mut().set(BridgeCore::default()).expect("reset runtime");
    });
    AUDIT.with(|a| {
        // `StableLog` has no clear; re-initialising over the same memories would keep the data,
        // so start from fresh memories.
        *a.borrow_mut() = StableLog::new(mem(MEM_AUDIT_INDEX), mem(MEM_AUDIT_DATA));
    });
    DEPOSITS.with(|m| *m.borrow_mut() = StableBTreeMap::new(mem(MEM_DEPOSITS)));
    RELEASES.with(|m| *m.borrow_mut() = StableBTreeMap::new(mem(MEM_RELEASES)));
    OWNER_DEPOSITS.with(|m| *m.borrow_mut() = StableBTreeMap::new(mem(MEM_OWNER_DEPOSITS)));
    REQUEST_IDS.with(|m| *m.borrow_mut() = StableBTreeMap::new(mem(MEM_REQUEST_IDS)));
    ACTIVE_DEPOSITS.with(|m| *m.borrow_mut() = StableBTreeMap::new(mem(MEM_ACTIVE_DEPOSITS)));
    ACTIVE_REMINTS.with(|m| *m.borrow_mut() = StableBTreeMap::new(mem(MEM_ACTIVE_REMINTS)));
    ACTIVE_RELEASES.with(|m| *m.borrow_mut() = StableBTreeMap::new(mem(MEM_ACTIVE_RELEASES)));
    MINT_OPS.with(|m| *m.borrow_mut() = StableBTreeMap::new(mem(MEM_MINT_OPS)));
    MINT_OPS_BY_DEPOSIT.with(|m| *m.borrow_mut() = StableBTreeMap::new(mem(MEM_MINT_OPS_BY_DEPOSIT)));
    OWNER_RELEASES.with(|m| *m.borrow_mut() = StableBTreeMap::new(mem(MEM_OWNER_RELEASES)));
    SENDER_RELEASES.with(|m| *m.borrow_mut() = StableBTreeMap::new(mem(MEM_SENDER_RELEASES)));
    RELEASE_BY_SEQUENCE.with(|m| *m.borrow_mut() = StableBTreeMap::new(mem(MEM_RELEASE_BY_SEQUENCE)));
    QUEUE_CURSORS.with(|c| *c.borrow_mut() = [None; 8]);
    HINT_CURSOR.with(|c| *c.borrow_mut() = None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn memory_ids_are_unique_and_never_reuse_the_core() {
        let ids: BTreeSet<u8> = MEMORY_LAYOUT.iter().map(|(i, _)| *i).collect();
        assert_eq!(ids.len(), MEMORY_LAYOUT.len(), "duplicate memory id");
        assert_eq!(MEMORY_LAYOUT[0].0, 0, "memory 0 stays the BB-52 SuiCore");
    }

    #[test]
    fn index_keys_sort_by_owner_then_id() {
        let a = Principal::from_slice(&[1, 2, 3]);
        let b = Principal::from_slice(&[1, 2, 4]);
        assert!(owner_key(&a, 9) < owner_key(&a, 10));
        assert!(owner_key(&a, u64::MAX) < owner_key(&b, 0), "one owner's slice never overlaps another's");
        assert!(request_key(&a, &[1; 15]).is_err());
        assert!(request_key(&a, &[1; 33]).is_err());
        assert_ne!(request_key(&a, &[1; 16]).unwrap(), request_key(&b, &[1; 16]).unwrap());
        assert_ne!(request_key(&a, &[1; 16]).unwrap(), request_key(&a, &[1; 17]).unwrap());
    }

    #[test]
    fn limits_and_fees_are_validated() {
        assert!(validate_limits(&default_limits()).is_ok());
        let mut l = default_limits();
        l.min_deposit_e8s = 0;
        assert!(validate_limits(&l).is_err());
        let mut l = default_limits();
        l.max_deposit_e8s = l.max_outstanding_e8s + 1;
        assert!(validate_limits(&l).is_err());
        let limits = default_limits();
        assert!(validate_fees(&BridgeFees { deposit_fee_e8s: 1, release_fee_e8s: 1 }, &limits).is_ok());
        assert!(validate_fees(&BridgeFees { deposit_fee_e8s: limits.min_deposit_e8s, release_fee_e8s: 0 }, &limits).is_err());
        assert!(validate_fees(&BridgeFees { deposit_fee_e8s: 0, release_fee_e8s: MAX_RELEASE_FEE_E8S + 1 }, &limits).is_err());
    }

    #[test]
    fn the_saga_part_of_install_is_validated() {
        let ledger = Principal::from_slice(&[7; 10]);
        let admin = Principal::from_slice(&[3; 10]);
        let args = |admins: Vec<Principal>, ledger: Principal| InitArgs {
            network: crate::ids::SuiNetwork::Testnet,
            chain_identifier: hex::encode(crate::ids::TESTNET_CHAIN_IDENTIFIER),
            mirror: None,
            icusd_ledger: ledger,
            admins,
            fee_recipient: None,
            fees: None,
            limits: None,
        };
        let ok = validate_runtime_init(&args(vec![admin, admin], ledger)).unwrap();
        assert_eq!(ok.admins, vec![admin], "duplicates are dropped");
        assert!(ok.pauses.deposits, "deposits start paused");
        assert!(validate_runtime_init(&args(vec![], ledger)).is_err());
        assert!(validate_runtime_init(&args(vec![Principal::anonymous()], ledger)).is_err());
        assert!(validate_runtime_init(&args(vec![admin], Principal::anonymous())).is_err());
        assert!(validate_stored_runtime(&ok).is_ok());
        let mut bad = ok.clone();
        bad.admins.clear();
        assert!(validate_stored_runtime(&bad).is_err());
    }

    fn sample_op() -> LedgerOp {
        LedgerOp {
            kind: LedgerOpKind::Release,
            from: Account { owner: Principal::from_slice(&[9; 10]), subaccount: Some([1; 32]) },
            to: Account { owner: Principal::from_slice(&[8; 10]), subaccount: None },
            spender: None,
            amount_e8s: 99,
            fee_e8s: Some(1),
            memo: vec![1, 2, 3],
            created_at_time: 5,
            attempts: 2,
            ambiguous_seen: true,
            status: LedgerOpStatus::Unknown { last_error: "x".into() },
            search_from_block: 3,
            search_next_block: Some(4),
            last_attempt_ns: Some(6),
            outflow_reserved: true,
        }
    }

    pub(crate) fn sample_release() -> ReleaseRecord {
        ReleaseRecord {
            return_id: [0xd9; 32],
            seq: 7,
            gate_sequence: 3,
            sui_sender: [0xbb; 32],
            client_nonce: 42,
            amount_e8s: 250,
            destination: Account { owner: Principal::from_slice(&[8; 10]), subaccount: None },
            icp_principal: vec![8; 10],
            icp_subaccount: vec![0; 32],
            tx_digest: [0x77; 32],
            checkpoint: 500,
            release_fee_e8s: 1,
            state: ReleaseState::Pending,
            transfer: Some(sample_op()),
            created_at_ns: 10,
            updated_at_ns: 11,
        }
    }

    pub(crate) fn sample_deposit(id: u64) -> DepositRecord {
        DepositRecord {
            id,
            kind: DepositKind::UserDeposit,
            owner: Principal::from_slice(&[3; 10]),
            from: Some(Account { owner: Principal::from_slice(&[3; 10]), subaccount: None }),
            client_request_id: vec![5; 16],
            sui_recipient: [0x8b; 32],
            amount_e8s: 100,
            deposit_fee_e8s: 1,
            mint_amount_e8s: 99,
            mint_id: Some([4; 32]),
            state: DepositState::Escrowed,
            pull: Some(LedgerOp { kind: LedgerOpKind::Pull, spender: Some(Account { owner: Principal::from_slice(&[9; 10]), subaccount: None }), ..sample_op() }),
            refund: None,
            mint_op: None,
            abandoned_mint_ops: vec![],
            delivered: None,
            hold_retry_at_ns: None,
            created_at_ns: 1,
            updated_at_ns: 2,
        }
    }

    #[test]
    fn a_stored_attempt_from_before_the_per_role_signing_counters_decodes_with_defaults() {
        let a = MintAttempt {
            generation: 1,
            tx_bcs: vec![1, 2, 3],
            signing_digest: [1; 32],
            tx_digest: [2; 32],
            mint_cap: StoredRef { id: [3; 32], version: 4, digest: [5; 32] },
            gas_coin: StoredRef { id: [6; 32], version: 7, digest: [8; 32] },
            gate_initial_shared_version: 9,
            gas_price: 10,
            gas_budget: 11,
            epoch: 12,
            built_checkpoint: 13,
            built_at_ns: 14,
            treasury_sig: None,
            gas_sig: None,
            treasury_sig_unknown: true,
            gas_sig_unknown: false,
            treasury_sign_calls: 3,
            gas_sign_calls: 2,
            sends: 0,
            last_send_ns: None,
            last_send_note: None,
            versions_changed_since_ns: None,
            last_check_ns: None,
            disposition: AttemptDisposition::Open,
        };
        // Drop the two new fields from the encoded map, as an image written by 1b71eae8 lacks them.
        let mut value: ciborium::value::Value = ciborium::de::from_reader(cbor_encode(&a).as_slice()).unwrap();
        if let ciborium::value::Value::Map(entries) = &mut value {
            entries.retain(|(k, _)| !matches!(k, ciborium::value::Value::Text(t) if t == "treasury_sign_calls" || t == "gas_sign_calls"));
        } else {
            panic!("an attempt encodes as a map");
        }
        let mut older = Vec::new();
        ciborium::ser::into_writer(&value, &mut older).unwrap();
        let decoded: MintAttempt = try_cbor_decode(&older).expect("an older image decodes");
        assert_eq!((decoded.treasury_sign_calls, decoded.gas_sign_calls), (0, 0));
        assert!(decoded.treasury_sig_unknown, "the stored uncertainty is kept");
        assert!(!decoded.may_be_complete(), "the gas role never signed and nothing is unknown for it");
        // And the current image round-trips exactly.
        assert_eq!(try_cbor_decode::<MintAttempt>(&cbor_encode(&a)).unwrap(), a);
    }

    #[test]
    fn an_observer_state_written_before_the_run_clock_existed_decodes_with_it_at_zero() {
        // The review correction added `last_run_ns` (additive, serde default): an image without it decodes.
        use ciborium::Value;
        let current = ObserverState { next_run_ns: 77, last_run_ns: 55, verified_through: 3, ..ObserverState::default() };
        assert_eq!(try_cbor_decode::<ObserverState>(&cbor_encode(&current)).unwrap(), current);
        let v: Value = ciborium::de::from_reader(cbor_encode(&current).as_slice()).unwrap();
        let Value::Map(entries) = v else { panic!("a CBOR map") };
        let kept: Vec<(Value, Value)> = entries.into_iter().filter(|(k, _)| !matches!(k, Value::Text(t) if t == "last_run_ns")).collect();
        let mut old = Vec::new();
        ciborium::ser::into_writer(&Value::Map(kept), &mut old).unwrap();
        let decoded: ObserverState = try_cbor_decode(&old).expect("an older image decodes");
        assert_eq!((decoded.last_run_ns, decoded.next_run_ns, decoded.verified_through), (0, 77, 3));
    }

    #[test]
    fn records_round_trip_and_older_images_decode() {
        let r = sample_release();
        assert_eq!(try_cbor_decode::<ReleaseRecord>(&cbor_encode(&r)).unwrap(), r);
        let d = sample_deposit(1);
        assert_eq!(try_cbor_decode::<DepositRecord>(&cbor_encode(&d)).unwrap(), d);
        let core = BridgeCore { initialized: true, config: BridgeConfig { admins: vec![Principal::from_slice(&[3; 10])], ..BridgeConfig::default() }, ..BridgeCore::default() };
        assert_eq!(try_cbor_decode::<BridgeCore>(&cbor_encode(&core)).unwrap(), core);

        // An image written before the optional fields existed decodes with their defaults: strip
        // every `#[serde(default)]` field of the core and of a deposit.
        use ciborium::Value;
        let strip = |bytes: Vec<u8>, drop: &[&str]| -> Vec<u8> {
            let v: Value = ciborium::de::from_reader(bytes.as_slice()).unwrap();
            let Value::Map(entries) = v else { panic!("a CBOR map") };
            let kept: Vec<(Value, Value)> =
                entries.into_iter().filter(|(k, _)| !matches!(k, Value::Text(t) if drop.contains(&t.as_str()))).collect();
            let mut out = Vec::new();
            ciborium::ser::into_writer(&Value::Map(kept), &mut out).unwrap();
            out
        };
        let old_core = strip(
            cbor_encode(&core),
            &["buckets", "halted", "next_deposit_id", "next_sweep_id", "next_release_seq", "windows", "hints", "ledger_fee_e8s", "ledger_minting_account", "ledger_tip_lower_bound", "sweeps"],
        );
        let decoded: BridgeCore = try_cbor_decode(&old_core).unwrap();
        assert_eq!(decoded.buckets, Buckets::default());
        assert!(decoded.sweeps.is_empty() && decoded.halted.is_none() && decoded.hints.is_empty());
        let old_dep = strip(cbor_encode(&d), &["mint_id", "pull", "refund", "mint_op", "abandoned_mint_ops", "delivered", "hold_retry_at_ns"]);
        let decoded: DepositRecord = try_cbor_decode(&old_dep).unwrap();
        assert!(decoded.mint_id.is_none() && decoded.pull.is_none() && decoded.abandoned_mint_ops.is_empty());
    }

    #[test]
    fn a_deposit_is_active_until_terminal_and_remints_are_counted_apart() {
        reset_runtime_for_tests();
        let mut d = sample_deposit(1);
        put_deposit(&d);
        index_owner_deposit(&d.owner, d.id);
        assert!(is_deposit_active(1));
        assert_eq!((active_deposit_count(), active_remint_count(), active_user_deposit_count()), (1, 0, 1));
        assert_eq!(active_deposits_owned_by(&d.owner), 1);
        let mut re = sample_deposit(2);
        re.kind = DepositKind::Remint { return_id: [1; 32] };
        re.owner = Principal::from_slice(&[0xbb; 10]);
        put_deposit(&re);
        assert_eq!((active_deposit_count(), active_remint_count(), active_user_deposit_count()), (2, 1, 1));
        d.state = DepositState::Delivered;
        put_deposit(&d);
        assert!(!is_deposit_active(1));
        assert_eq!(active_deposits_owned_by(&d.owner), 0);
        assert_eq!(list_owner_deposits(&d.owner, None, 10).len(), 1, "history keeps terminal deposits");
        let rkey = request_key(&d.owner, &d.client_request_id).unwrap();
        assert_eq!(lookup_request(&rkey), None);
        insert_request(rkey, 1);
        assert_eq!(lookup_request(&rkey), Some(1));
        reset_runtime_for_tests();
    }

    #[test]
    fn queue_windows_rotate_fairly_over_the_active_index() {
        reset_runtime_for_tests();
        for id in 0..200u64 {
            let mut d = sample_deposit(id);
            d.state = DepositState::PullUnknown;
            put_deposit(&d);
        }
        // Nothing matches: the cursor still advances a full window per pass and wraps, so every
        // id is examined within ceil(200 / 64) = 4 passes.
        let seen = RefCell::new(BTreeSet::new());
        for _ in 0..4 {
            let before = queue_cursor(Queue::Pulls);
            let taken = next_deposits(Queue::Pulls, 1000, |d| {
                seen.borrow_mut().insert(d.id);
                false
            });
            assert!(taken.is_empty());
            assert_ne!(queue_cursor(Queue::Pulls), before);
        }
        assert_eq!(seen.borrow().len(), 200);
        // A pass that takes `limit` matches stops at the last taken id, so no match is skipped.
        set_queue_cursor(Queue::Pulls, None);
        let a = next_deposits(Queue::Pulls, 3, |_| true);
        let b = next_deposits(Queue::Pulls, 3, |_| true);
        assert_eq!(a.iter().map(|d| d.id).collect::<Vec<_>>(), vec![0, 1, 2]);
        assert_eq!(b.iter().map(|d| d.id).collect::<Vec<_>>(), vec![3, 4, 5]);
        reset_runtime_for_tests();
    }

    #[test]
    fn releases_are_indexed_once_by_owner_sender_and_gate_sequence() {
        reset_runtime_for_tests();
        let r = sample_release();
        put_release(&r);
        index_release(&r);
        assert_eq!(active_release_count(), 1);
        assert_eq!(release_by_sequence(3), Some(r.return_id));
        assert_eq!(list_owner_releases(&r.destination.owner, None, 10).len(), 1);
        assert_eq!(list_sender_releases(&r.sui_sender, None, 10).len(), 1);
        assert_eq!(list_sender_releases(&[0xcc; 32], None, 10).len(), 0);
        assert_eq!(pending_release_total(), 250);
        let mut done = r.clone();
        done.state = ReleaseState::Released;
        put_release(&done);
        assert_eq!(active_release_count(), 0);
        assert_eq!(pending_release_total(), 0);
        assert_eq!(get_release(&r.return_id).unwrap().state, ReleaseState::Released, "history is kept");
        reset_runtime_for_tests();
    }

    #[test]
    fn transitions_are_applied_atomically_and_audited() {
        reset_runtime_for_tests();
        apply_transition(Transition::AdmitDeposit { amount: 5 }, AuditCtx::note("admit")).unwrap();
        assert_eq!(read_runtime(|c| c.buckets.reserved_pulls), 5);
        assert_eq!(audit_len(), 1);
        // A batch with one failing step changes nothing and logs nothing.
        let r = apply_transitions(&[Transition::PullSucceeded { amount: 5 }, Transition::PullFailed { amount: 1 }], AuditCtx::note("batch"));
        assert!(r.is_err());
        assert_eq!(read_runtime(|c| c.buckets.reserved_pulls), 5);
        assert_eq!(audit_len(), 1);
        assert_eq!(audit_page(0, 10)[0].transition, Some(Transition::AdmitDeposit { amount: 5 }));
        halt("first");
        halt("second");
        assert_eq!(read_runtime(|c| c.halted.clone().unwrap().reason), "first", "the first halt reason wins");
        assert!(is_halted());
        assert!(clear_halt());
        assert!(!clear_halt());
        reset_runtime_for_tests();
    }
}
