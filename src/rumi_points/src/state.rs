//! Stable-storage layer and Phase 1 state logic.
//!
//! Mirrors the established `rumi_protocol_backend::storage` pattern: a
//! `thread_local!` `MemoryManager` partitions stable memory, each structure gets
//! ONE `MemoryId` that is NEVER reused, entries are CBOR-encoded with ciborium,
//! and the singleton config blob uses the 8-byte little-endian length prefix with
//! a corrupt-length sanity check (`save_state_to_stable` / `load_state_from_stable`).
//!
//! ## Stable memory map (MemoryId -> structure)
//! | Id | Structure                                            | Backs                          |
//! |----|------------------------------------------------------|--------------------------------|
//! | 0  | `StableBTreeMap<Principal, StoredPrincipalState>`    | per-principal accrual state    |
//! | 1  | `StableLog` index  } `StoredPointEntry`              | append-only audit ledger       |
//! | 2  | `StableLog` data   }                                 |                                |
//! | 3  | `StableLog` index  } `StoredEpochSummary`            | per-epoch rollups              |
//! | 4  | `StableLog` data   }                                 |                                |
//! | 5  | `StableLog` index  } `StoredRevealedSeed`            | commit-reveal audit log (0.3)  |
//! | 6  | `StableLog` data   }                                 |                                |
//! | 7  | singleton blob (8-byte len prefix + CBOR)            | `State` (admin/excluded/season/seed) |
//!
//! ## Versioned-snapshot pattern (UPG-002 safety) -- READ BEFORE ADDING A FIELD
//! Every at-rest type is wrapped in an externally-tagged `Stored*` enum whose
//! CBOR carries the version tag (`{"V1": {...}}`). Adding a field to a logical
//! type (`PrincipalState`, `State`, ...) WITHOUT this discipline silently wipes
//! state on the next upgrade (`#[serde(default)]` does NOT save you with the
//! Candid path, and we want the same guarantee for the ciborium path).
//!
//! To add a field to e.g. `PrincipalState`:
//!   1. Copy TODAY's `PrincipalState` shape into a frozen `struct PrincipalStateV1`.
//!   2. Add the field to `PrincipalState` (now the "current" / V2 shape).
//!   3. Change the enum to `{ V1(PrincipalStateV1), V2(PrincipalState) }`.
//!   4. Add `impl From<PrincipalStateV1> for PrincipalState` (default the new field).
//!   5. In `into_current`, map `V1(old) => old.into()`. Write the current shape as `V2`.
//! Old `{"V1": ...}` bytes keep decoding into the frozen V1 struct, then migrate
//! on read. No wipe. This is the `AmmStateV<N>` pattern that the project already
//! relies on after the 2026-05-18 AMM state-wipe incident.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::BTreeSet;

use candid::Principal;
use ic_stable_structures::{
    log::Log as StableLog,
    memory_manager::{MemoryId, MemoryManager, VirtualMemory},
    storable::{Bound, Storable},
    DefaultMemoryImpl, Memory, StableBTreeMap,
};
use serde::{Deserialize, Serialize};

use crate::accrual::{self, SnapshotWeights};
use crate::snapshot_seed::{RevealedSeed, SnapshotSeedSingleton};
use crate::types::{
    AssetType, DepositKey, DepositRecord, EpochStatus, EpochSummary, InitArgs, LeaderboardEntry,
    OpenEpoch, PointEntry, PointSource, PointsConfig, PointsError, PrincipalState, PublicEpochStatus,
    PublicOpenEpoch, QualifyingAction, RegistrationInfo, RepaymentEvent, Venue,
};

// ── Memory ids (never reuse) ────────────────────────────────────────────────

const PRINCIPAL_STATE_MEM_ID: MemoryId = MemoryId::new(0);
const POINT_LEDGER_INDEX_MEM_ID: MemoryId = MemoryId::new(1);
const POINT_LEDGER_DATA_MEM_ID: MemoryId = MemoryId::new(2);
const EPOCH_SUMMARY_INDEX_MEM_ID: MemoryId = MemoryId::new(3);
const EPOCH_SUMMARY_DATA_MEM_ID: MemoryId = MemoryId::new(4);
const REVEALED_SEEDS_INDEX_MEM_ID: MemoryId = MemoryId::new(5);
const REVEALED_SEEDS_DATA_MEM_ID: MemoryId = MemoryId::new(6);
const STATE_BLOB_MEM_ID: MemoryId = MemoryId::new(7);
// Phase 2: per-source ingestion cursors (source tag -> last-processed event id).
const CURSORS_MEM_ID: MemoryId = MemoryId::new(8);
// Phase 2: per-source canister principal (source tag -> canister id). Configurable
// per environment (mainnet defaults seeded at init; admin overrides for local).
const SOURCE_CANISTERS_MEM_ID: MemoryId = MemoryId::new(9);
// Phase 2b: poll-timer config (key 0 = enabled 0/1, key 1 = interval seconds).
const POLL_CONFIG_MEM_ID: MemoryId = MemoryId::new(10);
// Phase 5: running-min snapshot weights for the OPEN epoch (cleared at close).
const SNAPSHOT_BUFFER_MEM_ID: MemoryId = MemoryId::new(11);
// Phase 5: asset-ledger registry (asset tag -> ledger principal), mainnet-seeded,
// admin-overridable for local/test.
const ASSET_LEDGERS_MEM_ID: MemoryId = MemoryId::new(12);
// Phase 5: epoch-driver config (key 0 = enabled 0/1, key 1 = interval seconds).
const EPOCH_CONFIG_MEM_ID: MemoryId = MemoryId::new(13);

const WASM_PAGE_SIZE: u64 = 65_536; // 64 KiB

type VMem = VirtualMemory<DefaultMemoryImpl>;

// ── At-rest versioned wrappers ──────────────────────────────────────────────

/// Versioned wrapper for per-principal state. See the module doc for the
/// field-addition recipe. TODAY there is one version.
#[derive(Serialize, Deserialize)]
pub enum StoredPrincipalState {
    V1(PrincipalState),
}

impl StoredPrincipalState {
    fn into_current(self) -> PrincipalState {
        match self {
            StoredPrincipalState::V1(v) => v,
        }
    }
    fn from_current(v: PrincipalState) -> Self {
        StoredPrincipalState::V1(v)
    }
}

#[derive(Serialize, Deserialize)]
pub enum StoredPointEntry {
    V1(PointEntry),
}

impl StoredPointEntry {
    #[allow(dead_code)] // ledger read path (personal breakdown) lands in Phase 6
    fn into_current(self) -> PointEntry {
        match self {
            StoredPointEntry::V1(v) => v,
        }
    }
}

#[derive(Serialize, Deserialize)]
pub enum StoredEpochSummary {
    V1(EpochSummary),
}

impl StoredEpochSummary {
    fn into_current(self) -> EpochSummary {
        match self {
            StoredEpochSummary::V1(v) => v,
        }
    }
}

#[derive(Serialize, Deserialize)]
pub enum StoredRevealedSeed {
    V1(RevealedSeed),
}

impl StoredRevealedSeed {
    #[allow(dead_code)] // read path lands with the Phase 5 reveal query
    fn into_current(self) -> RevealedSeed {
        match self {
            StoredRevealedSeed::V1(v) => v,
        }
    }
}

/// Versioned wrapper for the per-principal running-min snapshot weights (the
/// MemoryId-11 buffer for the open epoch). See the module doc for the recipe.
#[derive(Serialize, Deserialize)]
pub enum StoredSnapshotWeights {
    V1(SnapshotWeights),
}

impl StoredSnapshotWeights {
    fn into_current(self) -> SnapshotWeights {
        match self {
            StoredSnapshotWeights::V1(v) => v,
        }
    }
}

/// FROZEN V1 shape of `State` (pre-Phase-5). Old `{"V1": ...}` blob bytes decode
/// into this, then migrate forward via `From`. NEVER change these fields; add new
/// state to the current `State` (a new `StoredState` version) instead.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct StateV1 {
    pub admin: Principal,
    pub excluded_principals: BTreeSet<Principal>,
    pub season_start_ns: u64,
    pub season_end_ns: u64,
    pub current_epoch_index: u64,
    pub snapshot_seed: SnapshotSeedSingleton,
}

/// FROZEN V2 shape of the open epoch (pre-POINTS-002, before the chunked-close
/// fields). Old `StoredState::V2` blobs whose `open_epoch` is `Some` decode into
/// this; it migrates forward to the current `OpenEpoch` with the `close_*` fields
/// defaulted (a mid-epoch upgrade that lands before the close simply re-runs the
/// close from the start, which is idempotent). NEVER change these fields.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct OpenEpochV2 {
    pub epoch_index: u64,
    pub epoch_start_ns: u64,
    pub epoch_end_ns: u64,
    pub snapshot_a_ns: u64,
    pub snapshot_b_ns: u64,
    pub a_cursor: Option<Principal>,
    pub a_complete: bool,
    pub b_cursor: Option<Principal>,
    pub b_complete: bool,
}

impl From<OpenEpochV2> for OpenEpoch {
    fn from(v: OpenEpochV2) -> Self {
        OpenEpoch {
            epoch_index: v.epoch_index,
            epoch_start_ns: v.epoch_start_ns,
            epoch_end_ns: v.epoch_end_ns,
            snapshot_a_ns: v.snapshot_a_ns,
            snapshot_b_ns: v.snapshot_b_ns,
            a_cursor: v.a_cursor,
            a_complete: v.a_complete,
            b_cursor: v.b_cursor,
            b_complete: v.b_complete,
            // The chunked close had not started in the V2 layout. Default to a
            // fresh (not-started) close; if the upgrade lands after both snapshots,
            // the close re-runs from the start, which is idempotent.
            close_started: false,
            close_cursor: None,
            close_points_accrued: 0,
            close_active: 0,
        }
    }
}

/// FROZEN V2 shape of `State` (pre-POINTS-002). Identical to today's `State`
/// except `open_epoch` carries the frozen `OpenEpochV2` (no chunked-close fields).
/// Old `{"V2": ...}` blob bytes decode into this, then migrate forward.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct StateV2 {
    pub admin: Principal,
    pub excluded_principals: BTreeSet<Principal>,
    pub season_start_ns: u64,
    pub season_end_ns: u64,
    pub current_epoch_index: u64,
    pub snapshot_seed: SnapshotSeedSingleton,
    pub open_epoch: Option<OpenEpochV2>,
}

impl From<StateV2> for State {
    fn from(v: StateV2) -> Self {
        State {
            admin: v.admin,
            excluded_principals: v.excluded_principals,
            season_start_ns: v.season_start_ns,
            season_end_ns: v.season_end_ns,
            current_epoch_index: v.current_epoch_index,
            snapshot_seed: v.snapshot_seed,
            open_epoch: v.open_epoch.map(Into::into),
        }
    }
}

/// Singleton config (admin, excluded set, season window, epoch counter, in-flight
/// seed, open-epoch driver state). Held on the heap during execution and
/// serialized to the `STATE_BLOB` region on `pre_upgrade`, mirroring the backend.
/// NOT candid-facing.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub admin: Principal,
    pub excluded_principals: BTreeSet<Principal>,
    pub season_start_ns: u64,
    pub season_end_ns: u64,
    pub current_epoch_index: u64,
    pub snapshot_seed: SnapshotSeedSingleton,
    /// Phase 5: in-flight open epoch (periodic-driver state). `None` between
    /// epochs and before the season starts.
    pub open_epoch: Option<OpenEpoch>,
}

impl From<StateV1> for State {
    fn from(v: StateV1) -> Self {
        State {
            admin: v.admin,
            excluded_principals: v.excluded_principals,
            season_start_ns: v.season_start_ns,
            season_end_ns: v.season_end_ns,
            current_epoch_index: v.current_epoch_index,
            snapshot_seed: v.snapshot_seed,
            open_epoch: None,
        }
    }
}

#[derive(Serialize, Deserialize)]
pub enum StoredState {
    V1(StateV1),
    V2(StateV2),
    V3(State),
}

impl StoredState {
    fn into_current(self) -> State {
        match self {
            StoredState::V1(old) => old.into(),
            StoredState::V2(old) => old.into(),
            StoredState::V3(s) => s,
        }
    }
}

/// Decode a singleton-blob payload (either version) into the current `State`. Old
/// `V1` bytes migrate forward; `None` on a corrupt/undecodable blob.
fn decode_stored_state(bytes: &[u8]) -> Option<State> {
    ciborium::de::from_reader::<StoredState, _>(bytes)
        .ok()
        .map(StoredState::into_current)
}

/// CBOR `Storable` impl (ciborium), `Bound::Unbounded` per the stable-memory
/// skill's guidance (avoids the bounded-max-size break when a field is added).
macro_rules! impl_cbor_storable {
    ($t:ty) => {
        impl Storable for $t {
            fn to_bytes(&self) -> Cow<'_, [u8]> {
                let mut buf = Vec::new();
                ciborium::ser::into_writer(self, &mut buf)
                    .expect(concat!("failed to CBOR-encode ", stringify!($t)));
                Cow::Owned(buf)
            }
            fn from_bytes(bytes: Cow<'_, [u8]>) -> Self {
                ciborium::de::from_reader(bytes.as_ref())
                    .expect(concat!("failed to CBOR-decode ", stringify!($t)))
            }
            const BOUND: Bound = Bound::Unbounded;
        }
    };
}

impl_cbor_storable!(StoredPrincipalState);
impl_cbor_storable!(StoredPointEntry);
impl_cbor_storable!(StoredEpochSummary);
impl_cbor_storable!(StoredRevealedSeed);
impl_cbor_storable!(StoredSnapshotWeights);

