//! Permanent storage for completed collateral claim receipts.
//!
//! The legacy Stability Pool snapshot is a length-prefixed Candid blob at raw
//! stable-memory offset zero. Read and decode it before initializing the
//! MemoryManager: `MemoryManager::init` owns that offset and writes its header
//! there. Once migrated, the full SP snapshot is stored in a StableCell and
//! completed receipts live in an append-only StableBTreeMap.

use candid::{CandidType, Decode, Encode, Principal};
use ic_canister_log::log;
use ic_stable_structures::{
    memory_manager::{MemoryId, MemoryManager, VirtualMemory},
    storable::Bound,
    DefaultMemoryImpl, StableBTreeMap, StableCell, Storable,
};
use serde::{Deserialize, Serialize};
use std::{borrow::Cow, cell::RefCell, ops::Bound as RangeBound};

use crate::{
    state::{self, StabilityPoolState},
    types::{CompletedOutboundPayout, CompletedOutboundPayoutStatus},
};

type Memory = VirtualMemory<DefaultMemoryImpl>;
type ReceiptMap = StableBTreeMap<(Principal, u64), StableReceiptRecord, Memory>;
type InterestMap = StableBTreeMap<u64, StableInterestMintRecord, Memory>;
type SnapshotCell = StableCell<Vec<u8>, Memory>;
type VersionCell = StableCell<u8, Memory>;

const MEM_RECEIPTS: MemoryId = MemoryId::new(0);
const MEM_SNAPSHOT: MemoryId = MemoryId::new(1);
const MEM_LAYOUT_VERSION: MemoryId = MemoryId::new(2);
const MEM_INTEREST_MINT_RECEIPTS: MemoryId = MemoryId::new(3);
pub const LAYOUT_VERSION: u8 = 2;
const MAX_LEGACY_SNAPSHOT_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_CLAIM_RECEIPT_PAGE: usize = 100;
/// The heap snapshot keeps only a small cache; the old history query keeps its
/// former 10,000-row bound independently of that cache and the page size.
pub const CLAIM_RECEIPT_CACHE_LIMIT: usize = 100;
pub const COMPAT_CLAIM_RECEIPT_QUERY_LIMIT: usize = 10_000;
const MANAGER_HEADER_SIZE: usize = 2_080;
const MANAGER_BUCKET_TABLE_OFFSET: u64 = MANAGER_HEADER_SIZE as u64;
const MANAGER_BUCKET_COUNT: usize = 32_768;
const MANAGER_MEMORY_COUNT: usize = 255;

pub enum ExistingStableLayout {
    Empty,
    Legacy(StabilityPoolState),
    MemoryManager,
    Invalid(Vec<u8>),
}

thread_local! {
    static MEMORY_MANAGER: RefCell<Option<MemoryManager<DefaultMemoryImpl>>> = const { RefCell::new(None) };
    static RECEIPTS: RefCell<Option<ReceiptMap>> = const { RefCell::new(None) };
    static SNAPSHOT: RefCell<Option<SnapshotCell>> = const { RefCell::new(None) };
    static LAYOUT: RefCell<Option<VersionCell>> = const { RefCell::new(None) };
    static INTEREST_RECEIPTS: RefCell<Option<InterestMap>> = const { RefCell::new(None) };
}

#[derive(CandidType, Deserialize)]
struct InterestReceiptEnvelope { version: u8, payload: Vec<u8> }

#[derive(Clone, Debug, PartialEq, Eq)]
enum StableInterestMintRecord { Known(state::InterestMintReceipt), Opaque(Vec<u8>) }

impl Storable for StableInterestMintRecord {
    const BOUND: Bound = Bound::Unbounded;
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        let bytes = match self {
            Self::Known(receipt) => {
                let payload = Encode!(receipt).expect("encode SP interest receipt");
                Encode!(&InterestReceiptEnvelope { version: 1, payload }).expect("encode SP interest receipt envelope")
            }
            Self::Opaque(bytes) => bytes.clone(),
        };
        Cow::Owned(bytes)
    }
    fn from_bytes(bytes: Cow<'_, [u8]>) -> Self {
        let raw = bytes.into_owned();
        match Decode!(&raw, InterestReceiptEnvelope) {
            Ok(envelope) if envelope.version == 1 => Decode!(&envelope.payload, state::InterestMintReceipt)
                .map(Self::Known).unwrap_or(Self::Opaque(raw)),
            _ => Self::Opaque(raw),
        }
    }
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredReceipt {
    pub ledger: Principal,
    pub receipt: CompletedOutboundPayout,
}

/// Frozen permanent V1 payload. Keep this independent of the mutable pending
/// payout schema; every field here is part of the exact settled tuple or its
/// terminal receipt metadata.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct StoredReceiptV1 {
    ledger: Principal,
    gross_amount: u64,
    transfer_amount: u64,
    transfer_fee: u64,
    recipient: icrc_ledger_types::icrc1::account::Account,
    from_subaccount: Option<Vec<u8>>,
    transfer_memo: Vec<u8>,
    transfer_created_at_time_ns: u64,
    block_index: u64,
    completed_at_ns: u64,
}

#[derive(CandidType, Deserialize)]
struct StoredReceiptEnvelope {
    version: u8,
    payload: Vec<u8>,
}

/// Source-only layout-1 compatibility shape from before the V1 envelope was
/// introduced. The layout was not confirmed deployed; decode it narrowly so
/// an intermediate installation can be upgraded without losing receipts.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Deserialize)]
struct StoredReceiptV0 {
    ledger: Principal,
    receipt: CompletedOutboundPayout,
}

impl Storable for StoredReceiptV0 {
    const BOUND: Bound = Bound::Unbounded;

