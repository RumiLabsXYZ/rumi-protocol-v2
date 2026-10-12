use candid::{CandidType, Deserialize};
use ic_stable_structures::{StableBTreeMap, StableCell, Storable};
use serde::Serialize;
use std::borrow::Cow;
use std::cell::RefCell;

use crate::memory;

#[derive(CandidType, Clone, Debug, Deserialize, Serialize, PartialEq)]
pub enum LiquidationStatus {
    Completed,
    SwapFailed,
    TransferFailed,
    ConfirmFailed,
    ClaimFailed,
    AdminResolved,
}

#[derive(CandidType, Clone, Debug, Deserialize, Serialize)]
pub struct LiquidationRecordV1 {
    pub id: u64,
    pub vault_id: u64,
    pub timestamp: u64,
    pub status: LiquidationStatus,

    pub collateral_claimed_e8s: u64,
    pub debt_to_cover_e8s: u64,
    pub icp_swapped_e8s: u64,
    pub ckusdc_received_e6: u64,
    pub ckusdc_transferred_e6: u64,
    pub icp_to_treasury_e8s: u64,

    pub oracle_price_e8s: u64,
    pub effective_price_e8s: u64,
    pub slippage_bps: i32,

    pub error_message: Option<String>,
    pub confirm_retry_count: u8,
}

#[derive(CandidType, Clone, Debug, Deserialize, Serialize)]
pub enum LiquidationRecordVersioned {
    V1(LiquidationRecordV1),
}

impl Storable for LiquidationRecordVersioned {
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        Cow::Owned(candid::encode_one(self).expect("Failed to encode record"))
    }
    fn from_bytes(bytes: Cow<[u8]>) -> Self {
        candid::decode_one(&bytes).expect("Failed to decode record")
    }
    const BOUND: ic_stable_structures::storable::Bound =
        ic_stable_structures::storable::Bound::Unbounded;
}

thread_local! {
    static HISTORY: RefCell<Option<StableBTreeMap<u64, LiquidationRecordVersioned, memory::Mem>>> =
        RefCell::new(None);

    static NEXT_ID: RefCell<Option<StableCell<u64, memory::Mem>>> =
        RefCell::new(None);
}

/// Initialize history storage. Must be called after memory::init_memory_manager().
pub fn init_history() {
    HISTORY.with(|h| {
        *h.borrow_mut() = Some(StableBTreeMap::init(
            memory::get_memory(memory::MEM_ID_HISTORY),
        ));
    });
    NEXT_ID.with(|c| {
        *c.borrow_mut() = Some(
            StableCell::init(memory::get_memory(memory::MEM_ID_NEXT_ID), 0u64)
                .expect("Failed to init NEXT_ID cell"),
        );
    });
}

pub fn next_id() -> u64 {
    NEXT_ID.with(|c| {
        let mut borrow = c.borrow_mut();
        let cell = borrow.as_mut().expect("History not initialized");
        let id = *cell.get();
        cell.set(id + 1).expect("Failed to increment NEXT_ID");
        id
    })
}

/// Keep newly allocated history IDs beyond both the stable records and any
/// legacy events retained in the config snapshot. This also repairs bots that
/// already migrated only the historical 500-record prefix.
pub fn ensure_next_id_at_least(minimum: u64) {
    NEXT_ID.with(|c| {
        let mut borrow = c.borrow_mut();
        let cell = borrow.as_mut().expect("History not initialized");
        if *cell.get() < minimum {
            cell.set(minimum).expect("Failed to advance NEXT_ID");
        }
    });
}

/// Preserve legacy events that were not copied by the bounded migration.
/// Older versions could already have allocated stable IDs starting at 500,
/// so place the uncopied tail after the existing cursor and persist that
/// mapping in the config snapshot before allocating any new record IDs.
pub fn repair_legacy_history_index() {
    const MIGRATED_PREFIX: u64 = 500;
    let (legacy_len, existing_start) = crate::state::read_state(|s| {
        (
            u64::try_from(s.liquidation_events.len()).unwrap_or(u64::MAX),
            s.legacy_history_start_id,
        )
    });

    if legacy_len <= MIGRATED_PREFIX {
        ensure_next_id_at_least(legacy_len);
        return;
    }

    let start = existing_start.unwrap_or_else(current_next_id);
    let start = start.max(MIGRATED_PREFIX);
    let end = start.saturating_add(legacy_len - MIGRATED_PREFIX);
    crate::state::mutate_state(|s| s.legacy_history_start_id = Some(start));
    ensure_next_id_at_least(end);
}