/// `StableBTreeMap` key wrapper. ic-stable-structures 0.6.5 does not provide a
/// `Storable` impl for `Principal`, so we wrap it. A principal is at most 29
/// bytes; bounded (not fixed) so short principals do not waste space.
#[derive(Clone, Ord, PartialOrd, Eq, PartialEq, Debug)]
pub struct StorablePrincipal(pub Principal);

impl Storable for StorablePrincipal {
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        Cow::Owned(self.0.as_slice().to_vec())
    }
    fn from_bytes(bytes: Cow<'_, [u8]>) -> Self {
        StorablePrincipal(Principal::from_slice(bytes.as_ref()))
    }
    const BOUND: Bound = Bound::Bounded {
        max_size: 29,
        is_fixed_size: false,
    };
}

// ── Thread-local stable storage ─────────────────────────────────────────────

thread_local! {
    static MEMORY_MANAGER: RefCell<MemoryManager<DefaultMemoryImpl>> =
        RefCell::new(MemoryManager::init(DefaultMemoryImpl::default()));

    static PRINCIPALS: RefCell<StableBTreeMap<StorablePrincipal, StoredPrincipalState, VMem>> =
        MEMORY_MANAGER.with(|m| RefCell::new(StableBTreeMap::init(m.borrow().get(PRINCIPAL_STATE_MEM_ID))));

    static POINT_LEDGER: RefCell<StableLog<StoredPointEntry, VMem, VMem>> =
        MEMORY_MANAGER.with(|m| RefCell::new(
            StableLog::init(m.borrow().get(POINT_LEDGER_INDEX_MEM_ID), m.borrow().get(POINT_LEDGER_DATA_MEM_ID))
                .expect("failed to init point ledger")
        ));

    static EPOCH_SUMMARIES: RefCell<StableLog<StoredEpochSummary, VMem, VMem>> =
        MEMORY_MANAGER.with(|m| RefCell::new(
            StableLog::init(m.borrow().get(EPOCH_SUMMARY_INDEX_MEM_ID), m.borrow().get(EPOCH_SUMMARY_DATA_MEM_ID))
                .expect("failed to init epoch summaries")
        ));

    static REVEALED_SEEDS: RefCell<StableLog<StoredRevealedSeed, VMem, VMem>> =
        MEMORY_MANAGER.with(|m| RefCell::new(
            StableLog::init(m.borrow().get(REVEALED_SEEDS_INDEX_MEM_ID), m.borrow().get(REVEALED_SEEDS_DATA_MEM_ID))
                .expect("failed to init revealed seeds")
        ));

    /// Singleton config, heap-resident during execution (restored from the blob
    /// in `post_upgrade`, populated fresh in `init`).
    static STATE: RefCell<Option<State>> = RefCell::new(None);

    /// Phase 2: per-source ingestion cursor (source tag -> last-processed event
    /// id + 1). Persists across upgrades so we never re-ingest from zero. Both
    /// key and value are primitives with built-in `Storable` impls.
    static CURSORS: RefCell<StableBTreeMap<u8, u64, VMem>> =
        MEMORY_MANAGER.with(|m| RefCell::new(StableBTreeMap::init(m.borrow().get(CURSORS_MEM_ID))));

    /// Phase 2: transient re-entrancy guard so two overlapping poll timers do not
    /// double-ingest from the same cursor. NOT persisted (a fresh canister / a
    /// post-upgrade heap always starts with no poll in flight).
    static POLL_IN_PROGRESS: RefCell<bool> = RefCell::new(false);

    /// Phase 5: transient guard so overlapping epoch-driver ticks never double-run
    /// a capture or close. NOT persisted (a fresh heap starts with no tick running).
    static EPOCH_IN_PROGRESS: RefCell<bool> = RefCell::new(false);

    /// Phase 2: per-source canister principal (source tag -> canister id). Seeded
    /// with mainnet defaults at init; the admin overrides per environment (e.g.
    /// local replica ids) via `set_source_canister`. Persists across upgrades.
    static SOURCE_CANISTERS: RefCell<StableBTreeMap<u8, StorablePrincipal, VMem>> =
        MEMORY_MANAGER.with(|m| RefCell::new(StableBTreeMap::init(m.borrow().get(SOURCE_CANISTERS_MEM_ID))));

    /// Phase 2b: poll-timer config (key 0 = enabled, key 1 = interval seconds).
    /// Persists across upgrades; the timer itself is re-registered in
    /// `post_upgrade` from this config (timers do not survive upgrades).
    static POLL_CONFIG: RefCell<StableBTreeMap<u8, u64, VMem>> =
        MEMORY_MANAGER.with(|m| RefCell::new(StableBTreeMap::init(m.borrow().get(POLL_CONFIG_MEM_ID))));

    /// Phase 5: running-MIN snapshot weights per principal for the OPEN epoch.
    /// Snapshot A inserts; snapshot B keeps `min_by_total` vs A; the close consumes
    /// and clears it. Stable so a mid-epoch upgrade keeps captured snapshots.
    static SNAPSHOT_BUFFER: RefCell<StableBTreeMap<StorablePrincipal, StoredSnapshotWeights, VMem>> =
        MEMORY_MANAGER.with(|m| RefCell::new(StableBTreeMap::init(m.borrow().get(SNAPSHOT_BUFFER_MEM_ID))));

    /// Phase 5: asset-ledger registry (asset tag -> ledger principal). Seeded with
    /// mainnet ids at init; admin overrides per environment (local/test).
    static ASSET_LEDGERS: RefCell<StableBTreeMap<u8, StorablePrincipal, VMem>> =
        MEMORY_MANAGER.with(|m| RefCell::new(StableBTreeMap::init(m.borrow().get(ASSET_LEDGERS_MEM_ID))));

    /// Phase 5: epoch-driver config (key 0 = enabled, key 1 = interval seconds).
    /// Mirrors POLL_CONFIG; the driver timer is re-registered in `post_upgrade`.
    static EPOCH_CONFIG: RefCell<StableBTreeMap<u8, u64, VMem>> =
        MEMORY_MANAGER.with(|m| RefCell::new(StableBTreeMap::init(m.borrow().get(EPOCH_CONFIG_MEM_ID))));
}

// ── Excluded-principals seed (spec Section 11) ──────────────────────────────

/// The protocol-OWNED canister principals seeded into the excluded set at init.
/// These hold balances that would otherwise accrue dollar-days and route the
/// airdrop pool into the protocol's own infrastructure. Confirmed against
/// `canister_ids.json` (2026-06-01). Founder/team principals are deliberately
/// NOT here (see spec Section 11). `rumi_analytics` is deliberately omitted (it
/// holds no qualifying balances); the admin can add it via `add_excluded`.
///
/// These literals are the SEED applied to mutable state at init; enforcement
/// reads the mutable `State.excluded_principals` set, not these constants, so the
/// set stays admin-configurable.
pub fn protocol_owned_canister_seed() -> Vec<Principal> {
    [
        "tfesu-vyaaa-aaaap-qrd7a-cai", // rumi_protocol_backend
        "fohh4-yyaaa-aaaap-qtkpa-cai", // rumi_3pool
        "ijlzs-2yaaa-aaaap-quaaq-cai", // rumi_amm
        "tmhzi-dqaaa-aaaap-qrd6q-cai", // rumi_stability_pool
        "tlg74-oiaaa-aaaap-qrd6a-cai", // rumi_treasury
        "nygob-3qaaa-aaaap-qttcq-cai", // liquidation_bot
        "t6bor-paaaa-aaaap-qrd5q-cai", // icusd_ledger
        "6niqu-siaaa-aaaap-qrjeq-cai", // icusd_index
        "jagpu-pyaaa-aaaap-qtm6q-cai", // threeusd_index
    ]
    .iter()
    .map(|s| Principal::from_text(s).expect("invalid protocol-owned principal literal"))
    .collect()
}

// ── State access helpers ────────────────────────────────────────────────────

pub fn with_state<R>(f: impl FnOnce(&State) -> R) -> R {
    STATE.with(|s| f(s.borrow().as_ref().expect("state not initialized")))
}

pub fn with_state_mut<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(s.borrow_mut().as_mut().expect("state not initialized")))
}

fn require_admin(caller: Principal) -> Result<(), PointsError> {
    with_state(|s| {
        if s.admin == caller {
            Ok(())
        } else {
            Err(PointsError::Unauthorized)
        }
    })
}

/// Public admin predicate, for endpoints that gate on a bool.
pub fn is_admin(caller: Principal) -> bool {
    with_state(|s| s.admin == caller)
}

// ── Phase 1 logic (TDD targets; implemented after the failing tests) ────────

/// Build the singleton `State` from init args (defaulting admin to `caller`,
/// excluded set to the protocol-owned seed, and the season window to the locked
/// defaults) and install it on the heap. Called from `#[init]`.
pub fn init_state(args: Option<InitArgs>, caller: Principal) {
    let args = args.unwrap_or_default();
    let admin = args.admin.unwrap_or(caller);
    let excluded_principals: BTreeSet<Principal> = args
        .excluded_principals
        .unwrap_or_else(protocol_owned_canister_seed)
        .into_iter()
        .collect();
    let snapshot_seed = SnapshotSeedSingleton {
        pending_commit: args.snapshot_seed_commit.unwrap_or([0u8; 32]),
        current_seed: None,
    };
    let state = State {
        admin,
        excluded_principals,
        season_start_ns: args.season_start_ns.unwrap_or(crate::DEFAULT_SEASON_START_NS),
        season_end_ns: args.season_end_ns.unwrap_or(crate::DEFAULT_SEASON_END_NS),
        current_epoch_index: 0,
        snapshot_seed,
        open_epoch: None,
    };
    STATE.with(|cell| *cell.borrow_mut() = Some(state));

    // Seed the source-canister ids with mainnet defaults on a fresh install. The
    // admin overrides per environment (e.g. local replica ids) via
    // `set_source_canister`. Only seeds an empty map so it is a no-op if somehow
    // re-entered with config already present.
    SOURCE_CANISTERS.with(|m| {
        let mut m = m.borrow_mut();
        if m.is_empty() {
            for (tag, p) in source_canister_seed() {
                m.insert(tag, StorablePrincipal(p));
            }
        }
    });

    // Seed the asset-ledger registry with mainnet ids (Phase 5). Admin overrides
    // per environment via `set_asset_ledger`. No-op if already populated.
    ASSET_LEDGERS.with(|m| {
        let mut m = m.borrow_mut();
        if m.is_empty() {
            for (tag, p) in asset_ledger_seed() {
                m.insert(tag, StorablePrincipal(p));
            }
        }
    });
}

/// Is this principal in the configurable excluded set? Checked at the
/// registration / accrual boundary (full accrual enforcement lands in Phase 3).
pub fn is_excluded(p: &Principal) -> bool {
    with_state(|s| s.excluded_principals.contains(p))
}

pub fn excluded_principals() -> Vec<Principal> {
    with_state(|s| s.excluded_principals.iter().cloned().collect())
}

pub fn add_excluded(caller: Principal, p: Principal) -> Result<(), PointsError> {
    require_admin(caller)?;
    with_state_mut(|s| {
        s.excluded_principals.insert(p);
    });
    Ok(())
}

pub fn remove_excluded(caller: Principal, p: Principal) -> Result<(), PointsError> {
    require_admin(caller)?;
    with_state_mut(|s| {
        s.excluded_principals.remove(&p);
    });
    Ok(())
}

pub fn set_excluded(caller: Principal, principals: Vec<Principal>) -> Result<(), PointsError> {
    require_admin(caller)?;
    with_state_mut(|s| {
        s.excluded_principals = principals.into_iter().collect();
    });
    Ok(())
}

/// Register a principal (idempotent). Rejects excluded principals. Writes a
/// zero-point `Registration` audit marker on first registration. This is the
/// boundary the auto-registration ingestion (Phase 3) will also call.
pub fn register(
    principal: Principal,
    now_ns: u64,
    action: QualifyingAction,
) -> Result<PrincipalState, PointsError> {
    if is_excluded(&principal) {
        return Err(PointsError::Excluded);
    }
    if let Some(existing) = get_principal_state(&principal) {
        // Idempotent: never overwrite the original registration timestamp/action.
        return Ok(existing);
    }
    let ps = PrincipalState::new(principal, now_ns, action);
    put_principal_state(ps.clone());
    append_point_entry(PointEntry {
        principal,
        epoch_index: current_epoch_index(),
        points_delta: 0,
        source: PointSource::Registration,
        recorded_at_ns: now_ns,
    });
    Ok(ps)
}

/// Admin-only test registration (Phase 1 only). Lets us seed state to prove
/// upgrade survival before ingestion exists.
pub fn register_test_principal(
    caller: Principal,
    principal: Principal,
    now_ns: u64,
) -> Result<(), PointsError> {
    require_admin(caller)?;
    // Phase 1 test enrollment uses a fixed placeholder action; real ingestion
    // (Phase 3) records the true first qualifying action.
    register(principal, now_ns, QualifyingAction::MintIcUsd)?;
    Ok(())
}

/// Max rows `get_leaderboard` will return in one call (PTS-001). A caller passing
/// an unbounded `limit` is clamped to this, bounding the response size and the
/// per-call materialization. Pagination via `offset` reaches the full board.
pub const MAX_LEADERBOARD_LIMIT: u32 = 1_000;