    fn to_bytes(&self) -> Cow<'_, [u8]> {
        Cow::Owned(Encode!(self).expect("encode legacy raw SP receipt"))
    }

    fn from_bytes(bytes: Cow<'_, [u8]>) -> Self {
        Decode!(bytes.as_ref(), StoredReceiptV0).expect("decode legacy raw SP receipt")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum StableReceiptRecord {
    V1(StoredReceiptV1),
    V0(StoredReceiptV0),
    /// Retain unknown/corrupt bytes verbatim so merely opening a stable map
    /// cannot panic or overwrite a future-version receipt.
    Opaque(Vec<u8>),
}

impl StableReceiptRecord {
    fn from_runtime(stored: &StoredReceipt) -> Self {
        let payout = &stored.receipt.payout;
        Self::V1(StoredReceiptV1 {
            ledger: stored.ledger,
            gross_amount: payout.gross_amount,
            transfer_amount: payout.transfer_amount,
            transfer_fee: payout.transfer_fee,
            recipient: payout.recipient.clone(),
            from_subaccount: payout.from_subaccount.map(|value| value.to_vec()),
            transfer_memo: payout.transfer_memo.clone(),
            transfer_created_at_time_ns: payout.transfer_created_at_time_ns,
            block_index: stored.receipt.block_index,
            completed_at_ns: stored.receipt.completed_at_ns,
        })
    }

    fn into_runtime(self) -> Result<StoredReceipt, String> {
        match self {
            Self::V0(row) => Ok(StoredReceipt {
                ledger: row.ledger,
                receipt: row.receipt,
            }),
            Self::V1(row) => {
                let from_subaccount = row
                    .from_subaccount
                    .map(|bytes| {
                        bytes.try_into().map_err(|_| {
                            "SP stored receipt V1 has an invalid subaccount length".to_string()
                        })
                    })
                    .transpose()?;
                Ok(StoredReceipt {
                    ledger: row.ledger,
                    receipt: CompletedOutboundPayout {
                        payout: crate::types::PendingOutboundPayout {
                            gross_amount: row.gross_amount,
                            transfer_amount: row.transfer_amount,
                            transfer_fee: row.transfer_fee,
                            recipient: row.recipient,
                            from_subaccount,
                            transfer_memo: row.transfer_memo,
                            transfer_created_at_time_ns: row.transfer_created_at_time_ns,
                            dispatch_in_flight: false,
                            ambiguous_seen: false,
                            last_error: None,
                            dispatch_generation: 0,
                            reconciliation_attempts: 0,
                            last_reconciliation_at_ns: None,
                            candidate_block_index: Some(row.block_index),
                            candidate_block_index_raw: Some(row.block_index.to_string()),
                        },
                        block_index: row.block_index,
                        completed_at_ns: row.completed_at_ns,
                    },
                })
            }
            Self::Opaque(_) => Err("SP stored receipt uses an unsupported or malformed envelope".to_string()),
        }
    }
}

impl Storable for StableReceiptRecord {
    const BOUND: Bound = Bound::Unbounded;

    fn to_bytes(&self) -> Cow<'_, [u8]> {
        let bytes = match self {
            Self::V1(row) => {
                let payload = Encode!(row).expect("encode frozen SP claim receipt V1");
                Encode!(&StoredReceiptEnvelope { version: 1, payload })
                    .expect("encode SP claim receipt envelope")
            }
            Self::V0(row) => Encode!(row).expect("encode legacy raw SP receipt"),
            Self::Opaque(bytes) => bytes.clone(),
        };
        Cow::Owned(bytes)
    }

    fn from_bytes(bytes: Cow<'_, [u8]>) -> Self {
        let raw = bytes.into_owned();
        match Decode!(&raw, StoredReceiptEnvelope) {
            Ok(envelope) if envelope.version == 1 => match Decode!(&envelope.payload, StoredReceiptV1) {
                Ok(row) => Self::V1(row),
                Err(_) => Self::Opaque(raw),
            },
            Ok(_) => Self::Opaque(raw),
            Err(_) => match Decode!(&raw, StoredReceiptV0) {
                Ok(row) => Self::V0(row),
                Err(_) => Self::Opaque(raw),
            },
        }
    }
}

/// Initialize the manager and structures only after legacy bytes have been
/// captured. Memory IDs are part of the permanent on-canister layout.
pub fn init_layout() {
    let already_initialized = MEMORY_MANAGER.with(|manager| manager.borrow().is_some());
    if already_initialized {
        return;
    }
    let manager = MemoryManager::init(DefaultMemoryImpl::default());
    let receipts = StableBTreeMap::init(manager.get(MEM_RECEIPTS));
    let snapshot = StableCell::init(manager.get(MEM_SNAPSHOT), Vec::<u8>::new())
        .expect("initialize SP claim snapshot cell");
    let layout = StableCell::init(manager.get(MEM_LAYOUT_VERSION), 0u8)
        .expect("initialize SP claim layout marker");
    let interest_receipts = StableBTreeMap::init(manager.get(MEM_INTEREST_MINT_RECEIPTS));
    MEMORY_MANAGER.with(|cell| *cell.borrow_mut() = Some(manager));
    RECEIPTS.with(|cell| *cell.borrow_mut() = Some(receipts));
    SNAPSHOT.with(|cell| *cell.borrow_mut() = Some(snapshot));
    LAYOUT.with(|cell| *cell.borrow_mut() = Some(layout));
    INTEREST_RECEIPTS.with(|cell| *cell.borrow_mut() = Some(interest_receipts));
}

fn with_interest_receipts<R>(f: impl FnOnce(&InterestMap) -> R) -> R {
    INTEREST_RECEIPTS.with(|cell| f(cell.borrow().as_ref().expect("SP interest receipt store not initialized")))
}
fn with_interest_receipts_mut<R>(f: impl FnOnce(&mut InterestMap) -> R) -> R {
    INTEREST_RECEIPTS.with(|cell| f(cell.borrow_mut().as_mut().expect("SP interest receipt store not initialized")))
}

fn interest_status_for_map<M: ic_stable_structures::Memory>(
    map: &StableBTreeMap<u64, StableInterestMintRecord, M>,
    state: &StabilityPoolState,
    block: u64,
    payload: &state::InterestMintReceiptPayload,
    capacity: usize,
) -> state::InterestMintReceiptStatus {
    use state::{InterestMintReceiptDisposition as D, InterestMintReceiptStatus as S};
    if let Some(row) = map.get(&block) {
        return match row {
            StableInterestMintRecord::Opaque(_) => S::LegacyUnknown,
            StableInterestMintRecord::Known(row) => {
                let Some(old) = row.payload.as_ref() else { return S::LegacyUnknown; };
                if old != payload { return S::PayloadConflict; }
                match row.disposition {
                    D::Distributed | D::Forwarded(_) => S::Duplicate,
                    D::ForwardPending(id) => S::PendingForward(id),
                    D::LegacyUnknown | D::LegacyUnknownForward(_) => S::LegacyUnknown,
                }
            }
        };
    }
    if state.unallocated_interest_mint_index.as_ref().is_some_and(|i| i.contains_key(&block))
        || state.legacy_interest_mint_receipt_floor.as_ref().and_then(|f| f.through).is_some_and(|floor| block <= floor) {
        return S::LegacyUnknown;
    }
    if state.unallocated_interest_mint_index.is_none() { return S::OutsideReplayWindow; }
    if map.len() >= capacity as u64 { return S::CapacityReached; }
    S::New
}

pub fn interest_mint_receipt_status(state: &StabilityPoolState, block: u64, payload: &state::InterestMintReceiptPayload) -> state::InterestMintReceiptStatus {
    with_interest_receipts(|map| interest_status_for_map(map, state, block, payload, state::MAX_INTEREST_MINT_RECEIPTS))
}

/// Insert one exact accepted receipt. The caller performs this inside the same
/// IC message/state mutation as the accounting effect; a trap rolls both back.
pub fn record_interest_mint_receipt(pool: &StabilityPoolState, block: u64, receipt: state::InterestMintReceipt) -> state::InterestMintReceiptStatus {
    with_interest_receipts_mut(|map| record_interest_for_map(map, pool, block, receipt, state::MAX_INTEREST_MINT_RECEIPTS))
}

fn record_interest_for_map<M: ic_stable_structures::Memory>(map: &mut StableBTreeMap<u64, StableInterestMintRecord, M>, pool: &StabilityPoolState, block: u64, receipt: state::InterestMintReceipt, capacity: usize) -> state::InterestMintReceiptStatus {
    use state::InterestMintReceiptStatus as S;
    let Some(payload) = receipt.payload.as_ref() else { return S::LegacyUnknown; };
    match interest_status_for_map(map, pool, block, payload, capacity) {
        S::New => {
            map.insert(block, StableInterestMintRecord::Known(receipt));
            if map.len() == state::INTEREST_MINT_RECEIPT_WARNING_THRESHOLD as u64 {
                log!(crate::logs::INFO, "CL-10 interest receipt capacity warning: {} permanent rows reached the 80% threshold; admission remains fail-closed at hard capacity {} and rows are never pruned", map.len(), state::MAX_INTEREST_MINT_RECEIPTS);
            }
            S::New
        }
        status => status,
    }
}

