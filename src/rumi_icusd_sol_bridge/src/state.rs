//! Stable state. Every record is written directly into `ic-stable-structures`, so an upgrade
//! needs no `pre_upgrade` snapshot and cannot lose operations.
//!
//! Records are CBOR (`ciborium`). CBOR maps fields by name, so a field added later with
//! `#[serde(default)]` decodes from older bytes. (That is not true of Candid `Decode!`, which
//! is why Candid is never used for stored records here.)
//!
//! The `MemoryId` layout is append-only. `memory_layout_is_unique_and_pinned` pins it. BB-47
//! laid out the core cell and the audit log (ids 0 to 2); BB-48 appends the nonce pool and the
//! operations (ids 3 to 6); BB-49 appends its maps after id 6 and never reuses an id. The core
//! cell only ever grows `#[serde(default)]` fields, so a BB-47 snapshot still decodes
//! (`bb47_core_image_decodes_with_the_new_fields_defaulted`).

use crate::accounting::{AccountingError, Buckets, Transition};
use crate::ids;
use crate::ids::Hash32;
use crate::records::{slot_key, BootstrapTx, Lane, LaneHalt, LaneHalts, MintState, NonceSlot, Sig, SolOp};
use crate::tx::builders::TxProfile;
use crate::tx::{pubkey_from_base58, pubkey_to_base58, Pubkey, TokenProgram, SYSTEM_PROGRAM_ID};
use crate::types::{BridgeFees, BridgeLimits, InitArgs, LaneSettings, Pauses, RpcSettings, SolRpcCluster};
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
pub const MEM_AUDIT_INDEX: u8 = 1;
pub const MEM_AUDIT_DATA: u8 = 2;
pub const MEM_NONCE_SLOTS: u8 = 3;
pub const MEM_OPS: u8 = 4;
pub const MEM_ACTIVE_OPS: u8 = 5;
pub const MEM_OP_BY_EXTERNAL: u8 = 6;
pub const MEM_BOOTSTRAP_TXS: u8 = 7;
pub const MEM_DEPOSITS: u8 = 8;
pub const MEM_RELEASES: u8 = 9;
pub const MEM_OWNER_DEPOSITS: u8 = 10;
pub const MEM_REQUEST_IDS: u8 = 11;
pub const MEM_ACTIVE_DEPOSITS: u8 = 12;
pub const MEM_ACTIVE_REMINTS: u8 = 13;
pub const MEM_ACTIVE_RELEASES: u8 = 14;
pub const MEM_OWNER_RELEASES: u8 = 15;
pub const MEM_SENDER_RELEASES: u8 = 16;
pub const MEM_RELEASE_BY_BURN_OP: u8 = 17;

/// Append-only. Never reuse or reorder an id.
pub const MEMORY_LAYOUT: &[(u8, &str)] = &[
    (MEM_CORE, "core"),
    (MEM_AUDIT_INDEX, "audit_index"),
    (MEM_AUDIT_DATA, "audit_data"),
    (MEM_NONCE_SLOTS, "nonce_slots"),
    (MEM_OPS, "ops"),
    (MEM_ACTIVE_OPS, "active_ops"),
    (MEM_OP_BY_EXTERNAL, "op_by_external_id"),
    (MEM_BOOTSTRAP_TXS, "bootstrap_txs"),
    (MEM_DEPOSITS, "deposits"),
    (MEM_RELEASES, "releases"),
    (MEM_OWNER_DEPOSITS, "owner_deposits"),
    (MEM_REQUEST_IDS, "request_ids"),
    (MEM_ACTIVE_DEPOSITS, "active_deposits"),
    (MEM_ACTIVE_REMINTS, "active_reminting_deposits"),
    (MEM_ACTIVE_RELEASES, "active_releases"),
    (MEM_OWNER_RELEASES, "owner_releases"),
    (MEM_SENDER_RELEASES, "sender_releases"),
    (MEM_RELEASE_BY_BURN_OP, "release_by_burn_op"),
];

/// 1 = BB-47 (config, buckets, halt). 2 = BB-48 (adds the SOL RPC and lane settings, the role
/// keys, the mint, lane halts and the operation counter, all defaulted). 3 = BB-49 49a (the
/// `mint-account` public key, the mint claim state and the last bind note, all defaulted).
/// 4 = BB-49 49b (the ledger saga: limits, windows, id counters, ledger facts, sweeps and hints,
/// all defaulted; the deposit and release maps are new memories). `migrate_core` moves a stored
/// older version to the current one at upgrade.
pub const CORE_SCHEMA_VERSION: u32 = 4;
/// Maximum page size for list queries.
pub const MAX_PAGE: u32 = 100;

/// Base58 genesis hash of Solana mainnet-beta (profile `solana.json`, source
/// `docs.anza.xyz/clusters/available`, verified 2026-10-30). It decides the key rule only.
pub const MAINNET_GENESIS_HASH_B58: &str = "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d";
/// Threshold-Schnorr key names (note section 5): `key_1` on mainnet, `test_key_1` otherwise.
pub const MAINNET_KEY_NAME: &str = "key_1";
pub const TEST_KEY_NAME: &str = "test_key_1";

// ---------------------------------------------------------------------------
// CBOR storable helper
// ---------------------------------------------------------------------------

pub fn cbor_encode<T: Serialize>(v: &T) -> Vec<u8> {
    let mut buf = Vec::new();
    // Writing into a `Vec` cannot fail; a serialization error here would be a type bug.
    if let Err(e) = ciborium::ser::into_writer(v, &mut buf) {
        panic!("CBOR encode of bridge state failed: {e}");
    }
    buf
}

/// The checked decode, used by tests and by upgrade validation.
pub fn try_cbor_decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    ciborium::de::from_reader(bytes).map_err(|e| e.to_string())
}

macro_rules! cbor_storable {
    ($t:ty) => {
        impl Storable for $t {
            fn to_bytes(&self) -> Cow<'_, [u8]> {
                Cow::Owned(cbor_encode(self))
            }
            // `Storable::from_bytes` has no error channel. A record that does not decode is
            // corrupt state, and trapping (which rolls the message or upgrade back) is the
            // only safe response.
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
// Configuration records
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BridgeConfig {
    pub icusd_ledger: Principal,
    pub sol_rpc: Principal,
    pub schnorr_key_name: String,
    /// Cluster genesis hash: network identity in every id and every key path.
    pub genesis_hash: [u8; 32],
    pub admins: Vec<Principal>,
    pub fee_recipient: Option<Account>,
    pub token_program: TokenProgram,
    pub memo_program: [u8; 32],
    #[serde(default)]
    pub fees: BridgeFees,
    #[serde(default)]
    pub pauses: Pauses,
    /// SOL RPC settings (BB-48). `cluster` is `None` in a BB-47 snapshot: every call is refused
    /// until it is configured.
    #[serde(default)]
    pub rpc: RpcSettings,
    /// Nonce lane settings (BB-48).
    #[serde(default)]
    pub lane: LaneSettings,
    /// Deposit, capacity and rate limits (49b). A snapshot from before 49b gets the defaults.
    #[serde(default = "default_limits")]
    pub limits: BridgeLimits,
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
            sol_rpc: Principal::anonymous(),
            schnorr_key_name: String::new(),
            genesis_hash: [0u8; 32],
            admins: vec![],
            fee_recipient: None,
            token_program: TokenProgram::Token2022,
            memo_program: crate::tx::MEMO_PROGRAM_V3_ID,
            fees: BridgeFees::default(),
            // A fresh canister starts with deposits paused until a mint is bound and an
            // admin explicitly unpauses (the unpause path lands with BB-48).
            pauses: Pauses { deposits: true, mints: false, releases: false, observer: false },
            rpc: RpcSettings::default(),
            lane: LaneSettings::default(),
            limits: default_limits(),
        }
    }
}

/// The threshold-Schnorr key name the key rule requires for a cluster (note section 5): `key_1` for
/// mainnet-beta, `test_key_1` for every other cluster.
pub fn key_name_for_genesis(genesis_hash: &[u8; 32]) -> &'static str {
    if pubkey_to_base58(genesis_hash) == MAINNET_GENESIS_HASH_B58 {
        MAINNET_KEY_NAME
    } else {
        TEST_KEY_NAME
    }
}

/// Validate install arguments into a config. Pure, unit tested.
pub fn validate_init(args: &InitArgs) -> Result<BridgeConfig, String> {
    let genesis_hash = pubkey_from_base58(&args.genesis_hash).map_err(|e| format!("genesis_hash: {e}"))?;
    let mainnet = args.genesis_hash == MAINNET_GENESIS_HASH_B58;
    let expected_key = key_name_for_genesis(&genesis_hash);
    if args.schnorr_key_name != expected_key {
        return Err(format!(
            "this cluster ({}) requires threshold key {expected_key}, got {}",
            if mainnet { "mainnet-beta" } else { "not mainnet-beta" },
            args.schnorr_key_name
        ));
    }
    if args.admins.is_empty() {
        return Err("at least one admin is required".into());
    }
    if args.admins.iter().any(|p| *p == Principal::anonymous()) {
        return Err("anonymous principal cannot be an admin".into());
    }
    if args.icusd_ledger == Principal::anonymous() || args.sol_rpc == Principal::anonymous() {
        return Err("ledger and SOL RPC principals are required".into());
    }
    let memo_program = pubkey_from_base58(&args.memo_program).map_err(|e| format!("memo_program: {e}"))?;
    if memo_program == SYSTEM_PROGRAM_ID {
        return Err("memo_program cannot be the System Program".into());
    }
    let mut rpc = args.rpc.clone().unwrap_or_default();
    rpc.cluster = match (mainnet, rpc.cluster) {
        (true, None | Some(SolRpcCluster::Mainnet)) => Some(SolRpcCluster::Mainnet),
        (true, Some(other)) => return Err(format!("mainnet-beta cannot use SOL RPC cluster {other:?}")),
        (false, Some(SolRpcCluster::Devnet)) => Some(SolRpcCluster::Devnet),
        (false, Some(SolRpcCluster::Testnet)) => Some(SolRpcCluster::Testnet),
        (false, Some(SolRpcCluster::Mainnet)) => return Err("a non-mainnet genesis hash cannot use the Mainnet SOL RPC cluster".into()),
        (false, None) => return Err("rpc.cluster (Devnet or Testnet) is required for a non-mainnet genesis hash".into()),
    };
    validate_rpc_settings(&rpc)?;
    let lane = args.lane.clone().unwrap_or_default();
    lane.validate()?;
    Ok(BridgeConfig {
        icusd_ledger: args.icusd_ledger,
        sol_rpc: args.sol_rpc,
        schnorr_key_name: args.schnorr_key_name.clone(),
        genesis_hash,
        admins: dedup_principals(&args.admins),
        fee_recipient: args.fee_recipient,
        token_program: args.token_program,
        memo_program,
        rpc,
        lane,
        ..BridgeConfig::default()
    })
}

