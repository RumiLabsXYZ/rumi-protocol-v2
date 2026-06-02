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

use crate::snapshot_seed::{RevealedSeed, SnapshotSeedSingleton};
use crate::types::{
    EpochSummary, InitArgs, LeaderboardEntry, PointEntry, PointSource, PointsConfig, PointsError,
    PrincipalState, QualifyingAction, RegistrationInfo,
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

/// Singleton config (admin, excluded set, season window, epoch counter, in-flight
/// seed). Held on the heap during execution and serialized to the `STATE_BLOB`
/// region on `pre_upgrade`, mirroring the backend. NOT candid-facing.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub admin: Principal,
    pub excluded_principals: BTreeSet<Principal>,
    pub season_start_ns: u64,
    pub season_end_ns: u64,
    pub current_epoch_index: u64,
    pub snapshot_seed: SnapshotSeedSingleton,
}

#[derive(Serialize, Deserialize)]
pub enum StoredState {
    V1(State),
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
    };
    STATE.with(|cell| *cell.borrow_mut() = Some(state));
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

/// Ranked leaderboard (desc by points, principal as tiebreak), paginated, with
/// each entry's estimated share of the 5% pool in basis points.
pub fn leaderboard(offset: u32, limit: u32) -> Vec<LeaderboardEntry> {
    let mut all: Vec<(Principal, u128)> = PRINCIPALS.with(|m| {
        m.borrow()
            .iter()
            .map(|(k, v)| (k.0, v.into_current().total_points))
            .collect()
    });
    // Highest points first; principal id as a stable tiebreak.
    all.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let total_points_all: u128 = all.iter().map(|(_, pts)| *pts).sum();

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

/// Serialize the heap `State` (as `StoredState::V1`) to the singleton blob with
/// the 8-byte little-endian length prefix. Called from `#[pre_upgrade]`.
pub fn save_state_to_stable() {
    let bytes = with_state(|s| {
        let mut buf = Vec::new();
        ciborium::ser::into_writer(&StoredState::V1(s.clone()), &mut buf)
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
        match ciborium::de::from_reader::<StoredState, _>(buf.as_slice()) {
            Ok(StoredState::V1(s)) => Some(s),
            Err(e) => {
                ic_cdk::println!("[upgrade] failed to deserialize State: {:?}", e);
                None
            }
        }
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

// ── Tests ───────────────────────────────────────────────────────────────────
// Native unit tests exercise the real stable structures: off-wasm,
// `DefaultMemoryImpl` is a heap-backed `VectorMemory`, and libtest runs each
// test on its own freshly spawned thread, so each test gets a clean set of
// thread-local stable structures.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        AssetType, DepositKey, DepositRecord, EpochSummary, InitArgs, PrincipalState,
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
}