/// Mark exact forward rows after the Treasury ACK. Unknown legacy rows remain
/// unknown because their payload cannot be reconstructed from coalesced batches.
pub fn mark_interest_forwarded(blocks: &[u64], batch_id: u64) {
    with_interest_receipts_mut(|map| for block in blocks {
        if let Some(StableInterestMintRecord::Known(mut row)) = map.get(block) {
            if row.disposition == state::InterestMintReceiptDisposition::ForwardPending(batch_id) {
                row.disposition = state::InterestMintReceiptDisposition::Forwarded(batch_id);
                map.insert(*block, StableInterestMintRecord::Known(row));
            }
        }
    });
}

pub fn migrate_interest_mint_receipts(state: &mut StabilityPoolState) -> Result<(u64, Option<u64>), String> {
    with_interest_receipts_mut(|map| migrate_interest_map(map, state))
}

fn migrate_interest_map<M: ic_stable_structures::Memory>(map: &mut StableBTreeMap<u64, StableInterestMintRecord, M>, state: &mut StabilityPoolState) -> Result<(u64, Option<u64>), String> {
    if let Some(floor) = state.legacy_interest_mint_receipt_floor.as_ref() {
        return Ok((map.len(), floor.through));
    }
    let (through, rows) = state.legacy_interest_mint_receipt_rows();
    let mut missing = std::collections::BTreeSet::new();
    for (block, _) in &rows {
        if !map.contains_key(block) { missing.insert(*block); }
    }
    if map.len().saturating_add(missing.len() as u64) > state::MAX_INTEREST_MINT_RECEIPTS as u64 {
        return Err("legacy interest receipt history exceeds permanent receipt capacity".to_string());
    }
    for (block, row) in rows {
            if let Some(existing) = map.get(&block) {
                // Upgrade a generic processed tombstone to a forward-owned
                // tombstone when the same source also appears in an old batch.
                if let (StableInterestMintRecord::Known(mut old), state::InterestMintReceiptDisposition::LegacyUnknownForward(batch)) =
                    (existing, &row.disposition)
                {
                    if old.payload.is_none() && old.disposition == state::InterestMintReceiptDisposition::LegacyUnknown {
                        old.disposition = state::InterestMintReceiptDisposition::LegacyUnknownForward(*batch);
                        map.insert(block, StableInterestMintRecord::Known(old));
                    }
                }
                continue;
            }
            map.insert(block, StableInterestMintRecord::Known(row));
    }
    state.set_legacy_interest_mint_receipt_floor(through);
    Ok((map.len(), through))
}

pub fn interest_mint_receipt_capacity(state: &StabilityPoolState) -> state::InterestMintReceiptCapacity {
    with_interest_receipts(|map| interest_capacity_for_map(map, state))
}

fn interest_capacity_for_map<M: ic_stable_structures::Memory>(map: &StableBTreeMap<u64, StableInterestMintRecord, M>, state: &StabilityPoolState) -> state::InterestMintReceiptCapacity {
    let retained = map.len();
    let floor = state.legacy_interest_mint_receipt_floor.as_ref().and_then(|f| f.through);
    let warning = retained >= state::INTEREST_MINT_RECEIPT_WARNING_THRESHOLD as u64;
    let admission_blocked_reason = if retained >= state::MAX_INTEREST_MINT_RECEIPTS as u64 {
        Some("permanent_interest_receipt_capacity_reached".to_string())
    } else if state.unallocated_interest_mint_index.is_none() {
        Some("legacy_forward_receipt_index_unavailable".to_string())
    } else {
        None
    };
    state::InterestMintReceiptCapacity {
        retained: Some(retained), legacy_unknown: None,
        capacity: state::MAX_INTEREST_MINT_RECEIPTS as u64,
        warning_threshold: state::INTEREST_MINT_RECEIPT_WARNING_THRESHOLD as u64,
        warning, accepts_new: admission_blocked_reason.is_none(), admission_blocked_reason,
        legacy_floor: floor,
    }
}

/// A successfully decoded predecessor snapshot may start with bytes that
/// resemble a MemoryManager header. Clear its raw length prefix only after
/// decoding, so `MemoryManager::init` creates a fresh header instead of
/// attempting to load the predecessor's `MGR\0` collision as version zero.
pub fn clear_legacy_header_before_manager_init() {
    clear_legacy_header(&DefaultMemoryImpl::default());
}

fn clear_legacy_header<M: ic_stable_structures::Memory>(memory: &M) {
    memory.write(0, &[0; 8]);
}

fn with_receipts<R>(f: impl FnOnce(&ReceiptMap) -> R) -> R {
    RECEIPTS.with(|cell| {
        f(cell
            .borrow()
            .as_ref()
            .expect("SP receipt store not initialized"))
    })
}

fn with_receipts_mut<R>(f: impl FnOnce(&mut ReceiptMap) -> R) -> R {
    RECEIPTS.with(|cell| {
        f(cell
            .borrow_mut()
            .as_mut()
            .expect("SP receipt store not initialized"))
    })
}

fn with_snapshot<R>(f: impl FnOnce(&SnapshotCell) -> R) -> R {
    SNAPSHOT.with(|cell| {
        f(cell
            .borrow()
            .as_ref()
            .expect("SP snapshot cell not initialized"))
    })
}

fn with_snapshot_mut<R>(f: impl FnOnce(&mut SnapshotCell) -> R) -> R {
    SNAPSHOT.with(|cell| {
        f(cell
            .borrow_mut()
            .as_mut()
            .expect("SP snapshot cell not initialized"))
    })
}

fn with_layout<R>(f: impl FnOnce(&VersionCell) -> R) -> R {
    LAYOUT.with(|cell| {
        f(cell
            .borrow()
            .as_ref()
            .expect("SP layout marker not initialized"))
    })
}

fn with_layout_mut<R>(f: impl FnOnce(&mut VersionCell) -> R) -> R {
    LAYOUT.with(|cell| {
        f(cell
            .borrow_mut()
            .as_mut()
            .expect("SP layout marker not initialized"))
    })
}

pub fn layout_version() -> u8 {
    with_layout(|cell| *cell.get())
}

pub fn set_layout_version(version: u8) {
    with_layout_mut(|cell| cell.set(version).expect("write SP layout marker"));
}

pub fn save_snapshot(state: &StabilityPoolState) {
    let bytes = Encode!(state).expect("encode SP stable snapshot");
    with_snapshot_mut(|cell| cell.set(bytes).expect("write SP stable snapshot"));
}

pub fn load_snapshot() -> Option<StabilityPoolState> {
    let bytes = with_snapshot(|cell| cell.get().clone());
    if bytes.is_empty() {
        return None;
    }
    state::try_decode_state(&bytes)
}

/// Append a receipt without overwriting a different receipt at the same
/// owner/timestamp idempotency key. An identical row is an idempotent replay.
pub fn append(owner: Principal, stored: StoredReceipt) -> Result<(), String> {
    let key = (owner, stored.receipt.payout.transfer_created_at_time_ns);
    with_receipts_mut(|map| append_to_map(map, key, stored))
}

