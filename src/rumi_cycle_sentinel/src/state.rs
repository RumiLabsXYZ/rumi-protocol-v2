//! Stable-storage layer for Cycle Sentinel (Task 1c).
//!
//! Wires the pure domain types in `types.rs` into `ic-stable-structures`
//! storage: one `MemoryManager` partitions stable memory into 17 regions
//! (memory IDs 0-16, see `MEMORY_LAYOUT` below), each backing exactly one
//! `StableCell`/`StableBTreeMap`. This file owns storage + raw CRUD + count
//! bound enforcement at the storage boundary; it does **not** own
//! governance/funding *policy* (threshold math, proposal execution, rail
//! selection, timer wiring) — that is later tasks' job (see
//! `.superpowers/sdd/2026-09-13-cycle-sentinel-telemetry/task-1c-blueprint.md`
//! section 11, "Explicit non-goals for Task 1c").
//!
//! ## Versioned-envelope pattern (UPG-002 safety) -- READ BEFORE ADDING A FIELD
//! Every at-rest value is wrapped in a private, externally-tagged `Stored*`
//! enum whose Candid encoding carries the version tag (`variant { V1 = ... }`
//! at the wire level). Adding a field to a logical type WITHOUT this
//! discipline can silently break decoding of old stable bytes on the next
//! upgrade. To add a field to a logical type `T`:
//!   1. Freeze today's shape of `T` as `TV1` (copy the struct/enum).
//!   2. Add the field to `T` (now the "current" shape).
//!   3. Change the wrapper to `enum StoredT { V1(TV1), V2(T) }`.
//!   4. Add `impl From<TV1> for T` (default the new field).
//!   5. In `into_current`, map `V1(old) => old.into()`, `V2(v) => v`.
//!
//! Old `V1` bytes keep decoding into the frozen `TV1` shape, then migrate on
//! read. No wipe.
//!
//! ## Why Candid encode/decode, not CBOR
//! The closest in-repo analog (`rumi_points::state`) uses `ciborium` for the
//! `Storable` byte encoding. That is deliberately NOT replicated here: this
//! crate's `Cargo.toml` does not depend on `ciborium`, and per the Task 1c
//! coordinator correction this file must not add any new dependency. Every
//! `Stored*` wrapper below is therefore `CandidType + Deserialize` and its
//! `Storable::to_bytes`/`from_bytes` go through `candid::encode_one`/
//! `candid::decode_one` (both already available via the existing `candid`
//! dependency). Every wrapped type either already derives `CandidType`
//! (`types.rs`'s Candid-facing domain types) or derives it fresh here (the
//! five bookkeeping types this file owns).
//!
//! ## Why not the liquidation_bot pattern
//! `liquidation_bot/src/state.rs` predates `MemoryManager` adoption and
//! carries a manual raw-offset-0 `pre_upgrade`/`post_upgrade`
//! serialize/deserialize path. This crate starts clean, so every store here
//! is `MemoryManager`-backed and every write lands in stable memory
//! synchronously at the moment of the call — there is nothing to flush at
//! `pre_upgrade`, so this crate registers no `pre_upgrade` hook at all.
//! `post_upgrade` only needs to call `validate_whole_state` (below); every
//! `thread_local!` store below reopens its assigned region lazily via
//! `StableCell::init`/`StableBTreeMap::init`, exactly the calls used at
//! first install (`MemoryManager::init` itself already distinguishes
//! "reopen existing header" from "format fresh").
//!
//! ## Self-contained vs. whole-state invariants
//! Every type in `types.rs` re-validates the invariants checkable from its
//! own decoded fields alone (see that file's module doc and
//! `.superpowers/sdd/2026-09-13-cycle-sentinel-telemetry/task-1b-decode-review.md`
//! Part 5). This file is responsible for the invariants that need data no
//! single type's decode can see on its own: registry-wide reserved/duplicate
//! principal checks, target/operation daily caps against the *currently
//! loaded* `GlobalPolicy`, at-most-one-nonterminal-operation-per-target,
//! bidirectional operation<->reservation linkage (per-target and global),
//! the self-recovery in-flight link, and open-proposal-approvals-subset-of-
//! signers. `validate_whole_state` (bottom of this file) is the single
//! place all of these are checked, called from `lib.rs`'s `#[post_upgrade]`
//! hook (and available to call from `#[init]`-adjacent code / future repair
//! tooling too). It deliberately does NOT attempt to re-prove that a
//! resolved `FundingOperation` is ledger-truthful against a live external
//! ledger — Part 5 item 3 explains why that is inherently impossible from
//! stable bytes alone; this file trusts its own authenticated versioned
//! stable history for already-resolved external outcomes across an upgrade.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::BTreeSet;

use candid::{CandidType, Principal};
use ic_stable_structures::{
    memory_manager::{MemoryId, MemoryManager, VirtualMemory},
    storable::{Bound, Storable},
    DefaultMemoryImpl, StableBTreeMap, StableCell,
};
use serde::Deserialize;

use crate::types::{
    self, Alarm, AlarmKind, FundingOperation, FundingOperationV1, FundingOperationV2, FundingRail,
    FundingTrigger, GlobalPolicy, GlobalRollingSpendState, IcpSourceAttemptError,
    IcpSourceReserveError, IcpSourceReserveState, InitArgs, InitArgsError, PendingIcpSourceDebit,
    PendingReservation, PendingSourceDebit, ProposalRecord, ProposalStatus, ReservedPrincipalKind,
    Sample, SelfRecoveryState, SourceAttemptError, SourceReserveError, SourceReserveState,
    TargetRecord, TargetReservationState, TerminalFundingSummary, ValidatedInitArgs,
};

type VMem = VirtualMemory<DefaultMemoryImpl>;

/// `types::MAX_SAMPLES_PER_TARGET` as `u32`, for ring-slot arithmetic.
const MAX_SAMPLES_PER_TARGET_U32: u32 = types::MAX_SAMPLES_PER_TARGET as u32;

// ─────────────────────── Memory IDs (never reuse) ───────────────────────
//
// `MemoryId` (ic-stable-structures 0.6.x) has no public inner accessor (only
// `MemoryId::new`), so `MEMORY_LAYOUT` stores the constructed `MemoryId`
// values directly (it derives `Ord`/`Eq`, which is enough for the
// `memory_ids_unique` test below via a `BTreeSet`). Each constant is
// declared exactly once here and referenced by both the `thread_local!`
// initializers and `MEMORY_LAYOUT` — never inline a second `MemoryId::new(n)`
// literal anywhere else in this file.

const MEM_GLOBAL_CONFIG: MemoryId = MemoryId::new(0); // StableCell<StoredGlobalConfig>
const MEM_TARGET_REGISTRY: MemoryId = MemoryId::new(1); // StableBTreeMap<StorablePrincipal, StoredTargetRecord>
const MEM_PROPOSALS: MemoryId = MemoryId::new(2); // StableBTreeMap<u64, StoredProposalRecord>
const MEM_GOVERNANCE_COUNTERS: MemoryId = MemoryId::new(3); // StableCell<StoredGovernanceCounters>
const MEM_SAMPLES: MemoryId = MemoryId::new(4); // StableBTreeMap<SampleKey, StoredSample>
const MEM_SAMPLE_META: MemoryId = MemoryId::new(5); // StableBTreeMap<StorablePrincipal, StoredSampleMeta>
const MEM_ALARMS: MemoryId = MemoryId::new(6); // StableBTreeMap<u64, StoredAlarm>
const MEM_ALARM_COUNTERS: MemoryId = MemoryId::new(7); // StableCell<StoredAlarmCounters>
const MEM_FUNDING_OPERATIONS: MemoryId = MemoryId::new(8); // StableBTreeMap<u64, StoredFundingOperation>
const MEM_FUNDING_COUNTERS: MemoryId = MemoryId::new(9); // StableCell<StoredFundingCounters>
const MEM_TARGET_RESERVATIONS: MemoryId = MemoryId::new(10); // StableBTreeMap<StorablePrincipal, StoredTargetReservationState>
const MEM_GLOBAL_ROLLING_SPEND: MemoryId = MemoryId::new(11); // StableCell<StoredGlobalRollingSpendState> (singleton)
const MEM_SELF_RECOVERY: MemoryId = MemoryId::new(12); // StableCell<StoredSelfRecoveryState> (singleton)
const MEM_TERMINAL_SUMMARIES: MemoryId = MemoryId::new(13); // StableBTreeMap<u64, StoredTerminalFundingSummary>, keyed by operation_id
const MEM_SOURCE_RESERVE: MemoryId = MemoryId::new(14); // StableCell<StoredSourceReserveState> (singleton)
const MEM_SOURCE_REFRESH_GENERATION: MemoryId = MemoryId::new(15); // StableCell<StoredSourceRefreshGeneration> (singleton)
const MEM_ICP_SOURCE_RESERVE: MemoryId = MemoryId::new(16); // StableCell<StoredIcpSourceReserveState> (singleton)

/// Every stable memory slot this canister owns, paired with a human label.
/// Single source of truth for the layout; iterated by `memory_ids_unique`.
const MEMORY_LAYOUT: &[(MemoryId, &str)] = &[
    (MEM_GLOBAL_CONFIG, "global_config"),
    (MEM_TARGET_REGISTRY, "target_registry"),
    (MEM_PROPOSALS, "proposals"),
    (MEM_GOVERNANCE_COUNTERS, "governance_counters"),
    (MEM_SAMPLES, "samples"),
    (MEM_SAMPLE_META, "sample_meta"),
    (MEM_ALARMS, "alarms"),
    (MEM_ALARM_COUNTERS, "alarm_counters"),
    (MEM_FUNDING_OPERATIONS, "funding_operations"),
    (MEM_FUNDING_COUNTERS, "funding_counters"),
    (MEM_TARGET_RESERVATIONS, "target_reservations"),
    (MEM_GLOBAL_ROLLING_SPEND, "global_rolling_spend"),
    (MEM_SELF_RECOVERY, "self_recovery"),
    (MEM_TERMINAL_SUMMARIES, "terminal_summaries"),
    (MEM_SOURCE_RESERVE, "source_reserve"),
    (MEM_SOURCE_REFRESH_GENERATION, "source_refresh_generation"),
    (MEM_ICP_SOURCE_RESERVE, "icp_source_reserve"),
];

// ─────────────────────── state.rs-owned bookkeeping types ───────────────────────

/// Memory ID 0. `None` means "canister installed but `init()` has not run
/// yet" — every accessor traps loudly on `None` rather than fabricating a
/// policy (this repo's "trap, don't silently default" convention:
/// `liquidation_bot` UPG-001, `rumi_treasury::with_state`'s
/// `.expect(...)`). Signers/threshold are mutable here — deliberately NOT
/// re-using `types::ValidatedInitArgs` (which has no setters by design) —
/// governance (a later task) mutates this struct field-by-field through
/// `AddSigner`/`RemoveSigner`/`SetSignerThreshold`/`SetGlobalPolicy`-style
/// proposals, each ending in a single whole-struct `set_global_config`
/// replace. Fields are `pub(crate)` so a later governance module (same
/// crate) can read the current config and construct a replacement.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub(crate) struct GlobalConfig {
    pub(crate) signers: Vec<Principal>,
    pub(crate) approval_threshold: u32,
    pub(crate) global_policy: GlobalPolicy,
}

/// Memory ID 3.
#[derive(CandidType, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
struct GovernanceCounters {
    next_proposal_id: u64,
}

/// Memory ID 5, one entry per registered (or previously-registered) target.
/// Backs the sample ring at memory ID 4. `filled_slots` distinguishes "ring
/// not yet full — scan `0..filled_slots`" from "ring full and wrapping —
/// every slot `0..MAX_SAMPLES_PER_TARGET` is populated" (once `filled_slots`
/// reaches the max it saturates there forever, so this single field is
/// sufficient in both phases). Returned by `sample_meta` to later tasks, so
/// fields are `pub(crate)`.
///
/// `total_writes` is the monotonic, never-decreasing, all-time count of
/// `record_sample` calls for this target — the logical write ordinal
/// (final-correction Finding 2 / item 5). It is the source of truth for
/// `list_samples`'s cursor: `next_slot`/`filled_slots` alone cannot
/// distinguish "physical slot S currently holds write N" from "physical
/// slot S currently holds write N + k*MAX_SAMPLES_PER_TARGET" once the ring
/// has wrapped more than once, which is exactly what let a raw physical-slot
/// cursor alias to the wrong sample after recycling. `total_writes` fully
/// determines `filled_slots` (`= total_writes.min(MAX_SAMPLES_PER_TARGET)`)
/// and `next_slot` (`= total_writes % MAX_SAMPLES_PER_TARGET`); both fields
/// are kept as independently-maintained state (rather than computed on every
/// read) to minimize the diff against the pre-existing ring-write logic, and
/// `validate_whole_state`'s `ImpossibleSampleMeta` check re-derives and
/// cross-checks both from `total_writes` as a whole-state backstop.
#[derive(CandidType, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SampleMeta {
    pub(crate) next_slot: u32,
    pub(crate) filled_slots: u32,
    pub(crate) last_success_at_secs: Option<u64>,
    pub(crate) last_attempt_at_secs: Option<u64>,
    pub(crate) total_writes: u64,
}

/// Memory ID 7.
#[derive(CandidType, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
struct AlarmCounters {
    next_alarm_id: u64,
}

/// Memory ID 9. `last_created_at_time_ns` is the design's globally monotonic
/// `created_at_time` for OUTGOING LEDGER CALLS (nanosecond scale — feeds
/// `CyclesWithdrawSnapshot::created_at_time_ns` /
/// `IcpCmcSnapshot::created_at_time_ns`). It is NOT the same clock as
/// `FundingOperation::created_at_secs` (second-scale bookkeeping timestamp,
/// derived locally from `ic_cdk::api::time()` at the operation's `open()`
/// call site) — these two clocks answer different questions and must not be
/// conflated by later tasks.
#[derive(CandidType, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
struct FundingCounters {
    next_operation_id: u64,
    last_created_at_time_ns: u64,
}

// ─────────────────────── Versioned envelope wrappers ───────────────────────
//
// One `Stored*` enum per stable-memory value class: 9 wrap types already
// defined in `types.rs` (already `CandidType + Deserialize` there, because
// they are also Candid method payloads/return values elsewhere in the
// design), 5 wrap the bookkeeping types this file owns above. Every
// `Stored*` enum is `CandidType + Deserialize + Clone` and gets a
// `Storable` impl via `impl_candid_storable!` (Candid encode/decode,
// `Bound::Unbounded` — see the module doc for why unbounded, not a
// hand-picked byte ceiling: `types.rs`'s own constructors already bound
// entry *content*, so stable-memory growth is bounded by entry *count*,
// which this file enforces at insert time).

#[derive(CandidType, Deserialize, Clone)]
enum StoredGlobalConfig {
    V1(Option<GlobalConfig>),
}

impl StoredGlobalConfig {
    fn into_current(self) -> Option<GlobalConfig> {
        match self {
            Self::V1(v) => v,
        }
    }
}

#[derive(CandidType, Deserialize, Clone)]
enum StoredGovernanceCounters {
    V1(GovernanceCounters),
}

impl StoredGovernanceCounters {
    fn into_current(self) -> GovernanceCounters {
        match self {
            Self::V1(v) => v,
        }
    }
}

/// Frozen shape of `SampleMeta` from before it carried `total_writes` (Task
/// 1c final-correction pass, item 5). Never edited again — see the module
/// doc's versioned-envelope recipe.
#[derive(CandidType, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
struct SampleMetaV1 {
    next_slot: u32,
    filled_slots: u32,
    last_success_at_secs: Option<u64>,
    last_attempt_at_secs: Option<u64>,
}

/// Legacy V1 bytes predate `total_writes` entirely and, once `filled_slots`
/// has reached the ring's capacity, carry no way to recover how many times
/// the ring actually wrapped before this migration ever runs. Migration
/// deterministically and conservatively assumes NO wrap occurred —
/// `total_writes = filled_slots` — the same fail-safe-by-undercounting
/// choice `SelfRecoveryState::from_legacy` already makes for its own
/// unrecoverable field (see that migration's doc comment). This is safe
/// because the canister has never been deployed: no real V1 bytes with an
/// actual wrapped ring exist anywhere, so this assumption is never actually
/// exercised against live data — but the migration is still deterministic
/// and documented for the hypothetical case it ever were.
impl From<SampleMetaV1> for SampleMeta {
    fn from(legacy: SampleMetaV1) -> Self {
        SampleMeta {
            next_slot: legacy.next_slot,
            filled_slots: legacy.filled_slots,
            last_success_at_secs: legacy.last_success_at_secs,
            last_attempt_at_secs: legacy.last_attempt_at_secs,
            total_writes: legacy.filled_slots as u64,
        }
    }
}

#[derive(CandidType, Deserialize, Clone)]
enum StoredSampleMeta {
    V1(SampleMetaV1),
    V2(SampleMeta),
}

impl StoredSampleMeta {
    fn into_current(self) -> SampleMeta {
        match self {
            Self::V1(v) => v.into(),
            Self::V2(v) => v,
        }
    }
}

#[derive(CandidType, Deserialize, Clone)]
enum StoredAlarmCounters {
    V1(AlarmCounters),
}

impl StoredAlarmCounters {
    fn into_current(self) -> AlarmCounters {
        match self {
            Self::V1(v) => v,
        }
    }
}

#[derive(CandidType, Deserialize, Clone)]
enum StoredFundingCounters {
    V1(FundingCounters),
}

impl StoredFundingCounters {
    fn into_current(self) -> FundingCounters {
        match self {
            Self::V1(v) => v,
        }
    }
}

#[derive(CandidType, Deserialize, Clone)]
enum StoredTargetRecord {
    V1(TargetRecord),
}

impl StoredTargetRecord {
    fn into_current(self) -> TargetRecord {
        match self {
            Self::V1(v) => v,
        }
    }
}

#[derive(CandidType, Deserialize, Clone)]
enum StoredProposalRecord {
    V1(ProposalRecord),
}

impl StoredProposalRecord {
    fn into_current(self) -> ProposalRecord {
        match self {
            Self::V1(v) => v,
        }
    }
}

#[derive(CandidType, Deserialize, Clone)]
struct SampleV1 {
    timestamp_secs: u64,
    balance: types::AdvisoryCyclesBalance,
    state: types::PublicTargetState,
    burn_cycles_per_hour: Option<u128>,
}

impl From<SampleV1> for Sample {
    fn from(value: SampleV1) -> Self {
        Self {
            timestamp_secs: value.timestamp_secs,
            balance: Some(value.balance),
            state: value.state,
            reported_operational_healthy: None,
            burn_cycles_per_hour: value.burn_cycles_per_hour,
        }
    }
}

#[derive(CandidType, Deserialize, Clone)]
enum StoredSample {
    V1(SampleV1),
    V2(Sample),
}

impl StoredSample {
    fn into_current(self) -> Sample {
        match self {
            Self::V1(v) => v.into(),
            Self::V2(v) => v,
        }
    }
}

#[derive(CandidType, Deserialize, Clone)]
enum StoredAlarm {
    V1(Alarm),
}

impl StoredAlarm {
    fn into_current(self) -> Alarm {
        match self {
            Self::V1(v) => v,
        }
    }
}

#[derive(CandidType, Deserialize, Clone)]
enum StoredFundingOperation {
    V1(FundingOperationV1),
    V2(FundingOperationV2),
    V3(FundingOperation),
}

impl StoredFundingOperation {
    fn into_current(self) -> FundingOperation {
        match self {
            Self::V1(v) => v.into(),
            Self::V2(v) => v.into(),
            Self::V3(v) => v,
        }
    }
}

#[derive(CandidType, Deserialize, Clone)]
enum StoredTargetReservationState {
    V1(TargetReservationState),
}

impl StoredTargetReservationState {
    fn into_current(self) -> TargetReservationState {
        match self {
            Self::V1(v) => v,
        }
    }
}

#[derive(CandidType, Deserialize, Clone)]
enum StoredGlobalRollingSpendState {
    V1(GlobalRollingSpendState),
}

impl StoredGlobalRollingSpendState {
    fn into_current(self) -> GlobalRollingSpendState {
        match self {
            Self::V1(v) => v,
        }
    }
}

/// Frozen shape of `SelfRecoveryState` from before it carried its own
/// `RollingSpendLedger` (Task 1c accounting-correction pass). Per the
/// module doc's versioned-envelope recipe: this struct is never edited
/// again — it exists solely so old `V1` stable bytes keep decoding.
#[derive(CandidType, Deserialize, Clone)]
struct SelfRecoveryStateV1 {
    in_flight_operation_id: Option<u64>,
    last_recovery_at_secs: Option<u64>,
}

/// Legacy V1 bytes predate the rolling-spend ledger entirely and carry no
/// reserved-amount data for a possibly-in-flight operation, so migration
/// always starts from an empty ledger — see `SelfRecoveryState::from_legacy`
/// for why this is the deterministic, fail-safe choice rather than a guess.
impl From<SelfRecoveryStateV1> for SelfRecoveryState {
    fn from(legacy: SelfRecoveryStateV1) -> Self {
        SelfRecoveryState::from_legacy(legacy.last_recovery_at_secs)
    }
}

#[derive(CandidType, Deserialize, Clone)]
enum StoredSelfRecoveryState {
    V1(SelfRecoveryStateV1),
    V2(SelfRecoveryState),
}

impl StoredSelfRecoveryState {
    fn into_current(self) -> SelfRecoveryState {
        match self {
            Self::V1(v) => v.into(),
            Self::V2(v) => v,
        }
    }
}

#[derive(CandidType, Deserialize, Clone)]
enum StoredTerminalFundingSummary {
    V1(TerminalFundingSummary),
}

impl StoredTerminalFundingSummary {
    fn into_current(self) -> TerminalFundingSummary {
        match self {
            Self::V1(v) => v,
        }
    }
}

/// Memory ID 14 (Task 4).
#[derive(CandidType, Deserialize, Clone)]
enum StoredSourceReserveState {
    V1(SourceReserveState),
}

impl StoredSourceReserveState {
    fn into_current(self) -> SourceReserveState {
        match self {
            Self::V1(v) => v,
        }
    }
}

/// Memory ID 16.  This is intentionally a separate stable cell from the
/// Cycles Ledger source reserve: ICP e8s and T-cycles have independent
/// balances, fees, and settlement semantics.  V1 is the first persisted
/// shape for this new memory region; future fields must use V2 migration
/// rather than changing it in place.
#[derive(CandidType, Deserialize, Clone)]
enum StoredIcpSourceReserveState {
    V1(IcpSourceReserveState),
}

impl StoredIcpSourceReserveState {
    fn into_current(self) -> IcpSourceReserveState {
        match self {
            Self::V1(v) => v,
        }
    }
}

/// Memory ID 15. A durable generation for the Cycles Ledger source/cache
/// refresh transaction. It advances on every reservation, attempt,
/// settlement, or source-cache write, so a balance sampled before an await
/// cannot overwrite state that changed while the query was in flight.
#[derive(CandidType, Deserialize, Clone, Copy, Default)]
struct SourceRefreshGeneration {
    value: u64,
}

#[derive(CandidType, Deserialize, Clone)]
enum StoredSourceRefreshGeneration {
    V1(SourceRefreshGeneration),
}

impl StoredSourceRefreshGeneration {
    fn value(&self) -> u64 {
        match self {
            Self::V1(value) => value.value,
        }
    }
}

/// Candid-encoded `Storable` impl. `Bound::Unbounded` per the module doc:
/// the domain-level bound on entry content already lives in `types.rs`'s
/// own constructors, so a hand-picked `max_size` here would only duplicate
/// (and risk drifting from) a ceiling that file already owns.
macro_rules! impl_candid_storable {
    ($t:ty) => {
        impl Storable for $t {
            fn to_bytes(&self) -> Cow<'_, [u8]> {
                Cow::Owned(
                    candid::encode_one(self)
                        .expect(concat!("failed to Candid-encode ", stringify!($t))),
                )
            }
            fn from_bytes(bytes: Cow<'_, [u8]>) -> Self {
                candid::decode_one(bytes.as_ref())
                    .expect(concat!("failed to Candid-decode ", stringify!($t)))
            }
            const BOUND: Bound = Bound::Unbounded;
        }
    };
}

impl_candid_storable!(StoredGlobalConfig);
impl_candid_storable!(StoredGovernanceCounters);
impl_candid_storable!(StoredSampleMeta);
impl_candid_storable!(StoredAlarmCounters);
impl_candid_storable!(StoredFundingCounters);
impl_candid_storable!(StoredTargetRecord);
impl_candid_storable!(StoredProposalRecord);
impl_candid_storable!(StoredSample);
impl_candid_storable!(StoredAlarm);
impl_candid_storable!(StoredFundingOperation);
impl_candid_storable!(StoredTargetReservationState);
impl_candid_storable!(StoredGlobalRollingSpendState);
impl_candid_storable!(StoredSelfRecoveryState);
impl_candid_storable!(StoredTerminalFundingSummary);
impl_candid_storable!(StoredSourceReserveState);
impl_candid_storable!(StoredIcpSourceReserveState);
impl_candid_storable!(StoredSourceRefreshGeneration);

// ─────────────────────── Key types ───────────────────────

/// `StableBTreeMap` key wrapper for every store keyed directly by a target
/// principal (memory IDs 1, 5, 10). `ic-stable-structures` 0.6.x has no
/// built-in `Storable` impl for `Principal`. A principal is at most 29
/// bytes (opaque/self-authenticating ids; anonymous is 1 byte); bounded
/// (not fixed) so short principals don't waste space. Identical to
/// `rumi_points::StorablePrincipal` — same crate pin, same reasoning.
#[derive(Clone, Ord, PartialOrd, Eq, PartialEq, Debug)]
struct StorablePrincipal(Principal);

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

/// Memory ID 4 key: (target principal, ring slot). Fixed-size, 34 bytes:
/// a 29-byte zero-padded principal, one length byte, then the slot as a
/// 4-byte big-endian `u32`.
///
/// Two problems a naive "pad the principal to 29 bytes, concatenate the
/// slot" encoding would have, both resolved by the explicit length byte:
///
/// 1. **Ordering / range-scan scoping.** `StableBTreeMap` orders keys by
///    raw byte comparison of `to_bytes()`. Two *different* principals whose
///    raw bytes are a prefix of one another after zero-padding to 29 bytes
///    (e.g. anonymous `[0x04]` padded is byte-identical to a 2-byte
///    principal `[0x04, 0x00, ...]` padded) would otherwise produce
///    identical 29-byte prefixes, making a per-target range scan
///    (`range_start(p)..=range_end(p)`) return another target's rows. The
///    length byte, appended right after the padded principal and before
///    the slot, disambiguates: two principals with identical padded bytes
///    necessarily have different raw lengths, so their length bytes differ,
///    which keeps their key ranges non-overlapping.
/// 2. **Lossless decode.** Reconstructing a `Principal` from the padded
///    bytes by "trim trailing zero bytes" is lossy for a principal that
///    itself legitimately ends in one or more zero bytes. Storing the exact
///    unpadded length alongside removes the ambiguity: `from_bytes` slices
///    exactly `bytes[0..length]`, no trimming heuristic involved.
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
struct SampleKey {
    principal: Principal,
    slot: u32,
}

impl SampleKey {
    const PRINCIPAL_PAD_LEN: usize = 29;

    fn padded_principal(p: &Principal) -> [u8; Self::PRINCIPAL_PAD_LEN] {
        let raw = p.as_slice();
        let mut buf = [0u8; Self::PRINCIPAL_PAD_LEN];
        buf[..raw.len()].copy_from_slice(raw);
        buf
    }

    fn range_start(principal: Principal) -> Self {
        Self { principal, slot: 0 }
    }

    fn range_end(principal: Principal) -> Self {
        Self {
            principal,
            slot: u32::MAX,
        }
    }
}

impl Storable for SampleKey {
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        let mut buf = Vec::with_capacity(34);
        buf.extend_from_slice(&Self::padded_principal(&self.principal));
        buf.push(self.principal.as_slice().len() as u8);
        buf.extend_from_slice(&self.slot.to_be_bytes());
        Cow::Owned(buf)
    }

    fn from_bytes(bytes: Cow<'_, [u8]>) -> Self {
        let b = bytes.as_ref();
        let len = b[29] as usize;
        let principal = Principal::from_slice(&b[0..len]);
        let slot = u32::from_be_bytes([b[30], b[31], b[32], b[33]]);
        Self { principal, slot }
    }

    const BOUND: Bound = Bound::Bounded {
        max_size: 34,
        is_fixed_size: true,
    };
}

/// `SampleKey`'s `Ord` MUST agree with the lexicographic byte order of its
/// own `to_bytes()` — `StableBTreeMap` relies on `K: Ord` for in-memory
/// comparisons and persists entries in `to_bytes()` byte order, so any
/// mismatch between the two would corrupt iteration/range-scan order. A
/// derived field-order `Ord` (comparing `principal` via `Principal`'s own
/// `Ord` first) would NOT reliably agree with the padded/length-prefixed
/// byte encoding above — the same prefix-collision hazard `to_bytes()`
/// itself resolves. Deriving `Ord` from `to_bytes()` directly guarantees
/// agreement by construction.
impl Ord for SampleKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.to_bytes().cmp(&other.to_bytes())
    }
}

impl PartialOrd for SampleKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

// ─────────────────────── Thread-local stable structures ───────────────────────
//
// One `RefCell<Stable*>` per store, eagerly initialized. No `Option<State>`
// heap mirror anywhere in this file: every value is read directly from its
// `StableCell`/`StableBTreeMap` on demand, and none of these reads are
// hot-loop-critical the way a per-transaction balance lookup would be.

thread_local! {
    static MEMORY_MANAGER: RefCell<MemoryManager<DefaultMemoryImpl>> =
        RefCell::new(MemoryManager::init(DefaultMemoryImpl::default()));

    static GLOBAL_CONFIG: RefCell<StableCell<StoredGlobalConfig, VMem>> =
        MEMORY_MANAGER.with(|m| RefCell::new(
            StableCell::init(m.borrow().get(MEM_GLOBAL_CONFIG), StoredGlobalConfig::V1(None))
                .expect("rumi_cycle_sentinel: failed to init global config cell")
        ));

    static TARGET_REGISTRY: RefCell<StableBTreeMap<StorablePrincipal, StoredTargetRecord, VMem>> =
        MEMORY_MANAGER.with(|m| RefCell::new(StableBTreeMap::init(m.borrow().get(MEM_TARGET_REGISTRY))));

    static PROPOSALS: RefCell<StableBTreeMap<u64, StoredProposalRecord, VMem>> =
        MEMORY_MANAGER.with(|m| RefCell::new(StableBTreeMap::init(m.borrow().get(MEM_PROPOSALS))));

    static GOVERNANCE_COUNTERS: RefCell<StableCell<StoredGovernanceCounters, VMem>> =
        MEMORY_MANAGER.with(|m| RefCell::new(
            StableCell::init(m.borrow().get(MEM_GOVERNANCE_COUNTERS), StoredGovernanceCounters::V1(GovernanceCounters::default()))
                .expect("rumi_cycle_sentinel: failed to init governance counters cell")
        ));

    static SAMPLES: RefCell<StableBTreeMap<SampleKey, StoredSample, VMem>> =
        MEMORY_MANAGER.with(|m| RefCell::new(StableBTreeMap::init(m.borrow().get(MEM_SAMPLES))));

    static SAMPLE_META: RefCell<StableBTreeMap<StorablePrincipal, StoredSampleMeta, VMem>> =
        MEMORY_MANAGER.with(|m| RefCell::new(StableBTreeMap::init(m.borrow().get(MEM_SAMPLE_META))));

    static ALARMS: RefCell<StableBTreeMap<u64, StoredAlarm, VMem>> =
        MEMORY_MANAGER.with(|m| RefCell::new(StableBTreeMap::init(m.borrow().get(MEM_ALARMS))));

    static ALARM_COUNTERS: RefCell<StableCell<StoredAlarmCounters, VMem>> =
        MEMORY_MANAGER.with(|m| RefCell::new(
            StableCell::init(m.borrow().get(MEM_ALARM_COUNTERS), StoredAlarmCounters::V1(AlarmCounters::default()))
                .expect("rumi_cycle_sentinel: failed to init alarm counters cell")
        ));

    static FUNDING_OPERATIONS: RefCell<StableBTreeMap<u64, StoredFundingOperation, VMem>> =
        MEMORY_MANAGER.with(|m| RefCell::new(StableBTreeMap::init(m.borrow().get(MEM_FUNDING_OPERATIONS))));

    static FUNDING_COUNTERS: RefCell<StableCell<StoredFundingCounters, VMem>> =
        MEMORY_MANAGER.with(|m| RefCell::new(
            StableCell::init(m.borrow().get(MEM_FUNDING_COUNTERS), StoredFundingCounters::V1(FundingCounters::default()))
                .expect("rumi_cycle_sentinel: failed to init funding counters cell")
        ));

    static TARGET_RESERVATIONS: RefCell<StableBTreeMap<StorablePrincipal, StoredTargetReservationState, VMem>> =
        MEMORY_MANAGER.with(|m| RefCell::new(StableBTreeMap::init(m.borrow().get(MEM_TARGET_RESERVATIONS))));

    static GLOBAL_ROLLING_SPEND: RefCell<StableCell<StoredGlobalRollingSpendState, VMem>> =
        MEMORY_MANAGER.with(|m| RefCell::new(
            StableCell::init(m.borrow().get(MEM_GLOBAL_ROLLING_SPEND), StoredGlobalRollingSpendState::V1(GlobalRollingSpendState::new()))
                .expect("rumi_cycle_sentinel: failed to init global rolling spend cell")
        ));

    static SELF_RECOVERY: RefCell<StableCell<StoredSelfRecoveryState, VMem>> =
        MEMORY_MANAGER.with(|m| RefCell::new(
            StableCell::init(m.borrow().get(MEM_SELF_RECOVERY), StoredSelfRecoveryState::V2(SelfRecoveryState::new()))
                .expect("rumi_cycle_sentinel: failed to init self-recovery cell")
        ));

    static TERMINAL_SUMMARIES: RefCell<StableBTreeMap<u64, StoredTerminalFundingSummary, VMem>> =
        MEMORY_MANAGER.with(|m| RefCell::new(StableBTreeMap::init(m.borrow().get(MEM_TERMINAL_SUMMARIES))));

    static SOURCE_RESERVE: RefCell<StableCell<StoredSourceReserveState, VMem>> =
        MEMORY_MANAGER.with(|m| RefCell::new(
            StableCell::init(m.borrow().get(MEM_SOURCE_RESERVE), StoredSourceReserveState::V1(SourceReserveState::new()))
                .expect("rumi_cycle_sentinel: failed to init source reserve cell")
        ));

    static SOURCE_REFRESH_GENERATION: RefCell<StableCell<StoredSourceRefreshGeneration, VMem>> =
        MEMORY_MANAGER.with(|m| RefCell::new(
            StableCell::init(
                m.borrow().get(MEM_SOURCE_REFRESH_GENERATION),
                StoredSourceRefreshGeneration::V1(SourceRefreshGeneration::default()),
            )
            .expect("rumi_cycle_sentinel: failed to init source refresh generation cell")
        ));

    static ICP_SOURCE_RESERVE: RefCell<StableCell<StoredIcpSourceReserveState, VMem>> =
        MEMORY_MANAGER.with(|m| RefCell::new(
            StableCell::init(
                m.borrow().get(MEM_ICP_SOURCE_RESERVE),
                StoredIcpSourceReserveState::V1(IcpSourceReserveState::new()),
            )
            .expect("rumi_cycle_sentinel: failed to init ICP source reserve cell")
        ));
}

// ─────────────────────── init / global config ───────────────────────