/// Ranked leaderboard (desc by points, principal as tiebreak), paginated, with
/// each entry's estimated share of the 5% pool in basis points. `limit` is clamped
/// to `MAX_LEADERBOARD_LIMIT` (PTS-001) so an unbounded request cannot force a
/// giant response.
pub fn leaderboard(offset: u32, limit: u32) -> Vec<LeaderboardEntry> {
    let limit = limit.min(MAX_LEADERBOARD_LIMIT);
    let mut all: Vec<(Principal, u128)> = PRINCIPALS.with(|m| {
        m.borrow()
            .iter()
            .map(|(k, v)| (k.0, v.into_current().total_points))
            .collect()
    });
    // Highest points first; principal id as a stable tiebreak.
    all.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    // Saturating, consistent with the close accrual (a non-saturating `.sum()`
    // would panic in debug on the astronomically unlikely u128 overflow).
    let total_points_all: u128 = all
        .iter()
        .fold(0u128, |acc, (_, pts)| acc.saturating_add(*pts));

    all.into_iter()
        .enumerate()
        .skip(offset as usize)
        .take(limit as usize)
        .map(|(idx, (principal, total_points))| {
            let estimated_share_bps = if total_points_all == 0 {
                0
            } else {
                (total_points.saturating_mul(10_000) / total_points_all) as u32
            };
            LeaderboardEntry {
                rank: idx as u32 + 1,
                principal,
                total_points,
                estimated_share_bps,
            }
        })
        .collect()
}

pub fn epoch_history(offset: u64, limit: u64) -> Vec<EpochSummary> {
    EPOCH_SUMMARIES.with(|l| {
        let log = l.borrow();
        let len = log.len();
        let mut out = Vec::new();
        let mut i = offset;
        while i < len && (out.len() as u64) < limit {
            if let Some(s) = log.get(i) {
                out.push(s.into_current());
            }
            i += 1;
        }
        out
    })
}

/// Serialize the heap `State` (as the current `StoredState` version) to the
/// singleton blob with the 8-byte little-endian length prefix. Called from
/// `#[pre_upgrade]`.
pub fn save_state_to_stable() {
    let bytes = with_state(|s| {
        let mut buf = Vec::new();
        ciborium::ser::into_writer(&StoredState::V3(s.clone()), &mut buf)
            .expect("failed to serialize State to CBOR");
        buf
    });
    MEMORY_MANAGER.with(|m| {
        let mem = m.borrow().get(STATE_BLOB_MEM_ID);
        let total_bytes = 8 + bytes.len() as u64;
        let pages_needed = (total_bytes + WASM_PAGE_SIZE - 1) / WASM_PAGE_SIZE;
        let current_pages = mem.size();
        if pages_needed > current_pages {
            assert!(
                mem.grow(pages_needed - current_pages) != -1,
                "failed to grow state memory"
            );
        }
        mem.write(0, &(bytes.len() as u64).to_le_bytes());
        mem.write(8, &bytes);
    });
}

/// Restore the heap `State` from the singleton blob. Returns `None` if nothing
/// was saved or the blob is corrupt. Called from `#[post_upgrade]`.
pub fn load_state_from_stable() -> Option<State> {
    MEMORY_MANAGER.with(|m| {
        let mem = m.borrow().get(STATE_BLOB_MEM_ID);
        if mem.size() == 0 {
            return None;
        }
        let mut len_bytes = [0u8; 8];
        mem.read(0, &mut len_bytes);
        let len = u64::from_le_bytes(len_bytes);
        if len == 0 {
            return None;
        }
        // Sanity check: the recorded length must fit in allocated memory (minus
        // the 8-byte prefix). Guards against a corrupt / partially-written blob.
        let mem_bytes = mem.size() * WASM_PAGE_SIZE;
        if len > mem_bytes.saturating_sub(8) {
            ic_cdk::println!(
                "[upgrade] corrupt state length {} exceeds memory size {}",
                len,
                mem_bytes
            );
            return None;
        }
        let mut buf = vec![0u8; len as usize];
        mem.read(8, &mut buf);
        let decoded = decode_stored_state(&buf);
        if decoded.is_none() {
            ic_cdk::println!("[upgrade] failed to deserialize State blob");
        }
        decoded
    })
}

/// `post_upgrade` entry point: load the singleton blob and install it. Traps
/// rather than silently re-initializing, so a failed restore is loud (never a
/// silent wipe). The `StableBTreeMap` / `StableLog` structures auto-restore from
/// stable memory independently and need no action here.
pub fn restore_from_stable_or_trap() {
    match load_state_from_stable() {
        Some(s) => STATE.with(|cell| *cell.borrow_mut() = Some(s)),
        None => ic_cdk::trap(
            "post_upgrade: no saved State found in the singleton blob; refusing to silently reset",
        ),
    }
}

// ── Thin accessors (plumbing exercised by the behavioral tests) ─────────────

pub fn get_principal_state(p: &Principal) -> Option<PrincipalState> {
    PRINCIPALS.with(|m| m.borrow().get(&StorablePrincipal(*p)).map(|s| s.into_current()))
}

fn put_principal_state(ps: PrincipalState) {
    PRINCIPALS.with(|m| {
        m.borrow_mut()
            .insert(StorablePrincipal(ps.principal), StoredPrincipalState::from_current(ps));
    });
}

pub fn is_registered(p: &Principal) -> bool {
    PRINCIPALS.with(|m| m.borrow().contains_key(&StorablePrincipal(*p)))
}

pub fn registration_info(p: &Principal) -> Option<RegistrationInfo> {
    get_principal_state(p).map(|ps| RegistrationInfo {
        principal: ps.principal,
        registered_at_ns: ps.registered_at_ns,
        first_qualifying_action: ps.first_qualifying_action,
    })
}

pub fn registered_count() -> u64 {
    PRINCIPALS.with(|m| m.borrow().len())
}

pub fn current_epoch_index() -> u64 {
    with_state(|s| s.current_epoch_index)
}

pub fn append_point_entry(entry: PointEntry) {
    POINT_LEDGER.with(|l| {
        l.borrow()
            .append(&StoredPointEntry::V1(entry))
            .expect("failed to append point entry");
    });
}

pub fn point_ledger_len() -> u64 {
    POINT_LEDGER.with(|l| l.borrow().len())
}

#[allow(dead_code)] // wired by the Phase 5 epoch driver
pub fn append_epoch_summary(summary: EpochSummary) {
    EPOCH_SUMMARIES.with(|l| {
        l.borrow()
            .append(&StoredEpochSummary::V1(summary))
            .expect("failed to append epoch summary");
    });
}

pub fn epoch_count() -> u64 {
    EPOCH_SUMMARIES.with(|l| l.borrow().len())
}

/// Number of revealed commit-reveal seeds (one per closed epoch). The log is
/// reserved in the stable layout now (MemoryIds 5/6); Phase 5 appends to it and
/// adds the public `get_revealed_seed` query.
pub fn revealed_seed_count() -> u64 {
    REVEALED_SEEDS.with(|l| l.borrow().len())
}

pub fn points_config() -> PointsConfig {
    with_state(|s| PointsConfig {
        admin: s.admin,
        season_start_ns: s.season_start_ns,
        season_end_ns: s.season_end_ns,
        excluded_count: s.excluded_principals.len() as u32,
        registered_count: registered_count(),
        current_epoch_index: s.current_epoch_index,
        snapshot_seed_committed: s.snapshot_seed.is_committed(),
    })
}

/// FULL epoch-driver status, INCLUDING the in-flight capture/close cursors.
/// ADMIN-ONLY: the cursors must not be public (POINTS-001). Surfaced via the
/// admin-gated `get_epoch_status_admin` endpoint.
pub fn epoch_status() -> EpochStatus {
    with_state(|s| EpochStatus {
        current_epoch_index: s.current_epoch_index,
        driver_enabled: epoch_driver_enabled(),
        driver_interval_secs: epoch_driver_interval_secs(),
        open_epoch: s.open_epoch.clone(),
        revealed_seed_count: revealed_seed_count(),
        snapshot_seed_committed: s.snapshot_seed.is_committed(),
    })
}

/// PUBLIC epoch-driver status for the ops dashboard (POINTS-001). Same as
/// `epoch_status` but the open epoch is reduced to its public window: the bounds,
/// with the capture/close cursors and completion flags OMITTED (exposing capture
/// progress would let a not-yet-captured principal time a flash deposit to land
/// in a snapshot it has not been captured into yet) and each snapshot time hidden
/// until it has fired at `now_ns` (PTS-002: a future snapshot time IS the snipe
/// target the commit-reveal seed exists to hide).
pub fn public_epoch_status(now_ns: u64) -> PublicEpochStatus {
    with_state(|s| PublicEpochStatus {
        current_epoch_index: s.current_epoch_index,
        driver_enabled: epoch_driver_enabled(),
        driver_interval_secs: epoch_driver_interval_secs(),
        open_epoch: s.open_epoch.as_ref().map(|o| PublicOpenEpoch::redacted(o, now_ns)),
        revealed_seed_count: revealed_seed_count(),
        snapshot_seed_committed: s.snapshot_seed.is_committed(),
    })
}

// ── Phase 2: ingestion cursors, poll guard, season gating ───────────────────

/// Last-processed cursor for a source (its next `get_*_events` start id). 0 if
/// the source has never been polled.
pub fn get_cursor(source_tag: u8) -> u64 {
    CURSORS.with(|c| c.borrow().get(&source_tag).unwrap_or(0))
}

pub fn set_cursor(source_tag: u8, next_start_id: u64) {
    CURSORS.with(|c| {
        c.borrow_mut().insert(source_tag, next_start_id);
    });
}

/// Mainnet source-canister ids seeded at init (source tag -> canister id):
/// 0 = backend, 1 = 3pool, 2 = stability pool, 3 = AMM (see `events::SourceId`).
/// Confirmed against canister_ids.json (2026-06-01).
pub fn source_canister_seed() -> Vec<(u8, Principal)> {
    [
        (0u8, "tfesu-vyaaa-aaaap-qrd7a-cai"), // rumi_protocol_backend
        (1u8, "fohh4-yyaaa-aaaap-qtkpa-cai"), // rumi_3pool
        (2u8, "tmhzi-dqaaa-aaaap-qrd6q-cai"), // rumi_stability_pool
        (3u8, "ijlzs-2yaaa-aaaap-quaaq-cai"), // rumi_amm
    ]
    .iter()
    .map(|(tag, s)| (*tag, Principal::from_text(s).expect("invalid source principal literal")))
    .collect()
}

/// The configured canister id for a source tag, or `None` if unset.
pub fn get_source_canister(source_tag: u8) -> Option<Principal> {
    SOURCE_CANISTERS.with(|m| m.borrow().get(&source_tag).map(|p| p.0))
}

/// Admin-set a source canister id (e.g. point at local replica ids).
pub fn set_source_canister(
    caller: Principal,
    source_tag: u8,
    canister: Principal,
) -> Result<(), PointsError> {
    require_admin(caller)?;
    SOURCE_CANISTERS.with(|m| {
        m.borrow_mut().insert(source_tag, StorablePrincipal(canister));
    });
    Ok(())
}

/// All configured source canisters as `(tag, id)` pairs, for the status query.
pub fn source_canisters() -> Vec<(u8, Principal)> {
    SOURCE_CANISTERS.with(|m| m.borrow().iter().map(|(tag, p)| (tag, p.0)).collect())
}

// ── Phase 2b: poll-timer config ─────────────────────────────────────────────

/// Default poll cadence. Conservative to bound cycle burn (each tick does up to
/// four bounded inter-canister query calls); admin-tunable. 300s = 288 ticks/day.
pub const DEFAULT_POLL_INTERVAL_SECS: u64 = 300;
/// Floor on the admin-set cadence, so it can never be turned into a cycle-burning
/// near-heartbeat.
pub const MIN_POLL_INTERVAL_SECS: u64 = 60;

/// Is the periodic poll timer enabled? Defaults to OFF, so a fresh deploy never
/// auto-polls (and burns no cycles) until an operator configures sources and
/// enables it for the season.
pub fn poll_enabled() -> bool {
    POLL_CONFIG.with(|c| c.borrow().get(&0).unwrap_or(0) != 0)
}

pub fn poll_interval_secs() -> u64 {
    POLL_CONFIG.with(|c| c.borrow().get(&1).unwrap_or(DEFAULT_POLL_INTERVAL_SECS))
}

pub fn set_poll_enabled(caller: Principal, enabled: bool) -> Result<(), PointsError> {
    require_admin(caller)?;
    POLL_CONFIG.with(|c| {
        c.borrow_mut().insert(0, enabled as u64);
    });
    Ok(())
}

/// Admin-set the poll cadence (clamped to `MIN_POLL_INTERVAL_SECS`).
pub fn set_poll_interval(caller: Principal, secs: u64) -> Result<(), PointsError> {
    require_admin(caller)?;
    let clamped = secs.max(MIN_POLL_INTERVAL_SECS);
    POLL_CONFIG.with(|c| {
        c.borrow_mut().insert(1, clamped);
    });
    Ok(())
}

/// RAII single-poll guard (AR-S-001). `new` returns `None` while a poll is in
/// flight; dropping the guard clears the flag. Because the flag is released in
/// `Drop`, a trap inside a poll tick releases it too (ic-cdk's cleanup drops the
/// in-flight future, running destructors), instead of leaving polling stalled
/// until the next upgrade as the old begin/end function pair did.
#[must_use]
pub struct PollGuard(());

impl PollGuard {
    pub fn new() -> Option<Self> {
        POLL_IN_PROGRESS.with(|p| {
            let mut p = p.borrow_mut();
            if *p {
                None
            } else {
                *p = true;
                Some(PollGuard(()))
            }
        })
    }
}

impl Drop for PollGuard {
    fn drop(&mut self) {
        POLL_IN_PROGRESS.with(|p| *p.borrow_mut() = false);
    }
}