fn current_next_id() -> u64 {
    NEXT_ID.with(|c| {
        *c.borrow()
            .as_ref()
            .expect("History not initialized")
            .get()
    })
}

pub fn insert_record(record: LiquidationRecordVersioned) {
    let id = match &record {
        LiquidationRecordVersioned::V1(r) => r.id,
    };
    HISTORY.with(|h| {
        h.borrow_mut()
            .as_mut()
            .expect("History not initialized")
            .insert(id, record);
    });
}

pub fn get_record(id: u64) -> Option<LiquidationRecordVersioned> {
    HISTORY.with(|h| {
        h.borrow()
            .as_ref()
            .expect("History not initialized")
            .get(&id)
            .or_else(|| legacy_event_record(id))
    })
}

pub fn get_records(offset: u64, limit: u64) -> Vec<LiquidationRecordVersioned> {
    let limit = limit.min(1000);
    HISTORY.with(|h| {
        let borrow = h.borrow();
        let map = borrow.as_ref().expect("History not initialized");
        let count = record_count();
        if count == 0 || offset >= count {
            return vec![];
        }
        let start = count.saturating_sub(offset.saturating_add(limit));
        let end = count.saturating_sub(offset);
        (start..end)
            .filter_map(|id| map.get(&id).or_else(|| legacy_event_record(id)))
            .collect()
    })
}

pub fn record_count() -> u64 {
    let stable_count = NEXT_ID.with(|c| {
        *c.borrow()
            .as_ref()
            .expect("History not initialized")
            .get()
    });
    let legacy_end = crate::state::read_state(|s| {
        let legacy_len = u64::try_from(s.liquidation_events.len()).unwrap_or(u64::MAX);
        match s.legacy_history_start_id {
            Some(start) if legacy_len > 500 => {
                start.saturating_add(legacy_len.saturating_sub(500))
            }
            _ => legacy_len.min(500),
        }
    });
    stable_count.max(legacy_end)
}

/// Returns stuck records (TransferFailed or ConfirmFailed), scanning from newest first.
/// Capped at 100 results to avoid hitting the instruction limit on large histories.
pub fn get_stuck_records() -> Vec<LiquidationRecordVersioned> {
    const MAX_RESULTS: usize = 100;
    HISTORY.with(|h| {
        let borrow = h.borrow();
        let map = borrow.as_ref().expect("History not initialized");
        let count = record_count();
        let mut results = Vec::new();
        for id in (0..count).rev() {
            if results.len() >= MAX_RESULTS {
                break;
            }
            if let Some(record) = map.get(&id) {
                match &record {
                    LiquidationRecordVersioned::V1(r) => match r.status {
                        LiquidationStatus::TransferFailed
                        | LiquidationStatus::ConfirmFailed => results.push(record),
                        _ => {}
                    },
                }
            }
        }
        results
    })
}

pub fn update_record_status(id: u64, new_status: LiquidationStatus) {
    HISTORY.with(|h| {
        let mut borrow = h.borrow_mut();
        let map = borrow.as_mut().expect("History not initialized");
        if let Some(mut record) = map.get(&id) {
            match &mut record {
                LiquidationRecordVersioned::V1(ref mut r) => {
                    r.status = new_status;
                }
            }
            map.insert(id, record);
        }
    });
}

/// Migrate the first legacy events into the stable map. The config snapshot
/// keeps the full legacy vector, and the query path serves any tail that is
/// beyond this bounded upgrade-time copy.
pub fn migrate_legacy_events(events: &[crate::state::BotLiquidationEvent]) {
    for (index, event) in events.iter().take(500).enumerate() {
        insert_record(LiquidationRecordVersioned::V1(record_from_legacy_event(
            index as u64,
            event,
        )));
    }
}