/// Validates `args` and installs the global config exactly once. Traps
/// (via `assert!`) if called a second time — `init()` is a canister
/// lifecycle event, not a mutation path; re-init would silently discard a
/// live signer/policy configuration.
pub(crate) fn init(args: InitArgs) -> Result<(), InitArgsError> {
    let validated = ValidatedInitArgs::validate(args)?;
    GLOBAL_CONFIG.with(|cell| {
        let mut cell = cell.borrow_mut();
        let current = cell.get().clone().into_current();
        assert!(
            current.is_none(),
            "rumi_cycle_sentinel: init() called twice — global config already set"
        );
        let config = GlobalConfig {
            signers: validated.signers().to_vec(),
            approval_threshold: validated.approval_threshold(),
            global_policy: validated.global_policy().clone(),
        };
        cell.set(StoredGlobalConfig::V1(Some(config)))
            .expect("rumi_cycle_sentinel: global config fits one StableCell page");
    });
    Ok(())
}

/// Traps if read before `init()` has run — matches this repo's established
/// "trap, don't silently default" convention.
pub(crate) fn global_config() -> GlobalConfig {
    GLOBAL_CONFIG.with(|c| {
        c.borrow()
            .get()
            .clone()
            .into_current()
            .expect("rumi_cycle_sentinel: global config read before init()")
    })
}

/// Whole-cell replace: every governed mutation (signer add/remove,
/// threshold change, policy change — a later task's job) replaces the
/// entire validated `GlobalConfig` at once rather than patching individual
/// bytes.
pub(crate) fn set_global_config(config: GlobalConfig) {
    GLOBAL_CONFIG.with(|cell| {
        cell.borrow_mut()
            .set(StoredGlobalConfig::V1(Some(config)))
            .expect("rumi_cycle_sentinel: failed to write global config cell");
    });
}

pub(crate) fn is_signer(principal: Principal) -> bool {
    global_config().signers.contains(&principal)
}

// ─────────────────────── Target registry ───────────────────────

pub(crate) fn get_target(principal: Principal) -> Option<TargetRecord> {
    TARGET_REGISTRY.with(|m| {
        m.borrow()
            .get(&StorablePrincipal(principal))
            .map(|v| v.into_current())
    })
}

/// Internal fail-safe pause used by anomaly detection. It is intentionally
/// separate from the signer-gated governance pause endpoint and only changes
/// the target's pause bit; unpausing remains governed.
pub(crate) fn pause_target_for_anomaly(principal: Principal) -> bool {
    let Some(target) = get_target(principal) else {
        return false;
    };
    if target.paused() {
        return false;
    }
    insert_target(target.pause()).is_ok()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InsertTargetError {
    /// The registry is at `types::MAX_TARGETS` and `record`'s principal is
    /// not already a key — unlike alarms/proposals, a target has no
    /// "resolved/non-open" analog to safely evict, so the only self-enforcing
    /// option at the bound is to fail closed. Overwriting an EXISTING
    /// principal (an ordinary `apply_patch`/`pause`/`unpause` update) is
    /// always allowed regardless of how full the registry is, since it never
    /// grows the map.
    TooManyTargets,
}

/// Self-enforcing at the storage boundary: a genuinely new principal is
/// rejected once the registry is at `types::MAX_TARGETS`, so the whole-state
/// `TooManyTargets` check in `validate_whole_state` is a backstop for state
/// that reached the map some other way (e.g. a restored legacy snapshot),
/// never something this path can itself produce going forward.
pub(crate) fn insert_target(record: TargetRecord) -> Result<(), InsertTargetError> {
    let key = StorablePrincipal(record.principal());
    TARGET_REGISTRY.with(|m| {
        let mut map = m.borrow_mut();
        let is_new = map.get(&key).is_none();
        if is_new && map.len() as usize >= types::MAX_TARGETS {
            return Err(InsertTargetError::TooManyTargets);
        }
        map.insert(key, StoredTargetRecord::V1(record));
        Ok(())
    })
}

#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoveTargetError {
    /// An unresolved `FundingOperation` still targets this principal — its
    /// exact recipient/policy snapshot must remain reachable until the
    /// operation resolves (design: "editing or removing a target ... cannot
    /// alter an in-flight operation").
    UnresolvedOperationExists,
    /// `TARGET_RESERVATIONS` still holds a pending (in-flight) reservation
    /// for this principal.
    PendingReservationExists,
}

/// Fails closed while any unresolved operation or pending reservation exists
/// for `principal`. On success, cascades the delete to every store keyed by
/// this principal alone (`SAMPLE_META`, every `SAMPLES` ring slot,
/// `TARGET_RESERVATIONS`) so a register -> observe -> remove -> re-register
/// cycle can never leave a permanently orphaned entry (storage review
/// Finding 3). All removals happen synchronously in this one call — there is
/// no await boundary to interleave with.
pub(crate) fn remove_target(
    principal: Principal,
) -> Result<Option<TargetRecord>, RemoveTargetError> {
    let has_unresolved_operation = FUNDING_OPERATIONS.with(|m| {
        m.borrow().iter().any(|(_, v)| {
            let op = v.into_current();
            op.target() == principal && !op.state().is_resolved()
        })
    });
    if has_unresolved_operation {
        return Err(RemoveTargetError::UnresolvedOperationExists);
    }
    if get_target_reservation(principal)
        .in_flight_operation_id()
        .is_some()
    {
        return Err(RemoveTargetError::PendingReservationExists);
    }

    let removed = TARGET_REGISTRY.with(|m| {
        m.borrow_mut()
            .remove(&StorablePrincipal(principal))
            .map(|v| v.into_current())
    });
    if removed.is_some() {
        SAMPLE_META.with(|m| {
            m.borrow_mut().remove(&StorablePrincipal(principal));
        });
        SAMPLES.with(|m| {
            let mut map = m.borrow_mut();
            let keys: Vec<SampleKey> = map
                .range((
                    std::ops::Bound::Included(SampleKey::range_start(principal)),
                    std::ops::Bound::Included(SampleKey::range_end(principal)),
                ))
                .map(|(k, _)| k)
                .collect();
            for key in keys {
                map.remove(&key);
            }
        });
        TARGET_RESERVATIONS.with(|m| {
            m.borrow_mut().remove(&StorablePrincipal(principal));
        });
    }
    Ok(removed)
}

pub(crate) fn target_count() -> u64 {
    TARGET_REGISTRY.with(|m| m.borrow().len())
}

pub(crate) fn target_principals() -> BTreeSet<Principal> {
    TARGET_REGISTRY.with(|m| m.borrow().iter().map(|(k, _)| k.0).collect())
}

pub(crate) fn list_targets_after(cursor: Option<Principal>, limit: usize) -> Vec<TargetRecord> {
    TARGET_REGISTRY.with(|m| {
        let map = m.borrow();
        let start = match cursor {
            Some(p) => std::ops::Bound::Excluded(StorablePrincipal(p)),
            None => std::ops::Bound::Unbounded,
        };
        map.range((start, std::ops::Bound::Unbounded))
            .take(limit)
            .map(|(_, v)| v.into_current())
            .collect()
    })
}

// ─────────────────────── Proposals ───────────────────────

pub(crate) fn next_proposal_id() -> u64 {
    GOVERNANCE_COUNTERS.with(|cell| {
        let mut cell = cell.borrow_mut();
        let mut counters = cell.get().clone().into_current();
        let id = counters.next_proposal_id;
        counters.next_proposal_id = id
            .checked_add(1)
            .unwrap_or_else(|| ic_cdk::trap("rumi_cycle_sentinel: proposal id counter overflow"));
        cell.set(StoredGovernanceCounters::V1(counters))
            .expect("rumi_cycle_sentinel: failed to write governance counters cell");
        id
    })
}