fn append_to_map<M: ic_stable_structures::Memory>(
    map: &mut StableBTreeMap<(Principal, u64), StableReceiptRecord, M>,
    key: (Principal, u64),
    stored: StoredReceipt,
) -> Result<(), String> {
    let encoded = StableReceiptRecord::from_runtime(&stored);
    if let Some(previous) = map.get(&key) {
        return match previous.into_runtime() {
            Ok(previous) if StableReceiptRecord::from_runtime(&previous) == encoded => Ok(()),
            _ => Err("SP claim receipt idempotency-key collision or unknown stored version".to_string()),
        };
    }
    map.insert(key, encoded);
    Ok(())
}

/// Migrate every legacy completed row without replacing any permanent row.
/// The caller invokes this before persisting the new layout marker.
fn migrate_legacy_receipts<M: ic_stable_structures::Memory>(
    map: &mut StableBTreeMap<(Principal, u64), StableReceiptRecord, M>,
    state: &mut StabilityPoolState,
) -> Result<(), String> {
    let mut highest_timestamp = state.last_outbound_payout_created_at_ns.unwrap_or(0);
    if let Some(completed) = state.completed_outbound_payouts.as_ref() {
        for ((owner, ledger, timestamp), receipt) in completed {
            highest_timestamp = highest_timestamp.max(*timestamp);
            append_to_map(
                map,
                (*owner, *timestamp),
                StoredReceipt {
                    ledger: *ledger,
                    receipt: receipt.clone(),
                },
            )?;
        }
    }
    if let Some(pending) = state.pending_outbound_payouts.as_ref() {
        for payout in pending.values() {
            highest_timestamp = highest_timestamp.max(payout.transfer_created_at_time_ns);
        }
    }
    state.last_outbound_payout_created_at_ns = Some(highest_timestamp);
    if let Some(completed) = state.completed_outbound_payouts.as_mut() {
        while completed.len() > CLAIM_RECEIPT_CACHE_LIMIT {
            let Some(key) = completed.keys().next().copied() else {
                break;
            };
            completed.remove(&key);
        }
    }
    Ok(())
}

pub fn migrate_legacy_state(state: &mut StabilityPoolState) -> Result<(), String> {
    with_receipts_mut(|map| migrate_legacy_receipts(map, state))
}

pub fn get(owner: Principal, timestamp: u64) -> Result<Option<StoredReceipt>, String> {
    with_receipts(|map| map.get(&(owner, timestamp)).map(StableReceiptRecord::into_runtime).transpose())
}

/// The serialized compatibility cache may only contain rows that are still
/// present identically in the permanent journal. It is never used to decide
/// replay or idempotency outcomes.
pub fn validate_compatibility_cache(state: &mut StabilityPoolState) -> Result<(), String> {
    with_receipts(|map| validate_compatibility_cache_for_map(map, state))
}

fn validate_compatibility_cache_for_map<M: ic_stable_structures::Memory>(
    map: &StableBTreeMap<(Principal, u64), StableReceiptRecord, M>,
    state: &mut StabilityPoolState,
) -> Result<(), String> {
    let Some(cache) = state.completed_outbound_payouts.as_ref() else {
        return Ok(());
    };
    let mut cache_unverifiable = false;
    for ((owner, ledger, timestamp), receipt) in cache {
        let expected = StoredReceipt {
            ledger: *ledger,
            receipt: receipt.clone(),
        };
        let actual = match map
            .get(&(*owner, *timestamp))
            .map(StableReceiptRecord::into_runtime)
            .transpose()
        {
            Ok(Some(actual)) => actual,
            Ok(None) => {
                return Err(format!(
                    "SP compatibility receipt cache diverges from permanent journal at owner {owner}, timestamp {timestamp}"
                ));
            }
            Err(_) => {
                cache_unverifiable = true;
                break;
            }
        };
        if StableReceiptRecord::from_runtime(&actual) != StableReceiptRecord::from_runtime(&expected) {
            return Err(format!(
                "SP compatibility receipt cache diverges from permanent journal at owner {owner}, timestamp {timestamp}"
            ));
        }
    }
    if cache_unverifiable {
        // The cache is only a legacy compatibility projection. If a future
        // envelope version is not understood, omit it instead of trapping the
        // upgrade or preserving an unverifiable snapshot row.
        state.completed_outbound_payouts = None;
    }
    Ok(())
}

fn to_status(record: StableReceiptRecord) -> CompletedOutboundPayoutStatus {
    let stored = record
        .into_runtime()
        .unwrap_or_else(|error| ic_cdk::trap(&error));
    CompletedOutboundPayoutStatus {
        ledger: stored.ledger,
        gross_amount: stored.receipt.payout.gross_amount,
        transfer_amount: stored.receipt.payout.transfer_amount,
        transfer_fee: stored.receipt.payout.transfer_fee,
        recipient: stored.receipt.payout.recipient,
        from_subaccount: stored
            .receipt
            .payout
            .from_subaccount
            .map(|value| value.to_vec()),
        transfer_memo: stored.receipt.payout.transfer_memo,
        transfer_created_at_time_ns: stored.receipt.payout.transfer_created_at_time_ns,
        block_index: stored.receipt.block_index,
        completed_at_ns: stored.receipt.completed_at_ns,
    }
}

pub struct ReceiptPage {
    pub items: Vec<CompletedOutboundPayoutStatus>,
    pub next_cursor: Option<u64>,
    pub has_more: bool,
}

pub fn page(owner: Principal, after: Option<u64>, requested_limit: usize) -> ReceiptPage {
    with_receipts(|map| page_from_map(map, owner, after, requested_limit))
}

fn page_from_map<M: ic_stable_structures::Memory>(
    map: &StableBTreeMap<(Principal, u64), StableReceiptRecord, M>,
    owner: Principal,
    after: Option<u64>,
    requested_limit: usize,
) -> ReceiptPage {
    let limit = requested_limit.clamp(1, MAX_CLAIM_RECEIPT_PAGE);
    let start = match after {
        Some(timestamp) => RangeBound::Excluded((owner, timestamp)),
        None => RangeBound::Included((owner, 0)),
    };
    let end = RangeBound::Included((owner, u64::MAX));
    let mut rows = map.range((start, end)).take(limit + 1).collect::<Vec<_>>();
    let has_more = rows.len() > limit;
    if has_more {
        rows.pop();
    }
    let next_cursor = if has_more {
        rows.last().map(|((_, timestamp), _)| *timestamp)
    } else {
        None
    };
    ReceiptPage {
        items: rows
            .into_iter()
            .map(|(_, stored)| to_status(stored))
            .collect(),
        next_cursor,
        has_more,
    }
}

pub fn all_for_owner_compat(owner: Principal) -> Vec<CompletedOutboundPayoutStatus> {
    with_receipts(|map| all_for_owner_compat_from_map(map, owner))
}

fn all_for_owner_compat_from_map<M: ic_stable_structures::Memory>(
    map: &StableBTreeMap<(Principal, u64), StableReceiptRecord, M>,
    owner: Principal,
) -> Vec<CompletedOutboundPayoutStatus> {
    map.range((
        RangeBound::Included((owner, 0)),
        RangeBound::Included((owner, u64::MAX)),
    ))
    .take(COMPAT_CLAIM_RECEIPT_QUERY_LIMIT)
    .map(|(_, stored)| to_status(stored))
    .collect()
}

