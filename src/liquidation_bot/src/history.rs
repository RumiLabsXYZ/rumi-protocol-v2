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
    })
}

pub fn get_records(offset: u64, limit: u64) -> Vec<LiquidationRecordVersioned> {
    HISTORY.with(|h| {
        let borrow = h.borrow();
        let map = borrow.as_ref().expect("History not initialized");
        let count = record_count();
        if count == 0 || offset >= count {
            return vec![];
        }
        let start = count.saturating_sub(offset + limit);
        let end = count.saturating_sub(offset);
        (start..end).filter_map(|id| map.get(&id)).collect()
    })
}

pub fn record_count() -> u64 {
    NEXT_ID.with(|c| {
        *c.borrow()
            .as_ref()
            .expect("History not initialized")
            .get()
    })
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

/// Migrate legacy BotLiquidationEvent entries into the new stable map.
/// Capped at 500 entries to avoid trapping in post_upgrade due to instruction limit.
/// In practice the bot has far fewer legacy events than this.
pub fn migrate_legacy_events(events: &[crate::state::BotLiquidationEvent]) {
    let events = if events.len() > 500 { &events[..500] } else { events };
    for event in events {
        let id = next_id();
        let status = if event.success {
            LiquidationStatus::Completed
        } else {
            LiquidationStatus::SwapFailed
        };
        let record = LiquidationRecordV1 {
            id,
            vault_id: event.vault_id,
            timestamp: event.timestamp,
            status,
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
        };
        insert_record(LiquidationRecordVersioned::V1(record));
    }
}
