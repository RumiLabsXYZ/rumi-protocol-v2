use crate::event::Event;
use ic_stable_structures::{
    log::{Log as StableLog, NoSuchEntry},
    memory_manager::{MemoryId, MemoryManager, VirtualMemory},
    DefaultMemoryImpl, Memory,
};
use std::cell::RefCell;

const LOG_INDEX_MEMORY_ID: MemoryId = MemoryId::new(0);
const LOG_DATA_MEMORY_ID: MemoryId = MemoryId::new(1);
const SNAPSHOT_INDEX_MEMORY_ID: MemoryId = MemoryId::new(2);
const SNAPSHOT_DATA_MEMORY_ID: MemoryId = MemoryId::new(3);
const STATE_MEMORY_ID: MemoryId = MemoryId::new(4);
// Index-aligned timestamp side log. Most pre-existing event variants
// (Upgrade, every set_*, admin_mint, admin_*) lack an explicit
// `timestamp` field, so the explorer renders them with no time
// indicator. We can't change those variant shapes without breaking the
// stored CBOR, but we CAN write a timestamp at the same index in a
// parallel log on every `record_event`. The frontend reads
// `get_event_timestamps` and falls back to that when an event has no
// inline timestamp. Pre-existing rows surface as 0 (the timestamp log
// is empty before this change ships) — those stay timestampless,
// which matches today's behaviour.
const EVENT_TS_INDEX_MEMORY_ID: MemoryId = MemoryId::new(5);
const EVENT_TS_DATA_MEMORY_ID: MemoryId = MemoryId::new(6);

type VMem = VirtualMemory<DefaultMemoryImpl>;
type EventLog = StableLog<Vec<u8>, VMem, VMem>;
type SnapshotLog = StableLog<Vec<u8>, VMem, VMem>;
type TimestampLog = StableLog<u64, VMem, VMem>;

thread_local! {
    static MEMORY_MANAGER: RefCell<MemoryManager<DefaultMemoryImpl>> = RefCell::new(
        MemoryManager::init(DefaultMemoryImpl::default())
    );

    /// The log of the state modifications.
    static EVENTS: RefCell<EventLog> = MEMORY_MANAGER
        .with(|m|
              RefCell::new(
                  StableLog::init(
                      m.borrow().get(LOG_INDEX_MEMORY_ID),
                      m.borrow().get(LOG_DATA_MEMORY_ID)
                  ).expect("failed to initialize stable log")
              )
        );

    /// Hourly protocol snapshots for historical charts.
    static SNAPSHOTS: RefCell<SnapshotLog> = MEMORY_MANAGER
        .with(|m|
              RefCell::new(
                  StableLog::init(
                      m.borrow().get(SNAPSHOT_INDEX_MEMORY_ID),
                      m.borrow().get(SNAPSHOT_DATA_MEMORY_ID)
                  ).expect("failed to initialize snapshot log")
              )
        );

    /// Index-aligned `ic_cdk::api::time()` side log: position N here is the
    /// recording-time timestamp for `EVENTS[N]`. Empty for events recorded
    /// before this log shipped — those return 0 and the explorer keeps
    /// rendering "—" for them.
    static EVENT_TIMESTAMPS: RefCell<TimestampLog> = MEMORY_MANAGER
        .with(|m|
              RefCell::new(
                  StableLog::init(
                      m.borrow().get(EVENT_TS_INDEX_MEMORY_ID),
                      m.borrow().get(EVENT_TS_DATA_MEMORY_ID)
                  ).expect("failed to initialize event timestamp log")
              )
        );
}

pub struct EventIterator {
    buf: Vec<u8>,
    pos: u64,
}

impl Iterator for EventIterator {
    type Item = Event;

    fn next(&mut self) -> Option<Event> {
        EVENTS.with(|events| {
            let events = events.borrow();

            match events.read_entry(self.pos, &mut self.buf) {
                Ok(()) => {
                    self.pos = self.pos.saturating_add(1);
                    Some(decode_event(&self.buf))
                }
                Err(NoSuchEntry) => None,
            }
        })
    }

    fn nth(&mut self, n: usize) -> Option<Event> {
        self.pos = self.pos.saturating_add(n as u64);
        self.next()
    }
}

/// Encodes an event into a byte array.
fn encode_event(event: &Event) -> Vec<u8> {
    let mut buf = Vec::new();
    ciborium::ser::into_writer(event, &mut buf).expect("failed to encode a minter event");
    buf
}