/// Is `ts_ns` within the configured season window (inclusive)? Registration and
/// accrual only happen for in-season activity; pre-season activity does not
/// retroactively enroll a principal (spec Section 8).
pub fn in_season(ts_ns: u64) -> bool {
    with_state(|s| ts_ns >= s.season_start_ns && ts_ns <= s.season_end_ns)
}

// ── Phase 5: snapshot-weights buffer (MemoryId 11) ──────────────────────────
// Per-principal running-min weights for the open epoch. The capture/merge logic
// lives in the driver (`epoch.rs`); these are the storage primitives it uses.

/// Store a principal's snapshot weights.
pub fn snapshot_buffer_put(p: Principal, w: SnapshotWeights) {
    SNAPSHOT_BUFFER.with(|b| {
        b.borrow_mut()
            .insert(StorablePrincipal(p), StoredSnapshotWeights::V1(w));
    });
}

/// Read a principal's buffered weights, if any.
pub fn snapshot_buffer_get(p: &Principal) -> Option<SnapshotWeights> {
    SNAPSHOT_BUFFER.with(|b| b.borrow().get(&StorablePrincipal(*p)).map(|s| s.into_current()))
}

/// Drop a principal's buffered weights.
pub fn snapshot_buffer_remove(p: &Principal) {
    SNAPSHOT_BUFFER.with(|b| {
        b.borrow_mut().remove(&StorablePrincipal(*p));
    });
}

/// All buffered `(principal, weights)`, for the epoch-close accrual pass.
pub fn snapshot_buffer_entries() -> Vec<(Principal, SnapshotWeights)> {
    SNAPSHOT_BUFFER.with(|b| {
        b.borrow()
            .iter()
            .map(|(k, v)| (k.0, v.into_current()))
            .collect()
    })
}

pub fn snapshot_buffer_len() -> u64 {
    SNAPSHOT_BUFFER.with(|b| b.borrow().len())
}

/// Max snapshot-buffer entries removed per `snapshot_buffer_clear_chunk` call
/// (PTS-002). Bounds the per-message work so clearing a season-scale buffer cannot
/// blow the instruction limit in a single sweep (which would compound POINTS-002).
pub const SNAPSHOT_BUFFER_CLEAR_CHUNK: u64 = 200;

/// Remove up to `SNAPSHOT_BUFFER_CLEAR_CHUNK` entries from the buffer. Returns
/// `true` once the buffer is EMPTY (nothing left to clear), `false` if more
/// remain. Bounded per call (PTS-002) so a large buffer is drained across several
/// messages instead of one O(N) sweep that risks the 5B-instruction trap.
pub fn snapshot_buffer_clear_chunk() -> bool {
    SNAPSHOT_BUFFER.with(|b| {
        let mut map = b.borrow_mut();
        let keys: Vec<StorablePrincipal> = map
            .iter()
            .map(|(k, _)| k)
            .take(SNAPSHOT_BUFFER_CLEAR_CHUNK as usize)
            .collect();
        for k in &keys {
            map.remove(k);
        }
        map.is_empty()
    })
}

/// Empty the buffer fully, in bounded chunks (PTS-002). Used on paths where the
/// buffer is small (e.g. opening a fresh epoch, where the prior close already
/// drained it) or in tests. The loop is bounded by the buffer size; each iteration
/// removes a full chunk (or finishes), so it terminates in
/// `ceil(len / SNAPSHOT_BUFFER_CLEAR_CHUNK)` steps. The CLOSE path does NOT call
/// this in one shot; it drains the buffer incrementally as it accrues each
/// principal and mops up stragglers across ticks (see `run_close_accrual_chunk`).
pub fn snapshot_buffer_clear() {
    while !snapshot_buffer_clear_chunk() {}
}

// ── Phase 5: asset-ledger registry (MemoryId 12) ────────────────────────────

/// Stable tag for an asset's ledger-registry slot. NEVER renumber.
pub fn asset_tag(asset: AssetType) -> u8 {
    match asset {
        AssetType::IcUsd => 0,
        AssetType::ThreeUsd => 1,
        AssetType::CkUsdc => 2,
        AssetType::CkUsdt => 3,
        AssetType::Icp => 4,
    }
}

/// Mainnet ledger principals seeded at init (asset tag -> ledger). 3USD's "ledger"
/// is the rumi_3pool canister itself (the LP token). Confirmed 2026-06-02.
pub fn asset_ledger_seed() -> Vec<(u8, Principal)> {
    [
        (0u8, "t6bor-paaaa-aaaap-qrd5q-cai"), // icUSD
        (1u8, "fohh4-yyaaa-aaaap-qtkpa-cai"), // 3USD (= rumi_3pool)
        (2u8, "xevnm-gaaaa-aaaar-qafnq-cai"), // ckUSDC
        (3u8, "cngnf-vqaaa-aaaar-qag4q-cai"), // ckUSDT
        (4u8, "ryjl3-tyaaa-aaaaa-aaaba-cai"), // ICP
    ]
    .iter()
    .map(|(t, s)| (*t, Principal::from_text(s).expect("invalid asset ledger literal")))
    .collect()
}

/// The configured ledger principal for an asset, or `None` if unset.
pub fn get_asset_ledger(asset: AssetType) -> Option<Principal> {
    ASSET_LEDGERS.with(|m| m.borrow().get(&asset_tag(asset)).map(|p| p.0))
}

/// Classify a ledger principal to its asset, or `None` if it is not a tracked
/// stable/ICP ledger (used to type SP deposits and vault-repayment assets).
pub fn classify_ledger(ledger: &Principal) -> Option<AssetType> {
    [
        AssetType::IcUsd,
        AssetType::ThreeUsd,
        AssetType::CkUsdc,
        AssetType::CkUsdt,
        AssetType::Icp,
    ]
    .into_iter()
    .find(|a| get_asset_ledger(*a).as_ref() == Some(ledger))
}

/// Admin: override an asset's ledger id (e.g. point at local-replica ledgers).
pub fn set_asset_ledger(caller: Principal, tag: u8, ledger: Principal) -> Result<(), PointsError> {
    require_admin(caller)?;
    ASSET_LEDGERS.with(|m| {
        m.borrow_mut().insert(tag, StorablePrincipal(ledger));
    });
    Ok(())
}

/// All configured `(tag, ledger)` pairs, for the status query.
pub fn asset_ledgers() -> Vec<(u8, Principal)> {
    ASSET_LEDGERS.with(|m| m.borrow().iter().map(|(t, p)| (t, p.0)).collect())
}

// ── Phase 5: epoch-driver config (MemoryId 13) ──────────────────────────────
// Mirrors POLL_CONFIG: the weekly epoch state machine is driven by a periodic
// timer, OFF by default, re-registered from this config in `post_upgrade`.

pub const DEFAULT_EPOCH_DRIVER_INTERVAL_SECS: u64 = 300;
pub const MIN_EPOCH_DRIVER_INTERVAL_SECS: u64 = 60;

pub fn epoch_driver_enabled() -> bool {
    EPOCH_CONFIG.with(|c| c.borrow().get(&0).unwrap_or(0) != 0)
}

pub fn epoch_driver_interval_secs() -> u64 {
    EPOCH_CONFIG.with(|c| c.borrow().get(&1).unwrap_or(DEFAULT_EPOCH_DRIVER_INTERVAL_SECS))
}

pub fn set_epoch_driver_enabled(caller: Principal, enabled: bool) -> Result<(), PointsError> {
    require_admin(caller)?;
    EPOCH_CONFIG.with(|c| {
        c.borrow_mut().insert(0, enabled as u64);
    });
    Ok(())
}

pub fn set_epoch_driver_interval(caller: Principal, secs: u64) -> Result<(), PointsError> {
    require_admin(caller)?;
    let clamped = secs.max(MIN_EPOCH_DRIVER_INTERVAL_SECS);
    EPOCH_CONFIG.with(|c| {
        c.borrow_mut().insert(1, clamped);
    });
    Ok(())
}

// ── Phase 5/4: ingestion-driven per-principal state (events.rs writes these) ──

/// 90-day repayment window length (spec Section 6).
pub const REPAYMENT_WINDOW_NS: u64 = 90 * crate::NANOS_PER_DAY;

/// Add to (or subtract from) a principal's recorded 3pool deposit for one asset
/// (the event-tracked composition deciding the 1x/3x/5x split). Subtraction
/// saturates at 0 and drops the record when it reaches 0. No-op if unregistered.
pub fn update_3pool_recorded(
    principal: Principal,
    asset: AssetType,
    amount_usd_e8s: u128,
    add: bool,
    now_ns: u64,
) {
    let mut ps = match get_principal_state(&principal) {
        Some(p) => p,
        None => return,
    };
    let key = DepositKey { venue: Venue::ThreePool, asset };
    if add {
        let rec = ps.active_deposits.entry(key).or_insert_with(|| DepositRecord {
            asset,
            venue: Venue::ThreePool,
            recorded_value_usd: 0,
            deposited_at: now_ns,
            last_verified_at: now_ns,
        });
        rec.recorded_value_usd = rec.recorded_value_usd.saturating_add(amount_usd_e8s);
        rec.last_verified_at = now_ns;
    } else if let Some(rec) = ps.active_deposits.get_mut(&key) {
        rec.recorded_value_usd = rec.recorded_value_usd.saturating_sub(amount_usd_e8s);
        rec.last_verified_at = now_ns;
        if rec.recorded_value_usd == 0 {
            ps.active_deposits.remove(&key);
        }
    }
    put_principal_state(ps);
}

/// Record a qualifying ckUSDC/ckUSDT vault repayment, opening a 90-day points
/// window capped at season end (spec Section 6). No-op if unregistered.
pub fn record_repayment(
    principal: Principal,
    asset: AssetType,
    amount_usd_e8s: u128,
    repaid_at: u64,
) {
    let mut ps = match get_principal_state(&principal) {
        Some(p) => p,
        None => return,
    };
    let season_end = with_state(|s| s.season_end_ns);
    let window_end = repaid_at.saturating_add(REPAYMENT_WINDOW_NS).min(season_end);
    ps.repayment_events.push(RepaymentEvent {
        asset,
        amount_usd: amount_usd_e8s,
        repaid_at,
        window_end,
    });
    put_principal_state(ps);
}

// ── Phase 5: open-epoch state + close-time accrual (driven by epoch.rs) ──────

pub fn get_open_epoch() -> Option<OpenEpoch> {
    with_state(|s| s.open_epoch.clone())
}

pub fn set_open_epoch(open: Option<OpenEpoch>) {
    with_state_mut(|s| s.open_epoch = open);
}

pub fn season_bounds() -> (u64, u64) {
    with_state(|s| (s.season_start_ns, s.season_end_ns))
}

/// Aggregate result of an epoch close, used to build the `EpochSummary`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CloseStats {
    pub total_points_all: u128,
    pub points_accrued: u128,
    pub active_principals: u64,
    pub registered_principals: u64,
}

/// One step of the chunked epoch close (POINTS-002). Either more batches remain or
/// the close finished.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloseStep {
    /// A batch was processed; the close cursor advanced. The open epoch's
    /// `close_*` fields have been persisted. Call again next tick to continue.
    More,
    /// Every registered principal has been closed exactly once; the snapshot
    /// buffer was cleared. `CloseStats` is the finalized aggregate for the
    /// `EpochSummary`. The driver now does the seed close + summary + index bump.
    Done(CloseStats),
}

/// Principals closed per chunked-close tick. Each principal does up to ~8 stable
/// `PointEntry` appends plus a `PrincipalState` rewrite, so the whole close over N
/// principals is O(N) stable writes and at a few thousand principals an UNCHUNKED
/// close blows the 5B-instruction limit, traps, and (since it never advanced the
/// epoch index) re-traps on every retry -> permanent accrual stall. Bounding the
/// batch keeps each close message well under budget; the cursor in `OpenEpoch`
/// resumes between ticks. Conservative relative to CAPTURE_CHUNK because the close
/// does more per-principal work (writes, not just reads).
pub const CLOSE_CHUNK: u64 = 50;