/// Consensus thresholds must be satisfiable and the call budget sane.
pub fn validate_rpc_settings(r: &RpcSettings) -> Result<(), String> {
    use crate::types::ConsensusSetting;
    for (name, c) in [("verify", r.verify), ("discover", r.discover)] {
        if let ConsensusSetting::Threshold { total, min } = c {
            if min == 0 || total.is_some_and(|t| min > t) {
                return Err(format!("rpc.{name}: threshold min must be in 1..=total"));
            }
        }
    }
    if r.cycles_per_call == 0 {
        return Err("rpc.cycles_per_call must be > 0".into());
    }
    Ok(())
}

/// Check the configuration that survived an upgrade by the install-time rules. Upgrade
/// arguments carry no cluster or key, so the stored values are what the new module will use.
pub fn validate_stored_config(config: &BridgeConfig) -> Result<(), String> {
    let mainnet = pubkey_to_base58(&config.genesis_hash) == MAINNET_GENESIS_HASH_B58;
    let expected_key = if mainnet { MAINNET_KEY_NAME } else { TEST_KEY_NAME };
    if config.schnorr_key_name != expected_key {
        return Err(format!("stored key {} does not match the cluster rule ({expected_key})", config.schnorr_key_name));
    }
    if config.admins.is_empty() {
        return Err("stored admin list is empty".into());
    }
    config.lane.validate().map_err(|e| format!("stored lane settings: {e}"))?;
    validate_rpc_settings(&config.rpc).map_err(|e| format!("stored rpc settings: {e}"))?;
    Ok(())
}

fn dedup_principals(ps: &[Principal]) -> Vec<Principal> {
    let mut seen = BTreeSet::new();
    ps.iter().filter(|p| seen.insert(**p)).copied().collect()
}

/// Display name of the configured cluster. Only mainnet-beta is named: no other genesis hash
/// is recorded in the sources, so every other cluster reads as a test cluster.
pub fn cluster_name(genesis_hash: &[u8; 32]) -> &'static str {
    if pubkey_to_base58(genesis_hash) == MAINNET_GENESIS_HASH_B58 {
        "Solana"
    } else {
        "Solana (test cluster)"
    }
}

// ---------------------------------------------------------------------------
// Core singleton
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct HaltRecord {
    pub reason: String,
    pub at_ns: u64,
}

/// The public keys of the threshold-Ed25519 role keys, derived once through the signer seam and
/// cached. They are public information; the secret never leaves the subnet.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct RoleKeys {
    pub fee_payer: Pubkey,
    pub mint_authority: Pubkey,
    pub return_owner: Pubkey,
    /// The one-time `mint-account` key: its address IS the escrow mint. `None` in a BB-48 snapshot
    /// (the keys are then derived again).
    #[serde(default)]
    pub mint_account: Option<Pubkey>,
}

/// The shared admission and release window (49b): what was admitted for minting and what was
/// released to ICP since `window_start_ns`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Windows {
    pub window_start_ns: u64,
    pub admitted_in_window: u128,
    pub released_in_window: u128,
}

/// "Look at this Solana transaction sooner" (`notify_solana_return`). A hint never settles
/// anything: the return observer (49c) reads it only to fetch the signature early, and a return is
/// credited by the same strict verification whether or not it was hinted.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Hint {
    pub signature: Sig,
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
    pub instance_salt: [u8; 32],
    #[serde(default)]
    pub buckets: Buckets,
    #[serde(default)]
    pub halted: Option<HaltRecord>,
    /// BB-48: the cached role public keys (`None` until derived).
    #[serde(default)]
    pub keys: Option<RoleKeys>,
    /// BB-48: the escrow mint account (`None` until mint creation, BB-49).
    #[serde(default)]
    pub mint: Option<Pubkey>,
    /// BB-48: per-lane halts (note section 4).
    #[serde(default)]
    pub lane_halts: LaneHalts,
    /// BB-48: the next operation id.
    #[serde(default)]
    pub next_op_id: u64,
    /// 49a: where the claim on the escrow mint stands.
    #[serde(default)]
    pub mint_state: MintState,
    /// 49a: why the last bind attempt did not bind (for the status query).
    #[serde(default)]
    pub bind_note: Option<String>,
    /// 49b: the next deposit id.
    #[serde(default)]
    pub next_deposit_id: u64,
    /// 49b: the next fee sweep id.
    #[serde(default)]
    pub next_sweep_id: u64,
    /// 49b: the next release sequence (the creation order of release records).
    #[serde(default)]
    pub next_release_seq: u64,
    /// 49b: the admission and release window.
    #[serde(default)]
    pub windows: Windows,
    /// 49b: pending return hints (bounded).
    #[serde(default)]
    pub hints: Vec<Hint>,
    /// 49b: the icUSD ledger's transfer fee, refreshed by the timer.
    #[serde(default)]
    pub ledger_fee_e8s: Option<u64>,
    /// 49b: the icUSD ledger's minting account. A transfer to it is a burn, so releases addressed
    /// to it are held.
    #[serde(default)]
    pub ledger_minting_account: Option<Account>,
    /// 49b: a lower bound on the ledger log length, refreshed by the timer: where the ICRC-3 search
    /// of an operation created later starts.
    #[serde(default)]
    pub ledger_tip_lower_bound: u64,
    /// 49b: the most recent fee sweeps (bounded; terminal ones are dropped first).
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
            buckets: Buckets::default(),
            halted: None,
            keys: None,
            mint: None,
            lane_halts: LaneHalts::default(),
            next_op_id: 0,
            mint_state: MintState::None,
            bind_note: None,
            next_deposit_id: 0,
            next_sweep_id: 0,
            next_release_seq: 0,
            windows: Windows::default(),
            hints: vec![],
            ledger_fee_e8s: None,
            ledger_minting_account: None,
            ledger_tip_lower_bound: 0,
            sweeps: vec![],
        }
    }
}

impl BridgeCore {
    /// The builder profile for mints, burns and refunds: only once the escrow mint is `Bound`.
    pub fn tx_profile(&self) -> Result<TxProfile, String> {
        if self.mint_state != MintState::Bound {
            return Err(format!("the escrow mint is not bound (state {:?})", self.mint_state));
        }
        let mint = self.mint.ok_or("the escrow mint is not bound yet")?;
        self.profile_for(mint)
    }

    /// The profile used to CREATE the mint, the return account and the pool accounts: the mint is the
    /// address of the `mint-account` key. No claim state is required.
    pub fn creation_profile(&self) -> Result<TxProfile, String> {
        let mint = self.keys.as_ref().and_then(|k| k.mint_account).ok_or("the mint-account key is not derived yet")?;
        self.profile_for(mint)
    }

