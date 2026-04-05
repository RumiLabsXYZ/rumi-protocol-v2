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

type VMem = VirtualMemory<DefaultMemoryImpl>;
type EventLog = StableLog<Vec<u8>, VMem, VMem>;
type SnapshotLog = StableLog<Vec<u8>, VMem, VMem>;

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

/// Records a new minter event.
pub fn record_event(event: &Event) {
    let bytes = encode_event(event);
    EVENTS.with(|events| {
        events
            .borrow()
            .append(&bytes)
            .expect("failed to append an entry to the event log")
    });
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