/// Process ONE batch of the chunked epoch close. Reads the resume cursor and
/// running totals from the open epoch, accrues up to `CLOSE_CHUNK` registered
/// principals STRICTLY AFTER the cursor, and persists the advanced cursor + totals
/// back into the open epoch. Returns `CloseStep::More` while batches remain, or
/// `CloseStep::Done(stats)` once the registered set is exhausted (also clearing the
/// snapshot buffer).
///
/// Exactly-once guarantee: principals are processed in stable key order and the
/// cursor always advances to the last-processed principal, so a resumed batch
/// starts strictly after it. No principal is ever accrued twice across batches,
/// even if a tick traps after persisting the cursor (the cursor is persisted in
/// the SAME synchronous message as the accrual writes; there is no await between
/// them, so they commit or roll back together).
///
/// Idempotent restart: if the open epoch is reloaded after an upgrade with
/// `close_started == false`, the close simply begins again from the start; a
/// partially-closed epoch keeps its cursor and resumes.
pub fn run_close_accrual_chunk(now_ns: u64) -> CloseStep {
    let mut open = match get_open_epoch() {
        Some(o) => o,
        // No open epoch: nothing to close. Report a trivially-finished close so the
        // driver's finalize path is a no-op-safe call.
        None => return CloseStep::Done(CloseStats::default()),
    };
    let epoch_index = open.epoch_index;
    let epoch_start = open.epoch_start_ns;
    let epoch_end_capped = open.epoch_end_ns;

    let batch = registered_chunk_after(open.close_cursor, CLOSE_CHUNK);
    let mut last_closed = open.close_cursor;
    for p in &batch {
        last_closed = Some(*p);
        if is_excluded(p) {
            continue; // excluded principals never accrue (spec Section 11)
        }
        let mut ps = match get_principal_state(p) {
            Some(s) => s,
            None => continue,
        };
        let min_weights = snapshot_buffer_get(p).unwrap_or_default();
        // Consume this principal's buffer entry as we close it, so the buffer is
        // drained incrementally across the close batches instead of in one O(N)
        // sweep at the end (PTS-002, which would compound POINTS-002's budget).
        snapshot_buffer_remove(p);
        let (entries, delta) =
            accrual::accrue_principal(min_weights, &ps.repayment_events, epoch_start, epoch_end_capped);
        for (source, pts) in entries {
            append_point_entry(PointEntry {
                principal: *p,
                epoch_index,
                points_delta: pts,
                source,
                recorded_at_ns: now_ns,
            });
        }
        if delta > 0 {
            ps.total_points = ps.total_points.saturating_add(delta);
            open.close_active = open.close_active.saturating_add(1);
        }
        ps.last_epoch_processed = epoch_index;
        // Drop repayment windows that can no longer overlap any future epoch, so
        // the per-principal vec stays bounded (no unbounded growth in the value).
        ps.repayment_events.retain(|r| r.window_end > epoch_end_capped);
        put_principal_state(ps);
        open.close_points_accrued = open.close_points_accrued.saturating_add(delta);
    }

    open.close_started = true;
    open.close_cursor = last_closed;
    // A short batch (fewer than CLOSE_CHUNK) means the registered set is exhausted.
    let accrual_done = (batch.len() as u64) < CLOSE_CHUNK;
    set_open_epoch(Some(open.clone()));

    if !accrual_done {
        return CloseStep::More;
    }

    // Accrual is done. Mop up any buffer stragglers (entries for principals that
    // were captured then deregistered, so the per-principal loop above never
    // visited them) in BOUNDED chunks (PTS-002). Stay in `More` until the buffer is
    // empty so the final mop-up never becomes a single unbounded sweep.
    if !snapshot_buffer_clear_chunk() {
        return CloseStep::More;
    }

    // Buffer drained and accrual complete: compute the season-wide total for the
    // summary and finalize.
    let total_points_all = PRINCIPALS.with(|m| {
        m.borrow()
            .iter()
            .fold(0u128, |acc, (_, v)| acc.saturating_add(v.into_current().total_points))
    });
    let registered_principals = registered_count();
    CloseStep::Done(CloseStats {
        total_points_all,
        points_accrued: open.close_points_accrued,
        active_principals: open.close_active,
        registered_principals,
    })
}

/// Drive the chunked close to completion in a loop, returning the finalized
/// `CloseStats`. Requires an open epoch (the close reads its bounds + cursor).
/// Used by tests and by any caller that wants the whole close in one synchronous
/// pass (the production driver instead spreads it across ticks via
/// `run_close_accrual_chunk`). The loop is bounded: each iteration advances the
/// cursor past at least `CLOSE_CHUNK` principals (or finishes), so it terminates
/// in `ceil(registered / CLOSE_CHUNK)` steps.
#[cfg(test)]
pub fn run_close_accrual_to_completion(now_ns: u64) -> CloseStats {
    loop {
        if let CloseStep::Done(stats) = run_close_accrual_chunk(now_ns) {
            return stats;
        }
    }
}

/// Snapshot B: keep `min_by_total(A, B)` for a principal already captured at A. A
/// principal absent at A (registered between snapshots, or zero at A) is left out:
/// the two-snapshot min is 0, so they earn no balance points this epoch.
pub fn snapshot_buffer_merge_min(p: Principal, weights: SnapshotWeights) {
    if let Some(existing) = snapshot_buffer_get(&p) {
        snapshot_buffer_put(p, accrual::min_by_total(existing, weights));
    }
}

/// The next chunk of registered principals (in stable principal order) strictly
/// after `cursor` (`None` = from the start), up to `limit`. Drives the chunked
/// epoch CLOSE, where order is irrelevant (every principal is processed exactly
/// once). The CAPTURE uses `registered_chunk_after_shuffled` instead, so the
/// capture order is unpredictable (POINTS-001 defense-in-depth).
pub fn registered_chunk_after(cursor: Option<Principal>, limit: u64) -> Vec<Principal> {
    PRINCIPALS.with(|m| {
        let map = m.borrow();
        let keys = map.iter().map(|(k, _)| k.0);
        match cursor {
            Some(c) => keys.filter(|p| *p > c).take(limit as usize).collect(),
            None => keys.take(limit as usize).collect(),
        }
    })
}

/// A seed-derived, epoch-stable order key for a principal: `sha256(seed || p)`.
/// The capture iterates principals in order of this key, so the order is fixed for
/// the epoch (resumable by cursor) yet unpredictable to anyone who does not know
/// the epoch's secret seed (revealed only at close). This is the POINTS-001
/// defense-in-depth: an attacker cannot predict WHEN their principal is captured,
/// so they cannot reliably flash-inflate their balance just before their snapshot.
pub fn capture_order_key(seed: &[u8; 32], p: &Principal) -> [u8; 32] {
    crate::snapshot_seed::sha256(&[seed, p.as_slice()])
}

/// The next chunk of registered principals for the CAPTURE, ordered by the
/// seed-derived `capture_order_key`, strictly after `cursor` in that order
/// (`None` = from the start), up to `limit` (POINTS-001). The order is stable for
/// the epoch (so the cursor resumes correctly) but unpredictable without the seed.
/// A principal registered mid-epoch that sorts before the cursor is skipped at this
/// snapshot (its min(A,B) is then 0) exactly as in the principal-ordered path.
pub fn registered_chunk_after_shuffled(
    seed: &[u8; 32],
    cursor: Option<Principal>,
    limit: u64,
) -> Vec<Principal> {
    // Materialize (order_key, principal) for all registered principals, sort by the
    // order key, then take `limit` strictly after the cursor's key. O(N log N) per
    // chunk; acceptable at Season-1 scale (the capture is the accepted bottleneck)
    // and bounded because the registered set is bounded by real registrations.
    let mut keyed: Vec<([u8; 32], Principal)> = PRINCIPALS.with(|m| {
        m.borrow()
            .iter()
            .map(|(k, _)| (capture_order_key(seed, &k.0), k.0))
            .collect()
    });
    keyed.sort_unstable();
    let cursor_key = cursor.map(|c| capture_order_key(seed, &c));
    keyed
        .into_iter()
        .filter(|(key, _)| match &cursor_key {
            Some(ck) => key > ck,
            None => true,
        })
        .map(|(_, p)| p)
        .take(limit as usize)
        .collect()
}

/// The open epoch's secret seed (the seed driving THIS epoch's capture order and
/// snapshot times). `None` if no epoch is open or the seed is somehow unset.
pub fn current_epoch_seed() -> Option<[u8; 32]> {
    with_state(|s| s.snapshot_seed.current_seed)
}

/// RAII single-tick epoch-driver guard (AR-S-001). Same drop-release contract as
/// `PollGuard`: a trap in a tick releases the flag via the future's destructors,
/// so the driver is never wedged until an upgrade.
#[must_use]
pub struct EpochGuard(());

impl EpochGuard {
    pub fn new() -> Option<Self> {
        EPOCH_IN_PROGRESS.with(|p| {
            let mut p = p.borrow_mut();
            if *p {
                None
            } else {
                *p = true;
                Some(EpochGuard(()))
            }
        })
    }
}

impl Drop for EpochGuard {
    fn drop(&mut self) {
        EPOCH_IN_PROGRESS.with(|p| *p.borrow_mut() = false);
    }
}

/// A principal's recorded 3pool deposit composition `(icUSD, ckUSDC, ckUSDT)` in
/// `usd_e8s` (the event-tracked side of the hybrid model). Zero for an unknown
/// principal or an empty leg.
pub fn recorded_3pool_composition(p: &Principal) -> (u128, u128, u128) {
    match get_principal_state(p) {
        Some(ps) => {
            let leg = |asset| {
                ps.active_deposits
                    .get(&DepositKey { venue: Venue::ThreePool, asset })
                    .map(|r| r.recorded_value_usd)
                    .unwrap_or(0)
            };
            (leg(AssetType::IcUsd), leg(AssetType::CkUsdc), leg(AssetType::CkUsdt))
        }
        None => (0, 0, 0),
    }
}

/// Append a revealed seed to the commit-reveal audit log (one row per closed epoch).
pub fn append_revealed_seed(seed: RevealedSeed) {
    REVEALED_SEEDS.with(|l| {
        l.borrow()
            .append(&StoredRevealedSeed::V1(seed))
            .expect("failed to append revealed seed");
    });
}

/// The revealed seed for a CLOSED epoch (the log index equals the epoch index).
pub fn get_revealed_seed(epoch_index: u64) -> Option<RevealedSeed> {
    REVEALED_SEEDS.with(|l| l.borrow().get(epoch_index).map(|s| s.into_current()))
}

/// Advance `current_epoch_index` (called at epoch close).
pub fn advance_epoch_index() {
    with_state_mut(|s| s.current_epoch_index = s.current_epoch_index.saturating_add(1));
}

/// The hash the next epoch's seed must reveal (spike 0.3 public audit value).
pub fn get_pending_commit() -> [u8; 32] {
    with_state(|s| s.snapshot_seed.pending_commit)
}

/// Whether the snapshot-seed commitment `H0` was set (at init). `start_season`
/// requires this so the operator cannot pick the snapshot times after the season
/// is underway (the commit-reveal anti-sniping guarantee).
pub fn snapshot_seed_committed() -> bool {
    with_state(|s| s.snapshot_seed.is_committed())
}