    fn profile_for(&self, mint: Pubkey) -> Result<TxProfile, String> {
        let keys = self.keys.as_ref().ok_or("role keys are not derived yet")?;
        let profile = TxProfile {
            token_program: self.config.token_program,
            memo_program: self.config.memo_program,
            mint,
            fee_payer: keys.fee_payer,
            mint_authority: keys.mint_authority,
            return_owner: keys.return_owner,
        };
        profile.validate().map_err(|e| format!("invalid tx profile: {e}"))?;
        Ok(profile)
    }

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
cbor_storable!(NonceSlot);
cbor_storable!(SolOp);
cbor_storable!(BootstrapTx);

// ---------------------------------------------------------------------------
// Ledger operations (copied verbatim from the Base instance, `state.rs`)
//
// The record shape `ledger.rs` classifies and searches. Since 49b the deposit, release and sweep
// records below embed it (`flows::ledger_ops` is the one place that sends, resolves and searches).
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
// Deposits (ICP -> Solana, and held-claim re-mints)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum DepositKind {
    UserDeposit,
    /// Re-mint of a held release claim (a return the ICP release could not pay) to its Solana
    /// sender. Created by the mint saga (49c); the shape is fixed here.
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
    pub signature: Sig,
    pub slot: u64,
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
    /// The 32-byte Solana wallet that receives the tokens (its token account is derived).
    pub solana_recipient: Pubkey,
    /// `A`: escrowed amount (pull amount, or the held claim amount for a re-mint).
    pub amount_e8s: u64,
    pub deposit_fee_e8s: u64,
    /// `M = A - deposit_fee`.
    pub mint_amount_e8s: u64,
    /// The mint id (note section 2.1). It contains the ICP escrow-pull block index, so it exists
    /// only once the pull is proven; `None` before that.
    #[serde(default)]
    pub mint_id: Option<Hash32>,
    pub state: DepositState,
    #[serde(default)]
    pub pull: Option<LedgerOp>,
    #[serde(default)]
    pub refund: Option<LedgerOp>,
    /// The current mint operation (an id in the nonce lane), once the mint saga created it.
    #[serde(default)]
    pub mint_op: Option<u64>,
    /// Mint operations an admin abandoned after verifying by hand that they will never deliver.
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
// Releases (Solana -> ICP)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ReleaseState {
    /// In `pending_release`; the transfer is pending, unknown or not yet started.
    Pending,
    Released,
    /// In `held`; awaiting a re-mint to the Solana sender.
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

/// One ICP release owed for a custodied return whose burn is final. The only constructor is
/// `flows::release::open_release`, which requires that burn as a delivered burn operation, so no
/// record (and so no payout) exists before a final burn.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ReleaseRecord {
    /// The return id (note section 2.2): the primary key, and the memo of the ICP transfer.
    pub return_id: Hash32,
    /// Creation order (`BridgeCore::next_release_seq`).
    pub seq: u64,
    /// The delivered burn operation that made this release payable (one release per burn).
    pub burn_op_id: u64,
    /// The verified Solana sender (owner of the source token account of the return).
    pub solana_sender: Pubkey,
    /// `B`: the amount burned.
    pub amount_e8s: u64,
    /// The ICP account named by the return memo. The memo parser has already refused the anonymous
    /// principal, the management canister and this canister; the ledger's minting account (a
    /// transfer there burns the funds) is checked when the release is opened and holds it.
    pub destination: Account,
    pub return_signature: Sig,
    pub return_slot: u64,
    pub burn_signature: Sig,
    pub burn_slot: u64,
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
// Audit log
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct AuditRecord {
    pub seq: u64,
    pub at_ns: u64,
    pub transition: Option<Transition>,
    pub note: String,
    /// 49b: what the entry is about (all optional; older entries decode with `None`).
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
/// `[Solana wallet, release seq BE]`.
pub type SenderKey = [u8; 40];

pub fn sender_key(sender: &Pubkey, seq: u64) -> SenderKey {
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
    static MEMORY_MANAGER: RefCell<MemoryManager<DefaultMemoryImpl>> =
        RefCell::new(MemoryManager::init(DefaultMemoryImpl::default()));

    static CORE: RefCell<StableCell<BridgeCore, Mem>> = RefCell::new(
        StableCell::init(mem(MEM_CORE), BridgeCore::default()).expect("init core cell"));

    static AUDIT: RefCell<StableLog<AuditRecord, Mem, Mem>> = RefCell::new(
        StableLog::init(mem(MEM_AUDIT_INDEX), mem(MEM_AUDIT_DATA)).expect("init audit log"));

    static NONCE_SLOTS: RefCell<StableBTreeMap<u16, NonceSlot, Mem>> =
        RefCell::new(StableBTreeMap::init(mem(MEM_NONCE_SLOTS)));

    static OPS: RefCell<StableBTreeMap<u64, SolOp, Mem>> =
        RefCell::new(StableBTreeMap::init(mem(MEM_OPS)));

    /// Operations that are not terminal (`Delivered`), by id: the bounded set a tick walks.
    static ACTIVE_OPS: RefCell<StableBTreeMap<u64, (), Mem>> =
        RefCell::new(StableBTreeMap::init(mem(MEM_ACTIVE_OPS)));

    /// `external_id` (mint, burn or refund id) to operation id: one operation per id, forever.
    static OP_BY_EXTERNAL: RefCell<StableBTreeMap<[u8; 32], u64, Mem>> =
        RefCell::new(StableBTreeMap::init(mem(MEM_OP_BY_EXTERNAL)));

    /// One in-flight bootstrap transaction per target, by `BootstrapTarget::key`.
    static BOOTSTRAP_TXS: RefCell<StableBTreeMap<u32, BootstrapTx, Mem>> =
        RefCell::new(StableBTreeMap::init(mem(MEM_BOOTSTRAP_TXS)));

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

    /// Burn operation id to the release it made payable: one release per burn, forever.
    static RELEASE_BY_BURN_OP: RefCell<StableBTreeMap<u64, Hash32, Mem>> =
        RefCell::new(StableBTreeMap::init(mem(MEM_RELEASE_BY_BURN_OP)));
}

fn mem(id: u8) -> Mem {
    MEMORY_MANAGER.with(|m| m.borrow().get(MemoryId::new(id)))
}

pub fn read_core<R>(f: impl FnOnce(&BridgeCore) -> R) -> R {
    CORE.with(|c| f(c.borrow().get()))
}

/// Read-modify-write of the core cell. The closure runs on a copy; the copy is written back
/// only after the closure returns.
pub fn mutate_core<R>(f: impl FnOnce(&mut BridgeCore) -> R) -> R {
    CORE.with(|c| {
        let mut cell = c.borrow_mut();
        let mut core = cell.get().clone();
        let r = f(&mut core);
        cell.set(core).expect("write core cell");
        r
    })
}

/// Upgrade step: bring a stored core to the current schema. The new fields already decoded to
/// their defaults, so migrating 1 to 2 only stamps the version. Returns whether it changed.
pub fn migrate_core() -> bool {
    mutate_core(|c| {
        if c.schema_version < CORE_SCHEMA_VERSION {
            c.schema_version = CORE_SCHEMA_VERSION;
            true
        } else {
            false
        }
    })
}

// Nonce slots ---------------------------------------------------------------

pub fn get_slot(lane: Lane, index: u8) -> Option<NonceSlot> {
    NONCE_SLOTS.with(|m| m.borrow().get(&slot_key(lane, index)))
}

pub fn put_slot(slot: &NonceSlot) {
    NONCE_SLOTS.with(|m| m.borrow_mut().insert(slot_key(slot.lane, slot.index), slot.clone()));
}

/// The slots of one lane, ascending by index (at most 256).
pub fn slots_in_lane(lane: Lane) -> Vec<NonceSlot> {
    let lower = slot_key(lane, 0);
    let upper = slot_key(lane, u8::MAX);
    NONCE_SLOTS.with(|m| m.borrow().range(lower..=upper).map(|(_, v)| v).collect())
}

// Operations ----------------------------------------------------------------

pub fn get_op(id: u64) -> Option<SolOp> {
    OPS.with(|m| m.borrow().get(&id))
}

/// Insert or update an operation and keep the active set in sync. A new operation also claims
/// its external id.
pub fn put_op(op: &SolOp) {
    OPS.with(|m| m.borrow_mut().insert(op.id, op.clone()));
    ACTIVE_OPS.with(|m| {
        if op.is_terminal() {
            m.borrow_mut().remove(&op.id);
        } else {
            m.borrow_mut().insert(op.id, ());
        }
    });
    OP_BY_EXTERNAL.with(|m| m.borrow_mut().insert(op.external_id, op.id));
}

pub fn op_by_external(external_id: &[u8; 32]) -> Option<u64> {
    OP_BY_EXTERNAL.with(|m| m.borrow().get(external_id))
}

/// Ids of the non-terminal operations, ascending, at most `limit` (the lowest ids; use
/// [`active_op_page`] to walk the set fairly).
pub fn active_op_ids(limit: usize) -> Vec<u64> {
    ACTIVE_OPS.with(|m| m.borrow().iter().map(|(k, _)| k).take(limit).collect())
}

/// One page of the non-terminal operations after `after` (exclusive), at most `limit`, wrapping to
/// the lowest ids when the end of the set is reached. A walk that always passes the last id of the
/// previous page visits every operation within `ceil(n / limit)` pages, however many early
/// operations are held or blocked: no operation can starve another. Touches at most `limit`
/// index entries.
pub fn active_op_page(after: Option<u64>, limit: usize) -> Vec<u64> {
    use std::ops::Bound;
    ACTIVE_OPS.with(|m| {
        let m = m.borrow();
        let mut page: Vec<u64> = match after {
            Some(c) => m.range((Bound::Excluded(c), Bound::Unbounded)).map(|(k, _)| k).take(limit).collect(),
            None => m.iter().map(|(k, _)| k).take(limit).collect(),
        };
        if let Some(c) = after {
            if page.len() < limit {
                // Wrap around. These ids are all `<= c`, so none repeats a page entry.
                let room = limit - page.len();
                page.extend(m.range(..=c).map(|(k, _)| k).take(room));
            }
        }
        page
    })
}

thread_local! {
    /// Where the next tick's page starts. Heap only, on purpose: after an upgrade the walk simply
    /// restarts at the lowest id, and since the non-terminal index itself is stable there is
    /// nothing to rebuild.
    static TICK_CURSOR: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

pub fn tick_cursor() -> Option<u64> {
    TICK_CURSOR.with(|c| c.get())
}

pub fn set_tick_cursor(cursor: Option<u64>) {
    TICK_CURSOR.with(|c| c.set(cursor));
}

/// The ids the next tick processes (at most `limit`), advancing the cursor past them.
pub fn next_tick_page(limit: usize) -> Vec<u64> {
    let page = active_op_page(tick_cursor(), limit);
    set_tick_cursor(page.last().copied());
    page
}

pub fn active_op_count() -> u64 {
    ACTIVE_OPS.with(|m| m.borrow().len())
}

/// Allocate the next operation id.
pub fn next_op_id() -> u64 {
    mutate_core(|c| {
        let id = c.next_op_id;
        c.next_op_id += 1;
        id
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
/// checking each id against the active set (no record is decoded). Re-mint deposits are owned by
/// the canister itself and are deliberately not indexed: the canister is never the caller of an
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

/// Active deposits matching `pred`, oldest first.
pub fn deposits_in_state(pred: impl Fn(&DepositRecord) -> bool, limit: usize) -> Vec<DepositRecord> {
    let ids: Vec<u64> = ACTIVE_DEPOSITS.with(|m| m.borrow().iter().map(|(k, _)| k).collect());
    ids.into_iter().filter_map(get_deposit).filter(|d| pred(d)).take(limit).collect()
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

pub fn release_by_burn_op(burn_op_id: u64) -> Option<Hash32> {
    RELEASE_BY_BURN_OP.with(|m| m.borrow().get(&burn_op_id))
}

pub fn index_release(r: &ReleaseRecord) {
    RELEASE_BY_BURN_OP.with(|m| m.borrow_mut().insert(r.burn_op_id, r.return_id));
    SENDER_RELEASES.with(|m| m.borrow_mut().insert(sender_key(&r.solana_sender, r.seq), r.return_id));
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
    let ids: Vec<Hash32> =
        OWNER_RELEASES.with(|m| m.borrow().range(lower..=upper).rev().take(limit).map(|(_, v)| v).collect());
    ids.iter().filter_map(get_release).collect()
}

/// Releases created by returns from the Solana wallet `sender`, newest first. Lets a sender find
/// held claims (for example an unpayable destination) without local storage.
pub fn list_sender_releases(sender: &Pubkey, start_after_seq: Option<u64>, limit: u32) -> Vec<ReleaseRecord> {
    let limit = limit.clamp(1, MAX_PAGE) as usize;
    let upper = match start_after_seq {
        Some(0) => return vec![],
        Some(s) => sender_key(sender, s - 1),
        None => sender_key(sender, u64::MAX),
    };
    let lower = sender_key(sender, 0);
    let ids: Vec<Hash32> =
        SENDER_RELEASES.with(|m| m.borrow().range(lower..=upper).rev().take(limit).map(|(_, v)| v).collect());
    ids.iter().filter_map(get_release).collect()
}

/// Active releases matching `pred`, oldest first.
pub fn releases_in_state(pred: impl Fn(&ReleaseRecord) -> bool, limit: usize) -> Vec<ReleaseRecord> {
    let ids: Vec<Hash32> = ACTIVE_RELEASES.with(|m| m.borrow().iter().map(|(_, v)| v).collect());
    ids.iter().filter_map(get_release).filter(|r| pred(r)).take(limit).collect()
}

/// Sum of the amounts of releases in `Pending` (the part of `pending_release` that is spoken for).
pub fn pending_release_total() -> u128 {
    let ids: Vec<Hash32> = ACTIVE_RELEASES.with(|m| m.borrow().iter().map(|(_, v)| v).collect());
    ids.iter()
        .filter_map(get_release)
        .filter(|r| matches!(r.state, ReleaseState::Pending))
        .map(|r| r.amount_e8s as u128)
        .sum()
}

// Bootstrap transactions ----------------------------------------------------

pub fn get_bootstrap_tx(key: u32) -> Option<BootstrapTx> {
    BOOTSTRAP_TXS.with(|m| m.borrow().get(&key))
}

pub fn put_bootstrap_tx(tx: &BootstrapTx) {
    BOOTSTRAP_TXS.with(|m| m.borrow_mut().insert(tx.target.key(), tx.clone()));
}

pub fn remove_bootstrap_tx(key: u32) {
    BOOTSTRAP_TXS.with(|m| {
        m.borrow_mut().remove(&key);
    });
}

pub fn bootstrap_tx_count() -> u64 {
    BOOTSTRAP_TXS.with(|m| m.borrow().len())
}

pub fn lane_halt(lane: Lane) -> Option<LaneHalt> {
    read_core(|c| c.lane_halts.get(lane).cloned())
}

/// Halt one lane (the first reason wins) and log it.
pub fn halt_lane(lane: Lane, reason: impl Into<String>) {
    let reason = reason.into();
    let now = crate::env::now_ns();
    let newly = mutate_core(|c| {
        if c.lane_halts.get(lane).is_none() {
            c.lane_halts.set(lane, Some(LaneHalt { reason: reason.clone(), at_ns: now }));
            true
        } else {
            false
        }
    });
    if newly {
        append_audit(None, AuditCtx::note(format!("LANE HALT ({lane:?}): {reason}")));
    }
}

/// Clear a lane halt after a human resolved it. Returns whether a halt was cleared.
pub fn clear_lane_halt(lane: Lane) -> bool {
    let cleared = mutate_core(|c| {
        let had = c.lane_halts.get(lane).is_some();
        c.lane_halts.set(lane, None);
        had
    });
    if cleared {
        append_audit(None, AuditCtx::note(format!("lane halt cleared ({lane:?})")));
    }
    cleared
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

/// Test-only reset of every structure (host unit tests share thread-locals).
#[cfg(test)]
pub fn reset_for_tests() {
    CORE.with(|c| {
        c.borrow_mut().set(BridgeCore::default()).expect("reset core");
    });
    AUDIT.with(|a| {
        // `StableLog` has no clear; re-initialising over the same memories would keep the data,
        // so start from fresh memories.
        *a.borrow_mut() = StableLog::new(mem(MEM_AUDIT_INDEX), mem(MEM_AUDIT_DATA));
    });
    NONCE_SLOTS.with(|m| *m.borrow_mut() = StableBTreeMap::new(mem(MEM_NONCE_SLOTS)));
    OPS.with(|m| *m.borrow_mut() = StableBTreeMap::new(mem(MEM_OPS)));
    ACTIVE_OPS.with(|m| *m.borrow_mut() = StableBTreeMap::new(mem(MEM_ACTIVE_OPS)));
    OP_BY_EXTERNAL.with(|m| *m.borrow_mut() = StableBTreeMap::new(mem(MEM_OP_BY_EXTERNAL)));
    BOOTSTRAP_TXS.with(|m| *m.borrow_mut() = StableBTreeMap::new(mem(MEM_BOOTSTRAP_TXS)));
    DEPOSITS.with(|m| *m.borrow_mut() = StableBTreeMap::new(mem(MEM_DEPOSITS)));
    RELEASES.with(|m| *m.borrow_mut() = StableBTreeMap::new(mem(MEM_RELEASES)));
    OWNER_DEPOSITS.with(|m| *m.borrow_mut() = StableBTreeMap::new(mem(MEM_OWNER_DEPOSITS)));
    REQUEST_IDS.with(|m| *m.borrow_mut() = StableBTreeMap::new(mem(MEM_REQUEST_IDS)));
    ACTIVE_DEPOSITS.with(|m| *m.borrow_mut() = StableBTreeMap::new(mem(MEM_ACTIVE_DEPOSITS)));
    ACTIVE_REMINTS.with(|m| *m.borrow_mut() = StableBTreeMap::new(mem(MEM_ACTIVE_REMINTS)));
    ACTIVE_RELEASES.with(|m| *m.borrow_mut() = StableBTreeMap::new(mem(MEM_ACTIVE_RELEASES)));
    OWNER_RELEASES.with(|m| *m.borrow_mut() = StableBTreeMap::new(mem(MEM_OWNER_RELEASES)));
    SENDER_RELEASES.with(|m| *m.borrow_mut() = StableBTreeMap::new(mem(MEM_SENDER_RELEASES)));
    RELEASE_BY_BURN_OP.with(|m| *m.borrow_mut() = StableBTreeMap::new(mem(MEM_RELEASE_BY_BURN_OP)));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_args() -> InitArgs {
        InitArgs {
            icusd_ledger: Principal::from_slice(&[1; 10]),
            sol_rpc: Principal::from_slice(&[2; 10]),
            schnorr_key_name: TEST_KEY_NAME.into(),
            genesis_hash: pubkey_to_base58(&[0x42; 32]),
            admins: vec![Principal::from_slice(&[3; 10])],
            fee_recipient: None,
            token_program: TokenProgram::Token2022,
            memo_program: crate::tx::MEMO_PROGRAM_V3_B58.into(),
            rpc: Some(RpcSettings { cluster: Some(SolRpcCluster::Devnet), ..RpcSettings::default() }),
            lane: None,
        }
    }

    #[test]
    fn memory_layout_is_unique_and_pinned() {
        // Append-only: these exact ids and names are on stable memory forever.
        assert_eq!(
            MEMORY_LAYOUT,
            &[
                (0, "core"),
                (1, "audit_index"),
                (2, "audit_data"),
                (3, "nonce_slots"),
                (4, "ops"),
                (5, "active_ops"),
                (6, "op_by_external_id"),
                (7, "bootstrap_txs"),
                (8, "deposits"),
                (9, "releases"),
                (10, "owner_deposits"),
                (11, "request_ids"),
                (12, "active_deposits"),
                (13, "active_reminting_deposits"),
                (14, "active_releases"),
                (15, "owner_releases"),
                (16, "sender_releases"),
                (17, "release_by_burn_op")
            ]
        );
        let ids: BTreeSet<u8> = MEMORY_LAYOUT.iter().map(|(i, _)| *i).collect();
        let names: BTreeSet<&str> = MEMORY_LAYOUT.iter().map(|(_, n)| *n).collect();
        assert_eq!(ids.len(), MEMORY_LAYOUT.len());
        assert_eq!(names.len(), MEMORY_LAYOUT.len());
        // Ids are dense and ascending, so an append is visible in review.
        assert!(MEMORY_LAYOUT.windows(2).all(|w| w[1].0 == w[0].0 + 1));
    }

    #[test]
    fn valid_init_args_produce_a_config() {
        let c = validate_init(&init_args()).unwrap();
        assert_eq!(c.genesis_hash, [0x42; 32]);
        assert_eq!(c.token_program, TokenProgram::Token2022);
        assert_eq!(c.memo_program, crate::tx::MEMO_PROGRAM_V3_ID);
        assert!(c.pauses.deposits, "deposits start paused");
        assert_eq!(c.fees, BridgeFees::default());
        assert_eq!(c.rpc.cluster, Some(SolRpcCluster::Devnet));
        assert_eq!(c.lane, LaneSettings::default());
        assert_eq!(c.lane.nonce_pool_size, 4, "the pool starts at 4 accounts per lane");
    }

    #[test]
    fn rpc_cluster_follows_the_genesis_hash() {
        // Mainnet-beta: the cluster is derived when absent and cannot be anything else.
        let mut a = init_args();
        a.genesis_hash = MAINNET_GENESIS_HASH_B58.into();
        a.schnorr_key_name = MAINNET_KEY_NAME.into();
        a.rpc = None;
        assert_eq!(validate_init(&a).unwrap().rpc.cluster, Some(SolRpcCluster::Mainnet));
        a.rpc = Some(RpcSettings { cluster: Some(SolRpcCluster::Devnet), ..RpcSettings::default() });
        assert!(validate_init(&a).unwrap_err().contains("mainnet-beta cannot use"));
        // Any other cluster must say which one, and not Mainnet.
        let mut b = init_args();
        b.rpc = None;
        assert!(validate_init(&b).unwrap_err().contains("rpc.cluster"));
        b.rpc = Some(RpcSettings { cluster: Some(SolRpcCluster::Mainnet), ..RpcSettings::default() });
        assert!(validate_init(&b).unwrap_err().contains("non-mainnet"));
        b.rpc = Some(RpcSettings { cluster: Some(SolRpcCluster::Testnet), ..RpcSettings::default() });
        assert_eq!(validate_init(&b).unwrap().rpc.cluster, Some(SolRpcCluster::Testnet));
    }

    #[test]
    fn rpc_and_lane_settings_are_validated() {
        use crate::types::ConsensusSetting;
        let mut a = init_args();
        a.rpc = Some(RpcSettings {
            cluster: Some(SolRpcCluster::Devnet),
            discover: ConsensusSetting::Threshold { total: Some(3), min: 4 },
            ..RpcSettings::default()
        });
        assert!(validate_init(&a).unwrap_err().contains("threshold"));
        a.rpc = Some(RpcSettings { cluster: Some(SolRpcCluster::Devnet), cycles_per_call: 0, ..RpcSettings::default() });
        assert!(validate_init(&a).unwrap_err().contains("cycles_per_call"));
        let mut b = init_args();
        b.lane = Some(LaneSettings { nonce_pool_size: 0, ..LaneSettings::default() });
        assert!(validate_init(&b).is_err());
        b.lane = Some(LaneSettings { history_horizon_secs: 0, ..LaneSettings::default() });
        assert!(validate_init(&b).is_err());
        b.lane = Some(LaneSettings { cancel_after_secs: 10, rebroadcast_interval_secs: 30, ..LaneSettings::default() });
        assert!(validate_init(&b).is_err());
        b.lane = Some(LaneSettings { nonce_pool_size: 8, ..LaneSettings::default() });
        assert_eq!(validate_init(&b).unwrap().lane.nonce_pool_size, 8);
    }

    #[test]
    fn key_rule_mainnet_needs_key_1_and_everything_else_test_key_1() {
        let mut a = init_args();
        a.genesis_hash = MAINNET_GENESIS_HASH_B58.into();
        a.rpc = None;
        assert!(validate_init(&a).unwrap_err().contains("requires threshold key key_1"));
        a.schnorr_key_name = MAINNET_KEY_NAME.into();
        assert!(validate_init(&a).is_ok());
        let mut b = init_args();
        b.schnorr_key_name = MAINNET_KEY_NAME.into();
        assert!(validate_init(&b).unwrap_err().contains("requires threshold key test_key_1"));
        assert_eq!(cluster_name(&pubkey_from_base58(MAINNET_GENESIS_HASH_B58).unwrap()), "Solana");
        assert_eq!(cluster_name(&[0x42; 32]), "Solana (test cluster)");
    }

    #[test]
    fn stored_config_is_rechecked_at_upgrade() {
        let mut c = validate_init(&init_args()).unwrap();
        assert!(validate_stored_config(&c).is_ok());
        c.schnorr_key_name = MAINNET_KEY_NAME.into();
        assert!(validate_stored_config(&c).is_err(), "a test cluster must not hold the mainnet key name");
        let mut m = validate_init(&init_args()).unwrap();
        m.genesis_hash = pubkey_from_base58(MAINNET_GENESIS_HASH_B58).unwrap();
        assert!(validate_stored_config(&m).is_err(), "mainnet must not hold the test key name");
        m.schnorr_key_name = MAINNET_KEY_NAME.into();
        assert!(validate_stored_config(&m).is_ok());
        m.admins.clear();
        assert!(validate_stored_config(&m).is_err());
    }

    #[test]
    fn bad_init_args_are_refused() {
        let mut a = init_args();
        a.admins = vec![];
        assert!(validate_init(&a).unwrap_err().contains("admin"));
        let mut a = init_args();
        a.admins = vec![Principal::anonymous()];
        assert!(validate_init(&a).is_err());
        let mut a = init_args();
        a.sol_rpc = Principal::anonymous();
        assert!(validate_init(&a).is_err());
        let mut a = init_args();
        a.genesis_hash = "not base58 0OIl".into();
        assert!(validate_init(&a).is_err());
        let mut a = init_args();
        a.genesis_hash = "1111".into();
        assert!(validate_init(&a).is_err());
        let mut a = init_args();
        a.memo_program = "1".repeat(32);
        assert!(validate_init(&a).unwrap_err().contains("System Program"));
        let mut a = init_args();
        a.admins = vec![Principal::from_slice(&[3; 10]), Principal::from_slice(&[3; 10])];
        assert_eq!(validate_init(&a).unwrap().admins.len(), 1, "duplicates collapse");
    }

    /// The stable layout: a pinned CBOR image of a populated v1 core decodes to exactly that
    /// core. A renamed or retyped field breaks this test before it can wipe state on upgrade.
    /// The core as 49a wrote it (schema version 3): every 49b field at its default.
    fn sample_core_v3() -> BridgeCore {
        let config = validate_init(&init_args()).unwrap();
        BridgeCore {
            schema_version: 3,
            initialized: true,
            config,
            instance_salt: [0x5a; 32],
            buckets: Buckets { outstanding: 7, return_custodied: 2, burn_unknown: 1, stray: 3, ..Buckets::default() },
            halted: Some(HaltRecord { reason: "x".into(), at_ns: 9 }),
            keys: Some(RoleKeys { fee_payer: [0x11; 32], mint_authority: [0x22; 32], return_owner: [0x33; 32], mint_account: Some([0x44; 32]) }),
            mint: Some([0x44; 32]),
            lane_halts: LaneHalts { mint: Some(LaneHalt { reason: "m".into(), at_ns: 5 }), burn: None },
            next_op_id: 12,
            mint_state: MintState::Bound,
            bind_note: Some("n".into()),
            next_deposit_id: 0,
            next_sweep_id: 0,
            next_release_seq: 0,
            windows: Windows::default(),
            hints: vec![],
            ledger_fee_e8s: None,
            ledger_minting_account: None,
            ledger_tip_lower_bound: 0,
            sweeps: vec![],
        }
    }

    fn sample_op(kind: LedgerOpKind) -> LedgerOp {
        LedgerOp {
            kind,
            from: Account { owner: Principal::from_slice(&[7; 10]), subaccount: Some([8; 32]) },
            to: Account { owner: Principal::from_slice(&[9; 10]), subaccount: None },
            spender: None,
            amount_e8s: 90,
            fee_e8s: Some(10),
            memo: vec![1, 2, 3],
            created_at_time: 77,
            attempts: 2,
            ambiguous_seen: true,
            status: LedgerOpStatus::Unknown { last_error: "x".into() },
            search_from_block: 5,
            search_next_block: Some(6),
            last_attempt_ns: Some(78),
            outflow_reserved: true,
        }
    }

    /// The current core (schema version 4): every 49b field set to a distinct value.
    fn sample_core() -> BridgeCore {
        let mut config = sample_core_v3().config;
        config.limits = BridgeLimits { min_deposit_e8s: 2, max_deposit_e8s: 30, max_outstanding_e8s: 400, mint_window_limit_e8s: 50, release_window_limit_e8s: 60, window_seconds: 70, max_active_per_caller: 8, max_active_global: 90 };
        BridgeCore {
            schema_version: CORE_SCHEMA_VERSION,
            config,
            next_deposit_id: 11,
            next_sweep_id: 3,
            next_release_seq: 4,
            windows: Windows { window_start_ns: 5, admitted_in_window: 6, released_in_window: 7 },
            hints: vec![Hint { signature: Sig([0x31; 64]), submitted_by: Principal::from_slice(&[4; 10]), submitted_at_ns: 8 }],
            ledger_fee_e8s: Some(10_000),
            ledger_minting_account: Some(Account { owner: Principal::from_slice(&[0x4d; 10]), subaccount: None }),
            ledger_tip_lower_bound: 99,
            sweeps: vec![SweepRecord { id: 2, amount_e8s: 100, op: sample_op(LedgerOpKind::Sweep), created_at_ns: 12 }],
            ..sample_core_v3()
        }
    }

    /// The BB-48 core as BB-48 wrote it (schema version 2): no `mint-account` key, no claim state.
    fn bb48_core() -> BridgeCore {
        BridgeCore {
            schema_version: 2,
            keys: Some(RoleKeys { fee_payer: [0x11; 32], mint_authority: [0x22; 32], return_owner: [0x33; 32], mint_account: None }),
            mint_state: MintState::None,
            bind_note: None,
            ..sample_core_v3()
        }
    }

    /// The BB-47 core exactly as it was written then (schema version 1), for the snapshot test.
    fn bb47_core() -> BridgeCore {
        BridgeCore {
            schema_version: 1,
            initialized: true,
            config: BridgeConfig {
                icusd_ledger: Principal::from_slice(&[1; 10]),
                sol_rpc: Principal::from_slice(&[2; 10]),
                schnorr_key_name: TEST_KEY_NAME.into(),
                genesis_hash: [0x42; 32],
                admins: vec![Principal::from_slice(&[3; 10])],
                fee_recipient: None,
                token_program: TokenProgram::Token2022,
                memo_program: crate::tx::MEMO_PROGRAM_V3_ID,
                fees: BridgeFees::default(),
                pauses: Pauses { deposits: true, mints: false, releases: false, observer: false },
                // Fields BB-48 added: absent in the image, so they must default.
                rpc: RpcSettings::default(),
                lane: LaneSettings::default(),
                // 49b: absent in the image.
                limits: default_limits(),
            },
            instance_salt: [0x5a; 32],
            buckets: Buckets { outstanding: 7, return_custodied: 2, burn_unknown: 1, stray: 3, ..Buckets::default() },
            halted: Some(HaltRecord { reason: "x".into(), at_ns: 9 }),
            keys: None,
            mint: None,
            lane_halts: LaneHalts::default(),
            next_op_id: 0,
            mint_state: MintState::None,
            bind_note: None,
            next_deposit_id: 0,
            next_sweep_id: 0,
            next_release_seq: 0,
            windows: Windows::default(),
            hints: vec![],
            ledger_fee_e8s: None,
            ledger_minting_account: None,
            ledger_tip_lower_bound: 0,
            sweeps: vec![],
        }
    }

    #[test]
    fn core_round_trips_through_cbor_and_the_storable_impl() {
        let core = sample_core();
        let bytes = cbor_encode(&core);
        assert_eq!(try_cbor_decode::<BridgeCore>(&bytes).unwrap(), core);
        let stored = core.to_bytes();
        assert_eq!(BridgeCore::from_bytes(stored), core);
        assert!(try_cbor_decode::<BridgeCore>(&bytes[..bytes.len() - 1]).is_err(), "truncated bytes do not decode");
        assert!(try_cbor_decode::<BridgeCore>(&[0xff, 0x00]).is_err());
    }

    #[test]
    fn core_decodes_from_the_pinned_current_image() {
        // Field names are the schema. Pinned from `cbor_encode(&sample_core())` at BB-49 49b.
        let image = hex::decode(PINNED_CORE_V4).expect("hex");
        assert_eq!(try_cbor_decode::<BridgeCore>(&image).unwrap(), sample_core());
        assert_eq!(cbor_encode(&sample_core()), image, "the encoding itself is stable");
    }

    /// The 49a core, byte for byte as 49a wrote it (schema version 3), still decodes after 49b
    /// appended fields: the new fields default, the limits take the starting values, and the old
    /// fields keep their values.
    #[test]
    fn the_49a_core_image_decodes_with_the_49b_fields_defaulted() {
        let image = hex::decode(PINNED_CORE_V3).expect("hex");
        let decoded = try_cbor_decode::<BridgeCore>(&image).unwrap();
        assert_eq!(decoded.schema_version, 3);
        assert_eq!(decoded.config.limits, default_limits());
        let mut expected = sample_core_v3();
        expected.config.limits = default_limits();
        assert_eq!(decoded, expected);
        assert_eq!((decoded.next_deposit_id, decoded.next_sweep_id, decoded.next_release_seq), (0, 0, 0));
        assert_eq!(decoded.ledger_fee_e8s, None, "the ledger facts are read again by the timer");
        assert!(decoded.sweeps.is_empty() && decoded.hints.is_empty());
        assert_eq!(BridgeCore::from_bytes(decoded.to_bytes()), decoded);
    }

    /// The BB-48 core, byte for byte as BB-48 wrote it, still decodes after 49a appended fields.
    #[test]
    fn bb48_core_image_decodes_with_the_new_fields_defaulted() {
        let image = hex::decode(BB48_CORE_IMAGE).expect("hex");
        let decoded = try_cbor_decode::<BridgeCore>(&image).unwrap();
        let mut expected = bb48_core();
        expected.config.limits = default_limits();
        assert_eq!(decoded, expected);
        assert_eq!(decoded.schema_version, 2);
        assert_eq!(decoded.keys.as_ref().unwrap().mint_account, None, "the mint-account key is derived again");
        assert_eq!(decoded.mint_state, MintState::None, "an old snapshot has no claim on the mint: it must bind again");
        assert!(decoded.tx_profile().is_err(), "an unbound mint builds no operation messages");
        assert_eq!(BridgeCore::from_bytes(decoded.to_bytes()), decoded);
    }

    /// A snapshot of the BB-47 core, byte for byte as BB-47 wrote it, must still decode after
    /// BB-48 appended fields: new fields default, old fields keep their values.
    #[test]
    fn bb47_core_image_decodes_with_the_new_fields_defaulted() {
        let image = hex::decode(BB47_CORE_IMAGE).expect("hex");
        let decoded = try_cbor_decode::<BridgeCore>(&image).unwrap();
        assert_eq!(decoded, bb47_core());
        assert_eq!(decoded.schema_version, 1);
        assert_eq!(decoded.config.rpc.cluster, None, "a BB-47 snapshot has no SOL RPC cluster: calls are refused until it is set");
        assert_eq!(decoded.keys, None);
        assert_eq!(decoded.mint, None);
        assert_eq!(decoded.lane_halts, LaneHalts::default());
        assert_eq!(decoded.next_op_id, 0);
        // And the Storable round trip of the decoded value is lossless.
        assert_eq!(BridgeCore::from_bytes(decoded.to_bytes()), decoded);
    }

    #[test]
    fn migrate_core_stamps_the_schema_version_once() {
        reset_for_tests();
        mutate_core(|c| c.schema_version = 1);
        assert!(migrate_core());
        assert_eq!(read_core(|c| c.schema_version), CORE_SCHEMA_VERSION);
        assert!(!migrate_core(), "already current");
    }

    #[test]
    fn older_images_without_appended_fields_decode_with_defaults() {
        use ciborium::Value;
        // A v1 core written before `buckets`, `halted`, `fees` and `pauses` existed.
        let full: Value = ciborium::de::from_reader(cbor_encode(&sample_core()).as_slice()).unwrap();
        let strip = |v: &mut Value, keys: &[&str]| {
            if let Value::Map(entries) = v {
                entries.retain(|(k, _)| !matches!(k, Value::Text(t) if keys.contains(&t.as_str())));
            }
        };
        let mut old = full.clone();
        strip(
            &mut old,
            &[
                "buckets",
                "halted",
                "keys",
                "mint",
                "lane_halts",
                "next_op_id",
                "mint_state",
                "bind_note",
                "next_deposit_id",
                "next_sweep_id",
                "next_release_seq",
                "windows",
                "hints",
                "ledger_fee_e8s",
                "ledger_minting_account",
                "ledger_tip_lower_bound",
                "sweeps",
            ],
        );
        if let Value::Map(entries) = &mut old {
            for (k, v) in entries.iter_mut() {
                if matches!(k, Value::Text(t) if t == "config") {
                    strip(v, &["fees", "pauses", "rpc", "lane", "limits"]);
                }
            }
        }
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(&old, &mut bytes).unwrap();
        let decoded = try_cbor_decode::<BridgeCore>(&bytes).unwrap();
        assert_eq!(decoded.buckets, Buckets::default());
        assert_eq!(decoded.halted, None);
        assert_eq!(decoded.config.fees, BridgeFees::default());
        assert_eq!(decoded.config.pauses, Pauses::default());
        assert_eq!(decoded.config.rpc, RpcSettings::default());
        assert_eq!(decoded.config.lane, LaneSettings::default());
        assert_eq!(decoded.config.limits, default_limits());
        assert_eq!((decoded.next_deposit_id, decoded.windows, decoded.hints.len(), decoded.sweeps.len()), (0, Windows::default(), 0, 0));
        assert_eq!((decoded.keys, decoded.mint, decoded.next_op_id), (None, None, 0));
        assert_eq!(decoded.lane_halts, LaneHalts::default());
        assert_eq!(decoded.config.admins, sample_core().config.admins);
    }

    #[test]
    fn transitions_update_buckets_and_the_audit_log_atomically() {
        reset_for_tests();
        assert_eq!(audit_len(), 0);
        apply_transitions(
            &[Transition::AdmitDeposit { amount: 10 }, Transition::PullSucceeded { amount: 10 }, Transition::MintDelivered { amount: 10, fee: 0 }],
            AuditCtx::note("mint"),
        )
        .unwrap();
        assert_eq!(read_core(|c| c.buckets.outstanding), 10);
        assert_eq!(audit_len(), 3);
        // A failing batch changes nothing and logs nothing.
        let before = read_core(|c| c.buckets);
        assert!(apply_transitions(&[Transition::ReturnCustodied { amount: 4 }, Transition::BurnSigned { amount: 5 }], AuditCtx::note("bad")).is_err());
        assert_eq!(read_core(|c| c.buckets), before);
        assert_eq!(audit_len(), 3);
        apply_transition(Transition::ReturnCustodied { amount: 4 }, AuditCtx::note("return")).unwrap();
        assert_eq!(read_core(|c| (c.buckets.outstanding, c.buckets.return_custodied)), (6, 4));
        let page = audit_page(0, 10);
        assert_eq!(page.len(), 4);
        assert_eq!(page[3].transition, Some(Transition::ReturnCustodied { amount: 4 }));
        assert_eq!(page[3].seq, 3);
        assert_eq!(audit_page(2, 1).len(), 1);
    }

    #[test]
    fn the_first_halt_reason_wins() {
        reset_for_tests();
        assert!(!is_halted());
        halt("first");
        halt("second");
        assert_eq!(read_core(|c| c.halted.clone().map(|h| h.reason)), Some("first".to_string()));
        assert_eq!(audit_len(), 1);
    }

    #[test]
    fn escrow_account_uses_the_solana_label() {
        let core = BridgeCore::default();
        let me = Principal::from_slice(&[0xbb; 10]);
        let acct = core.escrow_account(me);
        assert_eq!(acct.owner, me);
        assert_eq!(acct.subaccount, Some(ids::escrow_subaccount()));
    }

    pub(crate) fn sample_deposit() -> DepositRecord {
        DepositRecord {
            id: 5,
            kind: DepositKind::Remint { return_id: [0x61; 32] },
            owner: Principal::from_slice(&[7; 10]),
            from: Some(Account { owner: Principal::from_slice(&[7; 10]), subaccount: Some([8; 32]) }),
            client_request_id: vec![1; 16],
            solana_recipient: [0x77; 32],
            amount_e8s: 100,
            deposit_fee_e8s: 3,
            mint_amount_e8s: 97,
            mint_id: Some([0x62; 32]),
            state: DepositState::MintHeld { reason: "r".into() },
            pull: Some(sample_op(LedgerOpKind::Pull)),
            refund: Some(sample_op(LedgerOpKind::Refund)),
            mint_op: Some(12),
            abandoned_mint_ops: vec![9, 10],
            delivered: Some(MintDelivery { signature: Sig([0x63; 64]), slot: 321 }),
            hold_retry_at_ns: Some(55),
            created_at_ns: 1,
            updated_at_ns: 2,
        }
    }

    pub(crate) fn sample_release() -> ReleaseRecord {
        ReleaseRecord {
            return_id: [0x71; 32],
            seq: 6,
            burn_op_id: 14,
            solana_sender: [0x72; 32],
            amount_e8s: 500,
            destination: Account { owner: Principal::from_slice(&[9; 10]), subaccount: Some([3; 32]) },
            return_signature: Sig([0x73; 64]),
            return_slot: 100,
            burn_signature: Sig([0x74; 64]),
            burn_slot: 101,
            release_fee_e8s: 7,
            state: ReleaseState::Held { reason: "h".into() },
            transfer: Some(sample_op(LedgerOpKind::Release)),
            created_at_ns: 3,
            updated_at_ns: 4,
        }
    }

    /// The ledger saga records round trip through the Storable impl, and a pinned image of each
    /// decodes to exactly the sample: a renamed or retyped field fails here, not at an upgrade.
    #[test]
    fn deposit_and_release_records_are_pinned() {
        let d = sample_deposit();
        assert_eq!(DepositRecord::from_bytes(d.to_bytes()), d);
        let image = hex::decode(PINNED_DEPOSIT_V1).expect("hex");
        assert_eq!(try_cbor_decode::<DepositRecord>(&image).unwrap(), d);
        assert_eq!(cbor_encode(&d), image);
        let r = sample_release();
        assert_eq!(ReleaseRecord::from_bytes(r.to_bytes()), r);
        let image = hex::decode(PINNED_RELEASE_V1).expect("hex");
        assert_eq!(try_cbor_decode::<ReleaseRecord>(&image).unwrap(), r);
        assert_eq!(cbor_encode(&r), image);
    }

    /// A deposit written with only the fields that existed when the record was first stored decodes
    /// with the appended ones defaulted (the `#[serde(default)]` discipline the records follow).
    #[test]
    fn a_deposit_without_the_appended_fields_decodes() {
        use ciborium::Value;
        let full: Value = ciborium::de::from_reader(cbor_encode(&sample_deposit()).as_slice()).unwrap();
        let Value::Map(mut entries) = full else { panic!("a map") };
        entries.retain(|(k, _)| {
            !matches!(k, Value::Text(t) if ["mint_id", "pull", "refund", "mint_op", "abandoned_mint_ops", "delivered", "hold_retry_at_ns"].contains(&t.as_str()))
        });
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(&Value::Map(entries), &mut bytes).unwrap();
        let d = try_cbor_decode::<DepositRecord>(&bytes).unwrap();
        assert_eq!((d.mint_id, d.pull, d.refund, d.mint_op, d.delivered, d.hold_retry_at_ns), (None, None, None, None, None, None));
        assert!(d.abandoned_mint_ops.is_empty());
    }

    #[test]
    fn the_deposit_and_release_indexes_keep_exact_active_sets() {
        reset_for_tests();
        let owner = Principal::from_slice(&[7; 10]);
        let mut d = sample_deposit();
        d.kind = DepositKind::UserDeposit;
        d.state = DepositState::Escrowed;
        put_deposit(&d);
        index_owner_deposit(&owner, d.id);
        assert_eq!((active_deposit_count(), active_user_deposit_count(), active_remint_count()), (1, 1, 0));
        assert_eq!(active_deposits_owned_by(&owner), 1);
        d.state = DepositState::Delivered;
        put_deposit(&d);
        assert_eq!((active_deposit_count(), active_deposits_owned_by(&owner)), (0, 0), "terminal leaves the active sets");
        assert_eq!(list_owner_deposits(&owner, None, 10).len(), 1, "but stays listed");
        let mut re = sample_deposit();
        re.id = 6;
        re.state = DepositState::Escrowed;
        put_deposit(&re);
        assert_eq!((active_deposit_count(), active_user_deposit_count(), active_remint_count()), (1, 0, 1));

        let r = sample_release();
        let mut pending = r.clone();
        pending.state = ReleaseState::Pending;
        put_release(&pending);
        index_release(&pending);
        assert_eq!(active_release_count(), 1);
        assert_eq!(pending_release_total(), 500);
        assert_eq!(release_by_burn_op(14), Some([0x71; 32]));
        assert_eq!(list_owner_releases(&pending.destination.owner, None, 10).len(), 1);
        assert_eq!(list_sender_releases(&pending.solana_sender, None, 10).len(), 1);
        assert!(list_sender_releases(&[0x99; 32], None, 10).is_empty());
        let mut done = pending.clone();
        done.state = ReleaseState::Released;
        put_release(&done);
        assert_eq!((active_release_count(), pending_release_total()), (0, 0));
        // Held claims are active but not part of the pending total.
        put_release(&r);
        assert_eq!((active_release_count(), pending_release_total()), (1, 0));
    }

    /// CBOR image of the BB-47 core (`bb47_core()`), exactly as BB-47 wrote it. NEVER regenerate
    /// this constant: it is the snapshot an upgrade from BB-47 must still read.
    const BB47_CORE_IMAGE: &str = concat!(
        "a66e736368656d615f76657273696f6e016b696e697469616c697a6564f566636f6e666967aa6c69637573645f6c6564",
        "6765724a0101010101010101010167736f6c5f7270634a02020202020202020202707363686e6f72725f6b65795f6e61",
        "6d656a746573745f6b65795f316c67656e657369735f6861736898201842184218421842184218421842184218421842",
        "18421842184218421842184218421842184218421842184218421842184218421842184218421842184218426661646d",
        "696e73814a030303030303030303036d6665655f726563697069656e74f66d746f6b656e5f70726f6772616d69546f6b",
        "656e323032326c6d656d6f5f70726f6772616d982005184a1853185a18991829182106184d182418e81871186018da18",
        "38187c187c183518b518dd18bc189218bb188118e4181f18a818401841051844188d6466656573a26f6465706f736974",
        "5f6665655f653873006f72656c656173655f6665655f6538730066706175736573a4686465706f73697473f5656d696e",
        "7473f46872656c6561736573f4686f62736572766572f46d696e7374616e63655f73616c749820185a185a185a185a18",
        "5a185a185a185a185a185a185a185a185a185a185a185a185a185a185a185a185a185a185a185a185a185a185a185a18",
        "5a185a185a185a676275636b657473ab6b6f75747374616e64696e67076c70656e64696e675f6d696e74007072657475",
        "726e5f637573746f64696564026c6275726e5f756e6b6e6f776e016f70656e64696e675f72656c65617365006e70656e",
        "64696e675f726566756e64006468656c64006466656573006e72657365727665645f70756c6c73006f756e6b6e6f776e",
        "5f6f7574666c6f7700657374726179036668616c746564a266726561736f6e61786561745f6e7309",
    );

    #[test]
    #[ignore = "writes the scratch hex files that the pinned images are generated from"]
    fn zz_dump_images() {
        let dir = std::env::var("RUMI_DUMP_DIR").unwrap_or_else(|_| "/tmp".into());
        std::fs::write(format!("{dir}/core_v4.hex"), hex::encode(cbor_encode(&sample_core()))).unwrap();
        std::fs::write(format!("{dir}/deposit_v1.hex"), hex::encode(cbor_encode(&sample_deposit()))).unwrap();
        std::fs::write(format!("{dir}/release_v1.hex"), hex::encode(cbor_encode(&sample_release()))).unwrap();
    }

    /// CBOR image of the BB-48 core (`bb48_core()`), exactly as BB-48 wrote it (schema version 2).
    /// NEVER regenerate this constant: it is the snapshot an upgrade from BB-48 must still read.
    const BB48_CORE_IMAGE: &str = concat!(
        "aa6e736368656d615f76657273696f6e026b696e697469616c697a6564f566636f6e666967ac6c69637573645f6c6564",
        "6765724a0101010101010101010167736f6c5f7270634a02020202020202020202707363686e6f72725f6b65795f6e61",
        "6d656a746573745f6b65795f316c67656e657369735f6861736898201842184218421842184218421842184218421842",
        "18421842184218421842184218421842184218421842184218421842184218421842184218421842184218426661646d",
        "696e73814a030303030303030303036d6665655f726563697069656e74f66d746f6b656e5f70726f6772616d69546f6b",
        "656e323032326c6d656d6f5f70726f6772616d982005184a1853185a18991829182106184d182418e81871186018da18",
        "38187c187c183518b518dd18bc189218bb188118e4181f18a818401841051844188d6466656573a26f6465706f736974",
        "5f6665655f653873006f72656c656173655f6665655f6538730066706175736573a4686465706f73697473f5656d696e",
        "7473f46872656c6561736573f4686f62736572766572f463727063a567636c7573746572664465766e65746676657269",
        "667968457175616c69747968646973636f766572a1695468726573686f6c64a265746f74616c03636d696e026f637963",
        "6c65735f7065725f63616c6c1b00000002540be40076726573706f6e73655f73697a655f657374696d617465f6646c61",
        "6e65a76f6e6f6e63655f706f6f6c5f73697a65047163616e63656c5f61667465725f7365637319012c7819726562726f",
        "6164636173745f696e74657276616c5f73656373181e747265636f6e63696c655f616c61726d5f736563731903847468",
        "6973746f72795f686f72697a6f6e5f736563731954606c6d61785f7265697373756573036e646973636f7665725f6c69",
        "6d69740a6d696e7374616e63655f73616c749820185a185a185a185a185a185a185a185a185a185a185a185a185a185a",
        "185a185a185a185a185a185a185a185a185a185a185a185a185a185a185a185a185a185a676275636b657473ab6b6f75",
        "747374616e64696e67076c70656e64696e675f6d696e74007072657475726e5f637573746f64696564026c6275726e5f",
        "756e6b6e6f776e016f70656e64696e675f72656c65617365006e70656e64696e675f726566756e64006468656c640064",
        "66656573006e72657365727665645f70756c6c73006f756e6b6e6f776e5f6f7574666c6f770065737472617903666861",
        "6c746564a266726561736f6e61786561745f6e7309646b657973a3696665655f70617965729820111111111111111111",
        "11111111111111111111111111111111111111111111116e6d696e745f617574686f7269747998201822182218221822",
        "182218221822182218221822182218221822182218221822182218221822182218221822182218221822182218221822",
        "18221822182218226c72657475726e5f6f776e6572982018331833183318331833183318331833183318331833183318",
        "331833183318331833183318331833183318331833183318331833183318331833183318331833646d696e7498201844",
        "184418441844184418441844184418441844184418441844184418441844184418441844184418441844184418441844",
        "18441844184418441844184418446a6c616e655f68616c7473a2646d696e74a266726561736f6e616d6561745f6e7305",
        "646275726ef66a6e6578745f6f705f69640c",
    );

    /// CBOR image of the 49a core (`sample_core_v3()`), exactly as 49a wrote it (schema version 3).
    /// NEVER regenerate this constant: it is the snapshot an upgrade from 49a must still read.
    const PINNED_CORE_V3: &str = concat!(
        "ac6e736368656d615f76657273696f6e036b696e697469616c697a6564f566636f6e666967ac6c69637573645f6c6564",
        "6765724a0101010101010101010167736f6c5f7270634a02020202020202020202707363686e6f72725f6b65795f6e61",
        "6d656a746573745f6b65795f316c67656e657369735f6861736898201842184218421842184218421842184218421842",
        "18421842184218421842184218421842184218421842184218421842184218421842184218421842184218426661646d",
        "696e73814a030303030303030303036d6665655f726563697069656e74f66d746f6b656e5f70726f6772616d69546f6b",
        "656e323032326c6d656d6f5f70726f6772616d982005184a1853185a18991829182106184d182418e81871186018da18",
        "38187c187c183518b518dd18bc189218bb188118e4181f18a818401841051844188d6466656573a26f6465706f736974",
        "5f6665655f653873006f72656c656173655f6665655f6538730066706175736573a4686465706f73697473f5656d696e",
        "7473f46872656c6561736573f4686f62736572766572f463727063a567636c7573746572664465766e65746676657269",
        "667968457175616c69747968646973636f766572a1695468726573686f6c64a265746f74616c03636d696e026f637963",
        "6c65735f7065725f63616c6c1b00000002540be40076726573706f6e73655f73697a655f657374696d617465f6646c61",
        "6e65a76f6e6f6e63655f706f6f6c5f73697a65047163616e63656c5f61667465725f7365637319012c7819726562726f",
        "6164636173745f696e74657276616c5f73656373181e747265636f6e63696c655f616c61726d5f736563731903847468",
        "6973746f72795f686f72697a6f6e5f736563731954606c6d61785f7265697373756573036e646973636f7665725f6c69",
        "6d69740a6d696e7374616e63655f73616c749820185a185a185a185a185a185a185a185a185a185a185a185a185a185a",
        "185a185a185a185a185a185a185a185a185a185a185a185a185a185a185a185a185a185a676275636b657473ab6b6f75",
        "747374616e64696e67076c70656e64696e675f6d696e74007072657475726e5f637573746f64696564026c6275726e5f",
        "756e6b6e6f776e016f70656e64696e675f72656c65617365006e70656e64696e675f726566756e64006468656c640064",
        "66656573006e72657365727665645f70756c6c73006f756e6b6e6f776e5f6f7574666c6f770065737472617903666861",
        "6c746564a266726561736f6e61786561745f6e7309646b657973a4696665655f70617965729820111111111111111111",
        "11111111111111111111111111111111111111111111116e6d696e745f617574686f7269747998201822182218221822",
        "182218221822182218221822182218221822182218221822182218221822182218221822182218221822182218221822",
        "18221822182218226c72657475726e5f6f776e6572982018331833183318331833183318331833183318331833183318",
        "3318331833183318331833183318331833183318331833183318331833183318331833183318336c6d696e745f616363",
        "6f756e749820184418441844184418441844184418441844184418441844184418441844184418441844184418441844",
        "18441844184418441844184418441844184418441844646d696e74982018441844184418441844184418441844184418",
        "4418441844184418441844184418441844184418441844184418441844184418441844184418441844184418446a6c61",
        "6e655f68616c7473a2646d696e74a266726561736f6e616d6561745f6e7305646275726ef66a6e6578745f6f705f6964",
        "0c6a6d696e745f737461746565426f756e646962696e645f6e6f7465616e",
    );

    /// CBOR image of `sample_core()` at BB-49 49b (schema version 4).
    const PINNED_CORE_V4: &str = concat!(
        "b56e736368656d615f76657273696f6e046b696e697469616c697a6564f566636f6e666967ad6c69637573645f6c656467",
        "65724a0101010101010101010167736f6c5f7270634a02020202020202020202707363686e6f72725f6b65795f6e616d65",
        "6a746573745f6b65795f316c67656e657369735f6861736898201842184218421842184218421842184218421842184218",
        "42184218421842184218421842184218421842184218421842184218421842184218421842184218426661646d696e7381",
        "4a030303030303030303036d6665655f726563697069656e74f66d746f6b656e5f70726f6772616d69546f6b656e323032",
        "326c6d656d6f5f70726f6772616d982005184a1853185a18991829182106184d182418e81871186018da1838187c187c18",
        "3518b518dd18bc189218bb188118e4181f18a818401841051844188d6466656573a26f6465706f7369745f6665655f6538",
        "73006f72656c656173655f6665655f6538730066706175736573a4686465706f73697473f5656d696e7473f46872656c65",
        "61736573f4686f62736572766572f463727063a567636c7573746572664465766e65746676657269667968457175616c69",
        "747968646973636f766572a1695468726573686f6c64a265746f74616c03636d696e026f6379636c65735f7065725f6361",
        "6c6c1b00000002540be40076726573706f6e73655f73697a655f657374696d617465f6646c616e65a76f6e6f6e63655f70",
        "6f6f6c5f73697a65047163616e63656c5f61667465725f7365637319012c7819726562726f6164636173745f696e746572",
        "76616c5f73656373181e747265636f6e63696c655f616c61726d5f7365637319038474686973746f72795f686f72697a6f",
        "6e5f736563731954606c6d61785f7265697373756573036e646973636f7665725f6c696d69740a666c696d697473a86f6d",
        "696e5f6465706f7369745f653873026f6d61785f6465706f7369745f653873181e736d61785f6f75747374616e64696e67",
        "5f653873190190756d696e745f77696e646f775f6c696d69745f6538731832781872656c656173655f77696e646f775f6c",
        "696d69745f653873183c6e77696e646f775f7365636f6e64731846756d61785f6163746976655f7065725f63616c6c6572",
        "08716d61785f6163746976655f676c6f62616c185a6d696e7374616e63655f73616c749820185a185a185a185a185a185a",
        "185a185a185a185a185a185a185a185a185a185a185a185a185a185a185a185a185a185a185a185a185a185a185a185a18",
        "5a185a676275636b657473ab6b6f75747374616e64696e67076c70656e64696e675f6d696e74007072657475726e5f6375",
        "73746f64696564026c6275726e5f756e6b6e6f776e016f70656e64696e675f72656c65617365006e70656e64696e675f72",
        "6566756e64006468656c64006466656573006e72657365727665645f70756c6c73006f756e6b6e6f776e5f6f7574666c6f",
        "7700657374726179036668616c746564a266726561736f6e61786561745f6e7309646b657973a4696665655f7061796572",
        "982011111111111111111111111111111111111111111111111111111111111111116e6d696e745f617574686f72697479",
        "98201822182218221822182218221822182218221822182218221822182218221822182218221822182218221822182218",
        "22182218221822182218221822182218226c72657475726e5f6f776e657298201833183318331833183318331833183318",
        "33183318331833183318331833183318331833183318331833183318331833183318331833183318331833183318336c6d",
        "696e745f6163636f756e749820184418441844184418441844184418441844184418441844184418441844184418441844",
        "18441844184418441844184418441844184418441844184418441844646d696e7498201844184418441844184418441844",
        "18441844184418441844184418441844184418441844184418441844184418441844184418441844184418441844184418",
        "446a6c616e655f68616c7473a2646d696e74a266726561736f6e616d6561745f6e7305646275726ef66a6e6578745f6f70",
        "5f69640c6a6d696e745f737461746565426f756e646962696e645f6e6f7465616e6f6e6578745f6465706f7369745f6964",
        "0b6d6e6578745f73776565705f696403706e6578745f72656c656173655f736571046777696e646f7773a36f77696e646f",
        "775f73746172745f6e73057261646d69747465645f696e5f77696e646f77067272656c65617365645f696e5f77696e646f",
        "77076568696e747381a3697369676e61747572655840313131313131313131313131313131313131313131313131313131",
        "313131313131313131313131313131313131313131313131313131313131313131313131316c7375626d69747465645f62",
        "794a040404040404040404046f7375626d69747465645f61745f6e73086e6c65646765725f6665655f653873192710766c",
        "65646765725f6d696e74696e675f6163636f756e74a2656f776e65724a4d4d4d4d4d4d4d4d4d4d6a7375626163636f756e",
        "74f6766c65646765725f7469705f6c6f7765725f626f756e6418636673776565707381a4626964026a616d6f756e745f65",
        "38731864626f70af646b696e646553776565706466726f6da2656f776e65724a070707070707070707076a737562616363",
        "6f756e749820080808080808080808080808080808080808080808080808080808080808080862746fa2656f776e65724a",
        "090909090909090909096a7375626163636f756e74f6677370656e646572f66a616d6f756e745f653873185a676665655f",
        "6538730a646d656d6f830102036f637265617465645f61745f74696d65184d68617474656d707473026e616d626967756f",
        "75735f7365656ef566737461747573a167556e6b6e6f776ea16a6c6173745f6572726f726178717365617263685f66726f",
        "6d5f626c6f636b05717365617263685f6e6578745f626c6f636b066f6c6173745f617474656d70745f6e73184e706f7574",
        "666c6f775f7265736572766564f56d637265617465645f61745f6e730c",
    );
    /// CBOR image of `sample_deposit()` as first stored (49b).
    const PINNED_DEPOSIT_V1: &str = concat!(
        "b362696405646b696e64a16652656d696e74a16972657475726e5f69649820186118611861186118611861186118611861",
        "18611861186118611861186118611861186118611861186118611861186118611861186118611861186118611861656f77",
        "6e65724a070707070707070707076466726f6da2656f776e65724a070707070707070707076a7375626163636f756e7498",
        "20080808080808080808080808080808080808080808080808080808080808080871636c69656e745f726571756573745f",
        "6964900101010101010101010101010101010170736f6c616e615f726563697069656e7498201877187718771877187718",
        "77187718771877187718771877187718771877187718771877187718771877187718771877187718771877187718771877",
        "187718776a616d6f756e745f65387318646f6465706f7369745f6665655f653873036f6d696e745f616d6f756e745f6538",
        "731861676d696e745f69649820186218621862186218621862186218621862186218621862186218621862186218621862",
        "18621862186218621862186218621862186218621862186218621862657374617465a1684d696e7448656c64a166726561",
        "736f6e61726470756c6caf646b696e646450756c6c6466726f6da2656f776e65724a070707070707070707076a73756261",
        "63636f756e749820080808080808080808080808080808080808080808080808080808080808080862746fa2656f776e65",
        "724a090909090909090909096a7375626163636f756e74f6677370656e646572f66a616d6f756e745f653873185a676665",
        "655f6538730a646d656d6f830102036f637265617465645f61745f74696d65184d68617474656d707473026e616d626967",
        "756f75735f7365656ef566737461747573a167556e6b6e6f776ea16a6c6173745f6572726f726178717365617263685f66",
        "726f6d5f626c6f636b05717365617263685f6e6578745f626c6f636b066f6c6173745f617474656d70745f6e73184e706f",
        "7574666c6f775f7265736572766564f566726566756e64af646b696e6466526566756e646466726f6da2656f776e65724a",
        "070707070707070707076a7375626163636f756e7498200808080808080808080808080808080808080808080808080808",
        "08080808080862746fa2656f776e65724a090909090909090909096a7375626163636f756e74f6677370656e646572f66a",
        "616d6f756e745f653873185a676665655f6538730a646d656d6f830102036f637265617465645f61745f74696d65184d68",
        "617474656d707473026e616d626967756f75735f7365656ef566737461747573a167556e6b6e6f776ea16a6c6173745f65",
        "72726f726178717365617263685f66726f6d5f626c6f636b05717365617263685f6e6578745f626c6f636b066f6c617374",
        "5f617474656d70745f6e73184e706f7574666c6f775f7265736572766564f5676d696e745f6f700c726162616e646f6e65",
        "645f6d696e745f6f707382090a6964656c697665726564a2697369676e6174757265584063636363636363636363636363",
        "63636363636363636363636363636363636363636363636363636363636363636363636363636363636363636363636363",
        "636364736c6f7419014170686f6c645f72657472795f61745f6e7318376d637265617465645f61745f6e73016d75706461",
        "7465645f61745f6e7302",
    );
    /// CBOR image of `sample_release()` as first stored (49b).
    const PINNED_RELEASE_V1: &str = concat!(
        "af6972657475726e5f69649820187118711871187118711871187118711871187118711871187118711871187118711871",
        "1871187118711871187118711871187118711871187118711871187163736571066a6275726e5f6f705f69640e6d736f6c",
        "616e615f73656e646572982018721872187218721872187218721872187218721872187218721872187218721872187218",
        "7218721872187218721872187218721872187218721872187218726a616d6f756e745f6538731901f46b64657374696e61",
        "74696f6ea2656f776e65724a090909090909090909096a7375626163636f756e7498200303030303030303030303030303",
        "0303030303030303030303030303030303037072657475726e5f7369676e61747572655840737373737373737373737373",
        "73737373737373737373737373737373737373737373737373737373737373737373737373737373737373737373737373",
        "7373736b72657475726e5f736c6f7418646e6275726e5f7369676e61747572655840747474747474747474747474747474",
        "74747474747474747474747474747474747474747474747474747474747474747474747474747474747474747474747474",
        "696275726e5f736c6f7418656f72656c656173655f6665655f65387307657374617465a16448656c64a166726561736f6e",
        "6168687472616e73666572af646b696e646752656c656173656466726f6da2656f776e65724a070707070707070707076a",
        "7375626163636f756e749820080808080808080808080808080808080808080808080808080808080808080862746fa265",
        "6f776e65724a090909090909090909096a7375626163636f756e74f6677370656e646572f66a616d6f756e745f65387318",
        "5a676665655f6538730a646d656d6f830102036f637265617465645f61745f74696d65184d68617474656d707473026e61",
        "6d626967756f75735f7365656ef566737461747573a167556e6b6e6f776ea16a6c6173745f6572726f7261787173656172",
        "63685f66726f6d5f626c6f636b05717365617263685f6e6578745f626c6f636b066f6c6173745f617474656d70745f6e73",
        "184e706f7574666c6f775f7265736572766564f56d637265617465645f61745f6e73036d757064617465645f61745f6e73",
        "04",
    );
}