fn legacy_event_record(id: u64) -> Option<LiquidationRecordVersioned> {
    crate::state::read_state(|s| {
        let index = if id < 500 {
            id
        } else {
            let start = s.legacy_history_start_id?;
            if id < start {
                return None;
            }
            500u64.checked_add(id - start)?
        };
        let index = usize::try_from(index).ok()?;
        s.liquidation_events
            .get(index)
            .map(|event| LiquidationRecordVersioned::V1(record_from_legacy_event(id, event)))
    })
}

fn record_from_legacy_event(
    id: u64,
    event: &crate::state::BotLiquidationEvent,
) -> LiquidationRecordV1 {
    LiquidationRecordV1 {
        id,
        vault_id: event.vault_id,
        timestamp: event.timestamp,
        status: if event.success {
            LiquidationStatus::Completed
        } else {
            LiquidationStatus::SwapFailed
        },
        collateral_claimed_e8s: event.collateral_received_e8s,
        debt_to_cover_e8s: event.debt_covered_e8s,
        icp_swapped_e8s: 0,
        ckusdc_received_e6: 0,
        ckusdc_transferred_e6: 0,
        icp_to_treasury_e8s: event.collateral_to_treasury_e8s,
        oracle_price_e8s: event.effective_price_e8s,
        effective_price_e8s: event.effective_price_e8s,
        slippage_bps: event.slippage_bps,
        error_message: event.error_message.clone(),
        confirm_retry_count: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy_event(vault_id: u64) -> crate::state::BotLiquidationEvent {
        crate::state::BotLiquidationEvent {
            timestamp: vault_id + 1_000,
            vault_id,
            debt_covered_e8s: 10,
            collateral_received_e8s: 20,
            icusd_burned_e8s: 10,
            collateral_to_treasury_e8s: 2,
            swap_route: "legacy".to_string(),
            effective_price_e8s: 3,
            slippage_bps: 4,
            success: true,
            error_message: None,
        }
    }

    #[test]
    fn legacy_history_over_500_remains_queryable_and_cursor_skips_tail() {
        crate::memory::init_memory_manager();
        init_history();
        let events: Vec<_> = (0..503).map(legacy_event).collect();
        crate::state::init_state(crate::state::BotState {
            liquidation_events: events.clone(),
            migrated_to_stable_structures: true,
            ..Default::default()
        });

        // Reproduce the former migration followed by one new stable record
        // taking ID 500 before this compatibility repair is installed.
        for (id, event) in events.iter().take(500).enumerate() {
            insert_record(LiquidationRecordVersioned::V1(record_from_legacy_event(
                id as u64, event,
            )));
        }
        insert_record(LiquidationRecordVersioned::V1(LiquidationRecordV1 {
            id: 500,
            vault_id: 90_000,
            timestamp: 90_001,
            status: LiquidationStatus::Completed,
            collateral_claimed_e8s: 0,
            debt_to_cover_e8s: 0,
            icp_swapped_e8s: 0,
            ckusdc_received_e6: 0,
            ckusdc_transferred_e6: 0,
            icp_to_treasury_e8s: 0,
            oracle_price_e8s: 0,
            effective_price_e8s: 0,
            slippage_bps: 0,
            error_message: None,
            confirm_retry_count: 0,
        }));
        ensure_next_id_at_least(501);
        repair_legacy_history_index();
        crate::state::save_config_to_stable();
        crate::state::load_config_from_stable();

        assert_eq!(record_count(), 504);
        for id in [0, 499, 501, 503] {
            let record = get_record(id).expect("legacy event remains queryable");
            match record {
                LiquidationRecordVersioned::V1(record) => {
                    assert_eq!(record.id, id);
                    let expected_vault = if id < 500 { id } else { id - 1 };
                    assert_eq!(record.vault_id, expected_vault);
                }
            }
        }
        let occupied = get_record(500).expect("new stable record remains queryable");
        match occupied {
            LiquidationRecordVersioned::V1(record) => assert_eq!(record.vault_id, 90_000),
        }
        let page = get_records(0, 1_000);
        assert_eq!(page.len(), 504);
        assert_eq!(next_id(), 504, "new records must not reuse the legacy tail IDs");
    }
}