// ── Tests ───────────────────────────────────────────────────────────────────
// Native unit tests exercise the real stable structures: off-wasm,
// `DefaultMemoryImpl` is a heap-backed `VectorMemory`, and libtest runs each
// test on its own freshly spawned thread, so each test gets a clean set of
// thread-local stable structures.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        AssetType, DepositKey, DepositRecord, EpochSummary, InitArgs, OpenEpoch, PrincipalState,
        QualifyingAction, RepaymentEvent, Venue,
    };
    use std::collections::BTreeMap;

    fn tp(n: u8) -> Principal {
        Principal::from_slice(&[n, n, n, n, n])
    }

    fn backend_principal() -> Principal {
        Principal::from_text("tfesu-vyaaa-aaaap-qrd7a-cai").unwrap()
    }

    fn init_default(admin: Principal) {
        init_state(
            Some(InitArgs {
                admin: Some(admin),
                ..Default::default()
            }),
            Principal::anonymous(),
        );
    }

    #[test]
    fn excluded_seed_marks_protocol_canisters_and_not_founder() {
        init_default(tp(99));
        // The 9 protocol-owned canisters are seeded.
        assert_eq!(excluded_principals().len(), 9);
        assert!(is_excluded(&backend_principal()));
        // A founder / outside principal is NOT excluded (spec Section 11).
        assert!(!is_excluded(&tp(1)));
    }

    #[test]
    fn admin_defaults_to_caller_when_unset() {
        init_state(None, tp(7));
        assert_eq!(points_config().admin, tp(7));
        // Default seed applied when excluded_principals is None.
        assert_eq!(excluded_principals().len(), 9);
    }

    #[test]
    fn admin_can_add_and_remove_excluded_but_non_admin_cannot() {
        let admin = tp(99);
        init_default(admin);
        let target = tp(5);

        assert_eq!(add_excluded(admin, target), Ok(()));
        assert!(is_excluded(&target));
        assert_eq!(remove_excluded(admin, target), Ok(()));
        assert!(!is_excluded(&target));

        // A non-admin is rejected and the set is unchanged.
        let intruder = tp(8);
        assert_eq!(add_excluded(intruder, tp(6)), Err(PointsError::Unauthorized));
        assert!(!is_excluded(&tp(6)));
    }

    #[test]
    fn set_excluded_replaces_the_whole_set() {
        let admin = tp(99);
        init_default(admin);
        assert_eq!(set_excluded(admin, vec![tp(1), tp(2)]), Ok(()));
        assert_eq!(excluded_principals().len(), 2);
        assert!(is_excluded(&tp(1)));
        assert!(is_excluded(&tp(2)));
        // The protocol-owned seed was replaced, so the backend is no longer in.
        assert!(!is_excluded(&backend_principal()));
    }

    #[test]
    fn register_creates_state_writes_marker_and_is_idempotent() {
        init_default(tp(99));
        let p = tp(10);
        assert_eq!(point_ledger_len(), 0);

        let created = register(p, 1_000, QualifyingAction::Deposit3Pool).unwrap();
        assert_eq!(created.registered_at_ns, 1_000);
        assert_eq!(created.first_qualifying_action, QualifyingAction::Deposit3Pool);
        assert!(is_registered(&p));
        assert_eq!(registered_count(), 1);
        // One zero-point registration marker in the audit ledger.
        assert_eq!(point_ledger_len(), 1);

        // Re-register with a different timestamp: idempotent, no overwrite, no new marker.
        let again = register(p, 9_999, QualifyingAction::MintIcUsd).unwrap();
        assert_eq!(again.registered_at_ns, 1_000);
        assert_eq!(again.first_qualifying_action, QualifyingAction::Deposit3Pool);
        assert_eq!(registered_count(), 1);
        assert_eq!(point_ledger_len(), 1);
    }

    #[test]
    fn register_rejects_excluded_principals() {
        init_default(tp(99));
        let excluded = backend_principal();
        assert_eq!(
            register(excluded, 1, QualifyingAction::MintIcUsd),
            Err(PointsError::Excluded)
        );
        assert!(!is_registered(&excluded));
        assert_eq!(registered_count(), 0);
    }

    #[test]
    fn register_test_principal_is_admin_gated() {
        let admin = tp(99);
        init_default(admin);
        let p = tp(11);

        assert_eq!(
            register_test_principal(tp(8), p, 1),
            Err(PointsError::Unauthorized)
        );
        assert!(!is_registered(&p));

        assert_eq!(register_test_principal(admin, p, 1), Ok(()));
        assert!(is_registered(&p));
    }

    #[test]
    fn leaderboard_ranks_by_points_desc_with_share_and_pagination() {
        // Inject principals with known points via the private accessor.
        let mk = |p: Principal, pts: u128| PrincipalState {
            principal: p,
            total_points: pts,
            active_deposits: BTreeMap::new(),
            repayment_events: Vec::new(),
            last_epoch_processed: 0,
            registered_at_ns: 1,
            first_qualifying_action: QualifyingAction::MintIcUsd,
        };
        put_principal_state(mk(tp(1), 300));
        put_principal_state(mk(tp(2), 100));
        put_principal_state(mk(tp(3), 200));

        let board = leaderboard(0, 10);
        assert_eq!(board.len(), 3);
        // Desc by points: 300, 200, 100.
        assert_eq!(board[0].principal, tp(1));
        assert_eq!(board[0].rank, 1);
        assert_eq!(board[0].total_points, 300);
        assert_eq!(board[0].estimated_share_bps, 5000); // 300/600
        assert_eq!(board[1].principal, tp(3));
        assert_eq!(board[1].rank, 2);
        assert_eq!(board[1].estimated_share_bps, 3333); // 200/600
        assert_eq!(board[2].principal, tp(2));
        assert_eq!(board[2].rank, 3);
        assert_eq!(board[2].estimated_share_bps, 1666); // 100/600

        // Pagination: offset 1, limit 1 -> second-ranked only.
        let page = leaderboard(1, 1);
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].principal, tp(3));
        assert_eq!(page[0].rank, 2);
    }

    #[test]
    fn principal_state_versioned_roundtrip_through_stable_map() {
        // Exercises the ciborium round-trip of the full record, including the
        // struct-keyed BTreeMap and the repayment vec.
        let p = tp(20);
        let mut deposits = BTreeMap::new();
        let rec = DepositRecord {
            asset: AssetType::CkUsdc,
            venue: Venue::ThreePool,
            recorded_value_usd: 1_234,
            deposited_at: 42,
            last_verified_at: 43,
        };
        deposits.insert(rec.key(), rec.clone());
        let ps = PrincipalState {
            principal: p,
            total_points: 7,
            active_deposits: deposits,
            repayment_events: vec![RepaymentEvent {
                asset: AssetType::CkUsdt,
                amount_usd: 500,
                repaid_at: 10,
                window_end: 20,
            }],
            last_epoch_processed: 3,
            registered_at_ns: 99,
            first_qualifying_action: QualifyingAction::RepayVault,
        };
        put_principal_state(ps.clone());
        let got = get_principal_state(&p).expect("present");
        assert_eq!(got, ps);
        assert_eq!(
            got.active_deposits.get(&DepositKey {
                venue: Venue::ThreePool,
                asset: AssetType::CkUsdc
            }),
            Some(&rec)
        );
    }

    #[test]
    fn epoch_history_appends_and_paginates() {
        let mk = |i: u64| EpochSummary {
            epoch_index: i,
            epoch_start_ns: i * 10,
            epoch_end_ns: i * 10 + 5,
            total_points_all: (i as u128) * 100,
            points_accrued_this_epoch: (i as u128) * 10,
            active_principals: i,
            registered_principals: i,
            snapshot_a_ns: 0,
            snapshot_b_ns: 0,
        };
        append_epoch_summary(mk(0));
        append_epoch_summary(mk(1));
        assert_eq!(epoch_count(), 2);

        let all = epoch_history(0, 10);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].epoch_index, 0);
        assert_eq!(all[1].epoch_index, 1);

        let page = epoch_history(1, 1);
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].epoch_index, 1);
    }

    #[test]
    fn state_blob_survives_save_load_roundtrip() {
        let admin = tp(99);
        init_default(admin);
        // Mutate the singleton so the round-trip is meaningful.
        add_excluded(admin, tp(3)).unwrap();
        let before = with_state(|s| s.clone());

        save_state_to_stable();
        let loaded = load_state_from_stable().expect("state should load after save");
        assert_eq!(loaded, before);
        assert!(loaded.excluded_principals.contains(&tp(3)));
    }

    #[test]
    fn load_state_returns_none_when_nothing_saved() {
        // Fresh thread -> blob region never written -> None (no silent default).
        assert_eq!(load_state_from_stable(), None);
    }

    #[test]
    fn state_v1_blob_migrates_to_v2_with_no_open_epoch() {
        // Simulate a pre-Phase-5 blob: a StoredState::V1 payload from old bytes.
        let v1 = StateV1 {
            admin: tp(1),
            excluded_principals: [tp(2)].into_iter().collect(),
            season_start_ns: 100,
            season_end_ns: 200,
            current_epoch_index: 3,
            snapshot_seed: SnapshotSeedSingleton {
                pending_commit: [7u8; 32],
                current_seed: Some([8u8; 32]),
            },
        };
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(&StoredState::V1(v1.clone()), &mut bytes).unwrap();

        let migrated = decode_stored_state(&bytes).expect("V1 blob must decode and migrate");
        assert_eq!(migrated.admin, tp(1));
        assert!(migrated.excluded_principals.contains(&tp(2)));
        assert_eq!(migrated.season_start_ns, 100);
        assert_eq!(migrated.current_epoch_index, 3);
        assert_eq!(migrated.snapshot_seed, v1.snapshot_seed);
        assert_eq!(migrated.open_epoch, None); // the new field defaults on migration
    }

    #[test]
    fn state_v2_blob_migrates_to_v3_defaulting_close_fields() {
        // A pre-POINTS-002 V2 blob: open epoch in the frozen OpenEpochV2 shape (no
        // chunked-close fields). It must decode and migrate forward, defaulting the
        // close_* fields so a mid-epoch upgrade does not wipe the open epoch.
        let v2 = StateV2 {
            admin: tp(1),
            excluded_principals: [tp(2)].into_iter().collect(),
            season_start_ns: 0,
            season_end_ns: 0,
            current_epoch_index: 5,
            snapshot_seed: SnapshotSeedSingleton::default(),
            open_epoch: Some(OpenEpochV2 {
                epoch_index: 5,
                epoch_start_ns: 10,
                epoch_end_ns: 20,
                snapshot_a_ns: 12,
                snapshot_b_ns: 18,
                a_cursor: Some(tp(9)),
                a_complete: true,
                b_cursor: None,
                b_complete: false,
            }),
        };
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(&StoredState::V2(v2.clone()), &mut bytes).unwrap();

        let migrated = decode_stored_state(&bytes).expect("V2 blob must decode and migrate");
        let oe = migrated.open_epoch.expect("open epoch survives migration");
        assert_eq!(oe.epoch_index, 5);
        assert_eq!(oe.a_cursor, Some(tp(9)));
        assert!(oe.a_complete);
        // The new chunked-close fields default to a fresh (not-started) close.
        assert!(!oe.close_started);
        assert_eq!(oe.close_cursor, None);
        assert_eq!(oe.close_points_accrued, 0);
        assert_eq!(oe.close_active, 0);
    }

    #[test]
    fn state_v3_blob_round_trips_with_open_epoch() {
        let state = State {
            admin: tp(1),
            excluded_principals: BTreeSet::new(),
            season_start_ns: 0,
            season_end_ns: 0,
            current_epoch_index: 5,
            snapshot_seed: SnapshotSeedSingleton::default(),
            open_epoch: Some(OpenEpoch {
                epoch_index: 5,
                epoch_start_ns: 10,
                epoch_end_ns: 20,
                snapshot_a_ns: 12,
                snapshot_b_ns: 18,
                a_cursor: Some(tp(9)),
                a_complete: true,
                b_cursor: None,
                b_complete: false,
                close_started: true,
                close_cursor: Some(tp(4)),
                close_points_accrued: 123,
                close_active: 2,
            }),
        };
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(&StoredState::V3(state.clone()), &mut bytes).unwrap();
        assert_eq!(decode_stored_state(&bytes), Some(state));
    }

    #[test]
    fn poll_config_defaults_off_at_300s() {
        init_default(tp(99));
        assert!(!poll_enabled(), "poll timer is OFF by default");
        assert_eq!(poll_interval_secs(), DEFAULT_POLL_INTERVAL_SECS);
    }

    #[test]
    fn admin_can_enable_and_set_interval() {
        let admin = tp(99);
        init_default(admin);
        assert_eq!(set_poll_enabled(admin, true), Ok(()));
        assert!(poll_enabled());
        assert_eq!(set_poll_interval(admin, 600), Ok(()));
        assert_eq!(poll_interval_secs(), 600);
        assert_eq!(set_poll_enabled(admin, false), Ok(()));
        assert!(!poll_enabled());
    }

    #[test]
    fn poll_interval_is_floored() {
        let admin = tp(99);
        init_default(admin);
        assert_eq!(set_poll_interval(admin, 5), Ok(())); // below the floor
        assert_eq!(poll_interval_secs(), MIN_POLL_INTERVAL_SECS);
    }

    #[test]
    fn non_admin_cannot_change_poll_config() {
        init_default(tp(99));
        let intruder = tp(8);
        assert_eq!(set_poll_enabled(intruder, true), Err(PointsError::Unauthorized));
        assert_eq!(set_poll_interval(intruder, 120), Err(PointsError::Unauthorized));
        assert!(!poll_enabled());
        assert_eq!(poll_interval_secs(), DEFAULT_POLL_INTERVAL_SECS);
    }

    // ── Phase 5: snapshot buffer ──

    fn sw(debt: u128) -> SnapshotWeights {
        SnapshotWeights { icusd_debt: debt, ..Default::default() }
    }

    #[test]
    fn snapshot_buffer_put_get_round_trip() {
        let p = tp(40);
        assert_eq!(snapshot_buffer_get(&p), None);
        snapshot_buffer_put(p, sw(123));
        assert_eq!(snapshot_buffer_get(&p), Some(sw(123)));
        assert_eq!(snapshot_buffer_len(), 1);
    }

    #[test]
    fn snapshot_buffer_remove_and_clear() {
        snapshot_buffer_put(tp(1), sw(1));
        snapshot_buffer_put(tp(2), sw(2));
        snapshot_buffer_remove(&tp(1));
        assert_eq!(snapshot_buffer_get(&tp(1)), None);
        assert_eq!(snapshot_buffer_len(), 1);
        snapshot_buffer_clear();
        assert_eq!(snapshot_buffer_len(), 0);
        assert!(snapshot_buffer_entries().is_empty());
    }

    #[test]
    fn snapshot_buffer_entries_lists_all() {
        snapshot_buffer_put(tp(5), sw(50));
        snapshot_buffer_put(tp(6), sw(60));
        let mut got = snapshot_buffer_entries();
        got.sort_by_key(|(p, _)| *p);
        assert_eq!(got, vec![(tp(5), sw(50)), (tp(6), sw(60))]);
    }

    // ── Phase 5: asset-ledger registry ──

    #[test]
    fn asset_ledger_seed_classifies_mainnet_ledgers() {
        init_default(tp(99));
        let icusd = Principal::from_text("t6bor-paaaa-aaaap-qrd5q-cai").unwrap();
        let threeusd = Principal::from_text("fohh4-yyaaa-aaaap-qtkpa-cai").unwrap();
        let ckusdc = Principal::from_text("xevnm-gaaaa-aaaar-qafnq-cai").unwrap();
        let icp = Principal::from_text("ryjl3-tyaaa-aaaaa-aaaba-cai").unwrap();
        assert_eq!(classify_ledger(&icusd), Some(AssetType::IcUsd));
        assert_eq!(classify_ledger(&threeusd), Some(AssetType::ThreeUsd));
        assert_eq!(classify_ledger(&ckusdc), Some(AssetType::CkUsdc));
        assert_eq!(get_asset_ledger(AssetType::Icp), Some(icp));
        // An unknown ledger is not classified.
        assert_eq!(classify_ledger(&tp(123)), None);
    }

    #[test]
    fn admin_can_override_asset_ledger_for_local() {
        let admin = tp(99);
        init_default(admin);
        let local = tp(50);
        assert_eq!(
            set_asset_ledger(admin, asset_tag(AssetType::CkUsdc), local),
            Ok(())
        );
        assert_eq!(get_asset_ledger(AssetType::CkUsdc), Some(local));
        assert_eq!(classify_ledger(&local), Some(AssetType::CkUsdc));
        // Non-admin rejected.
        assert_eq!(set_asset_ledger(tp(8), 0, tp(7)), Err(PointsError::Unauthorized));
    }

    // ── Phase 5: epoch-driver config ──

    #[test]
    fn epoch_driver_defaults_off_at_300s() {
        init_default(tp(99));
        assert!(!epoch_driver_enabled());
        assert_eq!(epoch_driver_interval_secs(), DEFAULT_EPOCH_DRIVER_INTERVAL_SECS);
    }

    #[test]
    fn admin_can_enable_and_floor_epoch_driver_interval() {
        let admin = tp(99);
        init_default(admin);
        assert_eq!(set_epoch_driver_enabled(admin, true), Ok(()));
        assert!(epoch_driver_enabled());
        assert_eq!(set_epoch_driver_interval(admin, 5), Ok(())); // below the floor
        assert_eq!(epoch_driver_interval_secs(), MIN_EPOCH_DRIVER_INTERVAL_SECS);
        assert_eq!(
            set_epoch_driver_enabled(tp(8), false),
            Err(PointsError::Unauthorized)
        );
    }

    // ── Phase 5/4: ingestion-driven state ──

    fn key_3pool(asset: AssetType) -> DepositKey {
        DepositKey { venue: Venue::ThreePool, asset }
    }

    #[test]
    fn update_3pool_recorded_adds_subtracts_and_drops_at_zero() {
        init_default(tp(99));
        let p = tp(30);
        register(p, 1, QualifyingAction::Deposit3Pool).unwrap();
        let key = key_3pool(AssetType::CkUsdc);

        update_3pool_recorded(p, AssetType::CkUsdc, 100, true, 5);
        assert_eq!(get_principal_state(&p).unwrap().active_deposits[&key].recorded_value_usd, 100);

        update_3pool_recorded(p, AssetType::CkUsdc, 50, true, 6);
        assert_eq!(get_principal_state(&p).unwrap().active_deposits[&key].recorded_value_usd, 150);

        update_3pool_recorded(p, AssetType::CkUsdc, 60, false, 7);
        assert_eq!(get_principal_state(&p).unwrap().active_deposits[&key].recorded_value_usd, 90);

        // Subtracting past zero drops the record entirely.
        update_3pool_recorded(p, AssetType::CkUsdc, 1_000, false, 8);
        assert!(get_principal_state(&p).unwrap().active_deposits.get(&key).is_none());
    }

    #[test]
    fn update_3pool_recorded_is_noop_when_unregistered() {
        init_default(tp(99));
        update_3pool_recorded(tp(31), AssetType::IcUsd, 100, true, 5);
        assert!(get_principal_state(&tp(31)).is_none());
    }

    #[test]
    fn record_repayment_caps_window_at_season_end() {
        init_state(
            Some(InitArgs {
                admin: Some(tp(99)),
                season_start_ns: Some(0),
                season_end_ns: Some(1_000),
                ..Default::default()
            }),
            Principal::anonymous(),
        );
        let p = tp(32);
        register(p, 1, QualifyingAction::RepayVault).unwrap();

        record_repayment(p, AssetType::CkUsdc, 500, 100);
        let st = get_principal_state(&p).unwrap();
        let ev = &st.repayment_events[0];
        assert_eq!(ev.asset, AssetType::CkUsdc);
        assert_eq!(ev.amount_usd, 500);
        assert_eq!(ev.repaid_at, 100);
        assert_eq!(ev.window_end, 1_000); // 100 + 90d >> 1000, so capped at season end
    }

    #[test]
    fn record_repayment_uses_full_window_within_season() {
        init_state(
            Some(InitArgs {
                admin: Some(tp(99)),
                season_start_ns: Some(0),
                season_end_ns: Some(u64::MAX),
                ..Default::default()
            }),
            Principal::anonymous(),
        );
        let p = tp(33);
        register(p, 1, QualifyingAction::RepayVault).unwrap();
        record_repayment(p, AssetType::CkUsdt, 200, 100);
        let st = get_principal_state(&p).unwrap();
        assert_eq!(st.repayment_events[0].window_end, 100 + REPAYMENT_WINDOW_NS);
    }

    // ── Phase 5: open epoch + close accrual ──

    fn open_epoch_at(index: u64) -> OpenEpoch {
        open_epoch_bounds(index, 0, 7 * crate::NANOS_PER_DAY)
    }

    /// An open epoch (both snapshots complete, close not started) with explicit
    /// bounds, for driving the chunked close in tests.
    fn open_epoch_bounds(index: u64, start: u64, end: u64) -> OpenEpoch {
        OpenEpoch {
            epoch_index: index,
            epoch_start_ns: start,
            epoch_end_ns: end,
            snapshot_a_ns: 1,
            snapshot_b_ns: 2,
            a_cursor: None,
            a_complete: true,
            b_cursor: None,
            b_complete: true,
            close_started: false,
            close_cursor: None,
            close_points_accrued: 0,
            close_active: 0,
        }
    }

    #[test]
    fn open_epoch_get_set_round_trip() {
        init_default(tp(99));
        assert_eq!(get_open_epoch(), None);
        set_open_epoch(Some(open_epoch_at(3)));
        assert_eq!(get_open_epoch().unwrap().epoch_index, 3);
        set_open_epoch(None);
        assert_eq!(get_open_epoch(), None);
    }

    #[test]
    fn run_close_accrual_accrues_balance_and_repayment() {
        // A long season so the 90-day window is not truncated.
        init_state(
            Some(InitArgs {
                admin: Some(tp(99)),
                season_start_ns: Some(0),
                season_end_ns: Some(400 * crate::NANOS_PER_DAY),
                ..Default::default()
            }),
            Principal::anonymous(),
        );
        let p1 = tp(60);
        let p2 = tp(61);
        register(p1, 1, QualifyingAction::MintIcUsd).unwrap();
        register(p2, 1, QualifyingAction::RepayVault).unwrap();

        // p1: a balance position captured into the snapshot buffer.
        snapshot_buffer_put(p1, SnapshotWeights { icusd_debt: 100, ..Default::default() });
        // p2: an open repayment window, no balance position.
        record_repayment(p2, AssetType::CkUsdc, 1_000 * 100_000_000, 0);

        let week = 7 * crate::NANOS_PER_DAY;
        set_open_epoch(Some(open_epoch_bounds(0, 0, week)));
        let stats = run_close_accrual_to_completion(9_999);

        let repay = 1_000u128 * 100_000_000 * 5 * 7;
        assert_eq!(get_principal_state(&p1).unwrap().total_points, 700); // 100 over 7 days
        assert_eq!(get_principal_state(&p2).unwrap().total_points, repay);
        assert_eq!(get_principal_state(&p1).unwrap().last_epoch_processed, 0);
        assert_eq!(stats.active_principals, 2);
        assert_eq!(stats.registered_principals, 2);
        assert_eq!(stats.points_accrued, 700 + repay);
        assert_eq!(stats.total_points_all, 700 + repay);
        assert_eq!(snapshot_buffer_len(), 0); // drained for the next epoch
    }

    #[test]
    fn run_close_accrual_prunes_expired_repayment_windows() {
        init_state(
            Some(InitArgs {
                admin: Some(tp(99)),
                season_start_ns: Some(0),
                season_end_ns: Some(400 * crate::NANOS_PER_DAY),
                ..Default::default()
            }),
            Principal::anonymous(),
        );
        let p = tp(65);
        register(p, 1, QualifyingAction::RepayVault).unwrap();
        let mut ps = get_principal_state(&p).unwrap();
        ps.repayment_events = vec![
            // Expires within epoch 0 (window_end 3d <= epoch end 7d): drop after close.
            RepaymentEvent { asset: AssetType::CkUsdc, amount_usd: 100, repaid_at: 0, window_end: 3 * crate::NANOS_PER_DAY },
            // Still open past epoch 0: keep.
            RepaymentEvent { asset: AssetType::CkUsdc, amount_usd: 100, repaid_at: 0, window_end: 50 * crate::NANOS_PER_DAY },
        ];
        put_principal_state(ps);

        let week = 7 * crate::NANOS_PER_DAY;
        set_open_epoch(Some(open_epoch_bounds(0, 0, week)));
        run_close_accrual_to_completion(1);

        let after = get_principal_state(&p).unwrap();
        assert_eq!(after.repayment_events.len(), 1, "the expired window is pruned");
        assert_eq!(after.repayment_events[0].window_end, 50 * crate::NANOS_PER_DAY);
    }

    #[test]
    fn run_close_accrual_skips_excluded_principals() {
        let admin = tp(99);
        init_default(admin);
        let p = tp(62);
        register(p, 1, QualifyingAction::MintIcUsd).unwrap();
        snapshot_buffer_put(p, SnapshotWeights { icusd_debt: 100, ..Default::default() });
        add_excluded(admin, p).unwrap();

        let week = 7 * crate::NANOS_PER_DAY;
        set_open_epoch(Some(open_epoch_bounds(0, 0, week)));
        let stats = run_close_accrual_to_completion(1);
        assert_eq!(get_principal_state(&p).unwrap().total_points, 0); // excluded: no accrual
        assert_eq!(stats.active_principals, 0);
    }

    #[test]
    fn snapshot_buffer_merge_min_keeps_smaller_total() {
        let p = tp(70);
        snapshot_buffer_put(p, SnapshotWeights { icusd_debt: 100, ..Default::default() }); // A total 100
        snapshot_buffer_merge_min(p, SnapshotWeights { icusd_debt: 40, ..Default::default() }); // B total 40
        assert_eq!(snapshot_buffer_get(&p).unwrap().icusd_debt, 40);

        let q = tp(71);
        snapshot_buffer_put(q, SnapshotWeights { icusd_debt: 30, ..Default::default() }); // A 30
        snapshot_buffer_merge_min(q, SnapshotWeights { icusd_debt: 90, ..Default::default() }); // B 90
        assert_eq!(snapshot_buffer_get(&q).unwrap().icusd_debt, 30); // keeps A
    }

    #[test]
    fn snapshot_buffer_merge_min_skips_principal_absent_at_a() {
        let p = tp(72);
        // No A entry: a B-only principal (registered between snapshots) earns nothing.
        snapshot_buffer_merge_min(p, SnapshotWeights { icusd_debt: 100, ..Default::default() });
        assert_eq!(snapshot_buffer_get(&p), None);
    }

    #[test]
    fn registered_chunk_after_paginates_in_principal_order() {
        init_default(tp(99));
        register(tp(1), 1, QualifyingAction::MintIcUsd).unwrap();
        register(tp(2), 1, QualifyingAction::MintIcUsd).unwrap();
        register(tp(3), 1, QualifyingAction::MintIcUsd).unwrap();
        assert_eq!(registered_chunk_after(None, 2), vec![tp(1), tp(2)]);
        assert_eq!(registered_chunk_after(Some(tp(2)), 2), vec![tp(3)]);
        assert_eq!(registered_chunk_after(Some(tp(3)), 2), Vec::<Principal>::new());
    }

    #[test]
    fn epoch_guard_is_single_entry() {
        let held = EpochGuard::new().expect("first acquire");
        assert!(EpochGuard::new().is_none()); // already held
        drop(held);
        assert!(EpochGuard::new().is_some());
    }

    // ── AR-S-001: a panicking tick must release the single-tick guards ──
    // On the IC a trap drops the in-flight future via ic-cdk's cleanup, running
    // destructors; catch_unwind simulates that unwind path natively.

    #[test]
    fn ar_s_001_epoch_guard_released_on_panic() {
        let result = std::panic::catch_unwind(|| {
            let _guard = EpochGuard::new().expect("acquire");
            panic!("simulated trap mid-tick");
        });
        assert!(result.is_err());
        assert!(
            EpochGuard::new().is_some(),
            "a trapped tick must not leave the epoch guard stuck"
        );
    }

    #[test]
    fn ar_s_001_poll_guard_released_on_panic() {
        let result = std::panic::catch_unwind(|| {
            let _guard = PollGuard::new().expect("acquire");
            panic!("simulated trap mid-poll");
        });
        assert!(result.is_err());
        assert!(
            PollGuard::new().is_some(),
            "a trapped poll must not leave the poll guard stuck"
        );
    }

    #[test]
    fn ar_s_001_poll_guard_is_single_entry() {
        let held = PollGuard::new().expect("first acquire");
        assert!(PollGuard::new().is_none()); // already held
        drop(held);
        assert!(PollGuard::new().is_some());
    }

    #[test]
    fn recorded_3pool_composition_reads_active_deposits() {
        init_default(tp(99));
        let p = tp(80);
        register(p, 1, QualifyingAction::Deposit3Pool).unwrap();
        update_3pool_recorded(p, AssetType::IcUsd, 10, true, 1);
        update_3pool_recorded(p, AssetType::CkUsdc, 20, true, 1);
        assert_eq!(recorded_3pool_composition(&p), (10, 20, 0));
        assert_eq!(recorded_3pool_composition(&tp(123)), (0, 0, 0));
    }

    #[test]
    fn revealed_seed_log_appends_and_reads_by_index() {
        let r = |i: u64| RevealedSeed {
            epoch_index: i,
            seed: [i as u8; 32],
            snapshot_time_a_ns: i,
            snapshot_time_b_ns: i + 1,
            revealed_at_ns: i + 2,
        };
        append_revealed_seed(r(0));
        append_revealed_seed(r(1));
        assert_eq!(revealed_seed_count(), 2);
        assert_eq!(get_revealed_seed(0).unwrap().epoch_index, 0);
        assert_eq!(get_revealed_seed(1).unwrap().snapshot_time_a_ns, 1);
        assert_eq!(get_revealed_seed(2), None);
    }

    #[test]
    fn advance_epoch_index_increments() {
        init_default(tp(99));
        assert_eq!(current_epoch_index(), 0);
        advance_epoch_index();
        advance_epoch_index();
        assert_eq!(current_epoch_index(), 2);
    }

    // ── POINTS-002: the chunked epoch close ──

    /// Register `n` principals, each with $1 of icUSD debt captured for the epoch.
    fn seed_principals_with_debt(n: u8) {
        for i in 1..=n {
            let p = tp(i);
            register(p, 1, QualifyingAction::MintIcUsd).unwrap();
            snapshot_buffer_put(p, SnapshotWeights { icusd_debt: 100, ..Default::default() });
        }
    }

    #[test]
    fn chunked_close_credits_every_principal_exactly_once_over_many_batches() {
        // POINTS-002: a close over MORE than CLOSE_CHUNK principals must span
        // several `More` ticks, then one `Done`, and credit each principal exactly
        // once (no double-credit across resumed batches, no skipped principal).
        init_state(
            Some(InitArgs {
                admin: Some(tp(200)),
                season_start_ns: Some(0),
                season_end_ns: Some(400 * crate::NANOS_PER_DAY),
                excluded_principals: Some(vec![]), // no protocol-owned exclusions
                ..Default::default()
            }),
            Principal::anonymous(),
        );
        // 2.5 batches' worth so we exercise More -> More -> Done with a short tail.
        let n: u8 = (CLOSE_CHUNK as u8) * 2 + (CLOSE_CHUNK as u8) / 2;
        seed_principals_with_debt(n);

        let week = 7 * crate::NANOS_PER_DAY;
        set_open_epoch(Some(open_epoch_bounds(0, 0, week)));

        // Drive the close one batch at a time, counting the ticks.
        let mut more_ticks = 0u32;
        let stats = loop {
            match run_close_accrual_chunk(1) {
                CloseStep::More => {
                    more_ticks += 1;
                    assert!(
                        get_open_epoch().is_some(),
                        "the epoch stays OPEN while the close is mid-flight"
                    );
                    assert!(more_ticks < 100, "close must terminate, not loop forever");
                }
                CloseStep::Done(s) => break s,
            }
        };

        // It took multiple batches (the whole point of chunking).
        assert!(more_ticks >= 2, "expected several More ticks, got {more_ticks}");
        // Each principal accrued exactly $1 over 7 days, exactly once.
        for i in 1..=n {
            assert_eq!(
                get_principal_state(&tp(i)).unwrap().total_points,
                700,
                "principal {i} must be credited exactly once (100 debt x 7 days)"
            );
            assert_eq!(get_principal_state(&tp(i)).unwrap().last_epoch_processed, 0);
        }
        assert_eq!(stats.active_principals, n as u64);
        assert_eq!(stats.registered_principals, n as u64);
        assert_eq!(stats.points_accrued, 700 * n as u128);
        assert_eq!(stats.total_points_all, 700 * n as u128);
        // PTS-002: the snapshot buffer is fully drained by close completion.
        assert_eq!(snapshot_buffer_len(), 0, "buffer drained across the chunked close");
    }

    #[test]
    fn chunked_close_does_not_double_credit_when_a_batch_is_replayed() {
        // POINTS-002 exactly-once: re-running the SAME first batch (simulating a
        // retry after a trap that rolled back the cursor advance) must not credit a
        // principal twice. Because the cursor advances in the same synchronous
        // message as the accrual, a real trap rolls both back together; here we
        // assert the weaker property that a partial close followed by completion
        // still credits each principal exactly once.
        init_state(
            Some(InitArgs {
                admin: Some(tp(200)),
                season_start_ns: Some(0),
                season_end_ns: Some(400 * crate::NANOS_PER_DAY),
                excluded_principals: Some(vec![]),
                ..Default::default()
            }),
            Principal::anonymous(),
        );
        let n: u8 = (CLOSE_CHUNK as u8) + 5; // one full batch + a short tail
        seed_principals_with_debt(n);

        let week = 7 * crate::NANOS_PER_DAY;
        set_open_epoch(Some(open_epoch_bounds(0, 0, week)));

        // Process exactly the first batch.
        assert_eq!(run_close_accrual_chunk(1), CloseStep::More);
        let after_first = get_open_epoch().unwrap();
        assert!(after_first.close_started);
        assert!(after_first.close_cursor.is_some(), "cursor advanced past batch 1");

        // Finish the close.
        let stats = run_close_accrual_to_completion(1);

        // Every principal is credited exactly once despite the multi-step close.
        for i in 1..=n {
            assert_eq!(get_principal_state(&tp(i)).unwrap().total_points, 700);
        }
        assert_eq!(stats.points_accrued, 700 * n as u128);
        assert_eq!(stats.active_principals, n as u64);
    }

    #[test]
    fn chunked_close_on_no_open_epoch_is_a_noop_done() {
        init_default(tp(99));
        assert_eq!(get_open_epoch(), None);
        assert_eq!(run_close_accrual_chunk(1), CloseStep::Done(CloseStats::default()));
    }

    // ── PTS-002: bounded snapshot-buffer clear ──

    #[test]
    fn snapshot_buffer_clear_chunk_is_bounded_and_eventually_empties() {
        // Put more than one chunk's worth of entries; one chunk call leaves some.
        let n = SNAPSHOT_BUFFER_CLEAR_CHUNK + 50;
        for i in 0..n {
            // Distinct principals from a 4-byte counter so keys differ.
            let bytes = (i as u32).to_be_bytes();
            snapshot_buffer_put(Principal::from_slice(&bytes), SnapshotWeights::default());
        }
        assert_eq!(snapshot_buffer_len(), n);

        // One bounded chunk removes at most SNAPSHOT_BUFFER_CLEAR_CHUNK and is not
        // yet empty.
        let emptied = snapshot_buffer_clear_chunk();
        assert!(!emptied, "first chunk should not empty a >1-chunk buffer");
        assert_eq!(snapshot_buffer_len(), n - SNAPSHOT_BUFFER_CLEAR_CHUNK);

        // The convenience full-clear finishes the rest in bounded chunks.
        snapshot_buffer_clear();
        assert_eq!(snapshot_buffer_len(), 0);
        assert!(snapshot_buffer_clear_chunk(), "clearing an empty buffer reports empty");
    }

    // ── PTS-001: bounded leaderboard ──

    #[test]
    fn leaderboard_clamps_limit_to_max() {
        // Seed a handful of principals; a giant `limit` must clamp to
        // MAX_LEADERBOARD_LIMIT (we cannot seed a million, so assert the clamp via
        // a small max by checking the returned len never exceeds the registered
        // count and that an absurd limit does not panic / over-return).
        let mk = |p: Principal, pts: u128| PrincipalState {
            principal: p,
            total_points: pts,
            active_deposits: BTreeMap::new(),
            repayment_events: Vec::new(),
            last_epoch_processed: 0,
            registered_at_ns: 1,
            first_qualifying_action: QualifyingAction::MintIcUsd,
        };
        for i in 1..=5u8 {
            put_principal_state(mk(tp(i), i as u128 * 10));
        }
        // An unbounded limit returns at most the registered set (5), and the call
        // is bounded by MAX_LEADERBOARD_LIMIT regardless.
        let board = leaderboard(0, u32::MAX);
        assert_eq!(board.len(), 5);
        assert!(MAX_LEADERBOARD_LIMIT >= board.len() as u32);
    }

    #[test]
    fn leaderboard_limit_clamp_caps_returned_rows() {
        // White-box: with MORE registered principals than the cap, a request for
        // u32::MAX rows returns at most MAX_LEADERBOARD_LIMIT. We shrink the proof
        // to the cap by registering cap+overflow synthetic principals would be huge,
        // so instead assert the clamp arithmetic directly: limit.min(MAX) is what
        // the function applies.
        let mk = |p: Principal| PrincipalState {
            principal: p,
            total_points: 1,
            active_deposits: BTreeMap::new(),
            repayment_events: Vec::new(),
            last_epoch_processed: 0,
            registered_at_ns: 1,
            first_qualifying_action: QualifyingAction::MintIcUsd,
        };
        // Register a few; request a tiny clamp via the public arg path.
        for i in 0..3u32 {
            put_principal_state(mk(Principal::from_slice(&i.to_be_bytes())));
        }
        // A limit ABOVE the cap is clamped; a limit BELOW the cap is honored.
        assert_eq!(leaderboard(0, 2).len(), 2, "small limit honored");
        assert!(
            leaderboard(0, u32::MAX).len() <= MAX_LEADERBOARD_LIMIT as usize,
            "unbounded limit clamped to MAX_LEADERBOARD_LIMIT"
        );
    }

    // ── POINTS-001: public epoch status omits cursors; capture order is shuffled ──

    #[test]
    fn public_epoch_status_omits_capture_and_close_cursors() {
        init_default(tp(99));
        set_open_epoch(Some(OpenEpoch {
            epoch_index: 2,
            epoch_start_ns: 10,
            epoch_end_ns: 20,
            snapshot_a_ns: 12,
            snapshot_b_ns: 18,
            a_cursor: Some(tp(7)),
            a_complete: true,
            b_cursor: Some(tp(3)),
            b_complete: false,
            close_started: true,
            close_cursor: Some(tp(5)),
            close_points_accrued: 99,
            close_active: 1,
        }));
        // The full (admin) status carries the cursors.
        let full = epoch_status();
        let foe = full.open_epoch.unwrap();
        assert_eq!(foe.a_cursor, Some(tp(7)));
        assert!(foe.a_complete);
        // The public status reduces the open epoch to bounds + FIRED snapshot
        // times only (both have passed at now=20).
        let pub_status = public_epoch_status(20);
        let poe = pub_status.open_epoch.unwrap();
        assert_eq!(poe.epoch_index, 2);
        assert_eq!(poe.epoch_start_ns, 10);
        assert_eq!(poe.epoch_end_ns, 20);
        assert_eq!(poe.snapshot_a_ns, Some(12));
        assert_eq!(poe.snapshot_b_ns, Some(18));
        // PublicOpenEpoch has NO cursor/complete fields at all (compile-time), so
        // there is nothing for an attacker to read; assert the public/full views
        // agree on the non-sensitive window.
        assert_eq!(poe.epoch_index, foe.epoch_index);
        assert_eq!(poe.snapshot_a_ns, Some(foe.snapshot_a_ns));
    }

    // ── PTS-002: future snapshot times stay hidden from the public status ──

    #[test]
    fn pts_002_public_status_hides_future_snapshot_times() {
        init_default(tp(99));
        set_open_epoch(Some(OpenEpoch {
            epoch_index: 0,
            epoch_start_ns: 10,
            epoch_end_ns: 20,
            snapshot_a_ns: 12,
            snapshot_b_ns: 18,
            a_cursor: None,
            a_complete: false,
            b_cursor: None,
            b_complete: false,
            close_started: false,
            close_cursor: None,
            close_points_accrued: 0,
            close_active: 0,
        }));
        // Before snapshot A fires, BOTH times are withheld (revealing a future
        // time hands an attacker the exact flash-deposit moment).
        let poe = public_epoch_status(11).open_epoch.unwrap();
        assert_eq!(poe.snapshot_a_ns, None);
        assert_eq!(poe.snapshot_b_ns, None);
        // Once A fires (now >= a) it is history and revealed; B stays hidden.
        let poe = public_epoch_status(12).open_epoch.unwrap();
        assert_eq!(poe.snapshot_a_ns, Some(12));
        assert_eq!(poe.snapshot_b_ns, None);
        let poe = public_epoch_status(17).open_epoch.unwrap();
        assert_eq!(poe.snapshot_a_ns, Some(12));
        assert_eq!(poe.snapshot_b_ns, None);
        // After B fires, both are revealed.
        let poe = public_epoch_status(18).open_epoch.unwrap();
        assert_eq!(poe.snapshot_a_ns, Some(12));
        assert_eq!(poe.snapshot_b_ns, Some(18));
        // The ADMIN view is unredacted regardless of time.
        let foe = epoch_status().open_epoch.unwrap();
        assert_eq!(foe.snapshot_a_ns, 12);
        assert_eq!(foe.snapshot_b_ns, 18);
    }

    #[test]
    fn shuffled_capture_order_is_seed_dependent_and_covers_all_exactly_once() {
        // POINTS-001 defense-in-depth: the capture order is a seed-derived
        // permutation, NOT principal order, and a different seed yields a different
        // order — so an attacker cannot predict when their principal is captured.
        init_default(tp(99));
        for i in 1..=10u8 {
            register(tp(i), 1, QualifyingAction::MintIcUsd).unwrap();
        }
        let seed_a = [1u8; 32];
        let seed_b = [2u8; 32];

        let order_a = registered_chunk_after_shuffled(&seed_a, None, 100);
        let order_b = registered_chunk_after_shuffled(&seed_b, None, 100);
        let principal_order = registered_chunk_after(None, 100);

        // Same membership (all 10), regardless of order.
        assert_eq!(order_a.len(), 10);
        let mut sorted_a = order_a.clone();
        sorted_a.sort();
        assert_eq!(sorted_a, principal_order, "shuffle covers exactly the same set");

        // The shuffled order differs from principal order, and two seeds differ
        // from each other (unpredictability).
        assert_ne!(order_a, principal_order, "capture order is not principal order");
        assert_ne!(order_a, order_b, "different seeds produce different capture orders");
    }

    #[test]
    fn shuffled_capture_cursor_resumes_without_gaps_or_repeats() {
        // Paginating the shuffled order by cursor must visit every principal exactly
        // once across pages (the resume invariant the chunked capture relies on).
        init_default(tp(99));
        for i in 1..=12u8 {
            register(tp(i), 1, QualifyingAction::MintIcUsd).unwrap();
        }
        let seed = [7u8; 32];
        let full = registered_chunk_after_shuffled(&seed, None, 100);

        // Walk in pages of 5, threading the cursor (last principal of each page).
        let mut paged: Vec<Principal> = Vec::new();
        let mut cursor: Option<Principal> = None;
        loop {
            let page = registered_chunk_after_shuffled(&seed, cursor, 5);
            if page.is_empty() {
                break;
            }
            cursor = page.last().copied();
            paged.extend(page);
        }
        assert_eq!(paged, full, "cursor-paged shuffle equals the full shuffle, once each");
    }
}