/// Inspect physical stable memory without initializing MemoryManager. A
/// bounded legacy Candid snapshot takes precedence over the `MGR` prefix,
/// because the legacy length word can itself begin with those bytes.
pub fn inspect_existing_layout() -> ExistingStableLayout {
    let pages = ic_cdk::api::stable::stable64_size();
    if pages == 0 {
        return ExistingStableLayout::Empty;
    }

    let mut length_bytes = [0u8; 8];
    ic_cdk::api::stable::stable64_read(0, &mut length_bytes);
    let length = u64::from_le_bytes(length_bytes);
    let available = pages.saturating_mul(65_536).saturating_sub(8);
    let mut candid_magic = [0u8; 4];
    if available >= 4 {
        ic_cdk::api::stable::stable64_read(8, &mut candid_magic);
    }
    let candidate = if length > 0
        && length <= available
        && length <= MAX_LEGACY_SNAPSHOT_BYTES
        && &candid_magic == b"DIDL"
    {
        let mut bytes = vec![0; length as usize];
        ic_cdk::api::stable::stable64_read(8, &mut bytes);
        state::try_decode_state(&bytes).map(|decoded| (decoded, bytes))
    } else {
        None
    };

    let mut preview = [0u8; 64];
    let preview_len = pages.saturating_mul(65_536).min(64) as usize;
    if preview_len > 0 {
        ic_cdk::api::stable::stable64_read(0, &mut preview[..preview_len]);
    }
    classify_existing_layout(
        candidate.map(|(state, _)| state),
        has_valid_manager_header(pages),
        preview[..preview_len].to_vec(),
    )
}

fn classify_existing_layout(
    decoded_legacy: Option<StabilityPoolState>,
    valid_manager_header: bool,
    invalid_preview: Vec<u8>,
) -> ExistingStableLayout {
    if let Some(state) = decoded_legacy {
        ExistingStableLayout::Legacy(state)
    } else if valid_manager_header {
        ExistingStableLayout::MemoryManager
    } else {
        ExistingStableLayout::Invalid(invalid_preview)
    }
}

/// Validate the full known MemoryManager v1 header and bucket accounting
/// before treating an `MGR` prefix as the new layout. The exact legacy Candid
/// decode is always attempted first by `inspect_existing_layout()`.
fn has_valid_manager_header(stable_pages: u64) -> bool {
    if stable_pages < 2 {
        return false;
    }
    let mut header = [0u8; MANAGER_HEADER_SIZE];
    ic_cdk::api::stable::stable64_read(0, &mut header);
    let mut buckets = [0u8; MANAGER_BUCKET_COUNT];
    ic_cdk::api::stable::stable64_read(MANAGER_BUCKET_TABLE_OFFSET, &mut buckets);
    is_valid_manager_metadata(stable_pages, &header, &buckets)
}

fn is_valid_manager_metadata(stable_pages: u64, header: &[u8], buckets: &[u8]) -> bool {
    if stable_pages < 2
        || header.len() != MANAGER_HEADER_SIZE
        || buckets.len() != MANAGER_BUCKET_COUNT
    {
        return false;
    }
    if &header[..3] != b"MGR"
        || header[3] != 1
        || u16::from_le_bytes([header[4], header[5]]) as usize > MANAGER_BUCKET_COUNT
        || u16::from_le_bytes([header[6], header[7]]) == 0
        || header[8..40].iter().any(|byte| *byte != 0)
    {
        return false;
    }

    let allocated = u16::from_le_bytes([header[4], header[5]]) as usize;
    let bucket_size_pages = u16::from_le_bytes([header[6], header[7]]) as u64;
    let mut memory_pages = [0u64; MANAGER_MEMORY_COUNT];
    let mut bucket_counts = [0usize; MANAGER_MEMORY_COUNT];
    for (index, pages) in memory_pages.iter_mut().enumerate() {
        let offset = 40 + index * 8;
        *pages = u64::from_le_bytes(header[offset..offset + 8].try_into().unwrap());
    }
    let mut found_allocated = 0usize;
    for bucket in buckets.iter().copied() {
        if bucket == u8::MAX {
            continue;
        }
        if bucket as usize >= MANAGER_MEMORY_COUNT {
            return false;
        }
        found_allocated += 1;
        bucket_counts[bucket as usize] += 1;
    }
    if found_allocated != allocated {
        return false;
    }
    for (pages, buckets) in memory_pages.iter().zip(bucket_counts) {
        let expected = pages.div_ceil(bucket_size_pages) as usize;
        if expected != buckets {
            return false;
        }
    }
    stable_pages >= 1 + bucket_size_pages.saturating_mul(allocated as u64)
}