pub(crate) fn get_proposal(id: u64) -> Option<ProposalRecord> {
    PROPOSALS.with(|m| m.borrow().get(&id).map(|v| v.into_current()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InsertProposalError {
    /// `PROPOSALS` is at `types::MAX_PROPOSALS`, `record.id` is not already
    /// a key, and every existing proposal is still `Open` — there is no
    /// safely removable (resolved/non-open) victim, so the insert fails
    /// closed rather than overflowing the map.
    TooManyOpenProposals,
}

/// Self-enforcing at the storage boundary (storage review Finding 2b):
/// overwriting an EXISTING proposal id (approval bookkeeping, a status
/// transition) is always allowed regardless of bound, since it never grows
/// the map. A genuinely new id at the bound evicts the oldest non-`Open`
/// (i.e. `Executed`/`Cancelled`) proposal by id; if every proposal is still
/// `Open`, the insert fails closed instead of overflowing the map.
pub(crate) fn insert_proposal(record: ProposalRecord) -> Result<(), InsertProposalError> {
    let id = record.id;
    PROPOSALS.with(|m| {
        let mut map = m.borrow_mut();
        let is_new = map.get(&id).is_none();
        if is_new && map.len() as usize >= types::MAX_PROPOSALS {
            let victim = map
                .iter()
                .map(|(k, v)| (k, v.into_current()))
                .filter(|(_, record)| record.status != ProposalStatus::Open)
                .min_by_key(|(id, _)| *id)
                .map(|(id, _)| id);
            match victim {
                Some(victim_id) => {
                    map.remove(&victim_id);
                }
                None => return Err(InsertProposalError::TooManyOpenProposals),
            }
        }
        map.insert(id, StoredProposalRecord::V1(record));
        Ok(())
    })
}

pub(crate) fn list_proposals_after(cursor: Option<u64>, limit: usize) -> Vec<ProposalRecord> {
    PROPOSALS.with(|m| {
        let map = m.borrow();
        let start = match cursor {
            Some(id) => std::ops::Bound::Excluded(id),
            None => std::ops::Bound::Unbounded,
        };
        map.range((start, std::ops::Bound::Unbounded))
            .take(limit)
            .map(|(_, v)| v.into_current())
            .collect()
    })
}

pub(crate) fn proposal_count() -> u64 {
    PROPOSALS.with(|m| m.borrow().len())
}

// ─────────────────────── Alarms ───────────────────────

pub(crate) fn next_alarm_id() -> u64 {
    ALARM_COUNTERS.with(|cell| {
        let mut cell = cell.borrow_mut();
        let mut counters = cell.get().clone().into_current();
        let id = counters.next_alarm_id;
        counters.next_alarm_id = id
            .checked_add(1)
            .unwrap_or_else(|| ic_cdk::trap("rumi_cycle_sentinel: alarm id counter overflow"));
        cell.set(StoredAlarmCounters::V1(counters))
            .expect("rumi_cycle_sentinel: failed to write alarm counters cell");
        id
    })
}

pub(crate) fn get_alarm(id: u64) -> Option<Alarm> {
    ALARMS.with(|m| m.borrow().get(&id).map(|v| v.into_current()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InsertAlarmError {
    /// `ALARMS` is at `types::MAX_ALARMS`, `alarm.id` is not already a key,
    /// and no existing alarm is `Resolved` — `Open` and `Acknowledged` are
    /// both still-active statuses (see the `alarms` module doc's "only
    /// `Resolved` is terminal" contract) and neither is ever an eviction
    /// victim, so there is no safely removable row and the insert fails
    /// closed rather than overflowing the map. Automatic detection
    /// (burn anomaly, low balance, unreachable, ...) must treat this as a
    /// signal to resolve/acknowledge existing alarms, not a reason to trap
    /// at the next `post_upgrade`.
    NoResolvedAlarmToEvict,
}

/// Self-enforcing at the storage boundary (storage review Finding 2,
/// narrowed by the alarm review's Blocker 1): overwriting an EXISTING alarm
/// id (e.g. acknowledging or resolving it) is always allowed regardless of
/// bound, since it never grows the map. A genuinely new id at the bound
/// evicts the oldest (`opened_at_secs`) `Resolved` alarm — the only status
/// the `alarms` module treats as terminal; an `Acknowledged` alarm is still
/// active and is NEVER an eviction candidate, no matter how much older it is
/// than the oldest `Resolved` row. If no `Resolved` alarm exists (every
/// alarm is `Open` and/or `Acknowledged`), the insert fails closed instead
/// of overflowing the map — matching `insert_proposal`'s and
/// `insert_terminal_summary`'s self-evicting pattern rather than relying on
/// a `post_upgrade` trap to ever catch this.
pub(crate) fn insert_alarm(alarm: Alarm) -> Result<(), InsertAlarmError> {
    let id = alarm.id;
    ALARMS.with(|m| {
        let mut map = m.borrow_mut();
        let is_new = map.get(&id).is_none();
        if is_new && map.len() as usize >= types::MAX_ALARMS {
            let victim = map
                .iter()
                .map(|(k, v)| (k, v.into_current()))
                .filter(|(_, a)| a.status == types::AlarmStatus::Resolved)
                .min_by_key(|(_, a)| a.opened_at_secs)
                .map(|(k, _)| k);
            match victim {
                Some(victim_id) => {
                    map.remove(&victim_id);
                }
                None => return Err(InsertAlarmError::NoResolvedAlarmToEvict),
            }
        }
        map.insert(id, StoredAlarm::V1(alarm));
        Ok(())
    })
}

pub(crate) fn list_alarms_after(cursor: Option<u64>, limit: usize) -> Vec<Alarm> {
    ALARMS.with(|m| {
        let map = m.borrow();
        let start = match cursor {
            Some(id) => std::ops::Bound::Excluded(id),
            None => std::ops::Bound::Unbounded,
        };
        map.range((start, std::ops::Bound::Unbounded))
            .take(limit)
            .map(|(_, v)| v.into_current())
            .collect()
    })
}

/// Linear scan — `MAX_ALARMS` = 1,024, only called from the hourly timer
/// path (a later task), never a query.
pub(crate) fn find_open_alarm(target: Option<Principal>, kind: AlarmKind) -> Option<Alarm> {
    ALARMS.with(|m| {
        m.borrow()
            .iter()
            .map(|(_, v)| v.into_current())
            .find(|alarm| {
                alarm.target == target
                    && alarm.kind == kind
                    && alarm.status == types::AlarmStatus::Open
            })
    })
}

/// Same linear scan as [`find_open_alarm`], widened to `Acknowledged` too:
/// the underlying condition has not cleared yet even once a signer has seen
/// the alarm, so dedup (`alarms::raise_at`) and auto-resolution
/// (`alarms::resolve_at`) must both treat `Open` and `Acknowledged` alike as
/// "active". Only `Resolved` is terminal. Kept here, alongside
/// `find_open_alarm`, rather than inside the `alarms` submodule below, so
/// every direct `ALARMS` scan stays in one place.
pub(crate) fn find_active_alarm(target: Option<Principal>, kind: AlarmKind) -> Option<Alarm> {
    ALARMS.with(|m| {
        m.borrow()
            .iter()
            .map(|(_, v)| v.into_current())
            .find(|alarm| {
                alarm.target == target
                    && alarm.kind == kind
                    && alarm.status != types::AlarmStatus::Resolved
            })
    })
}

// ─────────────────────── Alarm lifecycle (Task 2B) ───────────────────────
//
// Layered on the raw storage functions above (`insert_alarm`, `get_alarm`,
// `find_open_alarm`/`find_active_alarm`), exactly like `governance.rs` is
// layered on the rest of this file: nothing here checks a caller's
// identity — `governance::acknowledge_alarm_at` is the only caller-gated
// entry point that reaches `acknowledge_at`, and it calls `require_signer`
// before it ever does.
//
// **Dedup.** `raise_at` is idempotent for a still-active condition: a
// second `raise_at` for the same `(target, kind)` while the first alarm is
// still `Open` or `Acknowledged` reuses that alarm's id and performs no
// write at all, so a condition re-detected every sample interval never
// grows `ALARMS`. Once an alarm has actually `Resolved`, a fresh
// `raise_at` for the same `(target, kind)` is treated as a NEW incident and
// always allocates a new id — the resolved alarm's own history
// (`resolved_at_secs`, etc.) is left untouched rather than reopened in
// place. This is a deliberate design choice, not a re-derivation of an
// existing contract: it keeps each `Alarm` row an honest, append-only
// record of one incident's lifecycle instead of a mutable ledger that could
// silently overwrite a past resolution's timestamp.
//
// **Acknowledge.** `acknowledge_at` is idempotent the same way
// `ProposalRecord::record_approval` is: acknowledging an already
// `Acknowledged` alarm is a no-op (`Ok(false)`, no write, original
// `acknowledged_at_secs` preserved) rather than an error. Acknowledging a
// `Resolved` alarm is rejected — the condition already cleared, so there is
// nothing left to acknowledge.
//
// **Auto-resolve.** `resolve_at` is exact (only the matching
// `(target, kind)` alarm is touched) and idempotent (no active alarm for
// that pair — already resolved, or never raised — is a silent `None`, never
// a trap or an error): auto-resolution racing itself (e.g. two consecutive
// clean samples both trying to clear the same alarm) must be safe.
pub(crate) mod alarms {
    use super::{find_active_alarm, get_alarm, insert_alarm, next_alarm_id, InsertAlarmError};
    use crate::types::{Alarm, AlarmKind, AlarmStatus};
    use candid::{CandidType, Principal};
    use serde::Deserialize;

    #[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum AlarmError {
        AlarmNotFound,
        /// The alarm's condition already cleared; there is nothing left to
        /// acknowledge.
        AlarmAlreadyResolved,
        /// `ALARMS` is at `types::MAX_ALARMS` and no existing alarm is
        /// `Resolved` — see `InsertAlarmError::NoResolvedAlarmToEvict`. Only
        /// `raise_at` can hit this: `acknowledge_at`/`resolve_at` only ever
        /// overwrite an existing id, which `insert_alarm` always allows
        /// regardless of the bound.
        NoResolvedAlarmToEvict,
    }

    impl From<InsertAlarmError> for AlarmError {
        fn from(_: InsertAlarmError) -> Self {
            AlarmError::NoResolvedAlarmToEvict
        }
    }

    /// See the module doc's "Dedup" section for the full reuse-vs-reraise
    /// contract.
    pub(crate) fn raise_at(
        target: Option<Principal>,
        kind: AlarmKind,
        now_secs: u64,
    ) -> Result<u64, AlarmError> {
        if let Some(existing) = find_active_alarm(target, kind) {
            return Ok(existing.id);
        }
        let id = next_alarm_id();
        let alarm = Alarm {
            id,
            target,
            kind,
            status: AlarmStatus::Open,
            opened_at_secs: now_secs,
            acknowledged_at_secs: None,
            resolved_at_secs: None,
        };
        insert_alarm(alarm)?;
        Ok(id)
    }

    /// See the module doc's "Acknowledge" section. Not caller-gated itself —
    /// `governance::acknowledge_alarm_at` is.
    pub(crate) fn acknowledge_at(id: u64, now_secs: u64) -> Result<bool, AlarmError> {
        let mut alarm = get_alarm(id).ok_or(AlarmError::AlarmNotFound)?;
        match alarm.status {
            AlarmStatus::Acknowledged => Ok(false),
            AlarmStatus::Resolved => Err(AlarmError::AlarmAlreadyResolved),
            AlarmStatus::Open => {
                alarm.status = AlarmStatus::Acknowledged;
                alarm.acknowledged_at_secs = Some(now_secs);
                insert_alarm(alarm).expect(
                    "rumi_cycle_sentinel: acknowledging an existing alarm id never exceeds MAX_ALARMS",
                );
                Ok(true)
            }
        }
    }

    /// See the module doc's "Auto-resolve" section. Internal only — no
    /// Candid method reaches this in this task; a later observation/funding
    /// task calls it once a condition is confirmed cleared.
    pub(crate) fn resolve_at(
        target: Option<Principal>,
        kind: AlarmKind,
        now_secs: u64,
    ) -> Option<u64> {
        let mut alarm = find_active_alarm(target, kind)?;
        alarm.status = AlarmStatus::Resolved;
        alarm.resolved_at_secs = Some(now_secs);
        let id = alarm.id;
        insert_alarm(alarm)
            .expect("rumi_cycle_sentinel: resolving an existing alarm id never exceeds MAX_ALARMS");
        Some(id)
    }
}

// ─────────────────────── Samples ───────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecordSampleError {
    /// `target` has no `TARGET_REGISTRY` entry — recording a sample for it
    /// would create an orphan `SAMPLE_META`/`SAMPLES` history with no
    /// registered owner and no way to ever reach it through
    /// `remove_target`'s cascade (storage review Finding 3).
    UnregisteredTarget,
    /// `meta.total_writes.checked_add(1)` would overflow `u64`. Fails closed
    /// BEFORE any write happens (final-correction item 5's "check sequence
    /// overflow before any write") — an unrecordable sample is strictly
    /// preferable to a logical sequence number that silently wraps and
    /// aliases, which is the exact failure class this whole rework exists to
    /// eliminate.
    SequenceOverflow,
}

/// Ring insert: read `SampleMeta` (default if absent), write the sample at
/// `meta.next_slot`, advance `next_slot` modulo the ring size, cap
/// `filled_slots` at the ring size, increment the monotonic `total_writes`
/// sequence counter, and update the attempt/success timestamps from the
/// sample's own `timestamp_secs`/`state`. Rejects an unregistered target so
/// an orphan history can never be created, and rejects (before any write)
/// a `total_writes` increment that would overflow `u64`.
pub(crate) fn record_sample(target: Principal, sample: Sample) -> Result<(), RecordSampleError> {
    if get_target(target).is_none() {
        return Err(RecordSampleError::UnregisteredTarget);
    }
    let mut meta = sample_meta(target).unwrap_or_default();
    let total_writes = meta
        .total_writes
        .checked_add(1)
        .ok_or(RecordSampleError::SequenceOverflow)?;
    let slot = meta.next_slot;
    SAMPLES.with(|m| {
        m.borrow_mut().insert(
            SampleKey {
                principal: target,
                slot,
            },
            StoredSample::V2(sample.clone()),
        );
    });
    meta.next_slot = (meta.next_slot + 1) % MAX_SAMPLES_PER_TARGET_U32;
    meta.filled_slots = (meta.filled_slots + 1).min(MAX_SAMPLES_PER_TARGET_U32);
    meta.total_writes = total_writes;
    meta.last_attempt_at_secs = Some(sample.timestamp_secs);
    // A status reply can be successful while reporting an installed/stopped
    // target, but only a genuine balance observation is a successful sample
    // for stale-age/last-known-balance purposes.
    if sample.balance.is_some() && sample.state != types::PublicTargetState::Unreachable {
        meta.last_success_at_secs = Some(sample.timestamp_secs);
    }
    SAMPLE_META.with(|m| {
        m.borrow_mut()
            .insert(StorablePrincipal(target), StoredSampleMeta::V2(meta));
    });
    Ok(())
}

/// The chronological (oldest-to-newest) physical-slot order for a ring whose
/// metadata is `(filled_slots, next_slot)`.
///
/// Before the first wrap (`filled_slots < MAX_SAMPLES_PER_TARGET`),
/// `record_sample` always keeps `next_slot == filled_slots` (both start at 0
/// and advance together), so slot order `0..filled_slots` already IS write
/// order. Once the ring is full (`filled_slots == MAX_SAMPLES_PER_TARGET`),
/// the slot about to be overwritten next (`next_slot`) holds the oldest
/// surviving sample, and the slot just before it (`next_slot - 1`, wrapping)
/// holds the newest — so the chronological order rotates to start at
/// `next_slot` and wrap around through the rest of the ring.
fn chronological_slot_order(filled_slots: u32, next_slot: u32) -> Vec<u32> {
    if filled_slots < MAX_SAMPLES_PER_TARGET_U32 {
        (0..filled_slots).collect()
    } else {
        (next_slot..MAX_SAMPLES_PER_TARGET_U32)
            .chain(0..next_slot)
            .collect()
    }
}

/// The oldest logical sequence number (1-indexed, matching `total_writes`)
/// still retained in the ring, or `None` if nothing has ever been written.
/// Before the first wrap, every write ever made is still retained, so the
/// oldest is sequence `1`. Once the ring has wrapped (`total_writes >
/// MAX_SAMPLES_PER_TARGET`), only the most recent `MAX_SAMPLES_PER_TARGET`
/// writes survive, so the oldest retained is `total_writes -
/// MAX_SAMPLES_PER_TARGET + 1`.
fn oldest_retained_sequence(total_writes: u64) -> Option<u64> {
    if total_writes == 0 {
        None
    } else if total_writes <= MAX_SAMPLES_PER_TARGET_U32 as u64 {
        Some(1)
    } else {
        Some(total_writes - MAX_SAMPLES_PER_TARGET_U32 as u64 + 1)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SampleCursorError {
    /// `cursor` is greater than `total_writes` — it does not name any
    /// sequence number ever assigned to this target, past or present.
    FutureCursor,
    /// `cursor` is older than one-before the oldest currently-retained
    /// sequence: at least one sample the caller has not yet received was
    /// evicted by ring recycling before this call could ever return it.
    /// Final-correction Finding 2: resuming anyway (by guessing either
    /// "replay from the start" or "nothing new") would silently skip
    /// retained-but-undelivered history or replay already-seen history: this
    /// makes the gap an explicit, typed, fail-closed contract instead. A
    /// fresh start (`cursor: None`) never hits this — only an EXPLICIT stale
    /// cursor from a caller that fell behind can.
    StaleCursor,
}

/// Lists one target's sample history in true chronological (oldest-first)
/// order, correct both before and after the ring has wrapped.
///
/// **Cursor contract:** `cursor` is the logical sequence number (matching
/// `SampleMeta::total_writes` at the moment of the write, NOT a physical
/// ring slot — final-correction Finding 2 fixed exactly this aliasing bug)
/// of the last sample the caller already received; pass `None` to start
/// from the oldest surviving sample. A `cursor` greater than the most recent
/// sequence ever assigned is rejected as `FutureCursor`. A `cursor` that
/// predates the oldest currently-retained sample by more than one — i.e. at
/// least one retained-but-undelivered sample was evicted before the caller
/// ever saw it — is rejected as `StaleCursor` rather than silently skipping
/// it or replaying from the start. Because the cursor is a logical sequence
/// number rather than a physical slot, it can never alias to the wrong
/// sample after a physical slot is recycled: the two error cases above are
/// the only ambiguous states, and both are explicit, typed, fail-closed
/// rejections.
///
/// Returns `(sequence, Sample)` pairs — the sequence is exposed so a caller
/// can round-trip it back in as the next `cursor` — see the module's
/// non-goal note: no public Candid API depends on this helper yet.
pub(crate) fn list_samples(
    target: Principal,
    cursor: Option<u64>,
    limit: usize,
) -> Result<Vec<(u64, Sample)>, SampleCursorError> {
    let meta = sample_meta(target).unwrap_or_default();
    let order = chronological_slot_order(meta.filled_slots, meta.next_slot);
    let oldest = oldest_retained_sequence(meta.total_writes);
    let start_sequence = match cursor {
        None => oldest.unwrap_or(1),
        Some(seq) => {
            if seq > meta.total_writes {
                return Err(SampleCursorError::FutureCursor);
            }
            let floor = oldest.unwrap_or(1);
            if seq + 1 < floor {
                return Err(SampleCursorError::StaleCursor);
            }
            seq + 1
        }
    };
    let base = oldest.unwrap_or(start_sequence);
    Ok(SAMPLES.with(|m| {
        let map = m.borrow();
        order
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| {
                let seq = base + i as u64;
                if seq < start_sequence {
                    return None;
                }
                map.get(&SampleKey {
                    principal: target,
                    slot: *slot,
                })
                .map(|v| (seq, v.into_current()))
            })
            .take(limit)
            .collect()
    }))
}

pub(crate) fn sample_meta(target: Principal) -> Option<SampleMeta> {
    SAMPLE_META.with(|m| {
        m.borrow()
            .get(&StorablePrincipal(target))
            .map(|v| v.into_current())
    })
}

/// Returns the newest retained sample.  The logical cursor is important here:
/// after a ring wrap the newest physical slot is `next_slot - 1`, not the
/// numerically greatest slot.
pub(crate) fn latest_sample(target: Principal) -> Option<Sample> {
    let meta = sample_meta(target)?;
    let sequence = meta.total_writes.checked_sub(1)?;
    list_samples(target, Some(sequence), 1)
        .ok()
        .and_then(|mut rows| rows.pop().map(|(_, sample)| sample))
}

pub(crate) fn latest_successful_sample(target: Principal) -> Option<Sample> {
    list_samples(target, None, types::MAX_SAMPLES_PER_TARGET)
        .ok()
        .into_iter()
        .flatten()
        .rev()
        .map(|(_, sample)| sample)
        .find(|sample| {
            sample.balance.is_some() && sample.state != types::PublicTargetState::Unreachable
        })
}

/// Bounded scan over `TERMINAL_SUMMARIES` (<= `MAX_TERMINAL_SUMMARIES`
/// total) filtered to one target.
pub(crate) fn list_terminal_summaries_for_target(
    target: Principal,
    limit: usize,
) -> Vec<TerminalFundingSummary> {
    TERMINAL_SUMMARIES.with(|m| {
        m.borrow()
            .iter()
            .map(|(_, v)| v.into_current())
            .filter(|summary| summary.target() == target)
            .take(limit)
            .collect()
    })
}

/// Lists summaries by their globally monotonic operation id, strictly after
/// `cursor`, before filtering to a target.  The operation id is an internal
/// storage key and is used only to make public pagination stable when older
/// summaries are evicted between calls.
pub(crate) fn list_terminal_summaries_for_target_after(
    target: Principal,
    cursor: Option<u64>,
    limit: usize,
) -> Vec<TerminalFundingSummary> {
    TERMINAL_SUMMARIES.with(|m| {
        let map = m.borrow();
        let start = match cursor {
            Some(id) => std::ops::Bound::Excluded(id),
            None => std::ops::Bound::Unbounded,
        };
        map.range((start, std::ops::Bound::Unbounded))
            .map(|(_, v)| v.into_current())
            .filter(|summary| summary.target() == target)
            .take(limit)
            .collect()
    })
}

/// Returns the bounded summary-store coverage needed to prove whether an
/// interval can contain an evicted confirmed credit.
pub(crate) fn terminal_summary_coverage() -> (usize, Option<u64>) {
    TERMINAL_SUMMARIES.with(|m| {
        let map = m.borrow();
        let oldest = map
            .iter()
            .map(|(_, value)| value.into_current().resolved_at_secs())
            .min();
        (map.len() as usize, oldest)
    })
}

// ─────────────────────── Funding: operations, reservations, self-recovery ───────────────────────

pub(crate) fn next_operation_id() -> u64 {
    FUNDING_COUNTERS.with(|cell| {
        let mut cell = cell.borrow_mut();
        let mut counters = cell.get().clone().into_current();
        let id = counters.next_operation_id;
        counters.next_operation_id = id
            .checked_add(1)
            .unwrap_or_else(|| ic_cdk::trap("rumi_cycle_sentinel: operation id counter overflow"));
        cell.set(StoredFundingCounters::V1(counters))
            .expect("rumi_cycle_sentinel: failed to write funding counters cell");
        id
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MonotonicTimeError {
    /// `last_created_at_time_ns + 1` would overflow `u64`. Fails closed:
    /// nothing is persisted and the caller must not proceed with an
    /// outbound ledger call that needs a fresh `created_at_time`.
    Overflow,
}

/// Globally monotonic `created_at_time` allocation for outgoing ledger
/// calls: `max(now_ns, last_created_at_time_ns + 1)`, persisted to
/// `FUNDING_COUNTERS` before returning it (the design requires the
/// timestamp persisted *before* the outbound call, not just computed).
pub(crate) fn next_created_at_time_ns(now_ns: u64) -> Result<u64, MonotonicTimeError> {
    FUNDING_COUNTERS.with(|cell| {
        let mut cell = cell.borrow_mut();
        let mut counters = cell.get().clone().into_current();
        let floor = counters
            .last_created_at_time_ns
            .checked_add(1)
            .ok_or(MonotonicTimeError::Overflow)?;
        let next = now_ns.max(floor);
        counters.last_created_at_time_ns = next;
        cell.set(StoredFundingCounters::V1(counters))
            .expect("rumi_cycle_sentinel: failed to write funding counters cell");
        Ok(next)
    })
}

pub(crate) fn get_operation(id: u64) -> Option<FundingOperation> {
    FUNDING_OPERATIONS.with(|m| m.borrow().get(&id).map(|v| v.into_current()))
}

/// Raw keyed overwrite, no bound/consistency checks — private because every
/// external caller must go through either `insert_operation` (bound-checked
/// new-operation path) or `update_operation` (immutable-snapshot/transition
/// -checked existing-operation path).
fn raw_insert_operation(op: FundingOperation) {
    let id = op.id();
    FUNDING_OPERATIONS.with(|m| {
        m.borrow_mut().insert(id, StoredFundingOperation::V3(op));
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InsertOperationError {
    /// `FUNDING_OPERATIONS` is at `types::MAX_FUNDING_OPERATIONS` and `op.id()`
    /// is not already a key. Unlike alarms/proposals, there is no eviction
    /// option here — every entry in this store is by definition unresolved
    /// (a resolved one is compacted away in the same call that resolves it),
    /// so nothing is ever safely prunable. The caller (a later task's
    /// funding/self-recovery module) must already guarantee at most one
    /// unresolved operation per registered target plus the one self-recovery
    /// lane, so reaching this in practice means that invariant itself was
    /// violated upstream.
    TooManyOperations,
    /// `op.id()` already names a stored operation. `insert_operation` is
    /// new-operation-only — an existing id must go through `update_operation`
    /// instead, which enforces immutable-snapshot and legal-transition
    /// checks `insert_operation` deliberately does not (final-correction
    /// Finding 1: a bare same-key overwrite here would silently bypass every
    /// one of those checks for an already-open operation).
    AlreadyExists,
    /// `op.trigger() != FundingTrigger::SelfRecovery` and `op.target()` has
    /// no live `TARGET_REGISTRY` entry — mirrors `record_sample`'s
    /// `UnregisteredTarget` check, so an ordinary operation can never open
    /// an orphan snapshot with no registered owner. `SelfRecovery`-triggered
    /// operations are exempt: their target is always `sentinel_id`, which by
    /// construction never has a `TARGET_REGISTRY` entry.
    UnregisteredTarget,
}

/// Self-enforcing at the storage boundary: a genuinely new operation id is
/// rejected once the store is at `types::MAX_FUNDING_OPERATIONS`
/// (`MAX_TARGETS` ordinary unresolved operations plus one self-recovery
/// lane), so this is a fail-closed typed error instead of a `post_upgrade`
/// trap surfacing the overflow only after the fact. New-operation-only: an
/// id that already exists is rejected outright (`AlreadyExists`) rather than
/// silently overwritten — every existing-operation write must go through
/// `update_operation`, which enforces immutable-snapshot and legal-transition
/// checks on top of the exact same underlying store.
pub(crate) fn insert_operation(op: FundingOperation) -> Result<(), InsertOperationError> {
    let id = op.id();
    let is_new = FUNDING_OPERATIONS.with(|m| m.borrow().get(&id).is_none());
    if !is_new {
        return Err(InsertOperationError::AlreadyExists);
    }
    if op.trigger() != FundingTrigger::SelfRecovery && get_target(op.target()).is_none() {
        return Err(InsertOperationError::UnregisteredTarget);
    }
    let len = FUNDING_OPERATIONS.with(|m| m.borrow().len());
    if len as usize >= types::MAX_FUNDING_OPERATIONS {
        return Err(InsertOperationError::TooManyOperations);
    }
    raw_insert_operation(op);
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UpdateOperationError {
    /// No operation with this id exists yet — `update_operation` is only for
    /// an already-open operation; use `insert_operation` to open one.
    NotFound,
    /// One of `target`, `target_registry_revision`, `funding_policy`,
    /// `trigger`, `rail_arguments`, `reserved_amount_cycles`, or
    /// `created_at_secs` differs from the currently stored operation of the
    /// same id. Every one of those fields is an immutable snapshot taken at
    /// `FundingOperation::open` time (see that type's own doc comment) — a
    /// blind same-key overwrite could otherwise silently rewrite an
    /// operation's recipient or policy out from under an in-flight spend.
    ImmutableSnapshotChanged,
    /// `new_state` is not a legal `FundingOperationState::is_valid_successor`
    /// step from the currently stored operation's own state — rejects both a
    /// state regression and a jump that skips the rail's lifecycle graph,
    /// on top of the type-level check `record_attempt` already performs
    /// against whatever `self` the caller happened to be holding (which may
    /// be stale relative to what is actually persisted).
    InvalidTransition,
    /// `op.updated_at_secs()` is strictly less than the currently stored
    /// record's `updated_at_secs()`. `is_valid_successor` treats any
    /// same-phase transition as always legal (a deliberate, correct design
    /// choice for an indeterminate same-phase retry), which means the
    /// transition check above provides no protection when the incoming `op`
    /// is a STALE same-phase copy: a caller holding an older in-memory value
    /// could otherwise silently regress the stored timestamp (final-
    /// correction Finding N2).
    TimestampRegression,
    /// `op.attempts()` is shorter than, or diverges from, the currently
    /// stored record's `attempts()` at any shared ordinal. A legitimate
    /// update only ever EXTENDS the stored history (the caller's local copy
    /// was derived from a state at least as old as what is stored, possibly
    /// exactly matching it) — never replaces a prefix with a different
    /// history. Catches a stale concurrent writer whose own attempt history
    /// diverged from what a different, newer writer already persisted, even
    /// when `op.updated_at_secs()` itself is not a regression.
    AttemptHistoryDiverged,
}

/// Persists an update to an already-open operation. Re-validates against
/// the CURRENTLY STORED record (not just whatever state the caller's local
/// value came from) that every immutable snapshot field is unchanged, that
/// the state transition is legal, that the timestamp does not regress, and
/// that the incoming attempt history is a superset-preserving extension of
/// what is actually stored — preventing a blind overwrite of an operation's
/// immutable snapshot, an illegal transition, or a stale same-phase retry
/// from silently clobbering a concurrently-persisted attempt, from ever
/// reaching stable memory (storage review's mutation-helper correction, and
/// final-correction Finding N2: a typed rejection here, not a state that
/// only `post_upgrade` would catch — and for the timestamp/history axis,
/// not a silent lost-update `post_upgrade` could never catch at all).
pub(crate) fn update_operation(op: FundingOperation) -> Result<(), UpdateOperationError> {
    let existing = get_operation(op.id()).ok_or(UpdateOperationError::NotFound)?;
    if existing.target() != op.target()
        || existing.target_registry_revision() != op.target_registry_revision()
        || existing.funding_policy() != op.funding_policy()
        || existing.trigger() != op.trigger()
        || existing.rail_arguments() != op.rail_arguments()
        || existing.reserved_amount_cycles() != op.reserved_amount_cycles()
        || existing.created_at_secs() != op.created_at_secs()
    {
        return Err(UpdateOperationError::ImmutableSnapshotChanged);
    }
    let is_explicit_quarantined_reconciliation =
        existing.is_valid_quarantined_cycles_reconciliation(&op);
    let is_explicit_icp_quarantined_reconciliation =
        existing.is_valid_quarantined_icp_reconciliation(&op);
    let is_icp_refund_proof_attachment = existing.is_valid_icp_refund_proof_attachment(&op);
    let is_bounded_attempt_compaction = existing.is_valid_bounded_attempt_compaction_successor(&op);
    if !existing.state().is_valid_successor(&op.state())
        && !is_explicit_quarantined_reconciliation
        && !is_explicit_icp_quarantined_reconciliation
        && !is_icp_refund_proof_attachment
        && !is_bounded_attempt_compaction
    {
        return Err(UpdateOperationError::InvalidTransition);
    }
    if op.updated_at_secs() < existing.updated_at_secs() {
        return Err(UpdateOperationError::TimestampRegression);
    }
    let existing_attempts = existing.attempts().as_slice();
    let incoming_attempts = op.attempts().as_slice();
    if !is_explicit_quarantined_reconciliation
        && !is_explicit_icp_quarantined_reconciliation
        && !is_icp_refund_proof_attachment
        && !is_bounded_attempt_compaction
        && (incoming_attempts.len() < existing_attempts.len()
            || incoming_attempts[..existing_attempts.len()] != existing_attempts[..])
    {
        return Err(UpdateOperationError::AttemptHistoryDiverged);
    }
    raw_insert_operation(op);
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CompactOperationError {
    /// No operation with id `id` exists in `FUNDING_OPERATIONS`.
    Missing,
    /// The operation exists but has not reached a resolved state yet — a
    /// `Quarantined` or in-flight operation can never be compacted; it must
    /// remain reachable in full until it is genuinely done.
    Unresolved,
    /// `summary.operation_id() != id` — the caller passed a summary for a
    /// different operation than the one it asked to compact.
    WrongId,
    /// `summary`'s `target`/`rail`/`amount_cycles`/`outcome` disagree with
    /// the operation actually stored under `id`. This can only happen if the
    /// summary was constructed from a stale or different operation snapshot
    /// than what is currently persisted.
    Mismatched,
    /// `TARGET_RESERVATIONS` for `op.target()` still names `id` as its
    /// pending in-flight operation. Compacting now would remove the only
    /// record `validate_whole_state`'s bidirectional linkage check could
    /// ever match against, permanently bricking every future `post_upgrade`
    /// (final-correction Finding N1) — the caller must settle/release the
    /// target reservation first.
    TargetReservationStillPending,
    /// `GLOBAL_ROLLING_SPEND` still names `id` as one of its pending
    /// entries. Same hazard and required ordering as
    /// `TargetReservationStillPending`, for the global ledger.
    GlobalReservationStillPending,
    /// `op.trigger() == FundingTrigger::SelfRecovery` and `SELF_RECOVERY`
    /// still names `id` as its in-flight operation. Same hazard, for the
    /// self-recovery singleton lane.
    SelfRecoveryReservationStillPending,
    /// `op.rail() == FundingRail::CyclesLedger` and `SOURCE_RESERVE` (Task
    /// 4's shared Cycles Ledger source-account reservation, used by both
    /// ordinary and self-recovery operations alike) still names `id` as a
    /// pending debit. Same hazard as the other reservation checks above.
    SourceReservationStillPending,
    /// `op.rail() == FundingRail::IcpCmc` and the ICP source reserve still
    /// names `id` as a pending debit. The operation must settle/release its
    /// ICP hold before its full record is compacted.
    IcpSourceReservationStillPending,
}

/// The self-enforcing compaction primitive for `FUNDING_OPERATIONS`
/// (storage review Finding 1): first persists `summary` into the bounded,
/// self-evicting `TERMINAL_SUMMARIES` ring (reusing `insert_terminal_summary`,
/// so the 512-entry bound is enforced the same way as every other terminal
/// summary insert), THEN removes the full operation from
/// `FUNDING_OPERATIONS` — both synchronously, in this one call, with no
/// await boundary in between. Never evicts an unresolved operation: the
/// `Unresolved` check below is the only state-based gate, so this function
/// can never be used as a back-door eviction path for a live operation.
///
/// Also rejects compaction while a pending reservation (target, global, or
/// self-recovery, whichever applies to this operation's trigger) still
/// names `id` (final-correction Finding N1) — `remove_target`'s analogous
/// fail-closed guard was the template this was missing: removing the
/// operation's own record while a reservation still points at it leaves
/// `validate_whole_state`'s bidirectional linkage check with nothing to
/// match against, permanently trapping every future `post_upgrade`. The
/// caller must settle/release the exact reservation first.
pub(crate) fn compact_operation(
    id: u64,
    summary: TerminalFundingSummary,
) -> Result<(), CompactOperationError> {
    if summary.operation_id() != id {
        return Err(CompactOperationError::WrongId);
    }
    let op = get_operation(id).ok_or(CompactOperationError::Missing)?;
    let resolved_outcome = op
        .state()
        .resolved_outcome()
        .ok_or(CompactOperationError::Unresolved)?;
    if summary.target() != op.target()
        || summary.rail() != op.rail()
        || summary.amount_cycles() != op.reserved_amount_cycles()
        || summary.outcome() != resolved_outcome
    {
        return Err(CompactOperationError::Mismatched);
    }
    if op.trigger() == FundingTrigger::SelfRecovery {
        if get_self_recovery_state().in_flight_operation_id() == Some(id) {
            return Err(CompactOperationError::SelfRecoveryReservationStillPending);
        }
    } else {
        if get_target_reservation(op.target())
            .rolling_spend()
            .pending()
            .iter()
            .any(|p| p.operation_id == id)
        {
            return Err(CompactOperationError::TargetReservationStillPending);
        }
        if get_global_rolling_spend()
            .rolling_spend()
            .pending()
            .iter()
            .any(|p| p.operation_id == id)
        {
            return Err(CompactOperationError::GlobalReservationStillPending);
        }
    }
    if op.rail() == types::FundingRail::CyclesLedger
        && get_source_reserve()
            .pending()
            .iter()
            .any(|p| p.operation_id == id)
    {
        return Err(CompactOperationError::SourceReservationStillPending);
    }
    if op.rail() == types::FundingRail::IcpCmc
        && get_icp_source_reserve()
            .pending()
            .iter()
            .any(|p| p.operation_id == id)
    {
        return Err(CompactOperationError::IcpSourceReservationStillPending);
    }
    insert_terminal_summary(summary);
    FUNDING_OPERATIONS.with(|m| {
        m.borrow_mut().remove(&id);
    });
    Ok(())
}

/// Feeds the timer's "resume pending operations" step (a later task):
/// every operation still eligible for automatic retry, i.e.
/// `!state().stops_automatic_retry()`. This deliberately EXCLUDES
/// `Quarantined` operations (they still hold a reservation and are not yet
/// `is_resolved()`, but they need a *signer* action, not an automatic
/// retry) — see `validate_whole_state` below for the separate
/// reservation-linkage notion of "still holds a reservation", which is
/// `!is_resolved()` and does include `Quarantined`.
pub(crate) fn list_nonterminal_operations() -> Vec<FundingOperation> {
    FUNDING_OPERATIONS.with(|m| {
        m.borrow()
            .iter()
            .map(|(_, v)| v.into_current())
            .filter(|op| !op.state().stops_automatic_retry())
            .collect()
    })
}

/// Unresolved (`!is_resolved()`), non-`SelfRecovery` operations only — the
/// exact operation set `validate_whole_state`'s
/// `OperationDailyCapExceedsGlobalCap` check walks (a resolved operation is
/// an immutable historical snapshot never re-judged against a later policy;
/// self-recovery has its own independent cap, checked separately). Used by
/// `governance::check_targets_fit_global_cap` so a `SetGlobalPolicy`
/// proposal can never write a cap that would trap the very next
/// `post_upgrade`.
pub(crate) fn list_unresolved_ordinary_operations() -> Vec<FundingOperation> {
    FUNDING_OPERATIONS.with(|m| {
        m.borrow()
            .iter()
            .map(|(_, v)| v.into_current())
            .filter(|op| !op.state().is_resolved() && op.trigger() != FundingTrigger::SelfRecovery)
            .collect()
    })
}

/// Absence is a legitimate "never reserved" state, not corruption — returns
/// a fresh `TargetReservationState::new()` rather than `Option`.
pub(crate) fn get_target_reservation(principal: Principal) -> TargetReservationState {
    TARGET_RESERVATIONS.with(|m| {
        m.borrow()
            .get(&StorablePrincipal(principal))
            .map(|v| v.into_current())
            .unwrap_or_default()
    })
}

pub(crate) fn set_target_reservation(principal: Principal, state: TargetReservationState) {
    TARGET_RESERVATIONS.with(|m| {
        m.borrow_mut().insert(
            StorablePrincipal(principal),
            StoredTargetReservationState::V1(state),
        );
    });
    bump_source_refresh_generation();
}

pub(crate) fn get_global_rolling_spend() -> GlobalRollingSpendState {
    GLOBAL_ROLLING_SPEND.with(|c| c.borrow().get().clone().into_current())
}

pub(crate) fn set_global_rolling_spend(state: GlobalRollingSpendState) {
    GLOBAL_ROLLING_SPEND.with(|c| {
        c.borrow_mut()
            .set(StoredGlobalRollingSpendState::V1(state))
            .expect("rumi_cycle_sentinel: failed to write global rolling spend cell");
    });
    bump_source_refresh_generation();
}

pub(crate) fn get_self_recovery_state() -> SelfRecoveryState {
    SELF_RECOVERY.with(|c| c.borrow().get().clone().into_current())
}

pub(crate) fn set_self_recovery_state(state: SelfRecoveryState) {
    SELF_RECOVERY.with(|c| {
        c.borrow_mut()
            .set(StoredSelfRecoveryState::V2(state))
            .expect("rumi_cycle_sentinel: failed to write self-recovery cell");
    });
    bump_source_refresh_generation();
}

/// The ONLY write path for terminal summaries: takes an already-validated
/// `types::TerminalFundingSummary` (constructible only via
/// `TerminalFundingSummary::from_resolved`, never from raw bytes or an
/// external/untrusted decode path), inserts it keyed by `operation_id`,
/// then evicts the entry with the oldest `resolved_at_secs` if the store
/// exceeds `MAX_TERMINAL_SUMMARIES`.
pub(crate) fn insert_terminal_summary(summary: TerminalFundingSummary) {
    let id = summary.operation_id();
    TERMINAL_SUMMARIES.with(|m| {
        m.borrow_mut()
            .insert(id, StoredTerminalFundingSummary::V1(summary));
    });
    evict_oldest_terminal_summary_if_over_bound();
}

fn evict_oldest_terminal_summary_if_over_bound() {
    TERMINAL_SUMMARIES.with(|m| {
        let mut map = m.borrow_mut();
        if map.len() as usize <= types::MAX_TERMINAL_SUMMARIES {
            return;
        }
        let victim = map
            .iter()
            .map(|(k, v)| (k, v.into_current().resolved_at_secs()))
            .min_by_key(|(_, resolved_at_secs)| *resolved_at_secs)
            .map(|(k, _)| k);
        if let Some(key) = victim {
            map.remove(&key);
        }
    });
}

// ─────────────────────── Cycles Ledger source reserve/cache (Task 4) ───────────────────────

pub(crate) fn get_source_reserve() -> SourceReserveState {
    SOURCE_RESERVE.with(|c| c.borrow().get().clone().into_current())
}

/// Returns the durable generation captured by a source-cache refresh before
/// its first inter-canister await. The generation is deliberately advanced
/// by *all* reservation/attempt/settlement setters, not only the source cell:
/// a refresh result is stale if any funding bookkeeping changed while its
/// query was in flight.
pub(crate) fn source_refresh_generation() -> u64 {
    SOURCE_REFRESH_GENERATION.with(|c| c.borrow().get().value())
}

fn bump_source_refresh_generation() {
    SOURCE_REFRESH_GENERATION.with(|c| {
        let mut cell = c.borrow_mut();
        let next = cell
            .get()
            .value()
            .checked_add(1)
            .expect("rumi_cycle_sentinel: source refresh generation exhausted");
        cell.set(StoredSourceRefreshGeneration::V1(SourceRefreshGeneration {
            value: next,
        }))
        .expect("rumi_cycle_sentinel: failed to write source refresh generation");
    });
}

pub(crate) fn set_source_reserve(state: SourceReserveState) {
    SOURCE_RESERVE.with(|c| {
        c.borrow_mut()
            .set(StoredSourceReserveState::V1(state))
            .expect("rumi_cycle_sentinel: failed to write source reserve cell");
    });
    bump_source_refresh_generation();
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SourceRefreshCommitError {
    /// The cache query began from an older funding-state generation. Its
    /// balance/fee result must not overwrite reservations or a settlement
    /// committed while the query was in flight.
    StaleGeneration,
    SourceReserve(SourceReserveError),
}

/// Commits a queried Cycles Ledger cache only if no funding reservation,
/// attempt, settlement, or source-cache write occurred after the query began.
/// The generation check and following stable writes are synchronous in one IC
/// turn, so once the check passes there is no await boundary through which a
/// competing refresh can interleave.
pub(crate) fn commit_source_reserve_refresh(
    expected_generation: u64,
    balance_cycles: u128,
    fee_cycles: u128,
    as_of_secs: u64,
) -> Result<(), SourceRefreshCommitError> {
    if source_refresh_generation() != expected_generation {
        return Err(SourceRefreshCommitError::StaleGeneration);
    }
    let current = get_source_reserve();
    let updated = current
        .refresh(balance_cycles, fee_cycles, as_of_secs)
        .map_err(SourceRefreshCommitError::SourceReserve)?;
    // The check above is sufficient in the actor's synchronous execution
    // model, but retaining this second guard makes the linearization point
    // explicit and defensive if the storage implementation ever changes.
    if source_refresh_generation() != expected_generation {
        return Err(SourceRefreshCommitError::StaleGeneration);
    }
    set_source_reserve(updated);
    Ok(())
}

/// Marks a persisted source debit immediately before its external Cycles
/// Ledger call.  Keeping this transition in the stable source-reserve cell
/// lets refresh reject ambiguous observations while the call is in flight and
/// prevents a later settlement from applying a known debit twice.
pub(crate) fn mark_source_attempt(
    operation_id: u64,
    attempt_started_at_secs: u64,
) -> Result<(), SourceAttemptError> {
    let updated = get_source_reserve().mark_attempt(operation_id, attempt_started_at_secs)?;
    set_source_reserve(updated);
    Ok(())
}

// ─────────────────────── ICP Ledger source reserve/cache (Task 5) ───────────────────────

pub(crate) fn get_icp_source_reserve() -> IcpSourceReserveState {
    ICP_SOURCE_RESERVE.with(|c| c.borrow().get().clone().into_current())
}

pub(crate) fn set_icp_source_reserve(state: IcpSourceReserveState) {
    ICP_SOURCE_RESERVE.with(|c| {
        c.borrow_mut()
            .set(StoredIcpSourceReserveState::V1(state))
            .expect("rumi_cycle_sentinel: failed to write ICP source reserve cell");
    });
    bump_source_refresh_generation();
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IcpSourceRefreshCommitError {
    StaleGeneration,
    SourceReserve(IcpSourceReserveError),
}

/// Commits an ICP Ledger balance/fee query only when no funding state changed
/// while the query was in flight.  The shared generation is intentional: a
/// Cycles reservation and an ICP reservation must invalidate each other's
/// stale refresh result because both consume the same global funding state.
pub(crate) fn commit_icp_source_reserve_refresh(
    expected_generation: u64,
    balance_e8s: u128,
    fee_e8s: u128,
    as_of_secs: u64,
) -> Result<(), IcpSourceRefreshCommitError> {
    if source_refresh_generation() != expected_generation {
        return Err(IcpSourceRefreshCommitError::StaleGeneration);
    }
    let updated = get_icp_source_reserve()
        .refresh(balance_e8s, fee_e8s, as_of_secs)
        .map_err(IcpSourceRefreshCommitError::SourceReserve)?;
    if source_refresh_generation() != expected_generation {
        return Err(IcpSourceRefreshCommitError::StaleGeneration);
    }
    set_icp_source_reserve(updated);
    Ok(())
}

pub(crate) fn mark_icp_source_attempt(
    operation_id: u64,
    attempt_started_at_secs: u64,
) -> Result<(), IcpSourceAttemptError> {
    let updated = get_icp_source_reserve().mark_attempt(operation_id, attempt_started_at_secs)?;
    set_icp_source_reserve(updated);
    Ok(())
}

pub(crate) fn clear_icp_source_attempt(operation_id: u64) -> Result<(), IcpSourceAttemptError> {
    let updated = get_icp_source_reserve().clear_attempt(operation_id)?;
    set_icp_source_reserve(updated);
    Ok(())
}

/// Persists the ephemeral CMC notify-flight marker before the notify await.
/// `update_operation` still enforces the immutable snapshot and lifecycle
/// checks; only the marker field changes.
pub(crate) fn mark_icp_notify_attempt(
    operation_id: u64,
    attempt_started_at_secs: u64,
) -> Result<(), types::FundingOperationTransitionError> {
    let op = get_operation(operation_id)
        .ok_or(types::FundingOperationTransitionError::InvalidSuccessor)?;
    let marked = op.mark_notify_attempt(attempt_started_at_secs)?;
    update_operation(marked).map_err(|_| types::FundingOperationTransitionError::InvalidSuccessor)
}

/// Clears the CMC notify marker after the await has returned. Missing markers
/// are harmless and therefore idempotent; a missing operation is rejected.
pub(crate) fn clear_icp_notify_attempt(
    operation_id: u64,
) -> Result<(), types::FundingOperationTransitionError> {
    let op = get_operation(operation_id)
        .ok_or(types::FundingOperationTransitionError::InvalidSuccessor)?;
    let cleared = op.clear_notify_attempt()?;
    if cleared == op {
        return Ok(());
    }
    update_operation(cleared).map_err(|_| types::FundingOperationTransitionError::InvalidSuccessor)
}

/// Clears only the ephemeral ICP transfer-flight markers after an upgrade.
/// The durable pending debits and immutable operation snapshots remain intact,
/// so the interrupted operation can be retried with exactly the same args.
pub(crate) fn reset_icp_source_attempts_on_upgrade() {
    let updated = get_icp_source_reserve().reset_attempts();
    set_icp_source_reserve(updated);
}

/// Upgrade interrupts an in-message CMC notify await. Clear only that
/// ephemeral marker and leave the immutable transfer block, notify arguments,
/// and source hold intact so retry uses the same block after restart.
pub(crate) fn reset_icp_notify_attempts_on_upgrade() {
    let operations: Vec<FundingOperation> = FUNDING_OPERATIONS.with(|m| {
        m.borrow()
            .iter()
            .map(|(_, value)| value.into_current())
            .collect()
    });
    for op in operations {
        if op.notify_attempt_started_at_secs().is_some() {
            raw_insert_operation(op.reset_notify_attempt_on_upgrade());
        }
    }
}

// ─────────────────────── Whole-state validation ───────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StateValidationError {
    GlobalConfigUninitialized,
    EmptySigners,
    AnonymousSigner,
    DuplicateSigner(Principal),
    ThresholdZero,
    ThresholdExceedsSigners {
        threshold: u32,
        signer_count: u32,
    },
    TooManyTargets {
        count: u64,
    },
    TooManyFundingOperations {
        count: u64,
    },
    OrphanSampleMeta {
        target: Principal,
    },
    OrphanSample {
        target: Principal,
        slot: u32,
    },
    OrphanTargetReservation {
        target: Principal,
    },
    ImpossibleSampleMeta {
        target: Principal,
    },
    TargetRegistryKeyMismatch {
        key: Principal,
        record: Principal,
    },
    ReservedTargetPrincipal {
        target: Principal,
        kind: ReservedPrincipalKind,
    },
    TargetDailyCapExceedsGlobalCap {
        target: Principal,
    },
    OperationDailyCapExceedsGlobalCap {
        operation_id: u64,
    },
    /// `op.trigger() != FundingTrigger::SelfRecovery` and `op.target() ==
    /// sentinel_id`, for an UNRESOLVED operation (final-correction Finding
    /// N5) — an ordinary operation must never be able to move funds to the
    /// Sentinel's own principal through the ordinary lane; only the
    /// hard-coded `SelfRecovery` lane may ever target `sentinel_id`.
    OrdinaryOperationTargetsSentinel {
        operation_id: u64,
    },
    /// An UNRESOLVED, non-`SelfRecovery` operation's `target` has no live
    /// `TARGET_REGISTRY` entry (storage review Finding 3 hardening) — the
    /// dedicated whole-state backstop for `insert_operation`'s own
    /// `UnregisteredTarget` check, for state that reached this store some
    /// other way (a pre-fix legacy snapshot, or direct repair-tooling
    /// manipulation). Scoped to unresolved only: a resolved operation is an
    /// immutable historical snapshot that may legitimately outlive its
    /// target's removal (that is what `TERMINAL_SUMMARIES` is for).
    OrphanFundingOperationTarget {
        operation_id: u64,
        target: Principal,
    },
    FundingOperationKeyMismatch {
        key: u64,
        operation_id: u64,
    },
    IcpSnapshotInvalid {
        operation_id: u64,
    },
    TerminalSummaryKeyMismatch {
        key: u64,
        operation_id: u64,
    },
    AlarmKeyMismatch {
        key: u64,
        alarm_id: u64,
    },
    TooManyNonterminalOperationsForTarget {
        target: Principal,
        count: usize,
    },
    TargetReservationMissingOperation {
        target: Principal,
        operation_id: u64,
    },
    NonterminalOperationMissingTargetReservation {
        operation_id: u64,
        target: Principal,
    },
    TargetReservationAmountMismatch {
        target: Principal,
        operation_id: u64,
    },
    GlobalReservationMissingOperation {
        operation_id: u64,
    },
    NonterminalOperationMissingGlobalReservation {
        operation_id: u64,
    },
    GlobalReservationAmountMismatch {
        operation_id: u64,
    },
    SelfRecoveryOperationMissing {
        operation_id: u64,
    },
    SelfRecoveryOperationWrongTarget {
        operation_id: u64,
    },
    SelfRecoveryOperationWrongTrigger {
        operation_id: u64,
    },
    SelfRecoveryOperationAlreadyResolved {
        operation_id: u64,
    },
    SelfRecoveryOperationNotRecordedInFlight {
        operation_id: u64,
    },
    SelfRecoveryLedgerAmountMismatch {
        operation_id: u64,
    },
    SelfRecoveryLedgerExceedsDailyCap {
        operation_id: u64,
    },
    ProposalKeyMismatch {
        key: u64,
        record_id: u64,
    },
    ProposalApprovalNotASigner {
        proposal_id: u64,
        approver: Principal,
    },
    TooManyProposals {
        count: u64,
    },
    TooManyAlarms {
        count: u64,
    },
    TooManyTerminalSummaries {
        count: u64,
    },
    /// An unresolved, Cycles-Ledger-rail `FundingOperation` (ordinary or
    /// `SelfRecovery`) has no matching `SOURCE_RESERVE` pending debit — the
    /// Task 4 analog of `NonterminalOperationMissingTargetReservation`, for
    /// the shared source-account reservation both lanes draw from.
    NonterminalOperationMissingSourceReserve {
        operation_id: u64,
    },
    /// `SOURCE_RESERVE`'s pending debit for this amount disagrees with the
    /// operation's own snapshotted `amount_cycles + fee_cycles`.
    SourceReserveAmountMismatch {
        operation_id: u64,
    },
    /// `SOURCE_RESERVE` names a pending operation id with no matching
    /// unresolved Cycles-Ledger-rail `FundingOperation` — the reverse
    /// direction of `NonterminalOperationMissingSourceReserve`.
    SourceReserveMissingOperation {
        operation_id: u64,
    },
    /// A loaded rolling-spend ledger violates its combined settled-plus-
    /// pending slot bound.  This is checked here as well as by the nested
    /// decoder because migration/repair code can construct a whole state from
    /// multiple independently decoded values.
    RollingSpendCombinedCapacityExceeded {
        settled_count: usize,
        pending_count: usize,
    },
    /// A loaded rolling-spend ledger's amount total cannot be represented in
    /// the checked internal domain.
    RollingSpendAmountOverflow,
    /// A loaded Cycles Ledger source reserve's pending debit total cannot be
    /// represented in the checked internal domain.
    SourceReservePendingAmountOverflow,
    /// A loaded Cycles Ledger operation's amount plus fee overflows the
    /// internal amount domain.
    FundingOperationAmountPlusFeeOverflow {
        operation_id: u64,
    },
    /// An unresolved ICP/CMC operation has no matching ICP source reserve
    /// debit, or its held amount disagrees with the immutable transfer
    /// snapshot.
    NonterminalOperationMissingIcpSourceReserve {
        operation_id: u64,
    },
    IcpSourceReserveAmountMismatch {
        operation_id: u64,
    },
    /// The reverse direction of the ICP source linkage: a pending source
    /// hold names an operation that is not an unresolved ICP operation.
    IcpSourceReserveMissingOperation {
        operation_id: u64,
    },
    IcpSourceReservePendingAmountOverflow,
}

fn validate_signer_set(signers: &[Principal], threshold: u32) -> Result<(), StateValidationError> {
    if signers.is_empty() {
        return Err(StateValidationError::EmptySigners);
    }
    let mut seen = BTreeSet::new();
    for signer in signers {
        if *signer == Principal::anonymous() {
            return Err(StateValidationError::AnonymousSigner);
        }
        if !seen.insert(*signer) {
            return Err(StateValidationError::DuplicateSigner(*signer));
        }
    }
    if threshold == 0 {
        return Err(StateValidationError::ThresholdZero);
    }
    if threshold as usize > signers.len() {
        return Err(StateValidationError::ThresholdExceedsSigners {
            threshold,
            signer_count: signers.len() as u32,
        });
    }
    Ok(())
}

fn validate_rolling_spend_ledger(
    ledger: &types::RollingSpendLedger,
) -> Result<(), StateValidationError> {
    let combined = ledger
        .settled()
        .len()
        .checked_add(ledger.pending().len())
        .ok_or(StateValidationError::RollingSpendCombinedCapacityExceeded {
            settled_count: ledger.settled().len(),
            pending_count: ledger.pending().len(),
        })?;
    if combined > types::MAX_ROLLING_SPEND_SETTLED_ENTRIES {
        return Err(StateValidationError::RollingSpendCombinedCapacityExceeded {
            settled_count: ledger.settled().len(),
            pending_count: ledger.pending().len(),
        });
    }
    ledger
        .settled()
        .iter()
        .try_fold(0u128, |acc, entry| acc.checked_add(entry.amount_cycles))
        .ok_or(StateValidationError::RollingSpendAmountOverflow)?;
    ledger
        .pending()
        .iter()
        .try_fold(0u128, |acc, entry| acc.checked_add(entry.amount_cycles))
        .ok_or(StateValidationError::RollingSpendAmountOverflow)?;
    Ok(())
}

/// Every whole-state invariant this file is responsible for (see the
/// module doc's "Self-contained vs. whole-state invariants" section and
/// `task-1b-decode-review.md` Part 5). Called from `lib.rs`'s
/// `#[post_upgrade]` hook with `ic_cdk::id()`; takes `sentinel_id`
/// explicitly (rather than calling `ic_cdk::id()` itself) so it stays
/// callable from plain unit tests off the `wasm32` target.
///
/// Deliberately does NOT re-verify a resolved `FundingOperation` against a
/// live external ledger — see the module doc and Part 5 item 3: this file
/// trusts its own authenticated versioned stable history for already
/// resolved external outcomes across an upgrade, which is the only thing
/// achievable from stable bytes alone.
pub(crate) fn validate_whole_state(sentinel_id: Principal) -> Result<(), StateValidationError> {
    let config = GLOBAL_CONFIG
        .with(|c| c.borrow().get().clone().into_current())
        .ok_or(StateValidationError::GlobalConfigUninitialized)?;
    validate_signer_set(&config.signers, config.approval_threshold)?;
    let signers_set: BTreeSet<Principal> = config.signers.iter().copied().collect();
    let global_daily_cap_cycles = config.global_policy.global_daily_cap_cycles();

    // ── Target registry ──
    let targets: Vec<(Principal, TargetRecord)> = TARGET_REGISTRY.with(|m| {
        m.borrow()
            .iter()
            .map(|(k, v)| (k.0, v.into_current()))
            .collect()
    });
    if targets.len() as u64 > types::MAX_TARGETS as u64 {
        return Err(StateValidationError::TooManyTargets {
            count: targets.len() as u64,
        });
    }
    for (key_principal, record) in &targets {
        if *key_principal != record.principal() {
            return Err(StateValidationError::TargetRegistryKeyMismatch {
                key: *key_principal,
                record: record.principal(),
            });
        }
        if let Some(kind) = types::reserved_principal_kind(record.principal(), sentinel_id) {
            return Err(StateValidationError::ReservedTargetPrincipal {
                target: record.principal(),
                kind,
            });
        }
        if record.funding_policy().daily_cap_cycles() > global_daily_cap_cycles {
            return Err(StateValidationError::TargetDailyCapExceedsGlobalCap {
                target: record.principal(),
            });
        }
    }
    let registered_principals: BTreeSet<Principal> = targets.iter().map(|(p, _)| *p).collect();
    // `sentinel_id` itself can never be a `TARGET_REGISTRY` entry (it is
    // always a `ReservedPrincipalKind::SentinelSelf` rejection), so it is
    // exempt from "orphan" framing below — its own hard-coded self-recovery
    // lane is checked separately, not through these ordinary per-target
    // stores.
    let is_accepted_principal =
        |p: Principal| registered_principals.contains(&p) || p == sentinel_id;

    // ── Orphan sample/reservation stores (storage review Finding 3) ──
    //
    // `remove_target` cascades the delete of `SAMPLE_META`/`SAMPLES`/
    // `TARGET_RESERVATIONS` for its own principal, so none of the three
    // should ever carry an entry for a principal absent from the live
    // registry going forward. This is the `post_upgrade` backstop for state
    // that reached these stores some other way (a pre-fix legacy snapshot,
    // or direct test/repair-tooling manipulation).
    SAMPLE_META.with(|m| {
        for (key, value) in m.borrow().iter() {
            let target = key.0;
            if !is_accepted_principal(target) {
                return Err(StateValidationError::OrphanSampleMeta { target });
            }
            let meta = value.into_current();
            // `total_writes` is the source of truth `record_sample`
            // maintains `filled_slots`/`next_slot` in lockstep with —
            // `filled_slots == total_writes.min(MAX)` and `next_slot ==
            // total_writes % MAX` always hold for any state this file's own
            // mutation helper can produce (final-correction item 5).
            let expected_filled_slots =
                meta.total_writes.min(MAX_SAMPLES_PER_TARGET_U32 as u64) as u32;
            let expected_next_slot = (meta.total_writes % MAX_SAMPLES_PER_TARGET_U32 as u64) as u32;
            let impossible = meta.filled_slots > MAX_SAMPLES_PER_TARGET_U32
                || meta.next_slot >= MAX_SAMPLES_PER_TARGET_U32
                || meta.filled_slots != expected_filled_slots
                || meta.next_slot != expected_next_slot;
            if impossible {
                return Err(StateValidationError::ImpossibleSampleMeta { target });
            }
        }
        Ok(())
    })?;
    SAMPLES.with(|m| {
        for (key, _) in m.borrow().iter() {
            if !is_accepted_principal(key.principal) {
                return Err(StateValidationError::OrphanSample {
                    target: key.principal,
                    slot: key.slot,
                });
            }
        }
        Ok(())
    })?;
    TARGET_RESERVATIONS.with(|m| {
        for (key, _) in m.borrow().iter() {
            let target = key.0;
            if !is_accepted_principal(target) {
                return Err(StateValidationError::OrphanTargetReservation { target });
            }
        }
        Ok(())
    })?;

    // ── Funding operations ──
    let operations: Vec<(u64, FundingOperation)> = FUNDING_OPERATIONS.with(|m| {
        m.borrow()
            .iter()
            .map(|(k, v)| (k, v.into_current()))
            .collect()
    });
    if operations.len() as u64 > types::MAX_FUNDING_OPERATIONS as u64 {
        return Err(StateValidationError::TooManyFundingOperations {
            count: operations.len() as u64,
        });
    }
    for (key, op) in &operations {
        if *key != op.id() {
            return Err(StateValidationError::FundingOperationKeyMismatch {
                key: *key,
                operation_id: op.id(),
            });
        }
        if let types::FundingRailArguments::Icp(snapshot) = op.rail_arguments() {
            crate::icp_cmc::validate_snapshot(
                snapshot,
                Some(op.created_at_secs()),
                Some(sentinel_id),
            )
            .map_err(|_| StateValidationError::IcpSnapshotInvalid {
                operation_id: op.id(),
            })?;
            let minimum = crate::icp_cmc::cycles_with_headroom(op.funding_policy().refill_cycles())
                .map_err(|_| StateValidationError::IcpSnapshotInvalid {
                    operation_id: op.id(),
                })?;
            if snapshot.expected_cycles < minimum {
                return Err(StateValidationError::IcpSnapshotInvalid {
                    operation_id: op.id(),
                });
            }
            let state = match op.state() {
                types::FundingOperationState::Icp(state) => state,
                _ => unreachable!("filtered to ICP rail above"),
            };
            // A CMC refund index is first stored as an operation-bound hint.
            // It may remain unresolved only in Quarantined, and a verified
            // index must be exactly the same value promoted from that hint.
            if op.refund_block_index().is_some()
                && op.refund_block_hint() != op.refund_block_index()
            {
                return Err(StateValidationError::IcpSnapshotInvalid {
                    operation_id: op.id(),
                });
            }
            if op.refund_block_hint().is_some()
                && !matches!(
                    state,
                    types::IcpFundingState::Quarantined | types::IcpFundingState::Refunded
                )
            {
                return Err(StateValidationError::IcpSnapshotInvalid {
                    operation_id: op.id(),
                });
            }
            if matches!(state, types::IcpFundingState::Refunded)
                && op.refund_block_index().is_none()
            {
                return Err(StateValidationError::IcpSnapshotInvalid {
                    operation_id: op.id(),
                });
            }
            if let Some(marker) = op.notify_attempt_started_at_secs() {
                if state != types::IcpFundingState::NotifyPending
                    || op.confirmed_block_index().is_none()
                    || marker < op.updated_at_secs()
                {
                    return Err(StateValidationError::IcpSnapshotInvalid {
                        operation_id: op.id(),
                    });
                }
            }
        } else if op.refund_block_hint().is_some()
            || op.refund_block_index().is_some()
            || op.notify_attempt_started_at_secs().is_some()
            || op.actual_cycles().is_some()
        {
            return Err(StateValidationError::IcpSnapshotInvalid {
                operation_id: op.id(),
            });
        }
    }
    let operations: Vec<FundingOperation> = operations.into_iter().map(|(_, op)| op).collect();
    // Operation target eligibility, scoped to UNRESOLVED operations only (a
    // resolved operation is an immutable historical snapshot, same framing
    // as the daily-cap check below): a `SelfRecovery`-triggered operation's
    // target must be exactly `sentinel_id`, and an ordinary (non-
    // `SelfRecovery`) operation's target must never be `sentinel_id` — only
    // the hard-coded self-recovery lane may ever move funds to the
    // Sentinel's own principal (final-correction Finding N5).
    for op in operations.iter().filter(|op| !op.state().is_resolved()) {
        if op.trigger() == FundingTrigger::SelfRecovery {
            if op.target() != sentinel_id {
                return Err(StateValidationError::SelfRecoveryOperationWrongTarget {
                    operation_id: op.id(),
                });
            }
        } else if op.target() == sentinel_id {
            return Err(StateValidationError::OrdinaryOperationTargetsSentinel {
                operation_id: op.id(),
            });
        } else if get_target(op.target()).is_none() {
            return Err(StateValidationError::OrphanFundingOperationTarget {
                operation_id: op.id(),
                target: op.target(),
            });
        }
    }
    // Live global-cap comparison applies only to unresolved, ORDINARY
    // (non-`SelfRecovery`) operations: a resolved operation is an immutable
    // historical snapshot that a later, unrelated `SetGlobalPolicy` tightening
    // must never retroactively invalidate (there is no mutator that could
    // "fix" it, and no eviction path — see the accounting-correction report).
    // Self-recovery is excluded because its `funding_policy` field is a
    // `TargetFundingPolicy` placeholder only (no self-recovery op is ever a
    // registered target), so comparing it against the ordinary-targets
    // global cap is a category error; self-recovery's own cap is enforced
    // below via `SelfRecoveryPolicy.daily_cap_cycles`.
    for op in operations
        .iter()
        .filter(|op| !op.state().is_resolved() && op.trigger() != FundingTrigger::SelfRecovery)
    {
        if op.funding_policy().daily_cap_cycles() > global_daily_cap_cycles {
            return Err(StateValidationError::OperationDailyCapExceedsGlobalCap {
                operation_id: op.id(),
            });
        }
    }

    // "Nonterminal" for reservation-linkage purposes = not yet
    // `is_resolved()`. This intentionally INCLUDES `Quarantined` (still
    // holds a reservation, awaiting signer action) and `Unknown`/
    // `TransferUnknown` (still awaiting reconciliation), unlike
    // `list_nonterminal_operations()` above, which uses
    // `!stops_automatic_retry()` for a different purpose (the timer's
    // automatic-retry resume loop must skip `Quarantined`).
    //
    // `SelfRecovery`-triggered operations are deliberately EXCLUDED from
    // this grouping: self-recovery is a distinct hard-coded lane that must
    // never use the ordinary per-target `TARGET_RESERVATIONS`/global
    // `GLOBAL_ROLLING_SPEND` stores this grouping feeds below. Its own
    // singleton `SELF_RECOVERY` linkage is checked separately. Excluding it
    // here also means that if a self-recovery operation's id ever DOES leak
    // into either ordinary store, the bidirectional checks below correctly
    // reject it (the reverse direction finds no matching forward entry).
    let mut nonterminal_by_target: std::collections::BTreeMap<Principal, Vec<&FundingOperation>> =
        std::collections::BTreeMap::new();
    for op in operations
        .iter()
        .filter(|op| !op.state().is_resolved() && op.trigger() != FundingTrigger::SelfRecovery)
    {
        nonterminal_by_target
            .entry(op.target())
            .or_default()
            .push(op);
    }
    for (target, ops) in &nonterminal_by_target {
        if ops.len() > 1 {
            return Err(
                StateValidationError::TooManyNonterminalOperationsForTarget {
                    target: *target,
                    count: ops.len(),
                },
            );
        }
    }

    // ── Bidirectional per-target reservation linkage ──
    let reservations: Vec<(Principal, TargetReservationState)> = TARGET_RESERVATIONS.with(|m| {
        m.borrow()
            .iter()
            .map(|(k, v)| (k.0, v.into_current()))
            .collect()
    });
    let reservation_by_target: std::collections::BTreeMap<Principal, TargetReservationState> =
        reservations.into_iter().collect();

    for reservation in reservation_by_target.values() {
        validate_rolling_spend_ledger(reservation.rolling_spend())?;
    }

    for (target, ops) in &nonterminal_by_target {
        let op = ops[0];
        let matching_reservation = reservation_by_target
            .get(target)
            .filter(|r| r.in_flight_operation_id() == Some(op.id()));
        match matching_reservation {
            Some(reservation) => {
                if reservation.in_flight_amount_cycles() != Some(op.reserved_amount_cycles()) {
                    return Err(StateValidationError::TargetReservationAmountMismatch {
                        target: *target,
                        operation_id: op.id(),
                    });
                }
            }
            None => {
                return Err(StateValidationError::TargetReservationMissingOperation {
                    target: *target,
                    operation_id: op.id(),
                });
            }
        }
    }
    for (target, reservation) in &reservation_by_target {
        if let Some(op_id) = reservation.in_flight_operation_id() {
            let matches = nonterminal_by_target
                .get(target)
                .map(|ops| ops[0].id() == op_id)
                .unwrap_or(false);
            if !matches {
                return Err(
                    StateValidationError::NonterminalOperationMissingTargetReservation {
                        operation_id: op_id,
                        target: *target,
                    },
                );
            }
        }
    }

    // ── Bidirectional global reservation linkage ──
    //
    // `GlobalRollingSpendState` backs the ordinary per-target funding cap
    // only. `nonterminal_by_target` already excludes `SelfRecovery`
    // operations (see above), so this check neither requires nor accepts a
    // `GLOBAL_ROLLING_SPEND` entry for a self-recovery operation id —
    // self-recovery has its own independent ledger, checked separately
    // below.
    let global_rolling_spend =
        GLOBAL_ROLLING_SPEND.with(|c| c.borrow().get().clone().into_current());
    validate_rolling_spend_ledger(global_rolling_spend.rolling_spend())?;
    let global_pending: Vec<PendingReservation> =
        global_rolling_spend.rolling_spend().pending().to_vec();
    let global_pending_by_op: std::collections::BTreeMap<u64, PendingReservation> = global_pending
        .into_iter()
        .map(|p| (p.operation_id, p))
        .collect();

    for ops in nonterminal_by_target.values() {
        let op = ops[0];
        match global_pending_by_op.get(&op.id()) {
            Some(entry) if entry.amount_cycles == op.reserved_amount_cycles() => {}
            Some(_) => {
                return Err(StateValidationError::GlobalReservationAmountMismatch {
                    operation_id: op.id(),
                })
            }
            None => {
                return Err(
                    StateValidationError::NonterminalOperationMissingGlobalReservation {
                        operation_id: op.id(),
                    },
                )
            }
        }
    }
    let nonterminal_op_ids: BTreeSet<u64> = nonterminal_by_target
        .values()
        .map(|ops| ops[0].id())
        .collect();
    for op_id in global_pending_by_op.keys() {
        if !nonterminal_op_ids.contains(op_id) {
            return Err(StateValidationError::GlobalReservationMissingOperation {
                operation_id: *op_id,
            });
        }
    }

    // ── Self-recovery in-flight link (bidirectional) ──
    //
    // Self-recovery owns its own independent `RollingSpendLedger` (memory ID
    // 12), governed by `SelfRecoveryPolicy.daily_cap_cycles` — never the
    // ordinary per-target/global rolling-spend stores checked above. Every
    // check below is specific to this singleton lane.
    let self_recovery = SELF_RECOVERY.with(|c| c.borrow().get().clone().into_current());
    validate_rolling_spend_ledger(self_recovery.rolling_spend())?;

    // Cap consistency: a live pending reservation must never exceed the
    // CURRENTLY configured daily cap. Unlike `OperationDailyCapExceedsGlobalCap`
    // above (which is scoped to `!is_resolved()` precisely so a resolved
    // historical snapshot is never re-judged against a policy that postdates
    // it), this reservation is still live/unresolved by construction — it is
    // the legitimate "does an in-flight reservation still respect the live
    // cap" check, not a historical-snapshot check.
    if let Some(entry) = self_recovery.rolling_spend().pending().first() {
        if entry.amount_cycles
            > config
                .global_policy
                .self_recovery_policy()
                .daily_cap_cycles()
        {
            return Err(StateValidationError::SelfRecoveryLedgerExceedsDailyCap {
                operation_id: entry.operation_id,
            });
        }
    }

    // Forward: if the ledger claims an in-flight operation, it must be real
    // and consistent (target, trigger, unresolved, and exact amount).
    if let Some(op_id) = self_recovery.in_flight_operation_id() {
        let op = operations.iter().find(|op| op.id() == op_id).ok_or(
            StateValidationError::SelfRecoveryOperationMissing {
                operation_id: op_id,
            },
        )?;
        if op.target() != sentinel_id {
            return Err(StateValidationError::SelfRecoveryOperationWrongTarget {
                operation_id: op_id,
            });
        }
        if op.trigger() != FundingTrigger::SelfRecovery {
            return Err(StateValidationError::SelfRecoveryOperationWrongTrigger {
                operation_id: op_id,
            });
        }
        if op.state().is_resolved() {
            return Err(StateValidationError::SelfRecoveryOperationAlreadyResolved {
                operation_id: op_id,
            });
        }
        if self_recovery.in_flight_amount_cycles() != Some(op.reserved_amount_cycles()) {
            return Err(StateValidationError::SelfRecoveryLedgerAmountMismatch {
                operation_id: op_id,
            });
        }
    }

    // Reverse: every unresolved `SelfRecovery`-triggered operation must be
    // the singleton's recorded in-flight operation — otherwise a live
    // self-recovery spend could exist while `is_suppressing_distribution()`
    // reports `false`, silently defeating the design's distribution-
    // suppression guarantee.
    for op in operations
        .iter()
        .filter(|op| op.trigger() == FundingTrigger::SelfRecovery && !op.state().is_resolved())
    {
        if self_recovery.in_flight_operation_id() != Some(op.id()) {
            return Err(
                StateValidationError::SelfRecoveryOperationNotRecordedInFlight {
                    operation_id: op.id(),
                },
            );
        }
    }

    // ── Bidirectional Cycles Ledger source-reserve linkage (Task 4) ──
    //
    // `SOURCE_RESERVE` is shared by BOTH ordinary and `SelfRecovery`-
    // triggered operations on the Cycles Ledger rail (unlike
    // `TARGET_RESERVATIONS`/`GLOBAL_ROLLING_SPEND`, which back ordinary
    // targets only, and unlike `SELF_RECOVERY`, which backs its own daily
    // cap only) — see `types::SourceReserveState`'s own doc comment. Every
    // unresolved Cycles-rail operation, from either lane, must have exactly
    // one matching pending debit whose amount equals that operation's own
    // snapshotted `amount_cycles + fee_cycles`.
    let unresolved_cycles_ops: Vec<&FundingOperation> = operations
        .iter()
        .filter(|op| !op.state().is_resolved() && op.rail() == FundingRail::CyclesLedger)
        .collect();
    let source_reserve = SOURCE_RESERVE.with(|c| c.borrow().get().clone().into_current());
    if source_reserve.pending().len() > types::MAX_PENDING_SOURCE_DEBITS {
        return Err(StateValidationError::SourceReservePendingAmountOverflow);
    }
    source_reserve
        .pending_total_cycles()
        .map_err(|_| StateValidationError::SourceReservePendingAmountOverflow)?;
    let source_pending: std::collections::BTreeMap<u64, PendingSourceDebit> = source_reserve
        .pending()
        .iter()
        .map(|p| (p.operation_id, *p))
        .collect();
    for op in &unresolved_cycles_ops {
        let types::FundingRailArguments::Cycles(snapshot) = op.rail_arguments() else {
            unreachable!("filtered to FundingRail::CyclesLedger above");
        };
        let expected_amount = snapshot
            .amount_cycles
            .checked_add(snapshot.fee_cycles)
            .ok_or(
                StateValidationError::FundingOperationAmountPlusFeeOverflow {
                    operation_id: op.id(),
                },
            )?;
        match source_pending.get(&op.id()) {
            Some(entry) if entry.amount_plus_fee_cycles == expected_amount => {}
            Some(_) => {
                return Err(StateValidationError::SourceReserveAmountMismatch {
                    operation_id: op.id(),
                })
            }
            None => {
                return Err(
                    StateValidationError::NonterminalOperationMissingSourceReserve {
                        operation_id: op.id(),
                    },
                )
            }
        }
    }
    let unresolved_cycles_op_ids: BTreeSet<u64> =
        unresolved_cycles_ops.iter().map(|op| op.id()).collect();
    for operation_id in source_pending.keys() {
        if !unresolved_cycles_op_ids.contains(operation_id) {
            return Err(StateValidationError::SourceReserveMissingOperation {
                operation_id: *operation_id,
            });
        }
    }

    // ── Bidirectional ICP Ledger source-reserve linkage (Task 5) ──
    //
    // ICP/CMC fallback operations reserve the Sentinel's ICP balance plus the
    // exact ICRC-1 fee before `icrc1_transfer`.  The hold remains present for
    // every unresolved state, including TransferUnknown, NotifyPending, and
    // Quarantined.  It must be impossible to compact an operation while this
    // reverse link still exists, or to reload an operation that lost its hold
    // across an upgrade.
    let unresolved_icp_ops: Vec<&FundingOperation> = operations
        .iter()
        .filter(|op| !op.state().is_resolved() && op.rail() == FundingRail::IcpCmc)
        .collect();
    let icp_source_reserve = ICP_SOURCE_RESERVE.with(|c| c.borrow().get().clone().into_current());
    if icp_source_reserve.pending().len() > types::MAX_PENDING_ICP_SOURCE_DEBITS {
        return Err(StateValidationError::IcpSourceReservePendingAmountOverflow);
    }
    icp_source_reserve
        .pending_total_e8s()
        .map_err(|_| StateValidationError::IcpSourceReservePendingAmountOverflow)?;
    let icp_source_pending: std::collections::BTreeMap<u64, PendingIcpSourceDebit> =
        icp_source_reserve
            .pending()
            .iter()
            .map(|p| (p.operation_id, *p))
            .collect();
    for op in &unresolved_icp_ops {
        let types::FundingRailArguments::Icp(snapshot) = op.rail_arguments() else {
            unreachable!("filtered to FundingRail::IcpCmc above");
        };
        let expected_amount = (snapshot.amount_e8s as u128)
            .checked_add(snapshot.fee_e8s as u128)
            .ok_or(StateValidationError::IcpSourceReservePendingAmountOverflow)?;
        match icp_source_pending.get(&op.id()) {
            Some(entry) if entry.amount_plus_fee_e8s == expected_amount => {}
            Some(_) => {
                return Err(StateValidationError::IcpSourceReserveAmountMismatch {
                    operation_id: op.id(),
                })
            }
            None => {
                return Err(
                    StateValidationError::NonterminalOperationMissingIcpSourceReserve {
                        operation_id: op.id(),
                    },
                )
            }
        }
    }
    let unresolved_icp_op_ids: BTreeSet<u64> =
        unresolved_icp_ops.iter().map(|op| op.id()).collect();
    for operation_id in icp_source_pending.keys() {
        if !unresolved_icp_op_ids.contains(operation_id) {
            return Err(StateValidationError::IcpSourceReserveMissingOperation {
                operation_id: *operation_id,
            });
        }
    }

    // ── Proposals ──
    let proposals: Vec<(u64, ProposalRecord)> = PROPOSALS.with(|m| {
        m.borrow()
            .iter()
            .map(|(k, v)| (k, v.into_current()))
            .collect()
    });
    if proposals.len() as u64 > types::MAX_PROPOSALS as u64 {
        return Err(StateValidationError::TooManyProposals {
            count: proposals.len() as u64,
        });
    }
    for (key, record) in &proposals {
        if *key != record.id {
            return Err(StateValidationError::ProposalKeyMismatch {
                key: *key,
                record_id: record.id,
            });
        }
        // Only `Open` proposals are checked against the CURRENT signer set.
        // `Executed`/`Cancelled` proposals are closed historical records —
        // their approvals were valid against the signer set at the time and
        // must never be retroactively invalidated by a later, unrelated
        // `RemoveSigner` proposal (an ordinary governance action). There is
        // no mutator that could ever strip an approval from a closed
        // proposal, so re-checking it against a shrunk signer set would
        // brick every future upgrade permanently.
        if record.status == ProposalStatus::Open {
            for approver in record.approvals() {
                if !signers_set.contains(approver) {
                    return Err(StateValidationError::ProposalApprovalNotASigner {
                        proposal_id: record.id,
                        approver: *approver,
                    });
                }
            }
        }
    }

    // ── Bounded maps not already covered above ──
    let alarm_count = ALARMS.with(|m| m.borrow().len());
    if alarm_count > types::MAX_ALARMS as u64 {
        return Err(StateValidationError::TooManyAlarms { count: alarm_count });
    }
    ALARMS.with(|m| {
        for (key, value) in m.borrow().iter() {
            let alarm = value.into_current();
            if key != alarm.id {
                return Err(StateValidationError::AlarmKeyMismatch {
                    key,
                    alarm_id: alarm.id,
                });
            }
        }
        Ok(())
    })?;
    let terminal_summary_count = TERMINAL_SUMMARIES.with(|m| m.borrow().len());
    if terminal_summary_count > types::MAX_TERMINAL_SUMMARIES as u64 {
        return Err(StateValidationError::TooManyTerminalSummaries {
            count: terminal_summary_count,
        });
    }
    TERMINAL_SUMMARIES.with(|m| {
        for (key, value) in m.borrow().iter() {
            let summary = value.into_current();
            if key != summary.operation_id() {
                return Err(StateValidationError::TerminalSummaryKeyMismatch {
                    key,
                    operation_id: summary.operation_id(),
                });
            }
        }
        Ok(())
    })?;

    Ok(())
}

// ─────────────────────── Tests ───────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        AdvisoryCyclesBalance, AlarmStatus, Criticality, CyclesFundingState,
        CyclesWithdrawSnapshot, Environment, FundingAttemptResultClass, FundingOperationState,
        FundingRailArguments, GlobalPolicyArgs, GovernanceTimelocksArgs, ObservationMode,
        ProposalPayload, PublicTargetState, SelfRecoveryPolicyArgs, TargetArgs,
        TargetFundingPolicy, TargetFundingPolicyArgs, TargetPatch, TargetRegistrationContext,
    };
    use candid::Nat;

    // ── test fixtures ──

    fn test_sentinel_id() -> Principal {
        Principal::from_slice(&[9; 5])
    }

    fn test_signer(seed: u8) -> Principal {
        Principal::from_slice(&[seed; 10])
    }

    fn test_target_principal(seed: u8) -> Principal {
        Principal::from_slice(&[7, seed])
    }

    fn test_governance_timelocks_args() -> GovernanceTimelocksArgs {
        GovernanceTimelocksArgs {
            target_registry_secs: 3_600,
            spend_policy_secs: 3_600,
            signer_change_secs: 3_600,
            unpause_secs: 3_600,
        }
    }

    fn test_self_recovery_args(
        reserve: u128,
        cap: u128,
        threshold: u128,
        refill: u128,
    ) -> SelfRecoveryPolicyArgs {
        SelfRecoveryPolicyArgs {
            protected_reserve_cycles: Nat::from(reserve),
            daily_cap_cycles: Nat::from(cap),
            low_balance_threshold_cycles: Nat::from(threshold),
            refill_cycles: Nat::from(refill),
        }
    }

    fn test_global_policy_args(cap: u128) -> GlobalPolicyArgs {
        GlobalPolicyArgs {
            global_daily_cap_cycles: Nat::from(cap),
            sample_interval_secs: 300,
            stale_after_secs: 600,
            min_icp_reserve_e8s: Nat::from(100_000_000u64),
            timelocks: test_governance_timelocks_args(),
            self_recovery_policy: test_self_recovery_args(1_000, 100, 1, 10),
        }
    }

    fn test_global_policy(cap: u128) -> GlobalPolicy {
        GlobalPolicy::validate(&test_global_policy_args(cap)).unwrap()
    }

    fn test_init_args(signers: Vec<Principal>, threshold: u32, cap: u128) -> InitArgs {
        InitArgs {
            signers,
            approval_threshold: threshold,
            global_policy: test_global_policy_args(cap),
        }
    }

    fn test_funding_policy_args(low: u128, refill: u128, cap: u128) -> TargetFundingPolicyArgs {
        TargetFundingPolicyArgs {
            low_balance_threshold_cycles: Nat::from(low),
            refill_cycles: Nat::from(refill),
            daily_cap_cycles: Nat::from(cap),
            cooldown_secs: 60,
            burn_anomaly_limit_cycles_per_day: None,
        }
    }

    fn test_target_record(seed: u8, global: &GlobalPolicy) -> TargetRecord {
        let empty = BTreeSet::new();
        let ctx = TargetRegistrationContext {
            sentinel_id: test_sentinel_id(),
            existing_target_count: 0,
            existing_target_principals: &empty,
            global_policy: global,
        };
        let args = TargetArgs {
            principal: test_target_principal(seed),
            display_name: "svc".to_string(),
            project: "proj".to_string(),
            environment: Environment::Production,
            criticality: Criticality::Standard,
            observation_mode: ObservationMode::SelfReport,
            tags: vec![],
            funding_policy: test_funding_policy_args(1, 10, 100),
        };
        TargetRecord::register(args, &ctx).unwrap()
    }

    fn test_funding_operation(
        id: u64,
        target: Principal,
        trigger: FundingTrigger,
        global: &GlobalPolicy,
        now: u64,
    ) -> FundingOperation {
        let funding_policy =
            TargetFundingPolicy::validate(&test_funding_policy_args(1, 10, 100), global).unwrap();
        let rail_arguments = FundingRailArguments::Cycles(CyclesWithdrawSnapshot {
            destination: target,
            from_subaccount: None,
            amount_cycles: 10,
            fee_cycles: 0,
            created_at_time_ns: now * 1_000_000_000,
        });
        FundingOperation::open(
            id,
            target,
            1,
            funding_policy,
            trigger,
            rail_arguments,
            10,
            now,
        )
        .unwrap()
    }

    fn test_resolved_operation(
        id: u64,
        target: Principal,
        global: &GlobalPolicy,
        now: u64,
    ) -> FundingOperation {
        let op =
            test_funding_operation(id, target, FundingTrigger::LowBalanceAutoTopup, global, now);
        let op = op
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Submitted),
                now,
                FundingAttemptResultClass::Success,
            )
            .unwrap();
        let op = op
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Confirmed),
                now,
                FundingAttemptResultClass::Success,
            )
            .unwrap();
        op.record_attempt(
            FundingOperationState::Cycles(CyclesFundingState::Complete),
            now,
            FundingAttemptResultClass::Success,
        )
        .unwrap()
    }

    fn test_sample(secs: u64, state: PublicTargetState) -> Sample {
        Sample {
            timestamp_secs: secs,
            balance: Some(AdvisoryCyclesBalance::Exact(1_000_000)),
            state,
            reported_operational_healthy: None,
            burn_cycles_per_hour: Some(10),
        }
    }

    fn test_alarm(id: u64, target: Option<Principal>) -> Alarm {
        Alarm {
            id,
            target,
            kind: AlarmKind::LowBalance,
            status: types::AlarmStatus::Open,
            opened_at_secs: 100,
            acknowledged_at_secs: None,
            resolved_at_secs: None,
        }
    }

    fn init_test_state(signers: Vec<Principal>, threshold: u32, cap: u128) {
        init(test_init_args(signers, threshold, cap)).unwrap();
    }

    // ── memory layout ──

    #[test]
    fn memory_ids_unique() {
        let mut seen = BTreeSet::new();
        for (id, label) in MEMORY_LAYOUT {
            assert!(
                seen.insert(*id),
                "duplicate stable MemoryId {id:?} (store {label:?}) — pick an unused slot"
            );
        }
        assert_eq!(MEMORY_LAYOUT.len(), 17, "expected exactly 17 memory ids");
    }

    // ── round-trip tests, one per stable structure (17) ──

    #[test]
    fn global_config_round_trips() {
        let signers = vec![test_signer(1)];
        init_test_state(signers.clone(), 1, 1_000_000);
        let config = global_config();
        assert_eq!(config.signers, signers);
        assert_eq!(config.approval_threshold, 1);
    }

    #[test]
    fn target_registry_round_trips() {
        let global = test_global_policy(1_000);
        let record = test_target_record(1, &global);
        let principal = record.principal();
        insert_target(record.clone()).unwrap();
        assert_eq!(get_target(principal), Some(record));
    }

    #[test]
    fn proposals_round_trip() {
        let proposer = test_signer(1);
        let record = ProposalRecord::new(
            7,
            ProposalPayload::AddSigner {
                signer: test_signer(2),
            },
            proposer,
            100,
        );
        insert_proposal(record.clone()).unwrap();
        assert_eq!(get_proposal(7), Some(record));
    }

    #[test]
    fn governance_counters_round_trip() {
        let first = next_proposal_id();
        let second = next_proposal_id();
        assert_eq!(first, 0);
        assert_eq!(second, 1);
    }

    #[test]
    fn samples_round_trip() {
        let global = test_global_policy(1_000);
        let record = test_target_record(1, &global);
        let target = record.principal();
        insert_target(record).unwrap();
        let sample = test_sample(500, PublicTargetState::Healthy);
        record_sample(target, sample.clone()).unwrap();
        let samples = list_samples(target, None, 10).unwrap();
        assert_eq!(samples, vec![(1, sample)]);
    }

    #[test]
    fn record_sample_rejects_unregistered_target() {
        let target = test_target_principal(1);
        assert_eq!(
            record_sample(target, test_sample(1, PublicTargetState::Healthy)),
            Err(RecordSampleError::UnregisteredTarget)
        );
        assert_eq!(sample_meta(target), None);
    }

    #[test]
    fn sample_meta_round_trips() {
        let global = test_global_policy(1_000);
        let record = test_target_record(1, &global);
        let target = record.principal();
        insert_target(record).unwrap();
        assert_eq!(sample_meta(target), None);
        record_sample(target, test_sample(1, PublicTargetState::Healthy)).unwrap();
        let meta = sample_meta(target).unwrap();
        assert_eq!(meta.next_slot, 1);
        assert_eq!(meta.filled_slots, 1);
        assert_eq!(meta.last_success_at_secs, Some(1));
        assert_eq!(meta.last_attempt_at_secs, Some(1));
        assert_eq!(meta.total_writes, 1);
    }

    #[test]
    fn alarms_round_trip() {
        let alarm = test_alarm(3, Some(test_target_principal(1)));
        insert_alarm(alarm.clone()).unwrap();
        assert_eq!(get_alarm(3), Some(alarm));
    }

    #[test]
    fn alarm_counters_round_trip() {
        let first = next_alarm_id();
        let second = next_alarm_id();
        assert_eq!(first, 0);
        assert_eq!(second, 1);
    }

    #[test]
    fn funding_operations_round_trip() {
        let global = test_global_policy(1_000);
        let target = register_test_target(1, &global);
        let op =
            test_funding_operation(1, target, FundingTrigger::LowBalanceAutoTopup, &global, 10);
        insert_operation(op.clone()).unwrap();
        assert_eq!(get_operation(1), Some(op));
    }

    #[test]
    fn funding_counters_round_trip() {
        let first = next_operation_id();
        let second = next_operation_id();
        assert_eq!(first, 0);
        assert_eq!(second, 1);
    }

    #[test]
    fn target_reservations_round_trip() {
        let target = test_target_principal(1);
        let state = TargetReservationState::new()
            .reserve(1, 10, 100, 86_400, 1_000)
            .unwrap();
        set_target_reservation(target, state.clone());
        assert_eq!(get_target_reservation(target), state);
    }

    #[test]
    fn global_rolling_spend_round_trips() {
        let state = GlobalRollingSpendState::new()
            .reserve(1, 10, 100, 86_400, 1_000)
            .unwrap();
        set_global_rolling_spend(state.clone());
        assert_eq!(get_global_rolling_spend(), state);
    }

    #[test]
    fn self_recovery_state_round_trips() {
        let state = SelfRecoveryState::new()
            .begin(1, 10, 100, 86_400, 1_000)
            .unwrap();
        set_self_recovery_state(state.clone());
        assert_eq!(get_self_recovery_state(), state);
    }

    #[test]
    fn source_reserve_round_trips() {
        let state = SourceReserveState::new()
            .refresh(1_000, 1, 50)
            .unwrap()
            .reserve_ordinary(1, 11, 0, 50, 60)
            .unwrap();
        set_source_reserve(state.clone());
        assert_eq!(get_source_reserve(), state);
    }

    #[test]
    fn terminal_summaries_round_trip() {
        let global = test_global_policy(1_000);
        let target = test_target_principal(1);
        let op = test_resolved_operation(1, target, &global, 10);
        let summary = TerminalFundingSummary::from_resolved(&op, 20).unwrap();
        insert_terminal_summary(summary.clone());
        let listed = list_terminal_summaries_for_target(target, 10);
        assert_eq!(listed, vec![summary]);
    }

    // ── legacy envelope decode tests, one per Stored* type (14) ──
    //
    // Storage review Finding 5: `candid::encode_one(&StoredX::V1(value))` is
    // the IDENTICAL call `impl_candid_storable!`'s own `to_bytes()` makes
    // internally, so building "legacy" bytes that way is a round-trip test
    // wearing a legacy label — it can never notice `to_bytes()` itself
    // diverging from `candid::encode_one` (e.g. a future refactor that adds
    // a checksum or an outer wrapper). Every test below instead decodes a
    // FROZEN byte fixture that does not depend on calling `to_bytes()` (or
    // `candid::encode_one` inline) at all:
    //
    // - The four bookkeeping types this file owns alone (`GovernanceCounters`,
    //   `SampleMeta`, `AlarmCounters`, `FundingCounters`) use a test-only
    //   shadow struct with its own independent `#[derive(CandidType,
    //   Serialize)]`, mirroring the `SelfRecoveryStateV1` shadow-type pattern
    //   already used for the self-recovery migration test below.
    // - The remaining nine wrap `types.rs` domain types with enough nested
    //   custom-encoded fields (`BoundedName`, `TargetFundingPolicy`,
    //   `ProposalPayload`, `FundingRailArguments`, ...) that a hand-written
    //   shadow encoder would be large and easy to get subtly wrong,
    //   so they use the review's other sanctioned option instead: a
    //   committed, deterministic byte fixture captured once from a real
    //   `to_bytes()` call and frozen as a literal constant. A future change
    //   to `impl_candid_storable!`'s `to_bytes` cannot silently rewrite both
    //   the production encoder and this fixture in lockstep, because the
    //   fixture is data, not a call.

    #[test]
    fn stored_governance_counters_v1_legacy_envelope_decodes() {
        #[derive(CandidType, serde::Serialize)]
        struct RawGovernanceCountersV1ForTest {
            next_proposal_id: u64,
        }
        #[derive(CandidType, serde::Serialize)]
        enum RawStoredGovernanceCountersForTest {
            V1(RawGovernanceCountersV1ForTest),
        }
        let raw = RawStoredGovernanceCountersForTest::V1(RawGovernanceCountersV1ForTest {
            next_proposal_id: 42,
        });
        let bytes = candid::encode_one(&raw).unwrap();
        let decoded =
            <StoredGovernanceCounters as Storable>::from_bytes(Cow::Owned(bytes)).into_current();
        assert_eq!(decoded.next_proposal_id, 42);
    }

    #[test]
    fn stored_sample_meta_v1_legacy_envelope_decodes() {
        #[derive(CandidType, serde::Serialize)]
        struct RawSampleMetaV1ForTest {
            next_slot: u32,
            filled_slots: u32,
            last_success_at_secs: Option<u64>,
            last_attempt_at_secs: Option<u64>,
        }
        #[derive(CandidType, serde::Serialize)]
        enum RawStoredSampleMetaForTest {
            V1(RawSampleMetaV1ForTest),
        }
        let raw = RawStoredSampleMetaForTest::V1(RawSampleMetaV1ForTest {
            next_slot: 3,
            filled_slots: 3,
            last_success_at_secs: Some(10),
            last_attempt_at_secs: Some(10),
        });
        let bytes = candid::encode_one(&raw).unwrap();
        let decoded = <StoredSampleMeta as Storable>::from_bytes(Cow::Owned(bytes)).into_current();
        assert_eq!(decoded.next_slot, 3);
        assert_eq!(decoded.filled_slots, 3);
        assert_eq!(decoded.last_success_at_secs, Some(10));
        assert_eq!(decoded.last_attempt_at_secs, Some(10));
        // Pre-wrap migration (cursor final-fix, item 5): a legacy ring that
        // never wrapped has every write still retained, so `total_writes`
        // must migrate to exactly `filled_slots`.
        assert_eq!(decoded.total_writes, 3);
    }

    #[test]
    fn stored_sample_meta_v1_legacy_envelope_decodes_full_ring_assumes_no_wrap() {
        // A legacy V1 ring at full capacity carries no evidence of how many
        // times it actually wrapped before this migration ever runs.
        // `From<SampleMetaV1>` deliberately assumes no wrap occurred —
        // `total_writes = filled_slots` — the same conservative,
        // fail-safe-by-undercounting choice documented on that impl and on
        // `SelfRecoveryState::from_legacy`. Independent shadow encoder, not
        // the production `SampleMetaV1` type, per this file's V1 wire-proof
        // convention.
        #[derive(CandidType, serde::Serialize)]
        struct RawSampleMetaV1ForTest {
            next_slot: u32,
            filled_slots: u32,
            last_success_at_secs: Option<u64>,
            last_attempt_at_secs: Option<u64>,
        }
        #[derive(CandidType, serde::Serialize)]
        enum RawStoredSampleMetaForTest {
            V1(RawSampleMetaV1ForTest),
        }
        let raw = RawStoredSampleMetaForTest::V1(RawSampleMetaV1ForTest {
            next_slot: 0,
            filled_slots: MAX_SAMPLES_PER_TARGET_U32,
            last_success_at_secs: Some(20),
            last_attempt_at_secs: Some(20),
        });
        let bytes = candid::encode_one(&raw).unwrap();
        let decoded = <StoredSampleMeta as Storable>::from_bytes(Cow::Owned(bytes)).into_current();
        assert_eq!(decoded.next_slot, 0);
        assert_eq!(decoded.filled_slots, MAX_SAMPLES_PER_TARGET_U32);
        assert_eq!(decoded.total_writes, MAX_SAMPLES_PER_TARGET_U32 as u64);
    }

    #[test]
    fn stored_alarm_counters_v1_legacy_envelope_decodes() {
        #[derive(CandidType, serde::Serialize)]
        struct RawAlarmCountersV1ForTest {
            next_alarm_id: u64,
        }
        #[derive(CandidType, serde::Serialize)]
        enum RawStoredAlarmCountersForTest {
            V1(RawAlarmCountersV1ForTest),
        }
        let raw = RawStoredAlarmCountersForTest::V1(RawAlarmCountersV1ForTest { next_alarm_id: 7 });
        let bytes = candid::encode_one(&raw).unwrap();
        let decoded =
            <StoredAlarmCounters as Storable>::from_bytes(Cow::Owned(bytes)).into_current();
        assert_eq!(decoded.next_alarm_id, 7);
    }

    #[test]
    fn stored_funding_counters_v1_legacy_envelope_decodes() {
        #[derive(CandidType, serde::Serialize)]
        struct RawFundingCountersV1ForTest {
            next_operation_id: u64,
            last_created_at_time_ns: u64,
        }
        #[derive(CandidType, serde::Serialize)]
        enum RawStoredFundingCountersForTest {
            V1(RawFundingCountersV1ForTest),
        }
        let raw = RawStoredFundingCountersForTest::V1(RawFundingCountersV1ForTest {
            next_operation_id: 5,
            last_created_at_time_ns: 999,
        });
        let bytes = candid::encode_one(&raw).unwrap();
        let decoded =
            <StoredFundingCounters as Storable>::from_bytes(Cow::Owned(bytes)).into_current();
        assert_eq!(decoded.next_operation_id, 5);
        assert_eq!(decoded.last_created_at_time_ns, 999);
    }

    /// Frozen fixture captured once from `candid::encode_one(&StoredGlobalConfig::V1(Some(GlobalConfig {
    /// signers: vec![test_signer(1)], approval_threshold: 1, global_policy: test_global_policy(1_000) })))`.
    const GLOBAL_CONFIG_V1_FIXTURE: &[u8] = &[
        68, 73, 68, 76, 7, 107, 1, 155, 150, 1, 1, 110, 2, 108, 3, 175, 152, 182, 185, 2, 121, 201,
        246, 149, 135, 4, 3, 142, 253, 147, 132, 12, 4, 109, 104, 108, 6, 220, 160, 131, 174, 1,
        125, 233, 181, 164, 234, 1, 5, 167, 228, 235, 135, 7, 120, 251, 237, 208, 213, 9, 6, 171,
        141, 144, 255, 11, 120, 135, 177, 195, 229, 13, 125, 108, 4, 182, 195, 203, 188, 4, 125,
        239, 140, 220, 190, 7, 125, 192, 165, 231, 254, 10, 125, 161, 138, 171, 188, 15, 125, 108,
        4, 132, 249, 239, 237, 10, 120, 206, 177, 204, 176, 11, 120, 150, 224, 204, 137, 13, 120,
        156, 222, 135, 188, 15, 120, 1, 0, 0, 1, 1, 0, 0, 0, 1, 1, 10, 1, 1, 1, 1, 1, 1, 1, 1, 1,
        1, 232, 7, 10, 1, 100, 232, 7, 44, 1, 0, 0, 0, 0, 0, 0, 16, 14, 0, 0, 0, 0, 0, 0, 16, 14,
        0, 0, 0, 0, 0, 0, 16, 14, 0, 0, 0, 0, 0, 0, 16, 14, 0, 0, 0, 0, 0, 0, 88, 2, 0, 0, 0, 0, 0,
        0, 128, 194, 215, 47,
    ];

    #[test]
    fn stored_global_config_v1_legacy_envelope_decodes() {
        let global = test_global_policy(1_000);
        let expected = GlobalConfig {
            signers: vec![test_signer(1)],
            approval_threshold: 1,
            global_policy: global,
        };
        let decoded =
            <StoredGlobalConfig as Storable>::from_bytes(Cow::Borrowed(GLOBAL_CONFIG_V1_FIXTURE))
                .into_current();
        assert_eq!(decoded, Some(expected));
    }

    /// Frozen fixture captured once from `candid::encode_one(&StoredTargetRecord::V1(test_target_record(1, &test_global_policy(1_000))))`.
    const TARGET_RECORD_V1_FIXTURE: &[u8] = &[
        68, 73, 68, 76, 8, 107, 1, 155, 150, 1, 1, 108, 12, 174, 157, 177, 144, 1, 104, 160, 128,
        181, 186, 4, 126, 217, 233, 218, 231, 4, 2, 168, 233, 239, 169, 7, 113, 129, 137, 196, 241,
        7, 126, 239, 229, 129, 226, 10, 3, 211, 151, 192, 234, 10, 4, 219, 255, 198, 255, 12, 120,
        174, 129, 145, 252, 14, 126, 150, 194, 212, 148, 15, 5, 217, 165, 172, 175, 15, 113, 244,
        232, 234, 178, 15, 6, 109, 113, 107, 4, 130, 205, 221, 39, 127, 200, 236, 251, 54, 127,
        191, 145, 167, 195, 8, 127, 221, 230, 167, 161, 12, 127, 107, 5, 203, 242, 248, 96, 127,
        153, 148, 198, 187, 2, 127, 242, 232, 203, 190, 3, 127, 130, 229, 128, 185, 12, 127, 219,
        206, 250, 216, 15, 127, 107, 3, 128, 227, 197, 250, 7, 127, 178, 199, 178, 148, 10, 127,
        129, 207, 140, 150, 10, 127, 108, 5, 182, 195, 203, 188, 4, 125, 248, 138, 185, 221, 6, 7,
        239, 140, 220, 190, 7, 125, 192, 165, 231, 254, 10, 125, 150, 208, 216, 236, 15, 120, 110,
        125, 1, 0, 0, 1, 2, 7, 1, 0, 0, 3, 115, 118, 99, 0, 3, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4,
        112, 114, 111, 106, 10, 0, 1, 100, 60, 0, 0, 0, 0, 0, 0, 0,
    ];

    #[test]
    fn stored_target_record_v1_legacy_envelope_decodes() {
        let global = test_global_policy(1_000);
        let expected = test_target_record(1, &global);
        let decoded =
            <StoredTargetRecord as Storable>::from_bytes(Cow::Borrowed(TARGET_RECORD_V1_FIXTURE))
                .into_current();
        assert_eq!(decoded, expected);
    }

    /// Frozen fixture captured once from `candid::encode_one(&StoredProposalRecord::V1(ProposalRecord::new(1,
    /// ProposalPayload::RemoveSigner { signer: test_signer(2) }, test_signer(1), 10)))`.
    const PROPOSAL_RECORD_V1_FIXTURE: &[u8] = &[
        68, 73, 68, 76, 27, 107, 1, 155, 150, 1, 1, 108, 6, 219, 183, 1, 120, 178, 206, 239, 47, 2,
        180, 191, 212, 150, 11, 104, 183, 226, 194, 140, 12, 120, 142, 255, 214, 233, 14, 3, 144,
        176, 133, 139, 15, 26, 107, 3, 234, 223, 180, 164, 3, 127, 175, 193, 182, 200, 9, 127, 241,
        204, 157, 160, 12, 127, 107, 8, 203, 141, 245, 115, 4, 218, 204, 139, 246, 4, 5, 255, 181,
        171, 201, 10, 20, 213, 181, 179, 241, 11, 21, 215, 144, 216, 181, 13, 22, 142, 194, 162,
        161, 14, 21, 238, 178, 152, 222, 14, 4, 180, 156, 144, 188, 15, 25, 108, 1, 234, 227, 152,
        164, 11, 104, 108, 2, 174, 157, 177, 144, 1, 104, 200, 141, 220, 234, 11, 6, 108, 9, 160,
        128, 181, 186, 4, 7, 217, 233, 218, 231, 4, 8, 168, 233, 239, 169, 7, 10, 129, 137, 196,
        241, 7, 7, 239, 229, 129, 226, 10, 11, 211, 151, 192, 234, 10, 13, 150, 194, 212, 148, 15,
        15, 217, 165, 172, 175, 15, 10, 244, 232, 234, 178, 15, 17, 110, 126, 110, 9, 109, 113,
        110, 113, 110, 12, 107, 4, 130, 205, 221, 39, 127, 200, 236, 251, 54, 127, 191, 145, 167,
        195, 8, 127, 221, 230, 167, 161, 12, 127, 110, 14, 107, 5, 203, 242, 248, 96, 127, 153,
        148, 198, 187, 2, 127, 242, 232, 203, 190, 3, 127, 130, 229, 128, 185, 12, 127, 219, 206,
        250, 216, 15, 127, 110, 16, 107, 3, 128, 227, 197, 250, 7, 127, 178, 199, 178, 148, 10,
        127, 129, 207, 140, 150, 10, 127, 110, 18, 108, 5, 182, 195, 203, 188, 4, 125, 248, 138,
        185, 221, 6, 19, 239, 140, 220, 190, 7, 125, 192, 165, 231, 254, 10, 125, 150, 208, 216,
        236, 15, 120, 110, 125, 108, 1, 171, 135, 143, 165, 3, 121, 108, 1, 174, 157, 177, 144, 1,
        104, 108, 6, 220, 160, 131, 174, 1, 125, 233, 181, 164, 234, 1, 23, 167, 228, 235, 135, 7,
        120, 251, 237, 208, 213, 9, 24, 171, 141, 144, 255, 11, 120, 135, 177, 195, 229, 13, 125,
        108, 4, 182, 195, 203, 188, 4, 125, 239, 140, 220, 190, 7, 125, 192, 165, 231, 254, 10,
        125, 161, 138, 171, 188, 15, 125, 108, 4, 132, 249, 239, 237, 10, 120, 206, 177, 204, 176,
        11, 120, 150, 224, 204, 137, 13, 120, 156, 222, 135, 188, 15, 120, 108, 8, 174, 157, 177,
        144, 1, 104, 217, 233, 218, 231, 4, 9, 168, 233, 239, 169, 7, 113, 239, 229, 129, 226, 10,
        12, 211, 151, 192, 234, 10, 14, 150, 194, 212, 148, 15, 16, 217, 165, 172, 175, 15, 113,
        244, 232, 234, 178, 15, 18, 109, 104, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 1, 10, 1, 1, 1,
        1, 1, 1, 1, 1, 1, 1, 10, 0, 0, 0, 0, 0, 0, 0, 6, 1, 10, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 0,
    ];

    #[test]
    fn stored_proposal_record_v1_legacy_envelope_decodes() {
        let expected = ProposalRecord::new(
            1,
            ProposalPayload::RemoveSigner {
                signer: test_signer(2),
            },
            test_signer(1),
            10,
        );
        let decoded = <StoredProposalRecord as Storable>::from_bytes(Cow::Borrowed(
            PROPOSAL_RECORD_V1_FIXTURE,
        ))
        .into_current();
        assert_eq!(decoded, expected);
    }

    /// Frozen fixture captured once from `candid::encode_one(&StoredSample::V1(test_sample(42, PublicTargetState::Low)))`.
    const SAMPLE_V1_FIXTURE: &[u8] = &[
        68, 73, 68, 76, 5, 107, 1, 155, 150, 1, 1, 108, 4, 156, 186, 182, 156, 2, 2, 145, 236, 173,
        160, 8, 3, 136, 169, 232, 165, 11, 4, 139, 134, 198, 189, 12, 120, 107, 2, 159, 165, 134,
        82, 125, 226, 190, 182, 215, 1, 127, 107, 6, 244, 152, 232, 1, 127, 237, 243, 203, 133, 1,
        127, 189, 144, 186, 173, 3, 127, 244, 193, 177, 154, 6, 127, 129, 207, 140, 150, 10, 127,
        161, 145, 160, 157, 11, 127, 110, 125, 1, 0, 0, 0, 192, 132, 61, 0, 1, 10, 42, 0, 0, 0, 0,
        0, 0, 0,
    ];

    #[test]
    fn stored_sample_v1_legacy_envelope_decodes() {
        let expected = test_sample(42, PublicTargetState::Low);
        let decoded =
            <StoredSample as Storable>::from_bytes(Cow::Borrowed(SAMPLE_V1_FIXTURE)).into_current();
        assert_eq!(decoded, expected);
    }

    /// Frozen fixture captured once from `candid::encode_one(&StoredAlarm::V1(test_alarm(9, None)))`.
    const ALARM_V1_FIXTURE: &[u8] = &[
        68, 73, 68, 76, 6, 107, 1, 155, 150, 1, 1, 108, 7, 219, 183, 1, 120, 178, 206, 239, 47, 2,
        151, 216, 224, 141, 2, 3, 212, 194, 167, 184, 4, 4, 209, 230, 179, 183, 8, 5, 248, 141,
        154, 234, 13, 120, 199, 142, 140, 129, 15, 3, 107, 3, 234, 223, 180, 164, 3, 127, 232, 185,
        250, 161, 4, 127, 152, 240, 136, 179, 14, 127, 110, 120, 107, 5, 136, 222, 231, 160, 1,
        127, 210, 239, 217, 231, 1, 127, 148, 228, 206, 212, 5, 127, 244, 193, 177, 154, 6, 127,
        153, 188, 140, 216, 12, 127, 110, 104, 1, 0, 0, 9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 100, 0,
        0, 0, 0, 0, 0, 0, 0,
    ];

    #[test]
    fn stored_alarm_v1_legacy_envelope_decodes() {
        let expected = test_alarm(9, None);
        let decoded =
            <StoredAlarm as Storable>::from_bytes(Cow::Borrowed(ALARM_V1_FIXTURE)).into_current();
        assert_eq!(decoded, expected);
    }

    /// Frozen fixture captured once from `candid::encode_one(&StoredFundingOperation::V1(test_funding_operation(1,
    /// test_target_principal(1), FundingTrigger::ManualTopup, &test_global_policy(1_000), 10)))`.
    const FUNDING_OPERATION_V1_FIXTURE: &[u8] = &[
        68, 73, 68, 76, 17, 107, 1, 155, 150, 1, 1, 108, 12, 219, 183, 1, 120, 184, 170, 253, 174,
        2, 2, 189, 249, 155, 151, 4, 125, 198, 197, 220, 168, 5, 3, 138, 145, 155, 129, 7, 120,
        145, 236, 173, 160, 8, 6, 209, 230, 179, 183, 8, 104, 192, 138, 135, 222, 8, 9, 175, 204,
        140, 186, 11, 120, 233, 189, 198, 245, 11, 10, 183, 226, 194, 140, 12, 120, 244, 232, 234,
        178, 15, 15, 107, 3, 153, 212, 197, 232, 3, 127, 161, 250, 155, 221, 13, 127, 234, 239,
        194, 243, 15, 127, 109, 4, 108, 4, 206, 243, 165, 147, 2, 120, 150, 216, 166, 176, 2, 5,
        241, 172, 171, 178, 7, 121, 187, 208, 164, 143, 12, 6, 107, 4, 142, 204, 199, 235, 2, 127,
        219, 141, 233, 177, 5, 127, 163, 155, 253, 172, 8, 127, 136, 191, 140, 146, 13, 127, 107,
        2, 182, 246, 222, 1, 7, 173, 146, 159, 185, 11, 8, 107, 9, 238, 160, 128, 162, 1, 127, 255,
        222, 158, 160, 2, 127, 247, 141, 171, 203, 2, 127, 172, 177, 167, 204, 2, 127, 148, 209,
        233, 250, 2, 127, 217, 249, 230, 203, 5, 127, 156, 187, 243, 204, 12, 127, 150, 139, 202,
        219, 13, 127, 178, 211, 248, 253, 15, 127, 107, 7, 172, 177, 167, 204, 2, 127, 217, 249,
        230, 203, 5, 127, 191, 170, 184, 203, 6, 127, 234, 150, 177, 246, 10, 127, 156, 187, 243,
        204, 12, 127, 155, 137, 164, 212, 12, 127, 150, 139, 202, 219, 13, 127, 110, 120, 107, 2,
        182, 246, 222, 1, 11, 173, 146, 159, 185, 11, 14, 108, 10, 209, 206, 173, 52, 12, 202, 233,
        246, 169, 3, 120, 162, 180, 235, 188, 3, 120, 186, 137, 229, 194, 4, 120, 167, 230, 165,
        209, 5, 120, 128, 158, 190, 163, 7, 120, 185, 239, 147, 128, 8, 120, 193, 134, 171, 154,
        10, 13, 149, 232, 220, 186, 10, 104, 180, 238, 133, 184, 15, 125, 110, 13, 109, 123, 108,
        5, 142, 149, 238, 143, 1, 104, 162, 180, 235, 188, 3, 120, 166, 186, 238, 198, 5, 125, 162,
        222, 148, 235, 6, 12, 212, 255, 174, 244, 10, 125, 108, 5, 182, 195, 203, 188, 4, 125, 248,
        138, 185, 221, 6, 16, 239, 140, 220, 190, 7, 125, 192, 165, 231, 254, 10, 125, 150, 208,
        216, 236, 15, 120, 110, 125, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 10, 0, 10, 0, 0, 0, 0, 0,
        0, 0, 1, 0, 1, 2, 7, 1, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 2, 7, 1, 0, 228, 11, 84, 2, 0, 0,
        0, 0, 0, 10, 10, 0, 0, 0, 0, 0, 0, 0, 10, 0, 1, 100, 60, 0, 0, 0, 0, 0, 0, 0,
    ];

    #[test]
    fn stored_funding_operation_v1_legacy_envelope_decodes() {
        let global = test_global_policy(1_000);
        let target = test_target_principal(1);
        let expected = test_funding_operation(1, target, FundingTrigger::ManualTopup, &global, 10);
        let decoded = <StoredFundingOperation as Storable>::from_bytes(Cow::Borrowed(
            FUNDING_OPERATION_V1_FIXTURE,
        ))
        .into_current();
        assert_eq!(decoded, expected);
    }

    /// Frozen fixture captured once from `candid::encode_one(&StoredTargetReservationState::V1(TargetReservationState::new()
    /// .reserve(1, 10, 100, 86_400, 1_000).unwrap()))`.
    const TARGET_RESERVATION_STATE_V1_FIXTURE: &[u8] = &[
        68, 73, 68, 76, 7, 107, 1, 155, 150, 1, 1, 108, 2, 228, 139, 244, 128, 6, 2, 151, 139, 246,
        167, 8, 120, 108, 2, 249, 192, 168, 189, 2, 3, 215, 176, 178, 223, 2, 5, 109, 4, 108, 2,
        200, 169, 154, 248, 1, 120, 212, 255, 174, 244, 10, 125, 109, 6, 108, 3, 179, 128, 231,
        143, 6, 120, 212, 255, 174, 244, 10, 125, 247, 227, 234, 136, 12, 120, 1, 0, 0, 0, 1, 1, 0,
        0, 0, 0, 0, 0, 0, 10, 100, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ];

    #[test]
    fn stored_target_reservation_state_v1_legacy_envelope_decodes() {
        let expected = TargetReservationState::new()
            .reserve(1, 10, 100, 86_400, 1_000)
            .unwrap();
        let decoded = <StoredTargetReservationState as Storable>::from_bytes(Cow::Borrowed(
            TARGET_RESERVATION_STATE_V1_FIXTURE,
        ))
        .into_current();
        assert_eq!(decoded, expected);
    }

    /// Frozen fixture captured once from `candid::encode_one(&StoredGlobalRollingSpendState::V1(GlobalRollingSpendState::new()
    /// .reserve(1, 10, 100, 86_400, 1_000).unwrap()))`.
    const GLOBAL_ROLLING_SPEND_STATE_V1_FIXTURE: &[u8] = &[
        68, 73, 68, 76, 7, 107, 1, 155, 150, 1, 1, 108, 1, 228, 139, 244, 128, 6, 2, 108, 2, 249,
        192, 168, 189, 2, 3, 215, 176, 178, 223, 2, 5, 109, 4, 108, 2, 200, 169, 154, 248, 1, 120,
        212, 255, 174, 244, 10, 125, 109, 6, 108, 3, 179, 128, 231, 143, 6, 120, 212, 255, 174,
        244, 10, 125, 247, 227, 234, 136, 12, 120, 1, 0, 0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 0, 10, 100,
        0, 0, 0, 0, 0, 0, 0,
    ];

    #[test]
    fn stored_global_rolling_spend_state_v1_legacy_envelope_decodes() {
        let expected = GlobalRollingSpendState::new()
            .reserve(1, 10, 100, 86_400, 1_000)
            .unwrap();
        let decoded = <StoredGlobalRollingSpendState as Storable>::from_bytes(Cow::Borrowed(
            GLOBAL_ROLLING_SPEND_STATE_V1_FIXTURE,
        ))
        .into_current();
        assert_eq!(decoded, expected);
    }

    #[test]
    fn stored_self_recovery_state_v1_legacy_bytes_migrate_to_v2_with_empty_ledger() {
        // Bytes are built through an independently-defined shadow type —
        // NOT through `StoredSelfRecoveryState::V1`'s own `to_bytes()` (the
        // storage review's Finding 5 flagged the other 13 "legacy" tests as
        // tautological for exactly this reason: they wrap the SAME current
        // type, so they never exercise anything beyond this crate's own
        // round-trip encoder). Here `SelfRecoveryStateV1` is a genuinely
        // different, frozen shape (pre-ledger), so decoding this
        // independently-encoded blob into the current `SelfRecoveryState`
        // proves both the wire format AND the V1 -> V2 migration
        // (`SelfRecoveryState::from_legacy`) are correct — not just that
        // this crate's own encoder agrees with itself.
        #[derive(CandidType, serde::Serialize)]
        struct RawSelfRecoveryStateV1ForTest {
            in_flight_operation_id: Option<u64>,
            last_recovery_at_secs: Option<u64>,
        }
        #[derive(CandidType, serde::Serialize)]
        enum RawStoredSelfRecoveryStateForTest {
            V1(RawSelfRecoveryStateV1ForTest),
        }

        let raw = RawStoredSelfRecoveryStateForTest::V1(RawSelfRecoveryStateV1ForTest {
            in_flight_operation_id: Some(3),
            last_recovery_at_secs: Some(77),
        });
        let bytes = candid::encode_one(&raw).unwrap();
        let decoded =
            <StoredSelfRecoveryState as Storable>::from_bytes(Cow::Owned(bytes)).into_current();

        // A legacy in-flight marker cannot be safely carried forward (V1
        // has no reserved-amount data for the new ledger to require), so it
        // is deterministically dropped rather than guessed.
        assert_eq!(decoded.in_flight_operation_id(), None);
        assert!(!decoded.is_suppressing_distribution());
        // `last_recovery_at_secs` has no ledger dependency and migrates
        // unchanged.
        assert_eq!(decoded.last_recovery_at_secs(), Some(77));
    }

    #[test]
    fn stored_self_recovery_state_v2_envelope_round_trips() {
        let state = SelfRecoveryState::new()
            .begin(3, 10, 100, 86_400, 1_000)
            .unwrap();
        let bytes = candid::encode_one(StoredSelfRecoveryState::V2(state.clone())).unwrap();
        let decoded =
            <StoredSelfRecoveryState as Storable>::from_bytes(Cow::Owned(bytes)).into_current();
        assert_eq!(decoded, state);
    }

    #[test]
    fn stored_terminal_funding_summary_v1_legacy_envelope_decodes() {
        let global = test_global_policy(1_000);
        let target = test_target_principal(1);
        let op = test_resolved_operation(1, target, &global, 10);
        let expected = TerminalFundingSummary::from_resolved(&op, 20).unwrap();
        let decoded = <StoredTerminalFundingSummary as Storable>::from_bytes(Cow::Borrowed(
            TERMINAL_FUNDING_SUMMARY_V1_FIXTURE,
        ))
        .into_current();
        assert_eq!(decoded, expected);
    }

    /// Frozen fixture captured once from `candid::encode_one(&StoredTerminalFundingSummary::V1(TerminalFundingSummary::from_resolved(
    /// &test_resolved_operation(1, test_target_principal(1), &test_global_policy(1_000), 10), 20).unwrap()))`.
    const TERMINAL_FUNDING_SUMMARY_V1_FIXTURE: &[u8] = &[
        68, 73, 68, 76, 4, 107, 1, 155, 150, 1, 1, 108, 6, 210, 146, 145, 221, 4, 2, 179, 128, 231,
        143, 6, 120, 209, 230, 179, 183, 8, 104, 212, 255, 174, 244, 10, 125, 146, 241, 190, 222,
        13, 3, 199, 142, 140, 129, 15, 120, 107, 2, 227, 193, 207, 215, 7, 127, 182, 133, 169, 167,
        8, 127, 107, 3, 247, 141, 171, 203, 2, 127, 156, 187, 243, 204, 12, 127, 235, 130, 174,
        136, 15, 127, 1, 0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 2, 7, 1, 10, 2, 20, 0, 0, 0, 0, 0, 0,
        0,
    ];

    // ── key encoding ──

    #[test]
    fn storable_principal_round_trips_and_orders_correctly() {
        let anonymous = StorablePrincipal(Principal::anonymous());
        let short = StorablePrincipal(Principal::from_slice(&[1, 2, 3]));
        let full = StorablePrincipal(Principal::from_slice(&[7u8; 29]));

        for p in [&anonymous, &short, &full] {
            let bytes = p.to_bytes();
            let decoded = StorablePrincipal::from_bytes(bytes);
            assert_eq!(&decoded, p);
        }

        let mut map: StableBTreeMap<StorablePrincipal, u64, VMem> =
            StableBTreeMap::init(MEMORY_MANAGER.with(|m| m.borrow().get(MemoryId::new(200))));
        map.insert(anonymous.clone(), 1);
        map.insert(short.clone(), 2);
        map.insert(full.clone(), 3);
        assert_eq!(map.get(&anonymous), Some(1));
        assert_eq!(map.get(&short), Some(2));
        assert_eq!(map.get(&full), Some(3));
    }

    #[test]
    fn sample_key_round_trips_and_scopes_range_queries_to_one_target() {
        // Anonymous principal (raw bytes `[0x04]`) vs. a synthetic principal
        // whose raw bytes start with the same byte followed by a zero byte —
        // exactly the padded-prefix collision risk flagged in the blueprint.
        let anonymous = Principal::anonymous();
        assert_eq!(anonymous.as_slice(), &[0x04]);
        let colliding = Principal::from_slice(&[0x04, 0x00]);

        SAMPLES.with(|m| {
            let mut map = m.borrow_mut();
            map.insert(
                SampleKey {
                    principal: anonymous,
                    slot: 0,
                },
                StoredSample::V2(test_sample(1, PublicTargetState::Healthy)),
            );
            map.insert(
                SampleKey {
                    principal: colliding,
                    slot: 0,
                },
                StoredSample::V2(test_sample(2, PublicTargetState::Healthy)),
            );
        });

        let anonymous_rows = SAMPLES.with(|m| {
            m.borrow()
                .range((
                    std::ops::Bound::Included(SampleKey::range_start(anonymous)),
                    std::ops::Bound::Included(SampleKey::range_end(anonymous)),
                ))
                .map(|(k, _)| k)
                .collect::<Vec<_>>()
        });
        assert_eq!(
            anonymous_rows,
            vec![SampleKey {
                principal: anonymous,
                slot: 0
            }]
        );

        let colliding_rows = SAMPLES.with(|m| {
            m.borrow()
                .range((
                    std::ops::Bound::Included(SampleKey::range_start(colliding)),
                    std::ops::Bound::Included(SampleKey::range_end(colliding)),
                ))
                .map(|(k, _)| k)
                .collect::<Vec<_>>()
        });
        assert_eq!(
            colliding_rows,
            vec![SampleKey {
                principal: colliding,
                slot: 0
            }]
        );
    }

    #[test]
    fn sample_key_decode_is_lossless_for_principal_ending_in_zero_byte() {
        // A principal that legitimately ends in a zero byte must not be
        // confused with zero padding on decode.
        let principal = Principal::from_slice(&[1, 2, 0]);
        let key = SampleKey { principal, slot: 5 };
        let decoded = SampleKey::from_bytes(key.to_bytes());
        assert_eq!(decoded.principal, principal);
        assert_eq!(decoded.principal.as_slice(), &[1, 2, 0]);
        assert_eq!(decoded.slot, 5);
    }

    fn register_test_target(seed: u8, global: &GlobalPolicy) -> Principal {
        let record = test_target_record(seed, global);
        let principal = record.principal();
        insert_target(record).unwrap();
        principal
    }

    /// Like `register_test_target`, but with an explicit per-target daily
    /// cap instead of the hardcoded 100 `test_target_record` uses — needed
    /// when a test deliberately opens a `FundingOperation` whose own
    /// (independently snapshotted) `funding_policy` cap must exceed the
    /// global cap while the *registered target's* own cap must not, or vice
    /// versa.
    fn register_test_target_with_cap(
        seed: u8,
        global: &GlobalPolicy,
        daily_cap: u128,
    ) -> Principal {
        let empty = BTreeSet::new();
        let ctx = TargetRegistrationContext {
            sentinel_id: test_sentinel_id(),
            existing_target_count: 0,
            existing_target_principals: &empty,
            global_policy: global,
        };
        let record = TargetRecord::register(
            TargetArgs {
                principal: test_target_principal(seed),
                display_name: "svc".to_string(),
                project: "proj".to_string(),
                environment: Environment::Production,
                criticality: Criticality::Standard,
                observation_mode: ObservationMode::SelfReport,
                tags: vec![],
                funding_policy: test_funding_policy_args(1, 10, daily_cap),
            },
            &ctx,
        )
        .unwrap();
        let principal = record.principal();
        insert_target(record).unwrap();
        principal
    }

    #[test]
    fn sample_ring_wraps_and_overwrites_oldest_slot_past_max_samples_per_target() {
        let global = test_global_policy(1_000);
        let target = register_test_target(1, &global);
        for i in 0..(types::MAX_SAMPLES_PER_TARGET as u64 + 1) {
            record_sample(target, test_sample(i, PublicTargetState::Healthy)).unwrap();
        }
        let meta = sample_meta(target).unwrap();
        assert_eq!(meta.filled_slots, MAX_SAMPLES_PER_TARGET_U32);
        // Slot 0 was written first (timestamp 0), then overwritten once the
        // ring wrapped (timestamp MAX_SAMPLES_PER_TARGET).
        let slot0 = SAMPLES.with(|m| {
            m.borrow()
                .get(&SampleKey {
                    principal: target,
                    slot: 0,
                })
                .map(|v| v.into_current())
        });
        assert_eq!(
            slot0.unwrap().timestamp_secs,
            types::MAX_SAMPLES_PER_TARGET as u64
        );
    }

    #[test]
    fn record_sample_rejects_sequence_overflow_with_no_partial_write() {
        // Seed a `SampleMeta` one write away from `u64::MAX` (unreachable
        // through real writes, but directly constructible for this test) so
        // the very next `record_sample` call must hit
        // `total_writes.checked_add(1)`'s overflow branch — final-correction
        // item 5's "check sequence overflow before any write."
        let global = test_global_policy(1_000);
        let target = register_test_target(1, &global);
        let next_slot = 7u32;
        let sentinel = test_sample(999, PublicTargetState::Healthy);
        SAMPLES.with(|m| {
            m.borrow_mut().insert(
                SampleKey {
                    principal: target,
                    slot: next_slot,
                },
                StoredSample::V2(sentinel.clone()),
            );
        });
        let seeded_meta = SampleMeta {
            next_slot,
            filled_slots: MAX_SAMPLES_PER_TARGET_U32,
            last_success_at_secs: Some(1),
            last_attempt_at_secs: Some(1),
            total_writes: u64::MAX,
        };
        SAMPLE_META.with(|m| {
            m.borrow_mut()
                .insert(StorablePrincipal(target), StoredSampleMeta::V2(seeded_meta));
        });

        let result = record_sample(target, test_sample(1_000, PublicTargetState::Healthy));
        assert_eq!(result, Err(RecordSampleError::SequenceOverflow));

        // No partial write: `SampleMeta` is byte-for-byte unchanged, and the
        // slot that would have been overwritten still holds the sentinel
        // sample rather than the rejected write.
        assert_eq!(sample_meta(target), Some(seeded_meta));
        let slot_value = SAMPLES.with(|m| {
            m.borrow()
                .get(&SampleKey {
                    principal: target,
                    slot: next_slot,
                })
                .map(|v| v.into_current())
        });
        assert_eq!(slot_value, Some(sentinel));
    }

    // ── chronological sample history (storage review Finding 4) ──

    #[test]
    fn list_samples_is_chronological_before_any_wrap() {
        let global = test_global_policy(1_000);
        let target = register_test_target(1, &global);
        for i in 0..10u64 {
            record_sample(target, test_sample(i, PublicTargetState::Healthy)).unwrap();
        }
        let page = list_samples(target, None, 100).unwrap();
        let timestamps: Vec<u64> = page.iter().map(|(_, s)| s.timestamp_secs).collect();
        assert_eq!(timestamps, (0..10).collect::<Vec<_>>());
        // Before any wrap, the logical (1-indexed) sequence already IS write order.
        let sequences: Vec<u64> = page.iter().map(|(seq, _)| *seq).collect();
        assert_eq!(sequences, (1..=10).collect::<Vec<_>>());
    }

    #[test]
    fn list_samples_is_chronological_after_first_wrap() {
        let global = test_global_policy(1_000);
        let target = register_test_target(1, &global);
        let total = types::MAX_SAMPLES_PER_TARGET as u64 + 1;
        for i in 0..total {
            record_sample(target, test_sample(i, PublicTargetState::Healthy)).unwrap();
        }
        let page = list_samples(target, None, types::MAX_SAMPLES_PER_TARGET).unwrap();
        assert_eq!(page.len(), types::MAX_SAMPLES_PER_TARGET);
        let timestamps: Vec<u64> = page.iter().map(|(_, s)| s.timestamp_secs).collect();
        // Oldest surviving sample is timestamp 1 (timestamp 0 was
        // overwritten by the wrap); newest is `total - 1`. Strictly
        // ascending across the entire page, unlike raw slot order which
        // would put the just-overwritten slot 0 (newest) right after slot
        // `MAX_SAMPLES_PER_TARGET - 1`.
        let expected: Vec<u64> = (1..total).collect();
        assert_eq!(timestamps, expected);
    }

    #[test]
    fn list_samples_is_chronological_after_multiple_wraps() {
        let global = test_global_policy(1_000);
        let target = register_test_target(1, &global);
        let total = types::MAX_SAMPLES_PER_TARGET as u64 * 2 + 37;
        for i in 0..total {
            record_sample(target, test_sample(i, PublicTargetState::Healthy)).unwrap();
        }
        let page = list_samples(target, None, types::MAX_SAMPLES_PER_TARGET).unwrap();
        let timestamps: Vec<u64> = page.iter().map(|(_, s)| s.timestamp_secs).collect();
        let expected: Vec<u64> = ((total - types::MAX_SAMPLES_PER_TARGET as u64)..total).collect();
        assert_eq!(timestamps, expected);
    }

    #[test]
    fn list_samples_pagination_crosses_the_wrap_boundary() {
        let global = test_global_policy(1_000);
        let target = register_test_target(1, &global);
        let total = types::MAX_SAMPLES_PER_TARGET as u64 + 1;
        for i in 0..total {
            record_sample(target, test_sample(i, PublicTargetState::Healthy)).unwrap();
        }
        // Page through the whole chronological history in small pages and
        // confirm the concatenation is exactly the full ascending sequence
        // with no gap or duplicate at the wrap boundary.
        let mut cursor = None;
        let mut all_timestamps = Vec::new();
        loop {
            let page = list_samples(target, cursor, 7).unwrap();
            if page.is_empty() {
                break;
            }
            cursor = Some(page.last().unwrap().0);
            all_timestamps.extend(page.iter().map(|(_, s)| s.timestamp_secs));
        }
        let expected: Vec<u64> = (1..total).collect();
        assert_eq!(all_timestamps, expected);
    }

    #[test]
    fn list_samples_future_cursor_is_rejected() {
        let global = test_global_policy(1_000);
        let target = register_test_target(1, &global);
        for i in 0..5u64 {
            record_sample(target, test_sample(i, PublicTargetState::Healthy)).unwrap();
        }
        // A cursor naming a sequence number never assigned to this target
        // (999, when only 5 have ever been written) must not replay from
        // the start and must not silently return an empty page — it is an
        // explicit, typed `FutureCursor` rejection.
        assert_eq!(
            list_samples(target, Some(999), 10),
            Err(SampleCursorError::FutureCursor)
        );
    }

    #[test]
    fn list_samples_stale_cursor_is_rejected() {
        let global = test_global_policy(1_000);
        let target = register_test_target(1, &global);
        let max = types::MAX_SAMPLES_PER_TARGET as u64;
        let total = max * 2;
        for i in 0..total {
            record_sample(target, test_sample(i, PublicTargetState::Healthy)).unwrap();
        }
        // Oldest retained sequence is `total - max + 1`. A cursor two below
        // that names a next-needed sequence (`cursor + 1`) that is strictly
        // older than the oldest surviving sample — at least one
        // retained-but-undelivered sample was evicted before this call could
        // ever return it, so this must be a typed rejection, never a
        // silently-empty or silently-truncated page.
        let oldest = total - max + 1;
        assert_eq!(
            list_samples(target, Some(oldest - 2), 10),
            Err(SampleCursorError::StaleCursor)
        );
        // The exact boundary one sequence later (`cursor + 1 == oldest`) is
        // NOT stale — nothing was skipped.
        assert!(list_samples(target, Some(oldest - 1), 10).is_ok());
    }

    #[test]
    fn list_samples_slow_reader_former_slot_recycled_resumes_correctly() {
        // Storage review Finding 2's exact reproduction: a reader takes a
        // cursor, then enough writes land before its next call that the
        // *physical* ring slot backing that cursor's sequence number gets
        // recycled to hold a much newer, unrelated sample. A logical
        // sequence cursor must resume at the correct next sample regardless
        // — it can never alias to whatever now occupies that physical slot.
        let global = test_global_policy(1_000);
        let target = register_test_target(1, &global);
        let max = types::MAX_SAMPLES_PER_TARGET as u64;
        let first_batch = max + 3;
        for i in 0..first_batch {
            record_sample(target, test_sample(i, PublicTargetState::Healthy)).unwrap();
        }
        let page = list_samples(target, None, 3).unwrap();
        assert_eq!(page.len(), 3);
        let cursor = page.last().unwrap().0;
        let cursor_slot = (cursor - 1) % max;

        // Keep writing, one sample at a time, until a write lands exactly on
        // `cursor_slot` — overwriting the very sample `cursor` points at.
        let mut total = first_batch;
        loop {
            let written_slot = total % max;
            record_sample(target, test_sample(total, PublicTargetState::Healthy)).unwrap();
            total += 1;
            if written_slot == cursor_slot {
                break;
            }
        }

        let resumed = list_samples(target, Some(cursor), max as usize).unwrap();
        let expected_sequences: Vec<u64> = ((cursor + 1)..=total).collect();
        let got_sequences: Vec<u64> = resumed.iter().map(|(seq, _)| *seq).collect();
        assert_eq!(got_sequences, expected_sequences);
        // Sequence N's sample was written with timestamp N - 1.
        assert_eq!(resumed.first().unwrap().1.timestamp_secs, cursor);
        assert_eq!(resumed.last().unwrap().1.timestamp_secs, total - 1);
    }

    #[test]
    fn list_samples_on_empty_history_is_empty() {
        let global = test_global_policy(1_000);
        let target = register_test_target(1, &global);
        assert_eq!(list_samples(target, None, 10), Ok(Vec::new()));
    }

    #[test]
    fn list_samples_after_target_removal_is_empty() {
        let global = test_global_policy(1_000);
        let target = register_test_target(1, &global);
        record_sample(target, test_sample(1, PublicTargetState::Healthy)).unwrap();
        assert!(!list_samples(target, None, 10).unwrap().is_empty());
        remove_target(target).unwrap();
        assert_eq!(list_samples(target, None, 10), Ok(Vec::new()));
        assert_eq!(sample_meta(target), None);
    }

    #[test]
    fn terminal_summaries_evict_oldest_resolved_past_max_terminal_summaries() {
        let global = test_global_policy(1_000_000);
        let target = test_target_principal(1);
        for i in 0..(types::MAX_TERMINAL_SUMMARIES as u64 + 1) {
            let op = test_resolved_operation(i, target, &global, 10);
            let summary = TerminalFundingSummary::from_resolved(&op, i).unwrap();
            insert_terminal_summary(summary);
        }
        let count = TERMINAL_SUMMARIES.with(|m| m.borrow().len());
        assert_eq!(count, types::MAX_TERMINAL_SUMMARIES as u64);
        // The summary with resolved_at_secs == 0 (the smallest) was evicted.
        let survives_zero = TERMINAL_SUMMARIES.with(|m| m.borrow().get(&0).is_some());
        assert!(!survives_zero);
        let survives_last = TERMINAL_SUMMARIES.with(|m| {
            m.borrow()
                .get(&(types::MAX_TERMINAL_SUMMARIES as u64))
                .is_some()
        });
        assert!(survives_last);
    }

    // ── funding operation compaction (storage review Finding 1) ──

    #[test]
    fn compact_operation_persists_summary_and_removes_full_operation() {
        let global = test_global_policy(1_000_000);
        let target = register_test_target(1, &global);
        let op = test_resolved_operation(1, target, &global, 10);
        insert_operation(op.clone()).unwrap();
        let summary = TerminalFundingSummary::from_resolved(&op, 20).unwrap();
        assert_eq!(compact_operation(op.id(), summary.clone()), Ok(()));
        assert_eq!(get_operation(op.id()), None);
        assert_eq!(
            TERMINAL_SUMMARIES.with(|m| m.borrow().get(&op.id()).map(|v| v.into_current())),
            Some(summary)
        );
    }

    #[test]
    fn compact_operation_rejects_missing_operation() {
        let global = test_global_policy(1_000_000);
        let target = test_target_principal(1);
        let op = test_resolved_operation(1, target, &global, 10);
        let summary = TerminalFundingSummary::from_resolved(&op, 20).unwrap();
        // `op` was never inserted.
        assert_eq!(
            compact_operation(op.id(), summary),
            Err(CompactOperationError::Missing)
        );
    }

    #[test]
    fn compact_operation_rejects_unresolved_operation() {
        let global = test_global_policy(1_000_000);
        let target = register_test_target(1, &global);
        let op =
            test_funding_operation(1, target, FundingTrigger::LowBalanceAutoTopup, &global, 10);
        insert_operation(op.clone()).unwrap();
        // Build a summary as if `op` had resolved (it never did) purely to
        // drive `compact_operation`'s own id/resolved checks.
        let resolved_twin = test_resolved_operation(1, target, &global, 10);
        let summary = TerminalFundingSummary::from_resolved(&resolved_twin, 20).unwrap();
        assert_eq!(
            compact_operation(op.id(), summary),
            Err(CompactOperationError::Unresolved)
        );
        // Never evicts an unresolved operation.
        assert_eq!(get_operation(op.id()), Some(op));
    }

    #[test]
    fn compact_operation_rejects_wrong_id() {
        let global = test_global_policy(1_000_000);
        let target = register_test_target(1, &global);
        let op1 = test_resolved_operation(1, target, &global, 10);
        let op2 = test_resolved_operation(2, target, &global, 10);
        insert_operation(op1.clone()).unwrap();
        let summary_for_op2 = TerminalFundingSummary::from_resolved(&op2, 20).unwrap();
        // Caller asked to compact id 1 but passed a summary for id 2.
        assert_eq!(
            compact_operation(op1.id(), summary_for_op2),
            Err(CompactOperationError::WrongId)
        );
        assert_eq!(get_operation(op1.id()), Some(op1));
    }

    #[test]
    fn compact_operation_rejects_mismatched_summary() {
        let global = test_global_policy(1_000_000);
        let target = register_test_target(1, &global);
        let op = test_resolved_operation(1, target, &global, 10);
        insert_operation(op.clone()).unwrap();
        // Same operation_id, but built from a DIFFERENT resolved operation
        // (different target) so every content field disagrees with what is
        // actually stored under id 1.
        let mismatched_twin = test_resolved_operation(1, test_target_principal(2), &global, 10);
        let summary = TerminalFundingSummary::from_resolved(&mismatched_twin, 20).unwrap();
        assert_eq!(
            compact_operation(op.id(), summary),
            Err(CompactOperationError::Mismatched)
        );
        assert_eq!(get_operation(op.id()), Some(op));
    }

    #[test]
    fn compact_operation_enforces_terminal_summary_bound() {
        // Compaction reuses `insert_terminal_summary`'s own self-eviction, so
        // compacting past `MAX_TERMINAL_SUMMARIES` still bounds the ring
        // rather than growing it unboundedly.
        let global = test_global_policy(1_000_000_000);
        // A single reused, registered target is sufficient: `insert_operation`
        // only bounds the *global* `FUNDING_OPERATIONS` count, and each
        // iteration compacts (and thus removes) its operation before the
        // next is inserted, so there is never more than one live operation
        // for this target at a time. Registering `MAX_TERMINAL_SUMMARIES + 1`
        // (513) distinct targets would itself exceed `MAX_TARGETS` (128).
        let target = register_test_target(1, &global);
        for i in 0..(types::MAX_TERMINAL_SUMMARIES as u64 + 1) {
            let op = test_resolved_operation(i, target, &global, i);
            insert_operation(op.clone()).unwrap();
            let summary = TerminalFundingSummary::from_resolved(&op, i).unwrap();
            compact_operation(op.id(), summary).unwrap();
        }
        assert_eq!(
            TERMINAL_SUMMARIES.with(|m| m.borrow().len()),
            types::MAX_TERMINAL_SUMMARIES as u64
        );
    }

    /// Final invariant review Finding N1: compacting an operation while
    /// `TARGET_RESERVATIONS` still names it as pending would remove the only
    /// record `validate_whole_state`'s bidirectional linkage check could
    /// ever match against, permanently trapping every future `post_upgrade`.
    #[test]
    fn compact_operation_rejects_while_target_reservation_still_pending() {
        let global = test_global_policy(1_000_000);
        let target = register_test_target(1, &global);
        let op = test_resolved_operation(1, target, &global, 10);
        insert_operation(op.clone()).unwrap();
        let reservation = TargetReservationState::new()
            .reserve(op.id(), op.reserved_amount_cycles(), 10, 86_400, 1_000)
            .unwrap();
        set_target_reservation(target, reservation);
        let summary = TerminalFundingSummary::from_resolved(&op, 20).unwrap();
        assert_eq!(
            compact_operation(op.id(), summary),
            Err(CompactOperationError::TargetReservationStillPending)
        );
        // Compaction did not happen: the operation is still fully present.
        assert_eq!(get_operation(op.id()), Some(op));
    }

    /// Final invariant review Finding N1: same hazard, for `GLOBAL_ROLLING_SPEND`.
    #[test]
    fn compact_operation_rejects_while_global_reservation_still_pending() {
        let global = test_global_policy(1_000_000);
        let target = register_test_target(1, &global);
        let op = test_resolved_operation(1, target, &global, 10);
        insert_operation(op.clone()).unwrap();
        // The target reservation itself is NOT pending, so only the global
        // ledger's own guard is under test here.
        let global_spend = GlobalRollingSpendState::new()
            .reserve(op.id(), op.reserved_amount_cycles(), 10, 86_400, 1_000)
            .unwrap();
        set_global_rolling_spend(global_spend);
        let summary = TerminalFundingSummary::from_resolved(&op, 20).unwrap();
        assert_eq!(
            compact_operation(op.id(), summary),
            Err(CompactOperationError::GlobalReservationStillPending)
        );
        assert_eq!(get_operation(op.id()), Some(op));
    }

    /// Task 4: same hazard, for the shared Cycles Ledger `SOURCE_RESERVE`
    /// lane, on an ORDINARY operation (the self-recovery variant is covered
    /// separately below, since it exercises the `SelfRecovery` trigger).
    #[test]
    fn compact_operation_rejects_while_source_reservation_still_pending() {
        let global = test_global_policy(1_000_000);
        let target = register_test_target(1, &global);
        let op = test_resolved_operation(1, target, &global, 10);
        insert_operation(op.clone()).unwrap();
        // Neither the target nor global reservation is pending, so only the
        // source-reserve guard is under test here.
        let source = SourceReserveState::new()
            .refresh(1_000_000, 0, 10)
            .unwrap()
            .reserve_ordinary(op.id(), op.reserved_amount_cycles(), 0, 10, 86_400)
            .unwrap();
        set_source_reserve(source);
        let summary = TerminalFundingSummary::from_resolved(&op, 20).unwrap();
        assert_eq!(
            compact_operation(op.id(), summary),
            Err(CompactOperationError::SourceReservationStillPending)
        );
        assert_eq!(get_operation(op.id()), Some(op));
    }

    /// Final invariant review Finding N1: same hazard, for the `SELF_RECOVERY`
    /// singleton lane.
    #[test]
    fn compact_operation_rejects_while_self_recovery_reservation_still_pending() {
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000_000);
        let op = test_funding_operation(1, sentinel_id, FundingTrigger::SelfRecovery, &global, 10);
        insert_operation(op.clone()).unwrap();
        let op = op
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Submitted),
                10,
                FundingAttemptResultClass::Success,
            )
            .unwrap();
        update_operation(op.clone()).unwrap();
        let op = op
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Confirmed),
                10,
                FundingAttemptResultClass::Success,
            )
            .unwrap();
        update_operation(op.clone()).unwrap();
        let op = op
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Complete),
                10,
                FundingAttemptResultClass::Success,
            )
            .unwrap();
        update_operation(op.clone()).unwrap();
        set_self_recovery_state(
            SelfRecoveryState::new()
                .begin(op.id(), op.reserved_amount_cycles(), 10, 86_400, 100)
                .unwrap(),
        );
        let summary = TerminalFundingSummary::from_resolved(&op, 20).unwrap();
        assert_eq!(
            compact_operation(op.id(), summary),
            Err(CompactOperationError::SelfRecoveryReservationStillPending)
        );
        assert_eq!(get_operation(op.id()), Some(op));
    }

    // ── funding operation update (mutation-helper typed errors) ──

    #[test]
    fn update_operation_rejects_missing_operation() {
        let global = test_global_policy(1_000_000);
        let target = test_target_principal(1);
        let op =
            test_funding_operation(1, target, FundingTrigger::LowBalanceAutoTopup, &global, 10);
        assert_eq!(update_operation(op), Err(UpdateOperationError::NotFound));
    }

    #[test]
    fn update_operation_accepts_a_legal_transition() {
        let global = test_global_policy(1_000_000);
        let target = register_test_target(1, &global);
        let op =
            test_funding_operation(1, target, FundingTrigger::LowBalanceAutoTopup, &global, 10);
        insert_operation(op.clone()).unwrap();
        let advanced = op
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Submitted),
                11,
                FundingAttemptResultClass::Success,
            )
            .unwrap();
        assert_eq!(update_operation(advanced.clone()), Ok(()));
        assert_eq!(get_operation(op.id()), Some(advanced));
    }

    #[test]
    fn update_operation_rejects_illegal_transition() {
        let global = test_global_policy(1_000_000);
        let target = register_test_target(1, &global);
        let op =
            test_funding_operation(1, target, FundingTrigger::LowBalanceAutoTopup, &global, 10);
        insert_operation(op.clone()).unwrap();
        // Locally advance a copy through Submitted -> Confirmed -> Complete
        // WITHOUT ever persisting the intermediate steps via
        // `update_operation`, then try to persist the final `Complete` value
        // directly — the stored record is still `PlannedReserved`, and
        // `PlannedReserved -> Complete` is not a legal single-step edge.
        let completed = op
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Submitted),
                10,
                FundingAttemptResultClass::Success,
            )
            .unwrap()
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Confirmed),
                10,
                FundingAttemptResultClass::Success,
            )
            .unwrap()
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Complete),
                10,
                FundingAttemptResultClass::Success,
            )
            .unwrap();
        assert_eq!(
            update_operation(completed),
            Err(UpdateOperationError::InvalidTransition)
        );
        assert_eq!(get_operation(op.id()), Some(op));
    }

    #[test]
    fn update_operation_rejects_any_further_update_once_resolved() {
        let global = test_global_policy(1_000_000);
        let target = register_test_target(1, &global);
        let op =
            test_funding_operation(1, target, FundingTrigger::LowBalanceAutoTopup, &global, 10);
        insert_operation(op.clone()).unwrap();
        let submitted = op
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Submitted),
                10,
                FundingAttemptResultClass::Success,
            )
            .unwrap();
        update_operation(submitted.clone()).unwrap();
        let confirmed = submitted
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Confirmed),
                10,
                FundingAttemptResultClass::Success,
            )
            .unwrap();
        update_operation(confirmed.clone()).unwrap();
        let complete = confirmed
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Complete),
                10,
                FundingAttemptResultClass::Success,
            )
            .unwrap();
        update_operation(complete.clone()).unwrap();
        // A same-state replay of the now-resolved stored record is rejected
        // too, not just a jump — `is_valid_successor` returns `false` for
        // any successor (including itself) once `stops_automatic_retry()`.
        assert_eq!(
            update_operation(complete),
            Err(UpdateOperationError::InvalidTransition)
        );
    }

    #[test]
    fn update_operation_rejects_changed_immutable_snapshot() {
        let global = test_global_policy(1_000_000);
        let target = register_test_target(1, &global);
        let op =
            test_funding_operation(1, target, FundingTrigger::LowBalanceAutoTopup, &global, 10);
        insert_operation(op.clone()).unwrap();
        // Same id, but a different target than what is actually stored. The
        // rail arguments are rebuilt for the forged target too (not reused
        // from `op`), since `FundingOperation::open` now requires
        // `target == rail_arguments.embedded_destination()` — an
        // internally-consistent forged snapshot is what actually exercises
        // `update_operation`'s own immutable-snapshot check, not `open`'s.
        let forged_target = test_target_principal(2);
        let forged_rail = FundingRailArguments::Cycles(CyclesWithdrawSnapshot {
            destination: forged_target,
            from_subaccount: None,
            amount_cycles: op.reserved_amount_cycles(),
            fee_cycles: 0,
            created_at_time_ns: op.created_at_secs() * 1_000_000_000,
        });
        let forged = FundingOperation::open(
            op.id(),
            forged_target,
            op.target_registry_revision(),
            op.funding_policy().clone(),
            op.trigger(),
            forged_rail,
            op.reserved_amount_cycles(),
            op.created_at_secs(),
        )
        .unwrap();
        assert_eq!(
            update_operation(forged),
            Err(UpdateOperationError::ImmutableSnapshotChanged)
        );
        assert_eq!(get_operation(op.id()), Some(op));
    }

    /// Final invariant review Finding N2: a stale in-memory copy holding an
    /// older, same-phase retry must not be able to regress
    /// `updated_at_secs` relative to what a different, newer writer already
    /// persisted — `is_valid_successor` alone provides no protection here,
    /// since any same-phase transition is always legal by design (a
    /// deliberate rule for indeterminate retries).
    #[test]
    fn update_operation_rejects_timestamp_regression() {
        let global = test_global_policy(1_000_000);
        let target = register_test_target(1, &global);
        let op = test_funding_operation(1, target, FundingTrigger::LowBalanceAutoTopup, &global, 0);
        insert_operation(op.clone()).unwrap();
        // "B": advances and persists twice.
        let b1 = op
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Submitted),
                5,
                FundingAttemptResultClass::Success,
            )
            .unwrap();
        update_operation(b1.clone()).unwrap();
        let b2 = b1
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Submitted),
                8,
                FundingAttemptResultClass::Indeterminate,
            )
            .unwrap();
        update_operation(b2.clone()).unwrap();
        // "A": holds a STALE copy of `b1` (read before B's second write
        // landed) and advances it independently with an earlier timestamp
        // than what is now actually stored (`b2`, at 8).
        let a1 = b1
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Submitted),
                6,
                FundingAttemptResultClass::Indeterminate,
            )
            .unwrap();
        assert_eq!(
            update_operation(a1),
            Err(UpdateOperationError::TimestampRegression)
        );
        // `b2` (the true latest state) is untouched.
        assert_eq!(get_operation(op.id()), Some(b2));
    }

    /// Final invariant review Finding N2: even when the incoming timestamp
    /// is NOT a regression, an attempt history that diverges from (rather
    /// than extends) the currently stored history must still be rejected —
    /// otherwise a stale writer can silently clobber a concurrently
    /// persisted attempt with a different one at the same ordinal.
    #[test]
    fn update_operation_rejects_attempt_history_diverged() {
        let global = test_global_policy(1_000_000);
        let target = register_test_target(1, &global);
        let op = test_funding_operation(1, target, FundingTrigger::LowBalanceAutoTopup, &global, 0);
        insert_operation(op.clone()).unwrap();
        let b1 = op
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Submitted),
                5,
                FundingAttemptResultClass::Success,
            )
            .unwrap();
        update_operation(b1.clone()).unwrap();
        let b2 = b1
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Submitted),
                8,
                FundingAttemptResultClass::Indeterminate,
            )
            .unwrap();
        update_operation(b2.clone()).unwrap();
        // "A" holds the same stale `b1` copy and records a DIFFERENT second
        // attempt at a timestamp that is NOT a regression relative to `b2`
        // (9 > 8) — the timestamp check alone would accept this.
        let a1 = b1
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Submitted),
                9,
                FundingAttemptResultClass::Indeterminate,
            )
            .unwrap();
        assert!(a1.updated_at_secs() > b2.updated_at_secs());
        assert_eq!(
            update_operation(a1),
            Err(UpdateOperationError::AttemptHistoryDiverged)
        );
        // `b2`'s legitimately-persisted second attempt survives untouched.
        assert_eq!(get_operation(op.id()), Some(b2));
    }

    // ── funding operation insert bound (storage review Finding 1) ──

    #[test]
    fn insert_operation_fails_closed_at_bound() {
        let global = test_global_policy(1_000_000_000);
        // A single reused, registered target is sufficient here too:
        // `insert_operation` only bounds the global `FUNDING_OPERATIONS`
        // count, and registering `MAX_FUNDING_OPERATIONS` (129) distinct
        // targets would itself exceed `MAX_TARGETS` (128).
        let target = register_test_target(1, &global);
        for i in 0..(types::MAX_FUNDING_OPERATIONS as u64) {
            let op =
                test_funding_operation(i, target, FundingTrigger::LowBalanceAutoTopup, &global, 10);
            insert_operation(op).unwrap();
        }
        let overflow_op = test_funding_operation(
            types::MAX_FUNDING_OPERATIONS as u64,
            target,
            FundingTrigger::LowBalanceAutoTopup,
            &global,
            10,
        );
        assert_eq!(
            insert_operation(overflow_op),
            Err(InsertOperationError::TooManyOperations)
        );
    }

    // ── funding operation insert new-only / registration (storage review
    //    Finding 1, final invariant review Finding N5) ──

    #[test]
    fn insert_operation_rejects_an_already_existing_id() {
        let global = test_global_policy(1_000_000);
        let target = register_test_target(1, &global);
        let op =
            test_funding_operation(1, target, FundingTrigger::LowBalanceAutoTopup, &global, 10);
        insert_operation(op.clone()).unwrap();
        // Same id, would-be corrupted content (different target, illegal
        // single-step state jump) — `insert_operation` must reject this
        // outright rather than silently overwriting, regardless of content;
        // an existing id can only ever be updated via `update_operation`.
        let other_target = register_test_target(2, &global);
        let clobber = test_resolved_operation(1, other_target, &global, 10);
        assert_eq!(
            insert_operation(clobber),
            Err(InsertOperationError::AlreadyExists)
        );
        // The original record is untouched.
        assert_eq!(get_operation(1), Some(op));
    }

    #[test]
    fn insert_operation_rejects_an_unregistered_target() {
        let global = test_global_policy(1_000_000);
        // `target` deliberately never registered via `insert_target`.
        let target = test_target_principal(1);
        let op =
            test_funding_operation(1, target, FundingTrigger::LowBalanceAutoTopup, &global, 10);
        assert_eq!(
            insert_operation(op),
            Err(InsertOperationError::UnregisteredTarget)
        );
        assert_eq!(get_operation(1), None);
    }

    #[test]
    fn insert_operation_accepts_self_recovery_operation_targeting_unregistered_sentinel() {
        // `SelfRecovery`-triggered operations are exempt from the
        // registration check: their target is always `sentinel_id`, which by
        // construction never has (and never can have) a `TARGET_REGISTRY`
        // entry (`ReservedPrincipalKind::SentinelSelf`).
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000_000);
        let op = test_funding_operation(1, sentinel_id, FundingTrigger::SelfRecovery, &global, 10);
        assert_eq!(insert_operation(op.clone()), Ok(()));
        assert_eq!(get_operation(1), Some(op));
    }

    // ── init / global config gates ──

    #[test]
    fn init_rejects_second_call() {
        init_test_state(vec![test_signer(1)], 1, 1_000);
        let result = std::panic::catch_unwind(|| {
            init_test_state(vec![test_signer(2)], 1, 1_000);
        });
        assert!(result.is_err());
    }

    #[test]
    fn global_config_read_before_init_traps() {
        let result = std::panic::catch_unwind(global_config);
        assert!(result.is_err());
    }

    // ── monotonic timestamp / counters ──

    #[test]
    fn next_created_at_time_ns_is_globally_monotonic() {
        let first = next_created_at_time_ns(100).unwrap();
        assert_eq!(first, 100);
        // now <= last must still strictly advance (the `+1` branch).
        let second = next_created_at_time_ns(50).unwrap();
        assert_eq!(second, 101);
        assert!(second > first);
        // now > last advances to now directly.
        let third = next_created_at_time_ns(500).unwrap();
        assert_eq!(third, 500);
    }

    #[test]
    fn next_created_at_time_ns_fails_closed_on_overflow() {
        FUNDING_COUNTERS.with(|cell| {
            let mut cell = cell.borrow_mut();
            let counters = FundingCounters {
                next_operation_id: 0,
                last_created_at_time_ns: u64::MAX,
            };
            cell.set(StoredFundingCounters::V1(counters)).unwrap();
        });
        let result = next_created_at_time_ns(1);
        assert_eq!(result, Err(MonotonicTimeError::Overflow));
        // Failing closed must not have mutated the persisted counter.
        let unchanged = FUNDING_COUNTERS.with(|cell| {
            cell.borrow()
                .get()
                .clone()
                .into_current()
                .last_created_at_time_ns
        });
        assert_eq!(unchanged, u64::MAX);
    }

    #[test]
    fn next_proposal_id_and_next_alarm_id_and_next_operation_id_never_repeat() {
        let mut proposal_ids = BTreeSet::new();
        let mut alarm_ids = BTreeSet::new();
        let mut operation_ids = BTreeSet::new();
        for _ in 0..50 {
            assert!(proposal_ids.insert(next_proposal_id()));
            assert!(alarm_ids.insert(next_alarm_id()));
            assert!(operation_ids.insert(next_operation_id()));
        }
    }

    // ── stable reopen ──

    struct TestStores {
        target_registry: RefCell<StableBTreeMap<StorablePrincipal, StoredTargetRecord, VMem>>,
        samples: RefCell<StableBTreeMap<SampleKey, StoredSample, VMem>>,
        proposals: RefCell<StableBTreeMap<u64, StoredProposalRecord, VMem>>,
        global_config: RefCell<StableCell<StoredGlobalConfig, VMem>>,
    }

    fn open_all(memory: DefaultMemoryImpl) -> TestStores {
        let mm = MemoryManager::init(memory);
        TestStores {
            target_registry: RefCell::new(StableBTreeMap::init(mm.get(MEM_TARGET_REGISTRY))),
            samples: RefCell::new(StableBTreeMap::init(mm.get(MEM_SAMPLES))),
            proposals: RefCell::new(StableBTreeMap::init(mm.get(MEM_PROPOSALS))),
            global_config: RefCell::new(
                StableCell::init(mm.get(MEM_GLOBAL_CONFIG), StoredGlobalConfig::V1(None)).unwrap(),
            ),
        }
    }

    #[test]
    fn stable_state_survives_reopening_the_same_memory_manager() {
        let global = test_global_policy(1_000);
        let record = test_target_record(1, &global);
        let principal = record.principal();
        let sample = test_sample(1, PublicTargetState::Healthy);
        let proposal = ProposalRecord::new(
            1,
            ProposalPayload::AddSigner {
                signer: test_signer(2),
            },
            test_signer(1),
            10,
        );
        let config = GlobalConfig {
            signers: vec![test_signer(1)],
            approval_threshold: 1,
            global_policy: global,
        };

        let backing = DefaultMemoryImpl::default();
        {
            let stores = open_all(backing.clone());
            stores.target_registry.borrow_mut().insert(
                StorablePrincipal(principal),
                StoredTargetRecord::V1(record.clone()),
            );
            stores.samples.borrow_mut().insert(
                SampleKey { principal, slot: 0 },
                StoredSample::V2(sample.clone()),
            );
            stores
                .proposals
                .borrow_mut()
                .insert(1, StoredProposalRecord::V1(proposal.clone()));
            stores
                .global_config
                .borrow_mut()
                .set(StoredGlobalConfig::V1(Some(config.clone())))
                .unwrap();
        }
        let stores = open_all(backing);
        assert_eq!(
            stores
                .target_registry
                .borrow()
                .get(&StorablePrincipal(principal))
                .map(|v| v.into_current()),
            Some(record)
        );
        assert_eq!(
            stores
                .samples
                .borrow()
                .get(&SampleKey { principal, slot: 0 })
                .map(|v| v.into_current()),
            Some(sample)
        );
        assert_eq!(
            stores.proposals.borrow().get(&1).map(|v| v.into_current()),
            Some(proposal)
        );
        assert_eq!(
            stores
                .global_config
                .borrow()
                .get()
                .clone()
                .into_current()
                .map(|c| c.signers),
            Some(config.signers)
        );
    }

    // ── Task 1 state gates (accessor smoke tests) ──

    #[test]
    fn target_registry_accessors_and_bounds() {
        let global = test_global_policy(1_000);
        assert_eq!(target_count(), 0);
        let record = test_target_record(1, &global);
        insert_target(record.clone()).unwrap();
        assert_eq!(target_count(), 1);
        assert!(target_principals().contains(&record.principal()));
        assert_eq!(list_targets_after(None, 10), vec![record.clone()]);
        assert_eq!(remove_target(record.principal()), Ok(Some(record)));
        assert_eq!(target_count(), 0);
    }

    // ── remove_target: fail-closed + cascade delete (storage review Finding 3) ──

    #[test]
    fn remove_target_fails_closed_while_unresolved_operation_exists() {
        let global = test_global_policy(1_000_000);
        let record = test_target_record(1, &global);
        let target = record.principal();
        insert_target(record.clone()).unwrap();
        let op =
            test_funding_operation(1, target, FundingTrigger::LowBalanceAutoTopup, &global, 10);
        insert_operation(op).unwrap();
        assert_eq!(
            remove_target(target),
            Err(RemoveTargetError::UnresolvedOperationExists)
        );
        assert_eq!(get_target(target), Some(record));
    }

    #[test]
    fn remove_target_allows_removal_once_the_operation_resolves() {
        let global = test_global_policy(1_000_000);
        let record = test_target_record(1, &global);
        let target = record.principal();
        insert_target(record).unwrap();
        let op = test_resolved_operation(1, target, &global, 10);
        insert_operation(op).unwrap();
        assert!(remove_target(target).unwrap().is_some());
    }

    #[test]
    fn remove_target_fails_closed_while_pending_reservation_exists() {
        let global = test_global_policy(1_000_000);
        let record = test_target_record(1, &global);
        let target = record.principal();
        insert_target(record.clone()).unwrap();
        let reservation = TargetReservationState::new()
            .reserve(1, 10, 10, 86_400, 1_000)
            .unwrap();
        set_target_reservation(target, reservation);
        assert_eq!(
            remove_target(target),
            Err(RemoveTargetError::PendingReservationExists)
        );
        assert_eq!(get_target(target), Some(record));
    }

    #[test]
    fn remove_target_cascades_sample_meta_samples_and_reservations() {
        let global = test_global_policy(1_000_000);
        let record = test_target_record(1, &global);
        let target = record.principal();
        insert_target(record).unwrap();
        record_sample(target, test_sample(1, PublicTargetState::Healthy)).unwrap();
        // A settled (non-pending) reservation state is fine to carry — it
        // must still be cascaded away, not just the pending case.
        let reservation = TargetReservationState::new()
            .reserve(1, 10, 10, 86_400, 1_000)
            .unwrap()
            .settle_spend(1, 10, 20, 86_400)
            .unwrap();
        set_target_reservation(target, reservation);

        assert!(sample_meta(target).is_some());
        assert!(!list_samples(target, None, 10).unwrap().is_empty());

        assert!(remove_target(target).unwrap().is_some());

        assert_eq!(sample_meta(target), None);
        assert_eq!(list_samples(target, None, 10), Ok(Vec::new()));
        assert_eq!(
            get_target_reservation(target),
            TargetReservationState::new()
        );
        // Directly confirm the underlying stores, not just the accessor
        // defaults, so a bug that left a stale row but still defaulted the
        // accessor's `Option::unwrap_or_default()` path cannot hide.
        assert!(SAMPLE_META
            .with(|m| m.borrow().get(&StorablePrincipal(target)))
            .is_none());
        assert!(TARGET_RESERVATIONS
            .with(|m| m.borrow().get(&StorablePrincipal(target)))
            .is_none());
    }

    #[test]
    fn remove_target_of_unregistered_principal_is_a_no_op_ok() {
        let target = test_target_principal(1);
        assert_eq!(remove_target(target), Ok(None));
    }

    /// Self-enforcing eviction (storage review Finding 2b): `insert_proposal`
    /// itself evicts the oldest non-`Open` proposal once at the bound — there
    /// is no longer a separate opt-in eviction call the caller must remember.
    #[test]
    fn proposal_insert_self_evicts_oldest_non_open_at_bound() {
        for i in 0..(types::MAX_PROPOSALS as u64 + 1) {
            let mut record = ProposalRecord::new(
                i,
                ProposalPayload::AddSigner {
                    signer: test_signer(1),
                },
                test_signer(1),
                i,
            );
            if i == 0 {
                record.status = ProposalStatus::Executed;
            }
            insert_proposal(record).unwrap();
        }
        assert_eq!(proposal_count(), types::MAX_PROPOSALS as u64);
        assert_eq!(get_proposal(0), None);
    }

    /// If every proposal at the bound is still `Open` (nothing safely
    /// removable), a genuinely new id fails closed instead of overflowing
    /// the map.
    #[test]
    fn proposal_insert_fails_closed_when_every_proposal_is_open() {
        for i in 0..(types::MAX_PROPOSALS as u64) {
            let record = ProposalRecord::new(
                i,
                ProposalPayload::AddSigner {
                    signer: test_signer(1),
                },
                test_signer(1),
                i,
            );
            insert_proposal(record).unwrap();
        }
        let overflow_record = ProposalRecord::new(
            types::MAX_PROPOSALS as u64,
            ProposalPayload::AddSigner {
                signer: test_signer(1),
            },
            test_signer(1),
            0,
        );
        assert_eq!(
            insert_proposal(overflow_record),
            Err(InsertProposalError::TooManyOpenProposals)
        );
        assert_eq!(proposal_count(), types::MAX_PROPOSALS as u64);
    }

    /// Overwriting an EXISTING proposal id (an approval or status
    /// transition) is always allowed regardless of bound, since it never
    /// grows the map.
    #[test]
    fn proposal_insert_allows_overwrite_of_existing_id_at_bound() {
        for i in 0..(types::MAX_PROPOSALS as u64) {
            let record = ProposalRecord::new(
                i,
                ProposalPayload::AddSigner {
                    signer: test_signer(1),
                },
                test_signer(1),
                i,
            );
            insert_proposal(record).unwrap();
        }
        let mut updated = get_proposal(0).unwrap();
        updated.record_approval(test_signer(1));
        assert_eq!(insert_proposal(updated), Ok(()));
        assert_eq!(proposal_count(), types::MAX_PROPOSALS as u64);
    }

    #[test]
    fn find_open_alarm_matches_target_and_kind() {
        let target = test_target_principal(1);
        insert_alarm(test_alarm(1, Some(target))).unwrap();
        assert!(find_open_alarm(Some(target), AlarmKind::LowBalance).is_some());
        assert!(find_open_alarm(Some(target), AlarmKind::Unreachable).is_none());
        assert!(find_open_alarm(None, AlarmKind::LowBalance).is_none());
    }

    /// Self-enforcing eviction mirrors `insert_proposal`: the oldest
    /// `Resolved` alarm is evicted automatically once a genuinely new id
    /// arrives at the bound.
    #[test]
    fn alarm_insert_self_evicts_oldest_resolved_alarm_at_bound() {
        for i in 0..(types::MAX_ALARMS as u64 + 1) {
            let mut alarm = test_alarm(i, None);
            if i == 0 {
                alarm.status = types::AlarmStatus::Resolved;
            }
            insert_alarm(alarm).unwrap();
        }
        assert_eq!(ALARMS.with(|m| m.borrow().len()), types::MAX_ALARMS as u64);
        assert_eq!(get_alarm(0), None);
    }

    /// If every alarm at the bound is still `Open`, a genuinely new id fails
    /// closed instead of overflowing the map — this is the storage review's
    /// Finding 2 fix: automatic alarm detection can never silently grow past
    /// the bound and brick a future `post_upgrade`.
    #[test]
    fn alarm_insert_fails_closed_when_every_alarm_is_open() {
        for i in 0..(types::MAX_ALARMS as u64) {
            insert_alarm(test_alarm(i, None)).unwrap();
        }
        assert_eq!(
            insert_alarm(test_alarm(types::MAX_ALARMS as u64, None)),
            Err(InsertAlarmError::NoResolvedAlarmToEvict)
        );
        assert_eq!(ALARMS.with(|m| m.borrow().len()), types::MAX_ALARMS as u64);
    }

    /// Alarm review Blocker 1: fail-closed applies even when the map is full
    /// of a MIX of `Open` and `Acknowledged` rows (not just all-`Open`) —
    /// `Acknowledged` is still active per the `alarms` module's "only
    /// `Resolved` is terminal" contract and must never be treated as an
    /// eviction victim.
    #[test]
    fn alarm_insert_fails_closed_when_no_resolved_alarm_exists() {
        for i in 0..(types::MAX_ALARMS as u64) {
            let mut alarm = test_alarm(i, None);
            if i == 0 {
                alarm.status = types::AlarmStatus::Acknowledged;
            }
            insert_alarm(alarm).unwrap();
        }
        assert_eq!(
            insert_alarm(test_alarm(types::MAX_ALARMS as u64, None)),
            Err(InsertAlarmError::NoResolvedAlarmToEvict)
        );
        assert_eq!(ALARMS.with(|m| m.borrow().len()), types::MAX_ALARMS as u64);
    }

    /// Alarm review Blocker 1's exact reproduction, kept permanently as a
    /// regression: an older `Acknowledged` alarm (still active) must survive
    /// eviction even when a chronologically NEWER `Resolved` alarm (already
    /// terminal) exists in the same map — eviction picks by terminal status
    /// first, `opened_at_secs` only breaks ties among `Resolved` rows.
    #[test]
    fn raise_evicts_resolved_alarm_ahead_of_older_acknowledged_alarm_at_bound() {
        let mut ids = Vec::with_capacity(types::MAX_ALARMS);
        for i in 0..(types::MAX_ALARMS as u64) {
            let id = next_alarm_id();
            ids.push(id);
            let mut alarm = test_alarm(id, None);
            if i == 0 {
                // Oldest row, but ACTIVE — must never be evicted.
                alarm.status = types::AlarmStatus::Acknowledged;
                alarm.opened_at_secs = 1;
                alarm.acknowledged_at_secs = Some(1);
            } else if i == 1 {
                // Newer than id 0, but genuinely TERMINAL — the only valid
                // eviction victim here.
                alarm.status = types::AlarmStatus::Resolved;
                alarm.opened_at_secs = 50;
                alarm.resolved_at_secs = Some(50);
            } else {
                alarm.opened_at_secs = 100;
            }
            insert_alarm(alarm).unwrap();
        }
        let new_id =
            alarms::raise_at(Some(test_target_principal(99)), AlarmKind::Unreachable, 500).unwrap();
        assert_eq!(ALARMS.with(|m| m.borrow().len()), types::MAX_ALARMS as u64);
        // The live, older Acknowledged alarm survives...
        assert_eq!(
            get_alarm(ids[0]).map(|a| a.status),
            Some(types::AlarmStatus::Acknowledged)
        );
        // ...while the newer, but terminal, Resolved alarm is evicted.
        assert_eq!(get_alarm(ids[1]), None);
        assert!(get_alarm(new_id).is_some());
    }

    /// Overwriting an EXISTING alarm id (e.g. acknowledging/resolving it) is
    /// always allowed regardless of bound.
    #[test]
    fn alarm_insert_allows_overwrite_of_existing_id_at_bound() {
        for i in 0..(types::MAX_ALARMS as u64) {
            insert_alarm(test_alarm(i, None)).unwrap();
        }
        let mut resolved = test_alarm(0, None);
        resolved.status = types::AlarmStatus::Resolved;
        assert_eq!(insert_alarm(resolved), Ok(()));
        assert_eq!(ALARMS.with(|m| m.borrow().len()), types::MAX_ALARMS as u64);
    }

    // ── alarm lifecycle (Task 2B): raise / acknowledge / resolve ──

    #[test]
    fn raise_dedupes_active_alarm_for_same_target_and_kind() {
        let target = test_target_principal(1);
        let first = alarms::raise_at(Some(target), AlarmKind::LowBalance, 100).unwrap();
        let second = alarms::raise_at(Some(target), AlarmKind::LowBalance, 200).unwrap();
        assert_eq!(first, second);
        assert_eq!(ALARMS.with(|m| m.borrow().len()), 1);
        // The original `opened_at_secs` is untouched by the deduped call.
        assert_eq!(get_alarm(first).unwrap().opened_at_secs, 100);
    }

    #[test]
    fn raise_allocates_distinct_ids_for_distinct_kind_or_target() {
        let target_a = test_target_principal(1);
        let target_b = test_target_principal(2);
        let low_a = alarms::raise_at(Some(target_a), AlarmKind::LowBalance, 100).unwrap();
        let unreachable_a = alarms::raise_at(Some(target_a), AlarmKind::Unreachable, 100).unwrap();
        let low_b = alarms::raise_at(Some(target_b), AlarmKind::LowBalance, 100).unwrap();
        let global_low = alarms::raise_at(None, AlarmKind::LowBalance, 100).unwrap();
        let ids = [low_a, unreachable_a, low_b, global_low];
        let unique: BTreeSet<u64> = ids.iter().copied().collect();
        assert_eq!(unique.len(), ids.len());
        assert_eq!(ALARMS.with(|m| m.borrow().len()), 4);
    }

    /// Once `Resolved`, a fresh detection of the same condition is a NEW
    /// incident: `raise_at` allocates a new id rather than reopening the
    /// resolved row, and the resolved row's own timestamps are untouched.
    #[test]
    fn raise_reraises_with_new_id_after_resolve() {
        let target = test_target_principal(1);
        let first = alarms::raise_at(Some(target), AlarmKind::LowBalance, 100).unwrap();
        assert_eq!(
            alarms::resolve_at(Some(target), AlarmKind::LowBalance, 150),
            Some(first)
        );
        let second = alarms::raise_at(Some(target), AlarmKind::LowBalance, 200).unwrap();
        assert_ne!(first, second);
        assert_eq!(ALARMS.with(|m| m.borrow().len()), 2);
        let resolved = get_alarm(first).unwrap();
        assert_eq!(resolved.status, types::AlarmStatus::Resolved);
        assert_eq!(resolved.resolved_at_secs, Some(150));
        let reopened = get_alarm(second).unwrap();
        assert_eq!(reopened.status, types::AlarmStatus::Open);
        assert_eq!(reopened.opened_at_secs, 200);
    }

    /// Dedup covers `Acknowledged`, not only `Open`: the underlying
    /// condition is still active even though a signer has seen it, so
    /// `raise_at` must reuse the same id and must NOT reset status back to
    /// `Open`.
    #[test]
    fn raise_while_acknowledged_reuses_id_without_reopening_status() {
        let target = test_target_principal(1);
        let id = alarms::raise_at(Some(target), AlarmKind::LowBalance, 100).unwrap();
        assert_eq!(alarms::acknowledge_at(id, 150), Ok(true));
        let reused = alarms::raise_at(Some(target), AlarmKind::LowBalance, 200).unwrap();
        assert_eq!(id, reused);
        let alarm = get_alarm(id).unwrap();
        assert_eq!(alarm.status, types::AlarmStatus::Acknowledged);
        assert_eq!(alarm.acknowledged_at_secs, Some(150));
    }

    #[test]
    fn acknowledge_rejects_missing_alarm() {
        assert_eq!(
            alarms::acknowledge_at(9_999, 100),
            Err(alarms::AlarmError::AlarmNotFound)
        );
    }

    /// Idempotent replay: a second `acknowledge_at` on an already
    /// `Acknowledged` alarm is a no-op (`Ok(false)`) and does not overwrite
    /// the original `acknowledged_at_secs`.
    #[test]
    fn acknowledge_replay_is_idempotent_and_preserves_original_timestamp() {
        let id =
            alarms::raise_at(Some(test_target_principal(1)), AlarmKind::LowBalance, 100).unwrap();
        assert_eq!(alarms::acknowledge_at(id, 150), Ok(true));
        assert_eq!(alarms::acknowledge_at(id, 999), Ok(false));
        assert_eq!(get_alarm(id).unwrap().acknowledged_at_secs, Some(150));
    }

    #[test]
    fn acknowledge_rejects_resolved_alarm() {
        let target = test_target_principal(1);
        let id = alarms::raise_at(Some(target), AlarmKind::LowBalance, 100).unwrap();
        alarms::resolve_at(Some(target), AlarmKind::LowBalance, 150).unwrap();
        assert_eq!(
            alarms::acknowledge_at(id, 200),
            Err(alarms::AlarmError::AlarmAlreadyResolved)
        );
    }

    /// Auto-resolve reaches an `Acknowledged` alarm too, not only `Open`
    /// ones — acknowledgement suppresses noise for a signer, it does not
    /// change what "the condition cleared" means.
    #[test]
    fn resolve_clears_acknowledged_alarm() {
        let target = test_target_principal(1);
        let id = alarms::raise_at(Some(target), AlarmKind::LowBalance, 100).unwrap();
        alarms::acknowledge_at(id, 120).unwrap();
        assert_eq!(
            alarms::resolve_at(Some(target), AlarmKind::LowBalance, 150),
            Some(id)
        );
        assert_eq!(get_alarm(id).unwrap().status, types::AlarmStatus::Resolved);
    }

    /// Idempotent, exact: resolving with no matching active alarm (never
    /// raised, or already resolved) is a silent no-op, and resolving one
    /// `(target, kind)` pair never touches a different alarm.
    #[test]
    fn resolve_auto_resolve_replay_is_idempotent_and_exact() {
        let target = test_target_principal(1);
        assert_eq!(
            alarms::resolve_at(Some(target), AlarmKind::LowBalance, 100),
            None
        );
        let low = alarms::raise_at(Some(target), AlarmKind::LowBalance, 100).unwrap();
        let unreachable = alarms::raise_at(Some(target), AlarmKind::Unreachable, 100).unwrap();
        assert_eq!(
            alarms::resolve_at(Some(target), AlarmKind::LowBalance, 150),
            Some(low)
        );
        // Replay: already resolved, so a second call is a no-op.
        assert_eq!(
            alarms::resolve_at(Some(target), AlarmKind::LowBalance, 999),
            None
        );
        assert_eq!(get_alarm(low).unwrap().resolved_at_secs, Some(150));
        // The unrelated `Unreachable` alarm for the same target was never touched.
        assert_eq!(
            get_alarm(unreachable).unwrap().status,
            types::AlarmStatus::Open
        );
    }

    /// `raise_at` inherits `insert_alarm`'s fail-closed-at-bound behavior
    /// when every alarm is still `Open` and the new `(target, kind)` pair
    /// genuinely has no active alarm to dedupe against. Fills the bound via
    /// `next_alarm_id()` (not a hand-picked `0..MAX_ALARMS` range like the
    /// plain `insert_alarm` bound tests above use) so the alarm-id counter
    /// stays in sync with what is actually stored — `raise_at` itself always
    /// draws from that same counter, and a test that let the two drift out
    /// of sync could let `raise_at`'s own next id collide with an
    /// already-used one and silently overwrite it instead of hitting the
    /// bound.
    #[test]
    fn raise_fails_closed_when_every_alarm_is_open_at_bound() {
        for _ in 0..(types::MAX_ALARMS as u64) {
            let id = next_alarm_id();
            insert_alarm(test_alarm(id, None)).unwrap();
        }
        assert_eq!(
            alarms::raise_at(Some(test_target_principal(1)), AlarmKind::LowBalance, 100),
            Err(alarms::AlarmError::NoResolvedAlarmToEvict)
        );
        assert_eq!(ALARMS.with(|m| m.borrow().len()), types::MAX_ALARMS as u64);
    }

    /// `raise_at` inherits `insert_alarm`'s self-eviction of the oldest
    /// `Resolved` alarm at the bound. See the previous test's doc for why
    /// the fill loop draws ids from `next_alarm_id()`.
    #[test]
    fn raise_evicts_oldest_resolved_alarm_at_bound() {
        let mut ids = Vec::with_capacity(types::MAX_ALARMS);
        for i in 0..(types::MAX_ALARMS as u64) {
            let id = next_alarm_id();
            ids.push(id);
            let mut alarm = test_alarm(id, None);
            if i == 0 {
                alarm.status = types::AlarmStatus::Resolved;
            }
            insert_alarm(alarm).unwrap();
        }
        let new_id =
            alarms::raise_at(Some(test_target_principal(1)), AlarmKind::LowBalance, 100).unwrap();
        assert_eq!(ALARMS.with(|m| m.borrow().len()), types::MAX_ALARMS as u64);
        assert_eq!(get_alarm(ids[0]), None);
        assert!(get_alarm(new_id).is_some());
    }

    /// `acknowledge_at`/`resolve_at` only ever overwrite an existing id, so
    /// neither can fail at the bound even when every alarm is `Open`. See
    /// `raise_fails_closed_when_every_alarm_is_open_at_bound`'s doc for why
    /// the fill loop draws ids from `next_alarm_id()`.
    #[test]
    fn acknowledge_and_resolve_never_fail_at_bound() {
        let mut ids = Vec::with_capacity(types::MAX_ALARMS);
        for _ in 0..(types::MAX_ALARMS as u64) {
            let id = next_alarm_id();
            ids.push(id);
            insert_alarm(test_alarm(id, None)).unwrap();
        }
        assert_eq!(alarms::acknowledge_at(ids[0], 100), Ok(true));
        // Which specific alarm `resolve_at` picks among the many sharing
        // `(None, LowBalance)` is not the point here — only that neither
        // call fails/panics once the store is at its bound.
        assert!(alarms::resolve_at(None, AlarmKind::LowBalance, 100).is_some());
        assert_eq!(ALARMS.with(|m| m.borrow().len()), types::MAX_ALARMS as u64);
    }

    #[test]
    fn list_nonterminal_operations_excludes_quarantined_and_complete() {
        let global = test_global_policy(1_000_000);
        let target = register_test_target(1, &global);
        let pending =
            test_funding_operation(1, target, FundingTrigger::LowBalanceAutoTopup, &global, 10);
        let complete = test_resolved_operation(2, target, &global, 10);
        insert_operation(pending.clone()).unwrap();
        insert_operation(complete).unwrap();
        let nonterminal = list_nonterminal_operations();
        assert_eq!(nonterminal, vec![pending]);
    }

    #[test]
    fn unresolved_ordinary_operations_include_quarantined_work() {
        let global = test_global_policy(1_000_000);
        let target = register_test_target(3, &global);
        let op =
            test_funding_operation(3, target, FundingTrigger::LowBalanceAutoTopup, &global, 10);
        insert_operation(op.clone()).unwrap();
        let submitted = op
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Submitted),
                11,
                FundingAttemptResultClass::Success,
            )
            .unwrap();
        update_operation(submitted.clone()).unwrap();
        let confirmed = submitted
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Confirmed),
                12,
                FundingAttemptResultClass::Success,
            )
            .unwrap();
        update_operation(confirmed.clone()).unwrap();
        let quarantined = confirmed
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Quarantined),
                13,
                FundingAttemptResultClass::RetryableFailure,
            )
            .unwrap();
        update_operation(quarantined).unwrap();

        let unresolved = list_unresolved_ordinary_operations();
        assert!(unresolved.iter().any(|candidate| candidate.id() == 3));
        assert!(list_nonterminal_operations()
            .iter()
            .all(|candidate| candidate.id() != 3));
    }

    #[test]
    fn burn_anomaly_pauses_dedupes_resolves_but_never_unpauses() {
        let global = test_global_policy(1_000_000);
        let target = register_test_target(4, &global);
        init_test_state(vec![test_signer(1)], 1, 1_000_000);

        crate::observation::apply_burn_anomaly(target, Some(2), Some(24), 10);
        assert!(get_target(target).unwrap().paused());
        let first = find_active_alarm(Some(target), AlarmKind::BurnAnomaly).unwrap();
        assert_eq!(first.status, AlarmStatus::Open);

        crate::observation::apply_burn_anomaly(target, Some(2), Some(24), 11);
        let active = find_active_alarm(Some(target), AlarmKind::BurnAnomaly).unwrap();
        assert_eq!(active.id, first.id);
        assert_eq!(
            list_alarms_after(None, types::MAX_ALARMS)
                .iter()
                .filter(|alarm| alarm.kind == AlarmKind::BurnAnomaly)
                .count(),
            1
        );

        crate::observation::apply_burn_anomaly(target, Some(1), Some(24), 12);
        assert!(find_active_alarm(Some(target), AlarmKind::BurnAnomaly).is_none());
        assert_eq!(get_alarm(first.id).unwrap().status, AlarmStatus::Resolved);
        assert!(get_target(target).unwrap().paused());

        // Indeterminate burn does not clear history and does not alter the
        // governed pause state.
        crate::observation::apply_burn_anomaly(target, None, Some(24), 13);
        assert_eq!(get_alarm(first.id).unwrap().status, AlarmStatus::Resolved);
        assert!(get_target(target).unwrap().paused());
    }

    #[test]
    fn public_target_page_has_terminal_cursor_and_never_uses_epoch_fallback() {
        let global = test_global_policy(1_000_000);
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let first = register_test_target(5, &global);
        let second = register_test_target(6, &global);
        let third = register_test_target(7, &global);

        let page = crate::public_api::list_public_targets_at(None, 2, 100).unwrap();
        assert_eq!(page.items.len(), 2);
        assert!(page.next_cursor.is_some());
        assert_eq!(page.items[0].principal, first);
        assert_eq!(page.items[1].principal, second);
        let final_page =
            crate::public_api::list_public_targets_at(page.next_cursor, 2, 100).unwrap();
        assert_eq!(final_page.items.len(), 1);
        assert_eq!(final_page.items[0].principal, third);
        assert_eq!(final_page.next_cursor, None);

        let never_sampled = crate::public_api::get_public_target_at(first, 100).unwrap();
        assert_eq!(never_sampled.advisory_balance_cycles, None);
        assert_eq!(never_sampled.next_sample_at_secs, None);
    }

    #[test]
    fn public_topup_cursor_is_operation_id_stable_across_oldest_eviction() {
        let global = test_global_policy(1_000_000);
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let target = register_test_target(8, &global);
        for id in 1..=types::MAX_TERMINAL_SUMMARIES as u64 {
            let operation = test_resolved_operation(id, target, &global, id);
            insert_terminal_summary(TerminalFundingSummary::from_resolved(&operation, id).unwrap());
        }

        let first = crate::public_api::list_public_topups_at(target, None, 1).unwrap();
        assert_eq!(first.items.len(), 1);
        assert_eq!(first.items[0].resolved_at_secs, 1);
        let cursor = first.next_cursor.clone().expect("more retained summaries");

        // The cursor is an internal operation id, so eviction of the cursor's
        // row cannot cause a replay. The next row remains strictly after it.
        let operation = test_resolved_operation(513, target, &global, 513);
        insert_terminal_summary(TerminalFundingSummary::from_resolved(&operation, 513).unwrap());
        let second = crate::public_api::list_public_topups_at(target, Some(cursor), 1).unwrap();
        assert_eq!(second.items.len(), 1);
        assert_eq!(second.items[0].resolved_at_secs, 2);
    }

    // ── whole-state validation: positive ──

    #[test]
    fn validate_whole_state_accepts_consistent_state() {
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000);
        init_test_state(vec![test_signer(1)], 1, 1_000);
        let record = test_target_record(1, &global);
        insert_target(record.clone()).unwrap();
        let op = test_funding_operation(
            1,
            record.principal(),
            FundingTrigger::LowBalanceAutoTopup,
            &global,
            10,
        );
        insert_operation(op.clone()).unwrap();
        let reservation = TargetReservationState::new()
            .reserve(op.id(), op.reserved_amount_cycles(), 10, 86_400, 1_000)
            .unwrap();
        set_target_reservation(record.principal(), reservation);
        let global_spend = GlobalRollingSpendState::new()
            .reserve(op.id(), op.reserved_amount_cycles(), 10, 86_400, 1_000)
            .unwrap();
        set_global_rolling_spend(global_spend);
        let source = SourceReserveState::new()
            .refresh(1_000_000, 0, 10)
            .unwrap()
            .reserve_ordinary(op.id(), 10, 0, 10, 86_400)
            .unwrap();
        set_source_reserve(source);

        assert_eq!(validate_whole_state(sentinel_id), Ok(()));
    }

    // ── whole-state validation: negatives, one per invariant ──

    #[test]
    fn validate_whole_state_rejects_uninitialized_global_config() {
        assert_eq!(
            validate_whole_state(test_sentinel_id()),
            Err(StateValidationError::GlobalConfigUninitialized)
        );
    }

    #[test]
    fn validate_whole_state_rejects_empty_signers() {
        GLOBAL_CONFIG.with(|c| {
            c.borrow_mut()
                .set(StoredGlobalConfig::V1(Some(GlobalConfig {
                    signers: vec![],
                    approval_threshold: 1,
                    global_policy: test_global_policy(1_000),
                })))
                .unwrap();
        });
        assert_eq!(
            validate_whole_state(test_sentinel_id()),
            Err(StateValidationError::EmptySigners)
        );
    }

    #[test]
    fn validate_whole_state_rejects_anonymous_signer() {
        GLOBAL_CONFIG.with(|c| {
            c.borrow_mut()
                .set(StoredGlobalConfig::V1(Some(GlobalConfig {
                    signers: vec![Principal::anonymous()],
                    approval_threshold: 1,
                    global_policy: test_global_policy(1_000),
                })))
                .unwrap();
        });
        assert_eq!(
            validate_whole_state(test_sentinel_id()),
            Err(StateValidationError::AnonymousSigner)
        );
    }

    #[test]
    fn validate_whole_state_rejects_duplicate_signer() {
        let s = test_signer(1);
        GLOBAL_CONFIG.with(|c| {
            c.borrow_mut()
                .set(StoredGlobalConfig::V1(Some(GlobalConfig {
                    signers: vec![s, s],
                    approval_threshold: 1,
                    global_policy: test_global_policy(1_000),
                })))
                .unwrap();
        });
        assert_eq!(
            validate_whole_state(test_sentinel_id()),
            Err(StateValidationError::DuplicateSigner(s))
        );
    }

    #[test]
    fn validate_whole_state_rejects_threshold_exceeding_signers() {
        GLOBAL_CONFIG.with(|c| {
            c.borrow_mut()
                .set(StoredGlobalConfig::V1(Some(GlobalConfig {
                    signers: vec![test_signer(1)],
                    approval_threshold: 2,
                    global_policy: test_global_policy(1_000),
                })))
                .unwrap();
        });
        assert_eq!(
            validate_whole_state(test_sentinel_id()),
            Err(StateValidationError::ThresholdExceedsSigners {
                threshold: 2,
                signer_count: 1,
            })
        );
    }

    #[test]
    fn validate_whole_state_rejects_reserved_target_principal() {
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000);
        init_test_state(vec![test_signer(1)], 1, 1_000);
        // `TargetRecord` has no public constructor other than `register`,
        // which itself rejects a principal reserved against its own `ctx`.
        // Register against an UNRELATED sentinel id (so `register` does not
        // reject it) with `principal == sentinel_id`, simulating a record
        // that reached the registry before `sentinel_id` was reassigned —
        // exactly the class of drift `validate_whole_state` must catch that
        // `TargetRecord::register` cannot see at construction time.
        let empty = BTreeSet::new();
        let unrelated_ctx = TargetRegistrationContext {
            sentinel_id: test_target_principal(200),
            existing_target_count: 0,
            existing_target_principals: &empty,
            global_policy: &global,
        };
        let record = TargetRecord::register(
            TargetArgs {
                principal: sentinel_id,
                display_name: "svc".to_string(),
                project: "proj".to_string(),
                environment: Environment::Production,
                criticality: Criticality::Standard,
                observation_mode: ObservationMode::SelfReport,
                tags: vec![],
                funding_policy: test_funding_policy_args(1, 10, 100),
            },
            &unrelated_ctx,
        )
        .unwrap();
        insert_target(record).unwrap();
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::ReservedTargetPrincipal {
                target: sentinel_id,
                kind: types::ReservedPrincipalKind::SentinelSelf,
            })
        );
    }

    #[test]
    fn validate_whole_state_rejects_target_registry_key_mismatch() {
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000);
        init_test_state(vec![test_signer(1)], 1, 1_000);
        let record = test_target_record(1, &global);
        let wrong_key = test_target_principal(2);
        TARGET_REGISTRY.with(|m| {
            m.borrow_mut().insert(
                StorablePrincipal(wrong_key),
                StoredTargetRecord::V1(record.clone()),
            );
        });
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::TargetRegistryKeyMismatch {
                key: wrong_key,
                record: record.principal(),
            })
        );
    }

    /// Final invariant review Finding N4: `FUNDING_OPERATIONS` gets the same
    /// key-vs-record-id backstop `TARGET_REGISTRY`/`PROPOSALS` already had.
    #[test]
    fn validate_whole_state_rejects_funding_operation_key_mismatch() {
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000_000);
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let target = test_target_principal(1);
        let op = test_resolved_operation(1, target, &global, 10);
        let wrong_key = 99u64;
        FUNDING_OPERATIONS.with(|m| {
            m.borrow_mut()
                .insert(wrong_key, StoredFundingOperation::V3(op.clone()));
        });
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::FundingOperationKeyMismatch {
                key: wrong_key,
                operation_id: op.id(),
            })
        );
    }

    /// Final invariant review Finding N4: same backstop for `ALARMS`.
    #[test]
    fn validate_whole_state_rejects_alarm_key_mismatch() {
        let sentinel_id = test_sentinel_id();
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let alarm = test_alarm(1, None);
        let wrong_key = 99u64;
        ALARMS.with(|m| {
            m.borrow_mut()
                .insert(wrong_key, StoredAlarm::V1(alarm.clone()));
        });
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::AlarmKeyMismatch {
                key: wrong_key,
                alarm_id: alarm.id,
            })
        );
    }

    /// Final invariant review Finding N4: same backstop for `TERMINAL_SUMMARIES`.
    #[test]
    fn validate_whole_state_rejects_terminal_summary_key_mismatch() {
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000_000);
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let target = test_target_principal(1);
        let op = test_resolved_operation(1, target, &global, 10);
        let summary = TerminalFundingSummary::from_resolved(&op, 10).unwrap();
        let wrong_key = 99u64;
        TERMINAL_SUMMARIES.with(|m| {
            m.borrow_mut()
                .insert(wrong_key, StoredTerminalFundingSummary::V1(summary.clone()));
        });
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::TerminalSummaryKeyMismatch {
                key: wrong_key,
                operation_id: summary.operation_id(),
            })
        );
    }

    #[test]
    fn validate_whole_state_rejects_target_daily_cap_exceeding_global_cap() {
        let sentinel_id = test_sentinel_id();
        let high_cap_global = test_global_policy(1_000);
        init_test_state(vec![test_signer(1)], 1, 1_000);
        let record = test_target_record(1, &high_cap_global);
        insert_target(record.clone()).unwrap();
        // Governance lowers the global cap below the already-registered
        // target's own daily cap (100) — exactly the scenario Part 5 item 1
        // of the decode review requires this file to catch.
        let lower_cap_global = test_global_policy(50);
        set_global_config(GlobalConfig {
            signers: vec![test_signer(1)],
            approval_threshold: 1,
            global_policy: lower_cap_global,
        });
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::TargetDailyCapExceedsGlobalCap {
                target: record.principal(),
            })
        );
    }

    #[test]
    fn validate_whole_state_rejects_operation_daily_cap_exceeding_global_cap() {
        let sentinel_id = test_sentinel_id();
        let high_cap_global = test_global_policy(1_000);
        init_test_state(vec![test_signer(1)], 1, 1_000);
        // Registered with a target-level cap (10) that stays under BOTH
        // global caps below, so only the operation's own (independently
        // snapshotted, cap 100) `funding_policy` trips the check under test.
        let target = register_test_target_with_cap(1, &high_cap_global, 10);
        let op = test_funding_operation(
            1,
            target,
            FundingTrigger::LowBalanceAutoTopup,
            &high_cap_global,
            10,
        );
        insert_operation(op).unwrap();
        let lower_cap_global = test_global_policy(50);
        set_global_config(GlobalConfig {
            signers: vec![test_signer(1)],
            approval_threshold: 1,
            global_policy: lower_cap_global,
        });
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::OperationDailyCapExceedsGlobalCap { operation_id: 1 })
        );
    }

    /// Regression for accounting-correction Finding 2: a RESOLVED operation
    /// is an immutable historical snapshot. Tightening the global cap via an
    /// ordinary, later `SetGlobalPolicy` governance action must never
    /// retroactively fail an already-settled operation at the next
    /// `post_upgrade` — there is no mutator that could ever "fix" it and no
    /// eviction path for `FUNDING_OPERATIONS`, so before this fix, exercising
    /// this exact governance action would brick every future upgrade
    /// permanently.
    #[test]
    fn validate_whole_state_accepts_resolved_operation_exceeding_current_global_cap() {
        let sentinel_id = test_sentinel_id();
        let high_cap_global = test_global_policy(1_000);
        init_test_state(vec![test_signer(1)], 1, 1_000);
        // Same target-level-cap-vs-operation-level-cap split as the sibling
        // test above.
        let target = register_test_target_with_cap(1, &high_cap_global, 10);
        let op = test_resolved_operation(1, target, &high_cap_global, 10);
        insert_operation(op).unwrap();
        let lower_cap_global = test_global_policy(50);
        set_global_config(GlobalConfig {
            signers: vec![test_signer(1)],
            approval_threshold: 1,
            global_policy: lower_cap_global,
        });
        assert_eq!(validate_whole_state(sentinel_id), Ok(()));
    }

    /// Regression for accounting-correction Finding 4: a `SelfRecovery`
    /// operation's `funding_policy` is a `TargetFundingPolicy` placeholder
    /// only (no self-recovery operation is ever a registered target), so it
    /// must never be compared against the ordinary-targets global cap at
    /// all, resolved or not.
    #[test]
    fn validate_whole_state_ignores_self_recovery_operation_for_global_cap_check() {
        let sentinel_id = test_sentinel_id();
        let high_cap_global = test_global_policy(1_000);
        init_test_state(vec![test_signer(1)], 1, 1_000);
        let op = test_funding_operation(
            1,
            sentinel_id,
            FundingTrigger::SelfRecovery,
            &high_cap_global,
            10,
        );
        insert_operation(op.clone()).unwrap();
        let lower_cap_global = test_global_policy(50);
        set_global_config(GlobalConfig {
            signers: vec![test_signer(1)],
            approval_threshold: 1,
            global_policy: lower_cap_global,
        });
        set_self_recovery_state(
            SelfRecoveryState::new()
                .begin(op.id(), op.reserved_amount_cycles(), 10, 86_400, 100)
                .unwrap(),
        );
        set_source_reserve(
            SourceReserveState::new()
                .refresh(1_000_000, 0, 10)
                .unwrap()
                .reserve_self_recovery(op.id(), op.reserved_amount_cycles(), 10, 86_400)
                .unwrap(),
        );
        assert_eq!(validate_whole_state(sentinel_id), Ok(()));
    }

    #[test]
    fn validate_whole_state_rejects_too_many_nonterminal_operations_for_target() {
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000_000);
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let target = register_test_target(1, &global);
        let op1 =
            test_funding_operation(1, target, FundingTrigger::LowBalanceAutoTopup, &global, 10);
        let op2 = test_funding_operation(2, target, FundingTrigger::ManualTopup, &global, 10);
        insert_operation(op1).unwrap();
        insert_operation(op2).unwrap();
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::TooManyNonterminalOperationsForTarget { target, count: 2 })
        );
    }

    #[test]
    fn validate_whole_state_rejects_target_reservation_missing_operation() {
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000_000);
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let target = register_test_target(1, &global);
        let op =
            test_funding_operation(1, target, FundingTrigger::LowBalanceAutoTopup, &global, 10);
        insert_operation(op.clone()).unwrap();
        // No matching TargetReservationState inserted.
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::TargetReservationMissingOperation {
                target,
                operation_id: op.id(),
            })
        );
    }

    #[test]
    fn validate_whole_state_rejects_nonterminal_operation_missing_target_reservation_reverse() {
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000_000);
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let target = test_target_principal(1);
        insert_target(test_target_record_at(target, &global)).unwrap();
        // A reservation exists but no matching operation was ever inserted.
        let reservation = TargetReservationState::new()
            .reserve(42, 10, 10, 86_400, 1_000)
            .unwrap();
        set_target_reservation(target, reservation);
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(
                StateValidationError::NonterminalOperationMissingTargetReservation {
                    operation_id: 42,
                    target,
                }
            )
        );
    }

    #[test]
    fn validate_whole_state_rejects_target_reservation_amount_mismatch() {
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000_000);
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let target = test_target_principal(1);
        insert_target(test_target_record_at(target, &global)).unwrap();
        let op =
            test_funding_operation(1, target, FundingTrigger::LowBalanceAutoTopup, &global, 10);
        insert_operation(op.clone()).unwrap();
        let reservation = TargetReservationState::new()
            .reserve(op.id(), op.reserved_amount_cycles() + 1, 10, 86_400, 1_000)
            .unwrap();
        set_target_reservation(target, reservation);
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::TargetReservationAmountMismatch {
                target,
                operation_id: op.id(),
            })
        );
    }

    #[test]
    fn validate_whole_state_rejects_nonterminal_operation_missing_global_reservation() {
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000_000);
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let target = test_target_principal(1);
        insert_target(test_target_record_at(target, &global)).unwrap();
        let op =
            test_funding_operation(1, target, FundingTrigger::LowBalanceAutoTopup, &global, 10);
        insert_operation(op.clone()).unwrap();
        let reservation = TargetReservationState::new()
            .reserve(op.id(), op.reserved_amount_cycles(), 10, 86_400, 1_000)
            .unwrap();
        set_target_reservation(target, reservation);
        // No matching entry in the global rolling-spend ledger.
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(
                StateValidationError::NonterminalOperationMissingGlobalReservation {
                    operation_id: op.id(),
                }
            )
        );
    }

    #[test]
    fn validate_whole_state_rejects_global_reservation_missing_operation_reverse() {
        let sentinel_id = test_sentinel_id();
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let global_spend = GlobalRollingSpendState::new()
            .reserve(99, 10, 10, 86_400, 1_000)
            .unwrap();
        set_global_rolling_spend(global_spend);
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::GlobalReservationMissingOperation { operation_id: 99 })
        );
    }

    #[test]
    fn validate_whole_state_rejects_global_reservation_amount_mismatch() {
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000_000);
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let target = test_target_principal(1);
        insert_target(test_target_record_at(target, &global)).unwrap();
        let op =
            test_funding_operation(1, target, FundingTrigger::LowBalanceAutoTopup, &global, 10);
        insert_operation(op.clone()).unwrap();
        let reservation = TargetReservationState::new()
            .reserve(op.id(), op.reserved_amount_cycles(), 10, 86_400, 1_000)
            .unwrap();
        set_target_reservation(target, reservation);
        let global_spend = GlobalRollingSpendState::new()
            .reserve(op.id(), op.reserved_amount_cycles() + 1, 10, 86_400, 1_000)
            .unwrap();
        set_global_rolling_spend(global_spend);
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::GlobalReservationAmountMismatch {
                operation_id: op.id()
            })
        );
    }

    /// Builds the target/global reservation pair `validate_whole_state`
    /// already requires for `op`, so the source-reserve tests below isolate
    /// their failure to the new Task 4 check alone.
    fn set_up_matching_target_and_global_reservation(target: Principal, op: &FundingOperation) {
        let reservation = TargetReservationState::new()
            .reserve(op.id(), op.reserved_amount_cycles(), 10, 86_400, 1_000)
            .unwrap();
        set_target_reservation(target, reservation);
        let global_spend = GlobalRollingSpendState::new()
            .reserve(op.id(), op.reserved_amount_cycles(), 10, 86_400, 1_000)
            .unwrap();
        set_global_rolling_spend(global_spend);
    }

    #[test]
    fn validate_whole_state_rejects_unresolved_cycles_operation_missing_source_reserve() {
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000_000);
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let target = test_target_principal(1);
        insert_target(test_target_record_at(target, &global)).unwrap();
        let op =
            test_funding_operation(1, target, FundingTrigger::LowBalanceAutoTopup, &global, 10);
        insert_operation(op.clone()).unwrap();
        set_up_matching_target_and_global_reservation(target, &op);
        // No SOURCE_RESERVE entry set up at all.
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(
                StateValidationError::NonterminalOperationMissingSourceReserve {
                    operation_id: op.id()
                }
            )
        );
    }

    #[test]
    fn validate_whole_state_rejects_source_reserve_amount_mismatch() {
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000_000);
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let target = test_target_principal(1);
        insert_target(test_target_record_at(target, &global)).unwrap();
        let op =
            test_funding_operation(1, target, FundingTrigger::LowBalanceAutoTopup, &global, 10);
        insert_operation(op.clone()).unwrap();
        set_up_matching_target_and_global_reservation(target, &op);
        // Snapshot amount is 10 + 0 fee = 10; reserve a disagreeing amount.
        let source = SourceReserveState::new()
            .refresh(1_000_000, 0, 10)
            .unwrap()
            .reserve_ordinary(op.id(), 11, 0, 10, 86_400)
            .unwrap();
        set_source_reserve(source);
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::SourceReserveAmountMismatch {
                operation_id: op.id()
            })
        );
    }

    #[test]
    fn validate_whole_state_rejects_source_reserve_missing_operation_reverse() {
        let sentinel_id = test_sentinel_id();
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        // A pending source debit with no matching unresolved Cycles-rail
        // operation at all.
        let source = SourceReserveState::new()
            .refresh(1_000_000, 0, 10)
            .unwrap()
            .reserve_ordinary(999, 5, 0, 10, 86_400)
            .unwrap();
        set_source_reserve(source);
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::SourceReserveMissingOperation { operation_id: 999 })
        );
    }

    #[test]
    fn validate_whole_state_accepts_self_recovery_operation_with_matching_source_reserve() {
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000_000);
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let op = test_funding_operation(1, sentinel_id, FundingTrigger::SelfRecovery, &global, 10);
        insert_operation(op.clone()).unwrap();
        let self_recovery = SelfRecoveryState::new()
            .begin(op.id(), op.reserved_amount_cycles(), 10, 86_400, 1_000)
            .unwrap();
        set_self_recovery_state(self_recovery);
        let source = SourceReserveState::new()
            .refresh(1_000_000, 0, 10)
            .unwrap()
            .reserve_self_recovery(op.id(), 10, 10, 86_400)
            .unwrap();
        set_source_reserve(source);
        assert_eq!(validate_whole_state(sentinel_id), Ok(()));
    }

    #[test]
    fn validate_whole_state_rejects_self_recovery_operation_missing_source_reserve() {
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000_000);
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let op = test_funding_operation(1, sentinel_id, FundingTrigger::SelfRecovery, &global, 10);
        insert_operation(op.clone()).unwrap();
        let self_recovery = SelfRecoveryState::new()
            .begin(op.id(), op.reserved_amount_cycles(), 10, 86_400, 1_000)
            .unwrap();
        set_self_recovery_state(self_recovery);
        // No SOURCE_RESERVE entry: self-recovery also draws on the shared
        // source reserve, not just its own daily-cap ledger.
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(
                StateValidationError::NonterminalOperationMissingSourceReserve {
                    operation_id: op.id()
                }
            )
        );
    }

    #[test]
    fn validate_whole_state_rejects_self_recovery_operation_missing() {
        let sentinel_id = test_sentinel_id();
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        set_self_recovery_state(
            SelfRecoveryState::new()
                .begin(7, 10, 0, 86_400, 100)
                .unwrap(),
        );
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::SelfRecoveryOperationMissing { operation_id: 7 })
        );
    }

    #[test]
    fn validate_whole_state_rejects_self_recovery_operation_wrong_target() {
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000_000);
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let wrong_target = test_target_principal(1);
        let op = test_funding_operation(1, wrong_target, FundingTrigger::SelfRecovery, &global, 10);
        insert_operation(op.clone()).unwrap();
        // Deliberately no `TARGET_RESERVATIONS`/`GLOBAL_ROLLING_SPEND` entry:
        // a `SelfRecovery`-triggered operation must never use those ordinary
        // stores (Finding 4), so the self-recovery section is reached
        // directly rather than tripping the ordinary bidirectional checks.
        set_self_recovery_state(
            SelfRecoveryState::new()
                .begin(op.id(), op.reserved_amount_cycles(), 10, 86_400, 100)
                .unwrap(),
        );
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::SelfRecoveryOperationWrongTarget {
                operation_id: op.id()
            })
        );
    }

    #[test]
    fn validate_whole_state_rejects_self_recovery_operation_wrong_trigger() {
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000_000);
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        // This operation's trigger is NOT `SelfRecovery` even though its
        // target is `sentinel_id`. An UNRESOLVED operation shaped like this
        // is now rejected earlier and more generally by
        // `OrdinaryOperationTargetsSentinel` (final invariant review Finding
        // N5) before this self-recovery-specific check is ever reached, and
        // by `insert_operation`'s own registration check at construction
        // time — so a RESOLVED operation (exempt from both) is required to
        // reach this check, and can only be seeded by bypassing the checked
        // constructor, exactly like the other whole-state backstop tests in
        // this file (e.g. a pre-fix legacy snapshot or direct repair-tooling
        // manipulation).
        let op = test_resolved_operation(1, sentinel_id, &global, 10);
        FUNDING_OPERATIONS.with(|m| {
            m.borrow_mut()
                .insert(op.id(), StoredFundingOperation::V3(op.clone()));
        });
        set_self_recovery_state(
            SelfRecoveryState::new()
                .begin(op.id(), op.reserved_amount_cycles(), 10, 86_400, 100)
                .unwrap(),
        );
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::SelfRecoveryOperationWrongTrigger {
                operation_id: op.id()
            })
        );
    }

    /// Final invariant review Finding N5: an unresolved, ordinary-trigger
    /// operation must never be able to target `sentinel_id` — only the
    /// hard-coded `SelfRecovery` lane may. `insert_operation` itself already
    /// blocks this at construction time (the target is never registered, and
    /// can never be — `sentinel_id` is a reserved principal), so this state
    /// can only be reached the same way the sibling backstop tests reach
    /// their corrupted snapshots: bypassing the checked constructor.
    #[test]
    fn validate_whole_state_rejects_ordinary_operation_targeting_sentinel() {
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000_000);
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let op = test_funding_operation(1, sentinel_id, FundingTrigger::ManualTopup, &global, 10);
        FUNDING_OPERATIONS.with(|m| {
            m.borrow_mut()
                .insert(op.id(), StoredFundingOperation::V3(op.clone()));
        });
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::OrdinaryOperationTargetsSentinel {
                operation_id: op.id()
            })
        );
    }

    /// Storage review Finding 3 hardening: an unresolved, ordinary-trigger
    /// operation whose target has no live `TARGET_REGISTRY` entry (and is
    /// not `sentinel_id`, which is `OrdinaryOperationTargetsSentinel`'s own
    /// case) is rejected. `insert_operation` already blocks this at
    /// construction time, so — same as the sibling tests above — this state
    /// can only be reached by bypassing the checked constructor.
    #[test]
    fn validate_whole_state_rejects_orphan_funding_operation_target() {
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000_000);
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let target = test_target_principal(1);
        let op =
            test_funding_operation(1, target, FundingTrigger::LowBalanceAutoTopup, &global, 10);
        FUNDING_OPERATIONS.with(|m| {
            m.borrow_mut()
                .insert(op.id(), StoredFundingOperation::V3(op.clone()));
        });
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::OrphanFundingOperationTarget {
                operation_id: op.id(),
                target,
            })
        );
    }

    /// A RESOLVED operation is an immutable historical snapshot that may
    /// legitimately outlive its target's removal — `OrphanFundingOperationTarget`
    /// must not fire for it, mirroring `TERMINAL_SUMMARIES`' own survive-removal
    /// framing.
    #[test]
    fn validate_whole_state_accepts_resolved_operation_for_unregistered_target() {
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000_000);
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let target = test_target_principal(1);
        let op = test_resolved_operation(1, target, &global, 10);
        FUNDING_OPERATIONS.with(|m| {
            m.borrow_mut()
                .insert(op.id(), StoredFundingOperation::V3(op.clone()));
        });
        assert_eq!(validate_whole_state(sentinel_id), Ok(()));
    }

    #[test]
    fn validate_whole_state_rejects_self_recovery_operation_already_resolved() {
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000_000);
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let op = test_resolved_operation(1, sentinel_id, &global, 10);
        // Overwrite trigger via a fresh operation with the right target and
        // trigger but drive it to resolved via record_attempt directly.
        let op = FundingOperation::open(
            2,
            sentinel_id,
            1,
            op.funding_policy().clone(),
            FundingTrigger::SelfRecovery,
            op.rail_arguments().clone(),
            op.reserved_amount_cycles(),
            10,
        )
        .unwrap();
        let op = op
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Submitted),
                10,
                FundingAttemptResultClass::Success,
            )
            .unwrap();
        let op = op
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Confirmed),
                10,
                FundingAttemptResultClass::Success,
            )
            .unwrap();
        let op = op
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Complete),
                10,
                FundingAttemptResultClass::Success,
            )
            .unwrap();
        insert_operation(op.clone()).unwrap();
        set_self_recovery_state(
            SelfRecoveryState::new()
                .begin(op.id(), op.reserved_amount_cycles(), 10, 86_400, 100)
                .unwrap(),
        );
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::SelfRecoveryOperationAlreadyResolved {
                operation_id: op.id()
            })
        );
    }

    /// Regression for accounting-correction Finding 3 (reverse direction): a
    /// live, unresolved `SelfRecovery` operation exists but `SELF_RECOVERY`
    /// has no record of it (`in_flight_operation_id` stays `None`). Before
    /// this fix, `validate_whole_state` only ever walked the forward
    /// direction ("if the singleton claims an op, that op must be
    /// consistent") and never this reverse direction, so this exact state
    /// silently passed `post_upgrade` despite
    /// `SelfRecoveryState::is_suppressing_distribution()` incorrectly
    /// reporting `false` while a self-recovery spend was genuinely in
    /// flight — defeating the design's distribution-suppression guarantee.
    #[test]
    fn validate_whole_state_rejects_self_recovery_operation_not_recorded_in_flight() {
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000_000);
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let op = test_funding_operation(1, sentinel_id, FundingTrigger::SelfRecovery, &global, 10);
        insert_operation(op.clone()).unwrap();
        // SELF_RECOVERY is left at its default (`in_flight_operation_id:
        // None`) — never told about `op`.
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(
                StateValidationError::SelfRecoveryOperationNotRecordedInFlight {
                    operation_id: op.id()
                }
            )
        );
    }

    /// Same reverse-direction gap, but with a SECOND self-recovery operation
    /// concurrent with a correctly-recorded first one: the singleton ledger
    /// can only ever point at one operation id, so a second nonterminal
    /// `SelfRecovery` operation can never be the recorded one and must be
    /// rejected — this is also how the reverse check enforces "at most one
    /// nonterminal self-recovery operation" now that self-recovery is
    /// excluded from the ordinary per-target `TooManyNonterminalOperationsForTarget`
    /// grouping (Finding 4).
    #[test]
    fn validate_whole_state_rejects_second_concurrent_self_recovery_operation() {
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000_000);
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let op1 = test_funding_operation(1, sentinel_id, FundingTrigger::SelfRecovery, &global, 10);
        let op2 = test_funding_operation(2, sentinel_id, FundingTrigger::SelfRecovery, &global, 10);
        insert_operation(op1.clone()).unwrap();
        insert_operation(op2.clone()).unwrap();
        set_self_recovery_state(
            SelfRecoveryState::new()
                .begin(op1.id(), op1.reserved_amount_cycles(), 10, 86_400, 100)
                .unwrap(),
        );
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(
                StateValidationError::SelfRecoveryOperationNotRecordedInFlight {
                    operation_id: op2.id()
                }
            )
        );
    }

    /// Regression for accounting-correction Finding 4: a `SelfRecovery`
    /// operation must never be reflected in the ORDINARY per-target
    /// `TARGET_RESERVATIONS` store — self-recovery has its own singleton
    /// ledger. If it ever is (e.g. a future bug), the ordinary bidirectional
    /// per-target check must reject it, since `nonterminal_by_target`
    /// deliberately excludes `SelfRecovery`-triggered operations.
    #[test]
    fn validate_whole_state_rejects_self_recovery_operation_leaking_into_target_reservations() {
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000_000);
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let op = test_funding_operation(1, sentinel_id, FundingTrigger::SelfRecovery, &global, 10);
        insert_operation(op.clone()).unwrap();
        let reservation = TargetReservationState::new()
            .reserve(op.id(), op.reserved_amount_cycles(), 10, 86_400, 1_000)
            .unwrap();
        set_target_reservation(sentinel_id, reservation);
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(
                StateValidationError::NonterminalOperationMissingTargetReservation {
                    operation_id: op.id(),
                    target: sentinel_id,
                }
            )
        );
    }

    /// Same as above but for the ORDINARY global `GLOBAL_ROLLING_SPEND`
    /// ledger: a `SelfRecovery` operation's id must never appear there
    /// either.
    #[test]
    fn validate_whole_state_rejects_self_recovery_operation_leaking_into_global_rolling_spend() {
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000_000);
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let op = test_funding_operation(1, sentinel_id, FundingTrigger::SelfRecovery, &global, 10);
        insert_operation(op.clone()).unwrap();
        let global_spend = GlobalRollingSpendState::new()
            .reserve(op.id(), op.reserved_amount_cycles(), 10, 86_400, 1_000)
            .unwrap();
        set_global_rolling_spend(global_spend);
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::GlobalReservationMissingOperation {
                operation_id: op.id()
            })
        );
    }

    /// Regression for accounting-correction Finding 5 (forward amount
    /// linkage): the self-recovery ledger's recorded pending amount must
    /// exactly match the live operation's `reserved_amount_cycles()`.
    #[test]
    fn validate_whole_state_rejects_self_recovery_ledger_amount_mismatch() {
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000_000);
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let op = test_funding_operation(1, sentinel_id, FundingTrigger::SelfRecovery, &global, 10);
        insert_operation(op.clone()).unwrap();
        // Reserve a DIFFERENT amount than the operation actually carries.
        set_self_recovery_state(
            SelfRecoveryState::new()
                .begin(op.id(), op.reserved_amount_cycles() + 1, 10, 86_400, 100)
                .unwrap(),
        );
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::SelfRecoveryLedgerAmountMismatch {
                operation_id: op.id()
            })
        );
    }

    /// Regression for accounting-correction Finding 5 (cap consistency): a
    /// live pending self-recovery reservation must never exceed the
    /// CURRENTLY configured `SelfRecoveryPolicy.daily_cap_cycles` — governance
    /// tightening the self-recovery cap below an already-reserved live
    /// amount must be caught, not silently accepted.
    #[test]
    fn validate_whole_state_rejects_self_recovery_ledger_exceeding_daily_cap() {
        let sentinel_id = test_sentinel_id();
        // `test_self_recovery_args(1_000, 100, 1, 10)` — daily cap 100 in the
        // policy used at reservation time.
        let global = test_global_policy(1_000_000);
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let op = test_funding_operation(1, sentinel_id, FundingTrigger::SelfRecovery, &global, 10);
        insert_operation(op.clone()).unwrap();
        set_self_recovery_state(
            SelfRecoveryState::new()
                .begin(op.id(), op.reserved_amount_cycles(), 10, 86_400, 100)
                .unwrap(),
        );
        // Governance tightens the self-recovery daily cap below the amount
        // already reserved above (10).
        let mut args = test_global_policy_args(1_000_000);
        args.self_recovery_policy = test_self_recovery_args(1_000, 5, 1, 5);
        let tighter_global = GlobalPolicy::validate(&args).unwrap();
        set_global_config(GlobalConfig {
            signers: vec![test_signer(1)],
            approval_threshold: 1,
            global_policy: tighter_global,
        });
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::SelfRecoveryLedgerExceedsDailyCap {
                operation_id: op.id()
            })
        );
    }

    /// Positive companion to the self-recovery negatives above: a fully
    /// consistent self-recovery in-flight operation — recorded by the
    /// singleton, amount-linked, within the current daily cap, and absent
    /// from both ordinary reservation stores — passes whole-state
    /// validation.
    #[test]
    fn validate_whole_state_accepts_consistent_self_recovery_operation() {
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000_000);
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let op = test_funding_operation(1, sentinel_id, FundingTrigger::SelfRecovery, &global, 10);
        insert_operation(op.clone()).unwrap();
        set_self_recovery_state(
            SelfRecoveryState::new()
                .begin(op.id(), op.reserved_amount_cycles(), 10, 86_400, 100)
                .unwrap(),
        );
        set_source_reserve(
            SourceReserveState::new()
                .refresh(1_000_000, 0, 10)
                .unwrap()
                .reserve_self_recovery(op.id(), op.reserved_amount_cycles(), 10, 86_400)
                .unwrap(),
        );
        assert_eq!(validate_whole_state(sentinel_id), Ok(()));
    }

    #[test]
    fn validate_whole_state_rejects_proposal_key_mismatch() {
        let sentinel_id = test_sentinel_id();
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let record = ProposalRecord::new(
            5,
            ProposalPayload::AddSigner {
                signer: test_signer(2),
            },
            test_signer(1),
            10,
        );
        PROPOSALS.with(|m| {
            m.borrow_mut().insert(9, StoredProposalRecord::V1(record));
        });
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::ProposalKeyMismatch {
                key: 9,
                record_id: 5
            })
        );
    }

    #[test]
    fn validate_whole_state_rejects_proposal_approval_not_a_signer() {
        let sentinel_id = test_sentinel_id();
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let mut record = ProposalRecord::new(
            1,
            ProposalPayload::AddSigner {
                signer: test_signer(2),
            },
            test_signer(1),
            10,
        );
        let stranger = test_signer(99);
        record.record_approval(stranger);
        insert_proposal(record).unwrap();
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::ProposalApprovalNotASigner {
                proposal_id: 1,
                approver: stranger,
            })
        );
    }

    /// Regression for accounting-correction Finding 1: an `Executed`
    /// proposal is a closed historical record. Approving it while a signer
    /// who has SINCE been removed (an ordinary, expected `RemoveSigner`
    /// governance action) must never retroactively fail `validate_whole_state`
    /// — before this fix, the unfiltered loop checked every proposal
    /// regardless of `status`, so this exact, entirely ordinary sequence
    /// (approve as B while B is a signer, later remove B) would permanently
    /// brick every future `post_upgrade`.
    #[test]
    fn validate_whole_state_accepts_executed_proposal_approval_after_signer_removal() {
        let sentinel_id = test_sentinel_id();
        let signer_a = test_signer(1);
        let signer_b = test_signer(2);
        init_test_state(vec![signer_a, signer_b], 2, 1_000_000);
        let mut record = ProposalRecord::new(
            1,
            ProposalPayload::AddSigner {
                signer: test_signer(3),
            },
            signer_a,
            10,
        );
        record.record_approval(signer_a);
        record.record_approval(signer_b);
        record.status = ProposalStatus::Executed;
        insert_proposal(record).unwrap();

        // B is removed AFTER the proposal above already executed — an
        // ordinary, unrelated later governance action.
        set_global_config(GlobalConfig {
            signers: vec![signer_a],
            approval_threshold: 1,
            global_policy: test_global_policy(1_000_000),
        });

        assert_eq!(validate_whole_state(sentinel_id), Ok(()));
    }

    /// Same regression, `Cancelled` variant: a cancelled proposal is equally
    /// a closed historical record and must not be re-checked either.
    #[test]
    fn validate_whole_state_accepts_cancelled_proposal_approval_after_signer_removal() {
        let sentinel_id = test_sentinel_id();
        let signer_a = test_signer(1);
        let signer_b = test_signer(2);
        init_test_state(vec![signer_a, signer_b], 2, 1_000_000);
        let mut record = ProposalRecord::new(
            1,
            ProposalPayload::AddSigner {
                signer: test_signer(3),
            },
            signer_a,
            10,
        );
        record.record_approval(signer_b);
        record.status = ProposalStatus::Cancelled;
        insert_proposal(record).unwrap();

        set_global_config(GlobalConfig {
            signers: vec![signer_a],
            approval_threshold: 1,
            global_policy: test_global_policy(1_000_000),
        });

        assert_eq!(validate_whole_state(sentinel_id), Ok(()));
    }

    /// An `Open` proposal, by contrast, must still be checked against the
    /// current signer set — a since-removed approver on a still-open
    /// proposal is exactly the live-drift case Finding 1's fix must keep
    /// catching, distinguishing it from the historical-record cases above.
    #[test]
    fn validate_whole_state_rejects_open_proposal_approval_after_signer_removal() {
        let sentinel_id = test_sentinel_id();
        let signer_a = test_signer(1);
        let signer_b = test_signer(2);
        init_test_state(vec![signer_a, signer_b], 2, 1_000_000);
        let mut record = ProposalRecord::new(
            1,
            ProposalPayload::AddSigner {
                signer: test_signer(3),
            },
            signer_a,
            10,
        );
        record.record_approval(signer_b);
        // status stays Open (ProposalRecord::new's default).
        insert_proposal(record).unwrap();

        set_global_config(GlobalConfig {
            signers: vec![signer_a],
            approval_threshold: 1,
            global_policy: test_global_policy(1_000_000),
        });

        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::ProposalApprovalNotASigner {
                proposal_id: 1,
                approver: signer_b,
            })
        );
    }

    fn test_target_record_at(principal: Principal, global: &GlobalPolicy) -> TargetRecord {
        let empty = BTreeSet::new();
        let ctx = TargetRegistrationContext {
            sentinel_id: test_sentinel_id(),
            existing_target_count: 0,
            existing_target_principals: &empty,
            global_policy: global,
        };
        TargetRecord::register(
            TargetArgs {
                principal,
                display_name: "svc".to_string(),
                project: "proj".to_string(),
                environment: Environment::Production,
                criticality: Criticality::Standard,
                observation_mode: ObservationMode::SelfReport,
                tags: vec![],
                funding_policy: test_funding_policy_args(1, 10, 100),
            },
            &ctx,
        )
        .unwrap()
    }

    // ── whole-state validation: orphan sample/reservation stores (Finding 3) ──

    #[test]
    fn validate_whole_state_rejects_orphan_sample_meta() {
        let sentinel_id = test_sentinel_id();
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let target = test_target_principal(1);
        // Directly manipulate the map — never registered, so this can only
        // be reached by bypassing `record_sample`'s own registration check
        // (e.g. a pre-fix legacy snapshot).
        SAMPLE_META.with(|m| {
            m.borrow_mut().insert(
                StorablePrincipal(target),
                StoredSampleMeta::V1(SampleMetaV1 {
                    next_slot: 1,
                    filled_slots: 1,
                    last_success_at_secs: Some(1),
                    last_attempt_at_secs: Some(1),
                }),
            );
        });
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::OrphanSampleMeta { target })
        );
    }

    #[test]
    fn validate_whole_state_rejects_orphan_sample_row() {
        let sentinel_id = test_sentinel_id();
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let target = test_target_principal(1);
        SAMPLES.with(|m| {
            m.borrow_mut().insert(
                SampleKey {
                    principal: target,
                    slot: 0,
                },
                StoredSample::V2(test_sample(1, PublicTargetState::Healthy)),
            );
        });
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::OrphanSample { target, slot: 0 })
        );
    }

    #[test]
    fn validate_whole_state_rejects_orphan_target_reservation() {
        let sentinel_id = test_sentinel_id();
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let target = test_target_principal(1);
        let reservation = TargetReservationState::new()
            .reserve(1, 10, 10, 86_400, 1_000)
            .unwrap();
        set_target_reservation(target, reservation);
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::OrphanTargetReservation { target })
        );
    }

    #[test]
    fn validate_whole_state_accepts_sentinel_id_reservation_without_registration() {
        // `sentinel_id` can never be a `TARGET_REGISTRY` entry, so it is
        // exempt from the orphan-target-reservation framing above. Uses a
        // SETTLED (non-pending) reservation so no bidirectional
        // operation<->reservation linkage is in play — purely exercising the
        // orphan exemption in isolation.
        let sentinel_id = test_sentinel_id();
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let reservation = TargetReservationState::new()
            .reserve(1, 10, 10, 86_400, 1_000)
            .unwrap()
            .settle_spend(1, 10, 20, 86_400)
            .unwrap();
        set_target_reservation(sentinel_id, reservation);
        assert_eq!(validate_whole_state(sentinel_id), Ok(()));
    }

    #[test]
    fn validate_whole_state_rejects_impossible_sample_meta_next_slot_out_of_range() {
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000);
        init_test_state(vec![test_signer(1)], 1, 1_000);
        let target = register_test_target(1, &global);
        SAMPLE_META.with(|m| {
            m.borrow_mut().insert(
                StorablePrincipal(target),
                StoredSampleMeta::V1(SampleMetaV1 {
                    next_slot: MAX_SAMPLES_PER_TARGET_U32,
                    filled_slots: 1,
                    last_success_at_secs: None,
                    last_attempt_at_secs: None,
                }),
            );
        });
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::ImpossibleSampleMeta { target })
        );
    }

    #[test]
    fn validate_whole_state_rejects_impossible_sample_meta_next_slot_ahead_of_filled_slots() {
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000);
        init_test_state(vec![test_signer(1)], 1, 1_000);
        let target = register_test_target(1, &global);
        // Before a wrap, `next_slot` must exactly equal `filled_slots`.
        SAMPLE_META.with(|m| {
            m.borrow_mut().insert(
                StorablePrincipal(target),
                StoredSampleMeta::V1(SampleMetaV1 {
                    next_slot: 3,
                    filled_slots: 5,
                    last_success_at_secs: None,
                    last_attempt_at_secs: None,
                }),
            );
        });
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::ImpossibleSampleMeta { target })
        );
    }

    #[test]
    fn validate_whole_state_accepts_consistent_sample_meta_after_wrap() {
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000);
        init_test_state(vec![test_signer(1)], 1, 1_000);
        let target = register_test_target(1, &global);
        SAMPLE_META.with(|m| {
            m.borrow_mut().insert(
                StorablePrincipal(target),
                // V1's migration only ever sets `total_writes = filled_slots`
                // (no-wrap assumption), which cannot represent a post-wrap
                // `next_slot != 0` state — use V2 directly with a
                // `total_writes` that is actually consistent with
                // `next_slot == 5` after wrapping past a full ring once.
                StoredSampleMeta::V2(SampleMeta {
                    next_slot: 5,
                    filled_slots: MAX_SAMPLES_PER_TARGET_U32,
                    last_success_at_secs: None,
                    last_attempt_at_secs: None,
                    total_writes: MAX_SAMPLES_PER_TARGET_U32 as u64 + 5,
                }),
            );
        });
        assert_eq!(validate_whole_state(sentinel_id), Ok(()));
    }

    // ── whole-state validation: too many funding operations (Finding 1) ──

    #[test]
    fn validate_whole_state_rejects_too_many_funding_operations() {
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000_000_000);
        init_test_state(vec![test_signer(1)], 1, 1_000_000_000);
        // Bypasses `insert_operation`'s own self-enforcing bound via a
        // direct map insert, mirroring the targets/alarms/proposals backstop
        // tests above.
        for i in 0..=(types::MAX_FUNDING_OPERATIONS as u64) {
            let target = test_target_principal((i % 250) as u8 + 1);
            let op =
                test_funding_operation(i, target, FundingTrigger::LowBalanceAutoTopup, &global, 10);
            FUNDING_OPERATIONS.with(|m| {
                m.borrow_mut().insert(i, StoredFundingOperation::V3(op));
            });
        }
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::TooManyFundingOperations {
                count: types::MAX_FUNDING_OPERATIONS as u64 + 1,
            })
        );
    }

    /// `insert_target` itself is self-enforcing (storage review Finding 3b):
    /// this drives the bound via the direct map insert (bypassing
    /// `insert_target`'s own check) purely to exercise
    /// `validate_whole_state`'s independent backstop — see
    /// `target_insert_fails_closed_at_bound` below for the storage-boundary
    /// behavior itself.
    #[test]
    fn validate_whole_state_rejects_too_many_targets() {
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000);
        init_test_state(vec![test_signer(1)], 1, 1_000);
        for i in 0..=(types::MAX_TARGETS as u16) {
            let principal = Principal::from_slice(&[7, (i % 256) as u8, (i / 256) as u8]);
            let record = test_target_record_at(principal, &global);
            TARGET_REGISTRY.with(|m| {
                m.borrow_mut()
                    .insert(StorablePrincipal(principal), StoredTargetRecord::V1(record));
            });
        }
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::TooManyTargets {
                count: types::MAX_TARGETS as u64 + 1,
            })
        );
    }

    /// Self-enforcing at the storage boundary (storage review Finding 3b):
    /// a genuinely new principal is rejected once the registry is at
    /// `MAX_TARGETS`, since a target has no "resolved/removable" analog to
    /// evict — unlike alarms/proposals, this is fail-closed only.
    #[test]
    fn target_insert_fails_closed_at_bound() {
        let global = test_global_policy(1_000);
        for i in 0..(types::MAX_TARGETS as u16) {
            let principal = Principal::from_slice(&[7, (i % 256) as u8, (i / 256) as u8]);
            insert_target(test_target_record_at(principal, &global)).unwrap();
        }
        let overflow_principal =
            Principal::from_slice(&[7, (types::MAX_TARGETS % 256) as u8, 0xFF]);
        assert_eq!(
            insert_target(test_target_record_at(overflow_principal, &global)),
            Err(InsertTargetError::TooManyTargets)
        );
        assert_eq!(target_count(), types::MAX_TARGETS as u64);
    }

    /// Overwriting an EXISTING principal (an ordinary `apply_patch` update)
    /// is always allowed regardless of bound.
    #[test]
    fn target_insert_allows_overwrite_of_existing_principal_at_bound() {
        let global = test_global_policy(1_000);
        let mut first_principal = None;
        for i in 0..(types::MAX_TARGETS as u16) {
            let principal = Principal::from_slice(&[7, (i % 256) as u8, (i / 256) as u8]);
            if i == 0 {
                first_principal = Some(principal);
            }
            insert_target(test_target_record_at(principal, &global)).unwrap();
        }
        let existing = get_target(first_principal.unwrap()).unwrap();
        let patched = existing
            .apply_patch(
                TargetPatch {
                    display_name: Some("renamed".to_string()),
                    project: None,
                    environment: None,
                    criticality: None,
                    observation_mode: None,
                    tags: None,
                    funding_policy: None,
                    enabled: None,
                    auto_topup: None,
                },
                &global,
            )
            .unwrap();
        assert_eq!(insert_target(patched), Ok(()));
        assert_eq!(target_count(), types::MAX_TARGETS as u64);
    }

    /// See `validate_whole_state_rejects_too_many_targets`'s doc comment:
    /// this exercises the `post_upgrade` backstop by bypassing
    /// `insert_alarm`'s own self-enforcing bound via a direct map insert.
    #[test]
    fn validate_whole_state_rejects_too_many_alarms() {
        let sentinel_id = test_sentinel_id();
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        for i in 0..=(types::MAX_ALARMS as u64) {
            ALARMS.with(|m| {
                m.borrow_mut()
                    .insert(i, StoredAlarm::V1(test_alarm(i, None)));
            });
        }
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::TooManyAlarms {
                count: types::MAX_ALARMS as u64 + 1,
            })
        );
    }

    /// See the alarms variant above: bypasses `insert_proposal`'s own
    /// self-enforcing bound via a direct map insert to exercise
    /// `validate_whole_state`'s independent backstop.
    #[test]
    fn validate_whole_state_rejects_too_many_proposals() {
        let sentinel_id = test_sentinel_id();
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        for i in 0..=(types::MAX_PROPOSALS as u64) {
            let record = ProposalRecord::new(
                i,
                ProposalPayload::AddSigner {
                    signer: test_signer(1),
                },
                test_signer(1),
                i,
            );
            PROPOSALS.with(|m| {
                m.borrow_mut().insert(i, StoredProposalRecord::V1(record));
            });
        }
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::TooManyProposals {
                count: types::MAX_PROPOSALS as u64 + 1,
            })
        );
    }

    #[test]
    fn validate_whole_state_rejects_too_many_terminal_summaries() {
        // Insert directly into the map (bypassing insert_terminal_summary's
        // own eviction) to exercise validate_whole_state's own bound check.
        let sentinel_id = test_sentinel_id();
        let global = test_global_policy(1_000_000);
        init_test_state(vec![test_signer(1)], 1, 1_000_000);
        let target = test_target_principal(1);
        for i in 0..=(types::MAX_TERMINAL_SUMMARIES as u64) {
            let op = test_resolved_operation(i, target, &global, 10);
            let summary = TerminalFundingSummary::from_resolved(&op, i).unwrap();
            TERMINAL_SUMMARIES.with(|m| {
                m.borrow_mut()
                    .insert(i, StoredTerminalFundingSummary::V1(summary));
            });
        }
        assert_eq!(
            validate_whole_state(sentinel_id),
            Err(StateValidationError::TooManyTerminalSummaries {
                count: types::MAX_TERMINAL_SUMMARIES as u64 + 1,
            })
        );
    }
}