/// # Panics
///
/// This function panics if the event decoding fails.
fn decode_event(buf: &[u8]) -> Event {
    ciborium::de::from_reader(buf).expect("failed to decode a minter event")
}

/// Returns an iterator over all minter events.
pub fn events() -> impl Iterator<Item = Event> {
    EventIterator {
        buf: vec![],
        pos: 0,
    }
}

/// Returns the current number of events in the log.
pub fn count_events() -> u64 {
    EVENTS.with(|events| events.borrow().len())
}

/// Records a new minter event. Also stamps the recording-time timestamp into
/// the parallel `EVENT_TIMESTAMPS` log so explorer surfaces can render a real
/// time even for variants whose payload has no `timestamp` field (Upgrade,
/// every set_*, admin_*). The two logs always grow in lock-step from this
/// point forward — index N in EVENTS aligns with index N in EVENT_TIMESTAMPS.
pub fn record_event(event: &Event) {
    let bytes = encode_event(event);
    let now = ic_cdk::api::time();
    EVENTS.with(|events| {
        events
            .borrow()
            .append(&bytes)
            .expect("failed to append an entry to the event log")
    });
    EVENT_TIMESTAMPS.with(|ts| {
        ts.borrow()
            .append(&now)
            .expect("failed to append to the event timestamp log");
    });
}

/// Returns the recording-time timestamp for the event at the given **event-log
/// index**, or `None` for events that pre-date the side log (returned as 0 by
/// `get_event_timestamps` to keep the response shape index-aligned).
///
/// Index alignment: EVENT_TIMESTAMPS starts empty in the deploy that ships
/// this side log, but the EVENTS log already has thousands of entries. The
/// first push to EVENT_TIMESTAMPS therefore sits at side-log index 0 even
/// though the corresponding event is at EVENTS index `count_events_at_first_push`.
/// We reconstruct the offset on every read as `events_len - timestamps_len`,
/// which is invariant once we always push to both logs together.
pub fn get_event_timestamp(index: u64) -> Option<u64> {
    let events_len = count_events();
    EVENT_TIMESTAMPS.with(|ts| {
        let log = ts.borrow();
        let ts_len = log.len();
        let offset = events_len.saturating_sub(ts_len);
        if index < offset {
            return None;
        }
        log.get(index - offset)
    })
}

/// Returns up to `length` consecutive timestamps starting at the given
/// **event-log index**. Slots before the side log's coverage window or past
/// the end of EVENTS come back as 0 so the caller can use the returned vec as
/// a direct index-aligned overlay over an event slice.
pub fn get_event_timestamps(start: u64, length: u64) -> Vec<u64> {
    let events_len = count_events();
    EVENT_TIMESTAMPS.with(|ts| {
        let log = ts.borrow();
        let ts_len = log.len();
        let offset = events_len.saturating_sub(ts_len);
        let mut out = Vec::with_capacity(length as usize);
        for i in 0..length {
            let idx = start.saturating_add(i);
            if idx < offset || idx >= events_len {
                out.push(0);
                continue;
            }
            out.push(log.get(idx - offset).unwrap_or(0));
        }
        out
    })
}

// ── State Serialization (pre/post upgrade) ────────────────────────────────

const WASM_PAGE_SIZE: u64 = 65_536; // 64 KiB

/// Serializes the full State to stable memory (called in pre_upgrade).
/// Format: 8-byte little-endian length prefix, then CBOR-encoded state.
pub fn save_state_to_stable(state: &crate::state::State) {
    let bytes = {
        let mut buf = Vec::new();
        ciborium::ser::into_writer(state, &mut buf)
            .expect("failed to serialize State to CBOR");
        buf
    };

    MEMORY_MANAGER.with(|m| {
        let mem = m.borrow().get(STATE_MEMORY_ID);
        let total_bytes = 8 + bytes.len() as u64;
        let pages_needed = (total_bytes + WASM_PAGE_SIZE - 1) / WASM_PAGE_SIZE;
        let current_pages = mem.size();
        if pages_needed > current_pages {
            let grow_result = mem.grow(pages_needed - current_pages);
            assert!(grow_result != -1, "failed to grow state memory");
        }
        let len_bytes = (bytes.len() as u64).to_le_bytes();
        mem.write(0, &len_bytes);
        mem.write(8, &bytes);
    });
}