pub fn write_legacy_state_for_test(state: &StabilityPoolState) -> Vec<u8> {
    Encode!(state).expect("encode predecessor state snapshot")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ic_stable_structures::{
        memory_manager::MemoryManager, Memory as StableMemory, VectorMemory,
    };
    use icrc_ledger_types::icrc1::account::Account;
    use std::collections::BTreeMap;

    fn receipt(owner: Principal, timestamp: u64) -> CompletedOutboundPayout {
        CompletedOutboundPayout {
            payout: crate::types::PendingOutboundPayout {
                gross_amount: 120,
                transfer_amount: 100,
                transfer_fee: 20,
                recipient: Account {
                    owner,
                    subaccount: None,
                },
                from_subaccount: None,
                transfer_memo: vec![1, 2, 3],
                transfer_created_at_time_ns: timestamp,
                dispatch_in_flight: false,
                ambiguous_seen: false,
                last_error: None,
                dispatch_generation: 1,
                reconciliation_attempts: 0,
                last_reconciliation_at_ns: None,
                candidate_block_index: Some(timestamp),
                candidate_block_index_raw: Some(timestamp.to_string()),
            },
            block_index: timestamp + 100,
            completed_at_ns: timestamp + 200,
        }
    }

    fn map_for(memory: &VectorMemory) -> ReceiptMap {
        let manager = MemoryManager::init(memory.clone());
        StableBTreeMap::init(manager.get(MEM_RECEIPTS))
    }

    fn interest_map_for(memory: &VectorMemory) -> InterestMap {
        let manager = MemoryManager::init(memory.clone());
        StableBTreeMap::init(manager.get(MEM_INTEREST_MINT_RECEIPTS))
    }

    fn interest_payload(amount: u64) -> state::InterestMintReceiptPayload {
        state::InterestMintReceiptPayload { token_ledger: Principal::from_slice(&[9]), amount, collateral_type: None }
    }

    fn exact_distributed(payload: state::InterestMintReceiptPayload) -> state::InterestMintReceipt {
        state::InterestMintReceipt { payload: Some(payload), disposition: state::InterestMintReceiptDisposition::Distributed }
    }

    #[test]
    fn interest_receipts_are_exact_permanent_and_survive_map_reopen() {
        let memory = VectorMemory::default();
        let manager = MemoryManager::init(memory.clone());
        let mut layout = StableCell::init(manager.get(MEM_LAYOUT_VERSION), 0u8).unwrap();
        layout.set(1).unwrap(); // existing V1 MemoryManager layout
        drop(layout);
        let mut map = interest_map_for(&memory);
        let receipt = exact_distributed(interest_payload(123));
        let pool = StabilityPoolState::default();
        assert_eq!(record_interest_for_map(&mut map, &pool, 55, receipt.clone(), 2), state::InterestMintReceiptStatus::New);
        assert_eq!(record_interest_for_map(&mut map, &pool, 55, receipt.clone(), 2), state::InterestMintReceiptStatus::Duplicate);
        let conflict = exact_distributed(interest_payload(124));
        assert_eq!(record_interest_for_map(&mut map, &pool, 55, conflict, 2), state::InterestMintReceiptStatus::PayloadConflict);

        let mut sp = StabilityPoolState::default();
        let payload = interest_payload(123);
        assert_eq!(interest_status_for_map(&map, &sp, 55, &payload, 2), state::InterestMintReceiptStatus::Duplicate);
        let snapshot = Encode!(&sp).unwrap();
        sp = state::try_decode_state(&snapshot).unwrap();
        drop(map);
        let manager = MemoryManager::init(memory.clone());
        let mut layout = StableCell::init(manager.get(MEM_LAYOUT_VERSION), 0u8).unwrap();
        assert_eq!(*layout.get(), 1);
        layout.set(LAYOUT_VERSION).unwrap();
        drop(layout);
        let mut reopened = interest_map_for(&memory);
        let manager = MemoryManager::init(memory.clone());
        let layout = StableCell::init(manager.get(MEM_LAYOUT_VERSION), 0u8).unwrap();
        assert_eq!(*layout.get(), 2, "V1 to V2 marker advances without replacing the MemoryId 3 rows");
        drop(layout);
        assert_eq!(interest_status_for_map(&reopened, &sp, 55, &payload, 2), state::InterestMintReceiptStatus::Duplicate);
        assert_eq!(interest_status_for_map(&reopened, &sp, 56, &payload, 2), state::InterestMintReceiptStatus::New);
        assert_eq!(record_interest_for_map(&mut reopened, &sp, 56, exact_distributed(interest_payload(123)), 2), state::InterestMintReceiptStatus::New);
        assert_eq!(interest_status_for_map(&reopened, &sp, 57, &payload, 2), state::InterestMintReceiptStatus::CapacityReached);
    }

    #[test]
    fn interest_legacy_migration_keeps_evicted_floor_and_forward_rows_unknown() {
        let memory = VectorMemory::default();
        let mut map = interest_map_for(&memory);
        let mut sp = StabilityPoolState::default();
        sp.legacy_interest_mint_receipt_floor = None;
        sp.processed_interest_mint_blocks = Some([9_999, 10_000].into_iter().collect());
        sp.processed_interest_mint_block_high_watermark = None;
        let id = sp.queue_unallocated_interest_forward_at(10_000, Principal::from_slice(&[9]), 1, 1).unwrap();
        // Simulate an old state whose index and coalesced forward batch contain
        // a source ID but whose exact mint payload was never persisted.
        assert_eq!(id, 0);
        let (len, floor) = migrate_interest_map(&mut map, &mut sp).unwrap();
        assert_eq!(floor, Some(10_000));
        assert_eq!(len, 2);
        assert_eq!(sp.legacy_interest_mint_receipt_floor.as_ref().unwrap().through, Some(10_000));
        assert_eq!(interest_status_for_map(&map, &sp, 9_999, &interest_payload(1), 10), state::InterestMintReceiptStatus::LegacyUnknown);
        assert_eq!(interest_status_for_map(&map, &sp, 7_000, &interest_payload(1), 10), state::InterestMintReceiptStatus::LegacyUnknown);
        assert_eq!(interest_status_for_map(&map, &sp, 10_001, &interest_payload(1), 10), state::InterestMintReceiptStatus::New);
        assert!(matches!(map.get(&10_000), Some(StableInterestMintRecord::Known(row)) if matches!(row.disposition, state::InterestMintReceiptDisposition::LegacyUnknownForward(0))));

        let bytes = Encode!(&sp).unwrap();
        let mut restored = state::try_decode_state(&bytes).unwrap();
        assert_eq!(migrate_interest_map(&mut map, &mut restored).unwrap().1, Some(10_000));
        assert_eq!(interest_status_for_map(&map, &restored, 7_000, &interest_payload(1), 10), state::InterestMintReceiptStatus::LegacyUnknown);
    }

    #[test]
    fn opaque_interest_row_is_preserved_and_fails_closed() {
        let memory = VectorMemory::default();
        let mut map = interest_map_for(&memory);
        let unknown_bytes = vec![0xff, 0, 4, 9];
        map.insert(3, StableInterestMintRecord::Opaque(unknown_bytes.clone()));
        assert_eq!(interest_status_for_map(&map, &StabilityPoolState::default(), 3, &interest_payload(1), 10), state::InterestMintReceiptStatus::LegacyUnknown);
        assert_eq!(record_interest_for_map(&mut map, &StabilityPoolState::default(), 3, exact_distributed(interest_payload(1)), 10), state::InterestMintReceiptStatus::LegacyUnknown);
        drop(map);
        let reopened = interest_map_for(&memory);
        assert_eq!(reopened.get(&3), Some(StableInterestMintRecord::Opaque(unknown_bytes)));
    }

    #[test]
    fn interest_capacity_query_reports_legacy_index_admission_hold() {
        let memory = VectorMemory::default();
        let map = interest_map_for(&memory);
        let mut pool = StabilityPoolState::default();
        pool.unallocated_interest_mint_index = None;
        let capacity = interest_capacity_for_map(&map, &pool);
        assert!(!capacity.accepts_new);
        assert_eq!(capacity.admission_blocked_reason.as_deref(), Some("legacy_forward_receipt_index_unavailable"));
    }

    #[test]
    fn predecessor_snapshot_migrates_all_rows_and_pages_beyond_ten_thousand() {
        const ROWS: u64 = 10_025;
        let owner = Principal::from_slice(&[7]);
        let ledger = Principal::from_slice(&[8]);
        let mut legacy = StabilityPoolState::default();
        let completed = legacy.completed_outbound_payouts.as_mut().unwrap();
        for timestamp in 1..=ROWS {
            completed.insert((owner, ledger, timestamp), receipt(owner, timestamp));
        }
        let predecessor_blob = write_legacy_state_for_test(&legacy);
        let mut decoded = state::try_decode_state(&predecessor_blob).expect("decode predecessor");
        let memory = VectorMemory::default();
        let mut map = map_for(&memory);

        migrate_legacy_receipts(&mut map, &mut decoded).expect("migrate predecessor receipts");
        assert_eq!(map.len(), ROWS);
        assert_eq!(decoded.last_outbound_payout_created_at_ns, Some(ROWS));
        assert_eq!(
            decoded.completed_outbound_payouts.as_ref().unwrap().len(),
            CLAIM_RECEIPT_CACHE_LIMIT
        );
        assert!(validate_cache_against_map(&map, &decoded).is_ok());

        // An interrupted/retried migration sees identical rows as idempotent
        // and leaves the permanent history unchanged.
        migrate_legacy_receipts(&mut map, &mut decoded).expect("repeat migration");
        assert_eq!(map.len(), ROWS);

        let mut cursor = None;
        let mut seen = 0;
        loop {
            let result = page_from_map(&map, owner, cursor, 100);
            assert!(result.items.len() <= 100);
            seen += result.items.len() as u64;
            cursor = result.next_cursor;
            if !result.has_more {
                break;
            }
            assert!(cursor.is_some());
        }
        assert_eq!(seen, ROWS);

        // Preserve the prior no-argument endpoint's 10,000-row bound. It
        // reaches beyond the 100-row cache/page limit, but explicitly cannot
        // represent the full history once an owner has more than 10,000 rows.
        let compat = all_for_owner_compat_from_map(&map, owner);
        assert_eq!(compat.len(), COMPAT_CLAIM_RECEIPT_QUERY_LIMIT);
        assert_eq!(compat.first().unwrap().transfer_created_at_time_ns, 1);
        assert_eq!(
            compat.last().unwrap().transfer_created_at_time_ns,
            COMPAT_CLAIM_RECEIPT_QUERY_LIMIT as u64
        );
        assert!(!compat
            .iter()
            .any(|row| row.transfer_created_at_time_ns > COMPAT_CLAIM_RECEIPT_QUERY_LIMIT as u64));
    }

    fn validate_cache_against_map<M: ic_stable_structures::Memory>(
        map: &StableBTreeMap<(Principal, u64), StableReceiptRecord, M>,
        state: &StabilityPoolState,
    ) -> Result<(), String> {
        let Some(cache) = state.completed_outbound_payouts.as_ref() else {
            return Ok(());
        };
        for ((owner, ledger, timestamp), receipt) in cache {
            let expected = StoredReceipt {
                    ledger: *ledger,
                    receipt: receipt.clone(),
                };
            let matches = map
                .get(&(*owner, *timestamp))
                .and_then(|record| record.clone().into_runtime().ok())
                .is_some_and(|actual| {
                    StableReceiptRecord::from_runtime(&actual)
                        == StableReceiptRecord::from_runtime(&expected)
                });
            if !matches {
                return Err("compatibility cache diverged".into());
            }
        }
        Ok(())
    }

    #[test]
    fn reopened_layout_keeps_rows_and_replay_never_overwrites() {
        let owner = Principal::from_slice(&[7]);
        let ledger = Principal::from_slice(&[8]);
        let memory = VectorMemory::default();
        {
            let manager = MemoryManager::init(memory.clone());
            let mut map = StableBTreeMap::init(manager.get(MEM_RECEIPTS));
            let row = StoredReceipt {
                ledger,
                receipt: receipt(owner, 10),
            };
            append_to_map(&mut map, (owner, 10), row.clone()).unwrap();
            append_to_map(&mut map, (owner, 10), row.clone()).unwrap();
            assert!(append_to_map(
                &mut map,
                (owner, 10),
                StoredReceipt {
                    ledger,
                    receipt: receipt(owner, 11)
                }
            )
            .is_err());
            let mut marker = StableCell::init(manager.get(MEM_LAYOUT_VERSION), 0u8).unwrap();
            marker.set(LAYOUT_VERSION).unwrap();
        }
        let manager = MemoryManager::init(memory);
        let map: ReceiptMap = StableBTreeMap::init(manager.get(MEM_RECEIPTS));
        let marker = StableCell::init(manager.get(MEM_LAYOUT_VERSION), 0u8).unwrap();
        assert_eq!(*marker.get(), LAYOUT_VERSION);
        assert_eq!(map.len(), 1);
        assert_eq!(
            map.get(&(owner, 10))
                .unwrap()
                .clone()
                .into_runtime()
                .unwrap()
                .receipt
                .payout
                .transfer_created_at_time_ns,
            10
        );
    }

    #[test]
    fn frozen_receipt_v1_envelope_round_trips_and_unknown_version_stays_opaque() {
        let owner = Principal::from_slice(&[7]);
        let ledger = Principal::from_slice(&[8]);
        let runtime = StoredReceipt {
            ledger,
            receipt: receipt(owner, 55),
        };
        let stable = StableReceiptRecord::from_runtime(&runtime);
        let encoded = stable.to_bytes().into_owned();
        let envelope = Decode!(&encoded, StoredReceiptEnvelope).unwrap();
        assert_eq!(envelope.version, 1);
        let decoded = StableReceiptRecord::from_bytes(Cow::Owned(encoded.clone()));
        assert_eq!(decoded, stable);
        let restored = decoded.into_runtime().unwrap();
        assert_eq!(StableReceiptRecord::from_runtime(&restored), stable);

        let future_bytes = Encode!(&StoredReceiptEnvelope {
            version: 2,
            payload: vec![9, 8, 7],
        })
        .unwrap();
        let future = StableReceiptRecord::from_bytes(Cow::Owned(future_bytes.clone()));
        assert!(matches!(future, StableReceiptRecord::Opaque(_)));
        assert_eq!(future.to_bytes().as_ref(), future_bytes.as_slice());
        assert!(future.into_runtime().is_err());
    }

    #[test]
    fn raw_layout_one_receipt_bytes_decode_lazily_and_unknown_bytes_stay_opaque() {
        let owner = Principal::from_slice(&[7]);
        let ledger = Principal::from_slice(&[8]);
        let memory = VectorMemory::default();
        let head_receipt = StoredReceipt {
            ledger,
            receipt: receipt(owner, 70),
        };
        let legacy = StoredReceiptV0 {
            ledger,
            receipt: head_receipt.receipt.clone(),
        };
        let raw_head_bytes = Encode!(&legacy).unwrap();
        assert_eq!(raw_head_bytes, Encode!(&head_receipt).unwrap());
        assert_eq!(&raw_head_bytes[..4], b"DIDL");
        {
            let manager = MemoryManager::init(memory.clone());
            let mut legacy_map: StableBTreeMap<(Principal, u64), StoredReceiptV0, _> =
                StableBTreeMap::init(manager.get(MEM_RECEIPTS));
            legacy_map.insert((owner, 70), legacy);
            assert_eq!(
                legacy_map.get(&(owner, 70)).unwrap().to_bytes().as_ref(),
                raw_head_bytes.as_slice()
            );
        }

        // Reopen the same memory with the new layout-1 value decoder. This is
        // exactly the raw Candid value shape emitted by HEAD's StoredReceipt
        // Storable implementation before the V1 envelope was added.
        let manager = MemoryManager::init(memory);
        let mut map: ReceiptMap = StableBTreeMap::init(manager.get(MEM_RECEIPTS));
        assert!(matches!(
            map.get(&(owner, 70)),
            Some(StableReceiptRecord::V0(_))
        ));
        assert_eq!(
            map.get(&(owner, 70)).unwrap().to_bytes().as_ref(),
            raw_head_bytes
        );
        assert_eq!(map.len(), 1);

        // Bounded point reads/pages can decode V0 without rewriting or
        // scanning the append-only journal during post_upgrade.
        assert_eq!(
            map.get(&(owner, 70))
                .unwrap()
                .clone()
                .into_runtime()
                .unwrap()
                .receipt
                .payout
                .transfer_created_at_time_ns,
            70
        );
        let page = page_from_map(&map, owner, None, 10);
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].transfer_created_at_time_ns, 70);

        // Idempotent exact-key replay can verify a raw V0 value lazily without
        // replacing its original bytes. Different tuples fail closed.
        append_to_map(&mut map, (owner, 70), head_receipt).unwrap();
        assert_eq!(
            map.get(&(owner, 70)).unwrap().to_bytes().as_ref(),
            raw_head_bytes
        );
        let mut conflicting = StoredReceipt {
            ledger,
            receipt: receipt(owner, 71),
        };
        conflicting.receipt.payout.transfer_created_at_time_ns = 70;
        assert!(append_to_map(&mut map, (owner, 70), conflicting).is_err());

        map.insert((owner, 71), StableReceiptRecord::Opaque(vec![0xff, 0x00]));
        assert_eq!(map.len(), 2);
        assert_eq!(
            map.get(&(owner, 71)).unwrap().to_bytes().as_ref(),
            &[0xff, 0x00]
        );
    }

    #[test]
    fn unknown_receipt_version_drops_unverifiable_compatibility_cache() {
        let owner = Principal::from_slice(&[7]);
        let ledger = Principal::from_slice(&[8]);
        let memory = VectorMemory::default();
        let manager = MemoryManager::init(memory);
        let mut map: ReceiptMap = StableBTreeMap::init(manager.get(MEM_RECEIPTS));
        let future_bytes = Encode!(&StoredReceiptEnvelope {
            version: 2,
            payload: vec![1, 2, 3],
        })
        .unwrap();
        map.insert(
            (owner, 10),
            StableReceiptRecord::Opaque(future_bytes),
        );

        let mut state = StabilityPoolState::default();
        state.completed_outbound_payouts.as_mut().unwrap().insert(
            (owner, ledger, 10),
            receipt(owner, 10),
        );
        assert!(validate_compatibility_cache_for_map(&map, &mut state).is_ok());
        assert!(state.completed_outbound_payouts.is_none());
    }

    #[test]
    fn malformed_predecessor_and_duplicate_legacy_keys_fail_closed() {
        assert!(state::try_decode_state(&[0xff, 0x00]).is_none());

        let owner = Principal::from_slice(&[7]);
        let ledger_a = Principal::from_slice(&[8]);
        let ledger_b = Principal::from_slice(&[9]);
        let mut legacy = StabilityPoolState::default();
        let mut rows = BTreeMap::new();
        rows.insert((owner, ledger_a, 42), receipt(owner, 42));
        rows.insert((owner, ledger_b, 42), receipt(owner, 42));
        legacy.completed_outbound_payouts = Some(rows);
        let mut map = map_for(&VectorMemory::default());
        assert!(migrate_legacy_receipts(&mut map, &mut legacy).is_err());
        assert_eq!(map.len(), 1);
        assert_eq!(
            map.get(&(owner, 42))
                .unwrap()
                .clone()
                .into_runtime()
                .unwrap()
                .ledger,
            ledger_a
        );
    }

    #[test]
    fn legacy_length_word_that_starts_with_manager_magic_prefers_candid_decode() {
        let predecessor_blob = colliding_legacy_snapshot();
        let collision_length = predecessor_blob.len() as u64;
        let length_prefix = collision_length.to_le_bytes();
        assert_eq!(&length_prefix[..4], b"MGR\0");
        assert_eq!(&predecessor_blob[..4], b"DIDL");
        let decoded = state::try_decode_state(&predecessor_blob);
        assert!(
            decoded.is_some(),
            "exact-size predecessor Candid must decode"
        );
        assert!(matches!(
            classify_existing_layout(decoded, true, Vec::new()),
            ExistingStableLayout::Legacy(_)
        ));
        assert!(matches!(
            classify_existing_layout(None, false, length_prefix.to_vec()),
            ExistingStableLayout::Invalid(_)
        ));
    }

    fn colliding_legacy_snapshot() -> Vec<u8> {
        let collision_length = 5_392_205usize;
        let owner = Principal::from_slice(&[7]);
        let ledger = Principal::from_slice(&[8]);
        let mut legacy_state = StabilityPoolState::default();
        let mut pending = receipt(owner, 10).payout;
        pending.dispatch_in_flight = true;
        pending.last_error = Some(String::new());
        legacy_state
            .pending_outbound_payouts
            .as_mut()
            .unwrap()
            .insert((owner, ledger), pending);

        // Candid snapshots reject trailing padding, so make the old state
        // itself exactly the colliding length using its serialized error text.
        let mut low = 0usize;
        let mut high = collision_length;
        while low <= high {
            let middle = low + (high - low) / 2;
            legacy_state
                .pending_outbound_payouts
                .as_mut()
                .unwrap()
                .get_mut(&(owner, ledger))
                .unwrap()
                .last_error = Some("x".repeat(middle));
            let size = Encode!(&legacy_state).unwrap().len();
            if size == collision_length {
                break;
            } else if size < collision_length {
                low = middle + 1;
            } else {
                high = middle - 1;
            }
        }
        let predecessor_blob = Encode!(&legacy_state).unwrap();
        assert_eq!(predecessor_blob.len(), collision_length);
        predecessor_blob
    }

    #[test]
    fn decoded_legacy_collision_header_is_cleared_before_physical_manager_init() {
        let predecessor_blob = colliding_legacy_snapshot();
        let prefix = (predecessor_blob.len() as u64).to_le_bytes();
        assert_eq!(&prefix[..4], b"MGR\0");
        assert!(state::try_decode_state(&predecessor_blob).is_some());

        let memory = VectorMemory::default();
        let byte_len = 8 + predecessor_blob.len() as u64;
        let pages = byte_len.div_ceil(65_536);
        assert!(memory.grow(pages) >= 0);
        memory.write(0, &prefix);
        memory.write(8, &predecessor_blob);

        let mut observed_prefix = [0u8; 8];
        memory.read(0, &mut observed_prefix);
        assert_eq!(observed_prefix, prefix);
        clear_legacy_header(&memory);
        memory.read(0, &mut observed_prefix);
        assert_eq!(observed_prefix, [0; 8]);

        // Exercise the real stable-structures manager and map initialization
        // against the physical legacy bytes after clearing only their prefix.
        let manager = MemoryManager::init(memory.clone());
        let mut map: ReceiptMap = StableBTreeMap::init(manager.get(MEM_RECEIPTS));
        assert_eq!(map.len(), 0);
        let owner = Principal::from_slice(&[7]);
        let ledger = Principal::from_slice(&[8]);
        let value = StoredReceipt {
            ledger,
            receipt: receipt(owner, 10),
        };
        append_to_map(&mut map, (owner, 10), value).expect("initialize map over legacy memory");
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn manager_header_parser_accepts_reopened_layout_and_rejects_corruption() {
        let memory = VectorMemory::default();
        let manager = MemoryManager::init(memory.clone());
        let mut receipts = StableBTreeMap::init(manager.get(MEM_RECEIPTS));
        let owner = Principal::from_slice(&[7]);
        let ledger = Principal::from_slice(&[8]);
        append_to_map(
            &mut receipts,
            (owner, 10),
            StoredReceipt {
                ledger,
                receipt: receipt(owner, 10),
            },
        )
        .unwrap();
        let mut snapshot = StableCell::init(manager.get(MEM_SNAPSHOT), Vec::<u8>::new()).unwrap();
        snapshot
            .set(Encode!(&StabilityPoolState::default()).unwrap())
            .unwrap();
        let mut marker = StableCell::init(manager.get(MEM_LAYOUT_VERSION), 0u8).unwrap();
        marker.set(LAYOUT_VERSION).unwrap();

        let mut header = vec![0; MANAGER_HEADER_SIZE];
        let mut buckets = vec![0; MANAGER_BUCKET_COUNT];
        memory.read(0, &mut header);
        memory.read(MANAGER_BUCKET_TABLE_OFFSET, &mut buckets);
        assert!(is_valid_manager_metadata(memory.size(), &header, &buckets));

        header[3] = 0;
        assert!(!is_valid_manager_metadata(memory.size(), &header, &buckets));
    }
}
