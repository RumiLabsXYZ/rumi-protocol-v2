//! Stable state. Every record is written directly into `ic-stable-structures`, so an upgrade
//! needs no `pre_upgrade` snapshot and cannot lose operations.
//!
//! Records are CBOR (`ciborium`). CBOR maps fields by name, so a field added later with
//! `#[serde(default)]` decodes from older bytes. (That is not true of Candid `Decode!`, which
//! is why Candid is never used for stored records here.)
//!
//! The `MemoryId` layout is append-only. `memory_layout_is_unique_and_pinned` pins it. This
//! is the BB-47 skeleton layout (the core cell and the audit log); BB-48 and BB-49 append
//! their maps (mint records, nonce pool, returns, burns) after id 2 and never reuse an id.

use crate::accounting::{AccountingError, Buckets, Transition};
use crate::ids;
use crate::tx::{pubkey_from_base58, pubkey_to_base58, TokenProgram, SYSTEM_PROGRAM_ID};
use crate::types::{BridgeFees, InitArgs, Pauses};
use candid::Principal;
use ic_stable_structures::memory_manager::{MemoryId, MemoryManager, VirtualMemory};
use ic_stable_structures::storable::Bound;
use ic_stable_structures::{DefaultMemoryImpl, StableCell, StableLog, Storable};
use icrc_ledger_types::icrc1::account::Account;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::BTreeSet;

pub type Mem = VirtualMemory<DefaultMemoryImpl>;

pub const MEM_CORE: u8 = 0;
pub const MEM_AUDIT_INDEX: u8 = 1;
pub const MEM_AUDIT_DATA: u8 = 2;

/// Append-only. Never reuse or reorder an id.
pub const MEMORY_LAYOUT: &[(u8, &str)] = &[(MEM_CORE, "core"), (MEM_AUDIT_INDEX, "audit_index"), (MEM_AUDIT_DATA, "audit_data")];

pub const CORE_SCHEMA_VERSION: u32 = 1;
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
        }
    }
}

/// Validate install arguments into a config. Pure, unit tested.
pub fn validate_init(args: &InitArgs) -> Result<BridgeConfig, String> {
    let genesis_hash = pubkey_from_base58(&args.genesis_hash).map_err(|e| format!("genesis_hash: {e}"))?;
    let mainnet = args.genesis_hash == MAINNET_GENESIS_HASH_B58;
    let expected_key = if mainnet { MAINNET_KEY_NAME } else { TEST_KEY_NAME };
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
    Ok(BridgeConfig {
        icusd_ledger: args.icusd_ledger,
        sol_rpc: args.sol_rpc,
        schnorr_key_name: args.schnorr_key_name.clone(),
        genesis_hash,
        admins: dedup_principals(&args.admins),
        fee_recipient: args.fee_recipient,
        token_program: args.token_program,
        memo_program,
        ..BridgeConfig::default()
    })
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
}

cbor_storable!(BridgeCore);

// ---------------------------------------------------------------------------
// Ledger operations (copied verbatim from the Base instance, `state.rs`)
//
// The record shape `ledger.rs` classifies and searches. Nothing stores it yet: the deposit
// and release records that embed it arrive with BB-48 and BB-49.
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
// Audit log
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct AuditRecord {
    pub seq: u64,
    pub at_ns: u64,
    pub transition: Option<Transition>,
    pub note: String,
}

cbor_storable!(AuditRecord);

#[derive(Clone, Debug, Default)]
pub struct AuditCtx {
    pub note: String,
}

impl AuditCtx {
    pub fn note(note: impl Into<String>) -> Self {
        AuditCtx { note: note.into() }
    }
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
        let rec = AuditRecord { seq, at_ns, transition, note: ctx.note };
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
        }
    }

    #[test]
    fn memory_layout_is_unique_and_pinned() {
        // Append-only: these exact ids and names are on stable memory forever.
        assert_eq!(MEMORY_LAYOUT, &[(0, "core"), (1, "audit_index"), (2, "audit_data")]);
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
    }

    #[test]
    fn key_rule_mainnet_needs_key_1_and_everything_else_test_key_1() {
        let mut a = init_args();
        a.genesis_hash = MAINNET_GENESIS_HASH_B58.into();
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
    fn sample_core() -> BridgeCore {
        let config = validate_init(&init_args()).unwrap();
        BridgeCore {
            schema_version: CORE_SCHEMA_VERSION,
            initialized: true,
            config,
            instance_salt: [0x5a; 32],
            buckets: Buckets { outstanding: 7, return_custodied: 2, burn_unknown: 1, stray: 3, ..Buckets::default() },
            halted: Some(HaltRecord { reason: "x".into(), at_ns: 9 }),
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
    fn core_decodes_from_the_pinned_v1_image() {
        // Field names are the schema. Pinned from `cbor_encode(&sample_core())`.
        let image = hex::decode(PINNED_CORE_V1).expect("hex");
        assert_eq!(try_cbor_decode::<BridgeCore>(&image).unwrap(), sample_core());
        assert_eq!(cbor_encode(&sample_core()), image, "the encoding itself is stable");
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
        strip(&mut old, &["buckets", "halted"]);
        if let Value::Map(entries) = &mut old {
            for (k, v) in entries.iter_mut() {
                if matches!(k, Value::Text(t) if t == "config") {
                    strip(v, &["fees", "pauses"]);
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

    /// CBOR image of `sample_core()` (field names are the schema; arrays are `[u8; 32]`).
    const PINNED_CORE_V1: &str = concat!(
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
}