/// Attempts to restore State from stable memory.
///
/// Returns `None` ONLY when no snapshot has ever been written (the genuine
/// first-upgrade / no-state case), where `post_upgrade` legitimately rebuilds
/// from the event log.
///
/// If a snapshot IS present but cannot be restored (corrupt length prefix or a
/// ciborium decode failure, e.g. a `State` schema change the versioned in-place
/// decode could not absorb) this **traps** instead of returning `None`. Silently
/// falling back to event replay would be catastrophic: every `chains::` event
/// (`DepositObserved`, `ChainMintConfirmed`, `ChainBurnObserved`, ...) is a
/// replay NO-OP (see `event.rs`), so a replay-rebuilt `State` comes back with an
/// EMPTY `multi_chain` while ICP-native state is reconstructed. That wipes every
/// foreign-chain vault, supply, and settlement queue while real icUSD stays
/// minted on Monad/Solana, breaking `sum(chain_supplies) == total_debt` with no
/// recovery (the 2026-05-18 AMM state-wipe class, with a worse blast radius).
/// Trapping keeps the canister on the OLD wasm with stable memory intact, so the
/// operator can fix the schema and retry — a bricked-pending-fix upgrade is
/// strictly safer than a silent balance wipe.
pub fn load_state_from_stable() -> Option<crate::state::State> {
    MEMORY_MANAGER.with(|m| {
        let mem = m.borrow().get(STATE_MEMORY_ID);
        if mem.size() == 0 {
            return None; // No state memory allocated yet (genuine first upgrade).
        }
        let mut len_bytes = [0u8; 8];
        mem.read(0, &mut len_bytes);
        let len = u64::from_le_bytes(len_bytes);
        if len == 0 {
            return None; // No state saved yet (genuine first upgrade).
        }
        // A snapshot IS present from here on. Any failure below is corruption of
        // a real snapshot, NOT a missing one — trap, never silently fall back to
        // event replay (which would wipe `multi_chain`; see the fn doc comment).
        let mem_bytes = mem.size() * WASM_PAGE_SIZE;
        if len > mem_bytes.saturating_sub(8) {
            ic_cdk::trap(&corrupt_snapshot_trap_msg(&format!(
                "length prefix {} exceeds allocated state memory {} bytes",
                len, mem_bytes
            )));
        }
        let mut buf = vec![0u8; len as usize];
        mem.read(8, &mut buf);
        match decode_state_body(&buf) {
            Ok(state) => Some(state),
            Err(e) => ic_cdk::trap(&corrupt_snapshot_trap_msg(&e)),
        }
    })
}

/// Pure ciborium decode of a `State` snapshot body (the bytes AFTER the 8-byte
/// length prefix). Extracted from `load_state_from_stable` so the healthy
/// round-trip and the corrupt-input rejection are unit-testable without
/// thread-local stable memory.
pub fn decode_state_body(buf: &[u8]) -> Result<crate::state::State, String> {
    ciborium::de::from_reader::<crate::state::State, _>(buf)
        .map_err(|e| format!("ciborium decode failed: {e:?}"))
}

/// The trap message used when a present `State` snapshot cannot be restored.
/// See `load_state_from_stable` for why this is a trap, not a replay fallback.
fn corrupt_snapshot_trap_msg(detail: &str) -> String {
    format!(
        "[upgrade] ABORT: a State snapshot is present but could not be restored ({detail}). \
         Refusing to fall back to event replay: chain events are replay no-ops, so replay \
         would WIPE all foreign-chain vaults, supplies, and settlement queues (2026-05-18 \
         AMM state-wipe class). The canister stays on the OLD wasm with stable memory intact; \
         fix the State schema and retry the upgrade."
    )
}

// ── Protocol Snapshots ─────────────────────────────────────────────────────

fn encode_snapshot(snapshot: &crate::ProtocolSnapshot) -> Vec<u8> {
    let mut buf = Vec::new();
    ciborium::ser::into_writer(snapshot, &mut buf).expect("failed to encode snapshot");
    buf
}

fn decode_snapshot(buf: &[u8]) -> crate::ProtocolSnapshot {
    ciborium::de::from_reader(buf).expect("failed to decode snapshot")
}

pub fn record_snapshot(snapshot: &crate::ProtocolSnapshot) {
    let bytes = encode_snapshot(snapshot);
    SNAPSHOTS.with(|log| {
        log.borrow().append(&bytes).expect("failed to append snapshot");
    });
}

pub fn snapshots() -> SnapshotIterator {
    SnapshotIterator {
        buf: vec![],
        pos: 0,
    }
}

pub fn count_snapshots() -> u64 {
    SNAPSHOTS.with(|log| log.borrow().len())
}

pub struct SnapshotIterator {
    buf: Vec<u8>,
    pos: u64,
}

