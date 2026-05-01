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

/// Attempts to restore State from stable memory. Returns None if no state was saved.
pub fn load_state_from_stable() -> Option<crate::state::State> {
    MEMORY_MANAGER.with(|m| {
        let mem = m.borrow().get(STATE_MEMORY_ID);
        if mem.size() == 0 {
            return None; // No state memory allocated yet
        }
        let mut len_bytes = [0u8; 8];
        mem.read(0, &mut len_bytes);
        let len = u64::from_le_bytes(len_bytes);
        if len == 0 {
            return None; // No state saved yet
        }
        // Sanity check: length must fit within allocated memory (minus 8-byte prefix)
        let mem_bytes = mem.size() * WASM_PAGE_SIZE;
        if len > mem_bytes.saturating_sub(8) {
            ic_cdk::println!(
                "[upgrade]: corrupt state length {} exceeds memory size {}",
                len, mem_bytes
            );
            return None; // Fall back to event replay
        }
        let mut buf = vec![0u8; len as usize];
        mem.read(8, &mut buf);
        match ciborium::de::from_reader::<crate::state::State, _>(buf.as_slice()) {
            Ok(state) => Some(state),
            Err(e) => {
                ic_cdk::println!("[upgrade]: failed to deserialize state from stable memory: {:?}", e);
                None // Fall back to event replay
            }
        }
    })
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