impl Iterator for SnapshotIterator {
    type Item = crate::ProtocolSnapshot;

    fn next(&mut self) -> Option<crate::ProtocolSnapshot> {
        SNAPSHOTS.with(|snapshots| {
            let snapshots = snapshots.borrow();
            match snapshots.read_entry(self.pos, &mut self.buf) {
                Ok(()) => {
                    self.pos = self.pos.saturating_add(1);
                    Some(decode_snapshot(&self.buf))
                }
                Err(NoSuchEntry) => None,
            }
        })
    }

    fn nth(&mut self, n: usize) -> Option<crate::ProtocolSnapshot> {
        self.pos = self.pos.saturating_add(n as u64);
        self.next()
    }
}

#[cfg(test)]
mod state_snapshot_tests {
    //! Guards for the upgrade snapshot decode path. `load_state_from_stable`
    //! traps on a present-but-undecodable snapshot rather than silently
    //! replaying events (which would wipe `multi_chain`). These tests exercise
    //! the pure `decode_state_body` half so the healthy round-trip and the
    //! corrupt-input rejection are covered without a wasm/PocketIC harness.
    use super::*;
    use crate::chains::config::ChainId;
    use crate::chains::vault::{ChainVaultStatus, ChainVaultV1};
    use candid::Principal;

    fn encode_state(state: &crate::state::State) -> Vec<u8> {
        // Mirror `save_state_to_stable`'s encoder exactly.
        let mut buf = Vec::new();
        ciborium::ser::into_writer(state, &mut buf).expect("encode State to CBOR");
        buf
    }

    #[test]
    fn default_state_round_trips() {
        let state = crate::state::State::default();
        let bytes = encode_state(&state);
        let decoded = decode_state_body(&bytes).expect("default State must decode");
        assert_eq!(decoded.multi_chain.chain_supplies.len(), 0);
    }

    #[test]
    fn populated_multi_chain_survives_round_trip() {
        // The guard the audit asked for: a populated `multi_chain` sub-tree (a
        // foreign-chain vault + its supply) must survive the exact ciborium
        // encode/decode the upgrade path uses, so a future `State` field that
        // breaks CBOR decode fails HERE in CI rather than wiping vaults on a
        // live upgrade.
        let mut state = crate::state::State::default();
        let chain = ChainId(10143);
        state.multi_chain.chain_supplies.insert(chain, 5_000_000_000);
        state.multi_chain.chain_vaults.insert(
            1,
            ChainVaultV1 {
                vault_id: 1,
                owner: Principal::anonymous(),
                collateral_chain: chain,
                custody_address: "0xabc0000000000000000000000000000000000001".into(),
                collateral_amount_native: 1_000_000_000_000_000_000,
                debt_e8s: 5_000_000_000,
                mint_recipient: "0xabc0000000000000000000000000000000000002".into(),
                pending_mint_e8s: 0,
                status: ChainVaultStatus::Open,
                opened_at_ns: 42,
                owner_evm: None,
                last_interest_accrual_ns: 0,
                pending_interest_mint_e8s: 0,
            },
        );

        let bytes = encode_state(&state);
        let decoded = decode_state_body(&bytes).expect("populated State must decode");

        assert_eq!(
            decoded.multi_chain.chain_supplies.get(&chain),
            Some(&5_000_000_000)
        );
        let v = decoded
            .multi_chain
            .chain_vaults
            .get(&1)
            .expect("vault survives round-trip");
        assert_eq!(v.debt_e8s, 5_000_000_000);
        assert_eq!(v.status, ChainVaultStatus::Open);
        // The whole-State invariant the trap protects: chain supply == vault debt.
        assert_eq!(
            decoded.multi_chain.total_supply_all_chains_e8s(),
            decoded.multi_chain.total_chain_vault_debt_e8s()
        );
    }

    #[test]
    fn corrupt_bytes_are_rejected_not_silently_wiped() {
        // The real path turns this Err into a trap, never a silent replay/wipe.
        assert!(
            decode_state_body(b"\xff\x00not-a-valid-state-snapshot\xde\xad").is_err(),
            "corrupt bytes must fail to decode"
        );
    }

    #[test]
    fn truncated_snapshot_is_rejected() {
        let bytes = encode_state(&crate::state::State::default());
        let truncated = &bytes[..bytes.len() / 2];
        assert!(
            decode_state_body(truncated).is_err(),
            "a truncated snapshot must fail to decode (not silently wipe)"
        );
    }
}
