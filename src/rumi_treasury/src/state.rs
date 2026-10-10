use crate::types::{
    AssetBalance, AssetType, BalancesSnapshot, DepositRecord, TreasuryAction, TreasuryEvent,
    TreasuryInitArgs,
};
use candid::Principal;
use ic_stable_structures::memory_manager::{MemoryId, MemoryManager, VirtualMemory};
use ic_stable_structures::{DefaultMemoryImpl, StableBTreeMap, StableCell};
use std::cell::RefCell;
use std::collections::HashMap;

type Memory = VirtualMemory<DefaultMemoryImpl>;

// Stable memory layout.
//
// Each stable store owns exactly one MemoryManager slot. These IDs are
// PERMANENT: once a store is assigned a slot the number must never change or
// be reused for a different store, or an upgrade will read another store's
// bytes (silent state corruption). They are declared once here and referenced
// by both `init` and `restore_state` so the two code paths can never drift out
// of sync. `MEMORY_LAYOUT` is the single source of truth; `memory_ids_unique`
// (test) guards against two stores accidentally sharing a slot.
const MEM_DEPOSITS: u8 = 0; // StableBTreeMap<u64, DepositRecord>  (deposit log)
const MEM_CONFIG: u8 = 1; // StableCell<TreasuryConfig>          (ledger principals, paused flag)
const MEM_BALANCES: u8 = 2; // StableCell<BalancesSnapshot>        (asset balances — survives upgrades)
const MEM_EVENTS: u8 = 3; // StableBTreeMap<u64, TreasuryEvent>  (event log)
const MEM_WITHDRAWAL_CREATED_AT: u8 = 4; // StableBTreeMap<u64, u64> (request_id → first-attempt created_at_time)
const MEM_SP_UNALLOCATED_INTEREST_BLOCKS: u8 = 5; // StableBTreeMap<u64, u64> (backend mint block → deposit id)
const MEM_SP_UNALLOCATED_INTEREST_TRANSFER_BLOCKS: u8 = 6; // StableBTreeMap<u64, u64> (icUSD transfer block → deposit id)
const MEM_ICUSD_DEPOSIT_BLOCKS: u8 = 7; // StableBTreeMap<u64, u64> (icUSD transfer block → deposit ID; 0 means ambiguous)
const MEM_ICUSD_DEPOSIT_BLOCK_BACKFILL: u8 = 8; // StableCell<IcusdDepositBlockBackfill>
const MEM_ICUSD_DEPOSIT_INDEXED_THROUGH: u8 = 9; // StableCell<u64>

/// Keep each migration update bounded regardless of the size of the historic
/// treasury deposit log.
pub const ICUSD_BLOCK_BACKFILL_BATCH_SIZE: usize = 100;

/// Every stable memory slot this canister owns, paired with a human label.
/// Single source of truth for the layout; iterated by the uniqueness test.
const MEMORY_LAYOUT: &[(u8, &str)] = &[
    (MEM_DEPOSITS, "deposits"),
    (MEM_CONFIG, "config"),
    (MEM_BALANCES, "balances"),
    (MEM_EVENTS, "events"),
    (MEM_WITHDRAWAL_CREATED_AT, "withdrawal_created_at"),
    (
        MEM_SP_UNALLOCATED_INTEREST_BLOCKS,
        "sp_unallocated_interest_blocks",
    ),
    (
        MEM_SP_UNALLOCATED_INTEREST_TRANSFER_BLOCKS,
        "sp_unallocated_interest_transfer_blocks",
    ),
    (MEM_ICUSD_DEPOSIT_BLOCKS, "icusd_deposit_blocks"),
    (
        MEM_ICUSD_DEPOSIT_BLOCK_BACKFILL,
        "icusd_deposit_block_backfill",
    ),
    (MEM_ICUSD_DEPOSIT_INDEXED_THROUGH, "icusd_deposit_indexed_through"),
];

/// A block is marked ambiguous when more than one historic ICUSD deposit used
/// it. Deposit IDs start at one, so zero is reserved as the ambiguity marker.
/// Legacy block zero is always ambiguous because older code may have used zero
/// as an unknown-block sentinel rather than a real ledger block.
const AMBIGUOUS_BLOCK_INDEX: u64 = 0;

#[derive(candid::CandidType, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum IcusdDepositBlockBackfill {
    /// The stable cell did not exist on the previous canister version.
    Uninitialized,
    /// Historic deposit IDs in this inclusive range are being indexed.
    InProgress {
        next_deposit_id: u64,
        snapshot_max_id: u64,
    },
    /// The historic snapshot has been fully indexed. New ICUSD deposits are
    /// indexed synchronously alongside the deposit record.
    Complete,
}

/// Treasury state that persists across upgrades
pub struct TreasuryState {
    /// All deposit records, indexed by deposit ID
    pub deposits: StableBTreeMap<u64, DepositRecord, Memory>,
    /// Current balances by asset type (in-memory mirror of balances_cell)
    pub balances: HashMap<AssetType, AssetBalance>,
    /// Balances persisted to stable memory — written on every mutation
    pub balances_cell: StableCell<BalancesSnapshot, Memory>,
    /// Configuration data
    pub config: StableCell<TreasuryConfig, Memory>,
    /// Next available deposit ID
    pub next_deposit_id: u64,
    /// Event log (audit trail for all operations)
    pub events: StableBTreeMap<u64, TreasuryEvent, Memory>,
    /// Next available event ID
    pub next_event_id: u64,
    /// First-attempt `created_at_time` per withdrawal `request_id` (ICRC-003).
    /// Persisted so a retry after an upgrade still reuses the original
    /// timestamp and hits the ledger's dedup window.
    pub withdrawal_created_at: StableBTreeMap<u64, u64, Memory>,
    /// Backend mint block → deposit ID for exactly-once Stability Pool
    /// unallocated-interest reports.
    pub sp_unallocated_interest_blocks: StableBTreeMap<u64, u64, Memory>,
    /// Physical icUSD transfer block → deposit ID. Source receipts and
    /// transfer receipts must agree before any balance is credited.
    pub sp_unallocated_interest_transfer_blocks: StableBTreeMap<u64, u64, Memory>,
    /// Every ICUSD deposit's physical ledger block. Value zero means multiple
    /// legacy rows used the same block or a legacy row used block zero.
    pub icusd_deposit_blocks: StableBTreeMap<u64, u64, Memory>,
    /// Resumable, bounded migration state for `icusd_deposit_blocks`.
    pub icusd_deposit_block_backfill: StableCell<IcusdDepositBlockBackfill, Memory>,
    /// Highest deposit ID whose ICUSD block has been indexed. Detects rows
    /// written by an older Treasury after a downgrade and before re-upgrade.
    pub icusd_deposit_indexed_through: StableCell<u64, Memory>,
}

/// Treasury configuration stored in stable memory
#[derive(candid::CandidType, serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct TreasuryConfig {
    /// icUSD ledger canister
    pub icusd_ledger: Principal,
    /// ICP ledger canister
    pub icp_ledger: Principal,
    /// ckBTC ledger canister (optional)
    pub ckbtc_ledger: Option<Principal>,
    /// ckUSDT ledger canister (for vault repayment)
    pub ckusdt_ledger: Option<Principal>,
    /// ckUSDC ledger canister (for vault repayment)
    pub ckusdc_ledger: Option<Principal>,
    /// Whether treasury accepts new deposits
    pub is_paused: bool,
    /// This reporter can create only deduplicated ICUSD interest records; it
    /// receives no controller or withdrawal authority.
    #[serde(default)]
    pub stability_pool_reporter: Option<Principal>,
}

// Storable implementation for TreasuryConfig
impl ic_stable_structures::Storable for TreasuryConfig {
    fn to_bytes(&self) -> std::borrow::Cow<'_, [u8]> {
        std::borrow::Cow::Owned(candid::encode_one(self).unwrap())
    }

    fn from_bytes(bytes: std::borrow::Cow<[u8]>) -> Self {
        candid::decode_one(&bytes).unwrap()
    }

    const BOUND: ic_stable_structures::storable::Bound =
        ic_stable_structures::storable::Bound::Unbounded;
}

// Storable implementation for DepositRecord
impl ic_stable_structures::Storable for DepositRecord {
    fn to_bytes(&self) -> std::borrow::Cow<'_, [u8]> {
        std::borrow::Cow::Owned(candid::encode_one(self).unwrap())
    }

    fn from_bytes(bytes: std::borrow::Cow<[u8]>) -> Self {
        candid::decode_one(&bytes).unwrap()
    }

    const BOUND: ic_stable_structures::storable::Bound =
        ic_stable_structures::storable::Bound::Unbounded;
}

// Storable implementation for TreasuryEvent
impl ic_stable_structures::Storable for TreasuryEvent {
    fn to_bytes(&self) -> std::borrow::Cow<'_, [u8]> {
        std::borrow::Cow::Owned(candid::encode_one(self).unwrap())
    }

    fn from_bytes(bytes: std::borrow::Cow<[u8]>) -> Self {
        candid::decode_one(&bytes).unwrap()
    }

    const BOUND: ic_stable_structures::storable::Bound =
        ic_stable_structures::storable::Bound::Unbounded;
}

// Storable implementation for BalancesSnapshot
impl ic_stable_structures::Storable for BalancesSnapshot {
    fn to_bytes(&self) -> std::borrow::Cow<'_, [u8]> {
        std::borrow::Cow::Owned(candid::encode_one(self).unwrap())
    }

    fn from_bytes(bytes: std::borrow::Cow<[u8]>) -> Self {
        candid::decode_one(&bytes).unwrap()
    }

    const BOUND: ic_stable_structures::storable::Bound =
        ic_stable_structures::storable::Bound::Unbounded;
}

impl ic_stable_structures::Storable for IcusdDepositBlockBackfill {
    fn to_bytes(&self) -> std::borrow::Cow<'_, [u8]> {
        std::borrow::Cow::Owned(candid::encode_one(self).unwrap())
    }

    fn from_bytes(bytes: std::borrow::Cow<[u8]>) -> Self {
        candid::decode_one(&bytes).unwrap()
    }

    const BOUND: ic_stable_structures::storable::Bound =
        ic_stable_structures::storable::Bound::Unbounded;
}

thread_local! {
    static MEMORY_MANAGER: RefCell<MemoryManager<DefaultMemoryImpl>> =
        RefCell::new(MemoryManager::init(DefaultMemoryImpl::default()));

    static STATE: RefCell<Option<TreasuryState>> = RefCell::new(None);
}

/// Build the default empty balances HashMap.
fn empty_balances() -> HashMap<AssetType, AssetBalance> {
    let mut m = HashMap::new();
    m.insert(AssetType::ICUSD, AssetBalance::default());
    m.insert(AssetType::ICP, AssetBalance::default());
    m.insert(AssetType::CKBTC, AssetBalance::default());
    m.insert(AssetType::CKUSDT, AssetBalance::default());
    m.insert(AssetType::CKUSDC, AssetBalance::default());
    m
}

impl TreasuryState {
    /// Initialize treasury state with given arguments (first install only).
    pub fn init(args: TreasuryInitArgs) -> Self {
        MEMORY_MANAGER.with(|mm| {
            let memory_manager = mm.borrow();

            let config = TreasuryConfig {
                icusd_ledger: args.icusd_ledger,
                icp_ledger: args.icp_ledger,
                ckbtc_ledger: args.ckbtc_ledger,
                ckusdt_ledger: args.ckusdt_ledger,
                ckusdc_ledger: args.ckusdc_ledger,
                is_paused: false,
                stability_pool_reporter: None,
            };

            let balances = empty_balances();

            Self {
                deposits: StableBTreeMap::init(memory_manager.get(MemoryId::new(MEM_DEPOSITS))),
                balances_cell: StableCell::init(
                    memory_manager.get(MemoryId::new(MEM_BALANCES)),
                    BalancesSnapshot::default(),
                )
                .unwrap(),
                balances,
                config: StableCell::init(memory_manager.get(MemoryId::new(MEM_CONFIG)), config)
                    .unwrap(),
                next_deposit_id: 1,
                events: StableBTreeMap::init(memory_manager.get(MemoryId::new(MEM_EVENTS))),
                next_event_id: 1,
                withdrawal_created_at: StableBTreeMap::init(
                    memory_manager.get(MemoryId::new(MEM_WITHDRAWAL_CREATED_AT)),
                ),
                sp_unallocated_interest_blocks: StableBTreeMap::init(
                    memory_manager.get(MemoryId::new(MEM_SP_UNALLOCATED_INTEREST_BLOCKS)),
                ),
                sp_unallocated_interest_transfer_blocks: StableBTreeMap::init(
                    memory_manager.get(MemoryId::new(MEM_SP_UNALLOCATED_INTEREST_TRANSFER_BLOCKS)),
                ),
                icusd_deposit_blocks: StableBTreeMap::init(
                    memory_manager.get(MemoryId::new(MEM_ICUSD_DEPOSIT_BLOCKS)),
                ),
                icusd_deposit_block_backfill: StableCell::init(
                    memory_manager.get(MemoryId::new(MEM_ICUSD_DEPOSIT_BLOCK_BACKFILL)),
                    IcusdDepositBlockBackfill::Complete,
                )
                .unwrap(),
                icusd_deposit_indexed_through: StableCell::init(
                    memory_manager.get(MemoryId::new(MEM_ICUSD_DEPOSIT_INDEXED_THROUGH)),
                    0,
                )
                .unwrap(),
            }
        })
    }

    // ------------------------------------------------------------------
    // Balances persistence helper
    // ------------------------------------------------------------------

    /// Flush the in-memory balances HashMap to the stable `BalancesSnapshot` cell.
    /// Must be called after every mutation to `self.balances`.
    fn persist_balances(&mut self) {
        let snapshot = BalancesSnapshot {
            entries: self
                .balances
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        };
        // Ignore the error — StableCell::set only fails if the value is too
        // large for the memory region, which won't happen for 5 balance entries.
        let _ = self.balances_cell.set(snapshot);
    }

    // ------------------------------------------------------------------
    // Event logging
    // ------------------------------------------------------------------

    /// Record an event in the audit trail.
    pub fn push_event(&mut self, caller: Principal, action: TreasuryAction) {
        self.push_event_at(caller, action, ic_cdk::api::time());
    }

    /// Record an event with an explicit timestamp for deterministic state
    /// transition tests.
    pub fn push_event_at(&mut self, caller: Principal, action: TreasuryAction, timestamp: u64) {
        let event_id = self.next_event_id;
        self.next_event_id += 1;
        self.events.insert(
            event_id,
            TreasuryEvent {
                id: event_id,
                timestamp,
                caller,
                action,
            },
        );
    }

    /// Get events (paginated).
    pub fn get_events(&self, start: Option<u64>, limit: usize) -> Vec<TreasuryEvent> {
        let start_key = start.unwrap_or(0);
        self.events
            .range(start_key..)
            .take(limit)
            .map(|(_, event)| event)
            .collect()
    }

    /// Get total events count.
    pub fn get_events_count(&self) -> u64 {
        self.events.len()
    }

    // ------------------------------------------------------------------
    // Deposits / withdrawals
    // ------------------------------------------------------------------

    /// Add a new deposit record and update balances.
    pub fn add_deposit(&mut self, record: DepositRecord) -> u64 {
        let deposit_id = self.next_deposit_id;
        self.next_deposit_id += 1;

        // Update balance for this asset type
        if let Some(balance) = self.balances.get_mut(&record.asset_type) {
            balance.total += record.amount;
            balance.available += record.amount;
        }

        // Store the deposit record
        let mut final_record = record;
        final_record.id = deposit_id;
        let is_icusd = final_record.asset_type == AssetType::ICUSD;
        self.deposits.insert(deposit_id, final_record);

        // Non-ICUSD rows cannot create physical ICUSD block collisions, so
        // they can advance the contiguous watermark immediately. ICUSD rows
        // advance it only after their physical-block entry has been written.
        if !is_icusd
            && matches!(
                self.icusd_deposit_block_backfill.get(),
                IcusdDepositBlockBackfill::Complete
            )
        {
            let _ = self.icusd_deposit_indexed_through.set(deposit_id);
        }

        self.persist_balances();
        deposit_id
    }

    /// Record a production ICUSD deposit against the shared physical-block
    /// index. Only BorrowingFee/ICUSD retries are idempotent; every other
    /// deposit type rejects a previously indexed block.
    pub fn record_icusd_deposit_once(
        &mut self,
        record: DepositRecord,
    ) -> Result<(u64, bool), String> {
        if record.asset_type != AssetType::ICUSD {
            return Err("ICUSD block index only accepts ICUSD deposits".to_string());
        }
        if !matches!(
            self.icusd_deposit_block_backfill.get(),
            IcusdDepositBlockBackfill::Complete
        ) {
            return Err("ICUSD deposit block index backfill is in progress".to_string());
        }

        // The SP receipt index predates the shared index. If it contains a
        // block, never let a BorrowingFee retry treat that physical transfer
        // as its own, even if the shared index is inconsistent.
        if let Some(existing_id) = self
            .sp_unallocated_interest_transfer_blocks
            .get(&record.block_index)
        {
            return Err(format!(
                "ICUSD block {} is already recorded as Stability Pool deposit {}",
                record.block_index, existing_id
            ));
        }

        if let Some(existing_id) = self.icusd_deposit_blocks.get(&record.block_index) {
            if existing_id == AMBIGUOUS_BLOCK_INDEX {
                return Err(format!(
                    "ICUSD block {} is ambiguous in treasury history",
                    record.block_index
                ));
            }

            if record.deposit_type == crate::types::DepositType::BorrowingFee {
                let existing = self.deposits.get(&existing_id).ok_or_else(|| {
                    format!(
                        "ICUSD block {} points to missing treasury deposit {}",
                        record.block_index, existing_id
                    )
                })?;
                if existing.deposit_type == crate::types::DepositType::BorrowingFee
                    && existing.asset_type == AssetType::ICUSD
                    && existing.amount == record.amount
                    && existing.block_index == record.block_index
                    && existing.memo == record.memo
                {
                    return Ok((existing_id, false));
                }
                return Err(format!(
                    "ICUSD block {} conflicts with its existing treasury deposit",
                    record.block_index
                ));
            }

            return Err(format!(
                "ICUSD block {} is already recorded as treasury deposit {}",
                record.block_index, existing_id
            ));
        }

        let block_index = record.block_index;
        let deposit_id = self.add_deposit(record);
        self.icusd_deposit_blocks.insert(block_index, deposit_id);
        let _ = self.icusd_deposit_indexed_through.set(deposit_id);
        Ok((deposit_id, true))
    }

    /// Index at most `ICUSD_BLOCK_BACKFILL_BATCH_SIZE` historic deposit rows.
    /// The range is snapshotted when the upgrade is first restored, so newer
    /// ICUSD ingress remains held until this finite migration is complete.
    pub fn continue_icusd_deposit_block_backfill(&mut self) -> Result<(u64, u64, bool), String> {
        let (mut cursor, snapshot_max_id) = match self.icusd_deposit_block_backfill.get().clone() {
            IcusdDepositBlockBackfill::Uninitialized => {
                return Err("ICUSD block backfill was not initialized".to_string())
            }
            IcusdDepositBlockBackfill::InProgress {
                next_deposit_id,
                snapshot_max_id,
            } => (next_deposit_id, snapshot_max_id),
            IcusdDepositBlockBackfill::Complete => return Ok((0, 0, true)),
        };

        let rows: Vec<(u64, DepositRecord)> = self
            .deposits
            .range(cursor..=snapshot_max_id)
            .take(ICUSD_BLOCK_BACKFILL_BATCH_SIZE)
            .collect();
        let processed = rows.len() as u64;
        let last_processed_id = rows.last().map(|(id, _)| *id);
        for (deposit_id, record) in rows {
            if record.asset_type != AssetType::ICUSD {
                continue;
            }

            if record.block_index == 0 {
                // Historical zero may mean "unknown". Never use it to
                // authorize a retry, even if only one row currently exists.
                self.icusd_deposit_blocks
                    .insert(record.block_index, AMBIGUOUS_BLOCK_INDEX);
            } else if let Some(existing_id) = self.icusd_deposit_blocks.get(&record.block_index) {
                if existing_id != AMBIGUOUS_BLOCK_INDEX && existing_id != deposit_id {
                    self.icusd_deposit_blocks
                        .insert(record.block_index, AMBIGUOUS_BLOCK_INDEX);
                }
            } else {
                self.icusd_deposit_blocks
                    .insert(record.block_index, deposit_id);
            }
        }

        if let Some(last_processed_id) = last_processed_id {
            cursor = last_processed_id.checked_add(1).unwrap_or(last_processed_id);
        }
        let complete = processed == 0
            || last_processed_id
                .map(|last_id| last_id >= snapshot_max_id)
                .unwrap_or(false);
        let current_max_id = self.deposits.last_key_value().map(|(id, _)| id).unwrap_or(0);
        let status = if complete && current_max_id <= snapshot_max_id {
            self.icusd_deposit_indexed_through
                .set(snapshot_max_id)
                .map_err(|e| format!("Failed to persist ICUSD index watermark: {:?}", e))?;
            IcusdDepositBlockBackfill::Complete
        } else if complete {
            IcusdDepositBlockBackfill::InProgress {
                next_deposit_id: snapshot_max_id.checked_add(1).unwrap_or(snapshot_max_id),
                snapshot_max_id: current_max_id,
            }
        } else {
            IcusdDepositBlockBackfill::InProgress {
                next_deposit_id: cursor,
                snapshot_max_id,
            }
        };
        let migration_complete = matches!(&status, IcusdDepositBlockBackfill::Complete);
        self.icusd_deposit_block_backfill
            .set(status)
            .map_err(|e| format!("Failed to persist ICUSD block backfill: {:?}", e))?;

        Ok((processed, cursor, migration_complete))
    }

    /// Record a Stability Pool treasury forward exactly once per backend mint
    /// receipt. This method has no await, so lookup, deposit creation, and
    /// receipt indexing are one canister-state transition.
    pub fn record_sp_unallocated_interest_once(
        &mut self,
        amount: u64,
        transfer_block_index: u64,
        source_mint_blocks: &[u64],
    ) -> Result<(u64, bool), String> {
        self.record_sp_unallocated_interest_once_at(
            amount,
            transfer_block_index,
            source_mint_blocks,
            ic_cdk::api::time(),
        )
    }

    pub fn record_sp_unallocated_interest_once_at(
        &mut self,
        amount: u64,
        transfer_block_index: u64,
        source_mint_blocks: &[u64],
        now: u64,
    ) -> Result<(u64, bool), String> {
        if source_mint_blocks.is_empty() {
            return Err("source mint blocks cannot be empty".to_string());
        }
        if !matches!(
            self.icusd_deposit_block_backfill.get(),
            IcusdDepositBlockBackfill::Complete
        ) {
            return Err("ICUSD deposit block index backfill is in progress".to_string());
        }
        let existing: Vec<u64> = source_mint_blocks
            .iter()
            .filter_map(|block| self.sp_unallocated_interest_blocks.get(block))
            .collect();
        if !existing.is_empty() {
            if existing.len() == source_mint_blocks.len()
                && existing.iter().all(|id| *id == existing[0])
            {
                let existing_id = existing[0];
                let existing_deposit = self.deposits.get(&existing_id).ok_or_else(|| {
                    format!(
                        "Stability Pool source receipt points to missing deposit {}",
                        existing_id
                    )
                })?;
                if existing_deposit.deposit_type == crate::types::DepositType::InterestRevenue
                    && existing_deposit.asset_type == crate::types::AssetType::ICUSD
                    && existing_deposit.amount == amount
                    && existing_deposit.block_index == transfer_block_index
                    && existing_deposit.memo.as_deref()
                        == Some("stability-pool unallocated interest")
                    && self.icusd_deposit_blocks.get(&transfer_block_index) == Some(existing_id)
                    && self
                        .sp_unallocated_interest_transfer_blocks
                        .get(&transfer_block_index)
                        == Some(existing_id)
                {
                    return Ok((existing_id, false));
                }
                return Err(
                    "Stability Pool source receipt conflicts with its ICUSD transfer block"
                        .to_string(),
                );
            }
            return Err("one or more source mint blocks were already recorded".to_string());
        }
        if let Some(existing_transfer) = self
            .sp_unallocated_interest_transfer_blocks
            .get(&transfer_block_index)
        {
            return Err(format!(
                "transfer block {} is already recorded as deposit {}",
                transfer_block_index, existing_transfer
            ));
        }

        let (deposit_id, newly_recorded) = self.record_icusd_deposit_once(DepositRecord {
            id: 0,
            deposit_type: crate::types::DepositType::InterestRevenue,
            asset_type: crate::types::AssetType::ICUSD,
            amount,
            block_index: transfer_block_index,
            timestamp: now,
            memo: Some("stability-pool unallocated interest".to_string()),
        })?;
        if !newly_recorded {
            return Err("ICUSD transfer block already has a non-SP deposit".to_string());
        }
        for source_mint_block in source_mint_blocks {
            self.sp_unallocated_interest_blocks
                .insert(*source_mint_block, deposit_id);
        }
        self.sp_unallocated_interest_transfer_blocks
            .insert(transfer_block_index, deposit_id);
        Ok((deposit_id, true))
    }

    /// Reserve `amount` from bookkeeping before attempting a withdrawal transfer.
    pub fn withdraw(&mut self, asset_type: AssetType, amount: u64) -> Result<(), String> {
        let balance = self
            .balances
            .get_mut(&asset_type)
            .ok_or_else(|| format!("Unknown asset type: {:?}", asset_type))?;

        if balance.available < amount {
            return Err(format!(
                "Insufficient balance. Available: {}, requested: {}",
                balance.available, amount
            ));
        }

        balance.total -= amount;
        balance.available -= amount;
        self.persist_balances();
        Ok(())
    }

    /// Restore balance after a failed withdrawal transfer.
    pub fn restore_balance(&mut self, asset_type: &AssetType, amount: u64) {
        if let Some(balance) = self.balances.get_mut(asset_type) {
            balance.total += amount;
            balance.available += amount;
        }
        self.persist_balances();
    }

    /// ICRC-003: the `created_at_time` to use for this withdrawal request.
    /// The first attempt's timestamp is persisted per `request_id` and reused
    /// on retries so the ledger's `(created_at_time, memo, ...)` dedup
    /// actually fires. Entries older than the ledger dedup window can no
    /// longer dedup (the original transaction has expired), so they are
    /// replaced with `now`; expired entries are pruned to bound growth.
    pub fn created_at_time_for_request(&mut self, request_id: u64, now: u64) -> u64 {
        // ICP/ICRC ledgers keep transactions for dedup for 24 hours.
        const LEDGER_DEDUP_WINDOW_NANOS: u64 = 24 * 60 * 60 * 1_000_000_000;
        let cutoff = now.saturating_sub(LEDGER_DEDUP_WINDOW_NANOS);
        // Withdrawals are controller-only and rare, so a full scan is cheap.
        let expired: Vec<u64> = self
            .withdrawal_created_at
            .iter()
            .filter(|(_, t)| *t < cutoff)
            .map(|(id, _)| id)
            .collect();
        for id in expired {
            self.withdrawal_created_at.remove(&id);
        }
        if let Some(t) = self.withdrawal_created_at.get(&request_id) {
            return t;
        }
        self.withdrawal_created_at.insert(request_id, now);
        now
    }

    /// Drop the persisted `created_at_time` for a request whose timestamp the
    /// ledger rejected (TooOld/CreatedInFuture), so the next attempt gets a
    /// fresh one instead of failing forever.
    pub fn clear_request_created_at(&mut self, request_id: u64) {
        self.withdrawal_created_at.remove(&request_id);
    }

    // ------------------------------------------------------------------
    // Config helpers
    // ------------------------------------------------------------------

    /// Get current configuration.
    pub fn get_config(&self) -> TreasuryConfig {
        self.config.get().clone()
    }

    /// Pause/unpause treasury.
    pub fn set_paused(&mut self, paused: bool) -> Result<(), String> {
        let mut config = self.config.get().clone();
        config.is_paused = paused;
        self.config
            .set(config)
            .map_err(|e| format!("Failed to update pause state: {:?}", e))?;
        Ok(())
    }

    pub fn set_stability_pool_reporter(
        &mut self,
        reporter: Option<Principal>,
    ) -> Result<(), String> {
        let mut config = self.config.get().clone();
        config.stability_pool_reporter = reporter;
        self.config
            .set(config)
            .map_err(|e| format!("Failed to update stability pool reporter: {:?}", e))?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Queries
    // ------------------------------------------------------------------

    /// Get all deposits (paginated).
    pub fn get_deposits(&self, start: Option<u64>, limit: usize) -> Vec<DepositRecord> {
        let start_key = start.unwrap_or(0);
        self.deposits
            .range(start_key..)
            .take(limit)
            .map(|(_, record)| record)
            .collect()
    }

    /// Get total deposits count.
    pub fn get_deposits_count(&self) -> u64 {
        self.deposits.len()
    }
}

// ======================================================================
// Module-level state helpers
// ======================================================================

/// Initialize the treasury state (first install).
pub fn init_state(args: TreasuryInitArgs) {
    STATE.with(|s| {
        *s.borrow_mut() = Some(TreasuryState::init(args));
    });
}

/// Restore treasury state from stable memory after upgrade.
///
/// `StableBTreeMap::init` and `StableCell::init` re-open existing stable
/// memory regions (they don't overwrite). Balances are read from the
/// persisted `BalancesSnapshot` cell — no need to replay deposits.
pub fn restore_state() {
    STATE.with(|s| {
        MEMORY_MANAGER.with(|mm| {
            let memory_manager = mm.borrow();

            // Re-open the stable structures — reads existing data from stable memory
            let deposits: StableBTreeMap<u64, DepositRecord, Memory> =
                StableBTreeMap::init(memory_manager.get(MemoryId::new(MEM_DEPOSITS)));

            // Dummy default for StableCell::init — real value is read from stable memory
            let dummy_config = TreasuryConfig {
                icusd_ledger: Principal::anonymous(),
                icp_ledger: Principal::anonymous(),
                ckbtc_ledger: None,
                ckusdt_ledger: None,
                ckusdc_ledger: None,
                is_paused: true,
                stability_pool_reporter: None,
            };
            let config =
                StableCell::init(memory_manager.get(MemoryId::new(MEM_CONFIG)), dummy_config)
                    .unwrap();

            // Read persisted balances.
            // On first upgrade from old code the cell won't exist yet, so the
            // default is an empty snapshot — we fall back to deposit replay.
            let balances_cell: StableCell<BalancesSnapshot, Memory> = StableCell::init(
                memory_manager.get(MemoryId::new(MEM_BALANCES)),
                BalancesSnapshot::default(),
            )
            .unwrap();

            let snapshot = balances_cell.get().clone();
            let balances = if snapshot.entries.is_empty() {
                // First upgrade from pre-BalancesSnapshot code: reconstruct
                // from deposit records (withdrawals still lost — acceptable
                // since treasury has had no withdrawals yet).
                let mut b = empty_balances();
                for (_id, record) in deposits.iter() {
                    if let Some(balance) = b.get_mut(&record.asset_type) {
                        balance.total += record.amount;
                        balance.available += record.amount;
                    }
                }
                b
            } else {
                // Normal path: restore from persisted snapshot.
                let mut b = empty_balances();
                for (asset, bal) in snapshot.entries {
                    b.insert(asset, bal);
                }
                b
            };

            // Compute next_deposit_id from max key in the deposit map.
            // StableBTreeMap supports logarithmic last-key lookup; avoid
            // scanning the entire deposit log during an upgrade.
            let max_id = deposits.last_key_value().map(|(id, _)| id).unwrap_or(0);
            let next_deposit_id = if max_id > 0 { max_id + 1 } else { 1 };

            // Re-open events stable map
            let events: StableBTreeMap<u64, TreasuryEvent, Memory> =
                StableBTreeMap::init(memory_manager.get(MemoryId::new(MEM_EVENTS)));
            let max_event_id = events.last_key_value().map(|(id, _)| id).unwrap_or(0);
            let next_event_id = if max_event_id > 0 {
                max_event_id + 1
            } else {
                1
            };

            // Re-open withdrawal created_at_time map
            let withdrawal_created_at: StableBTreeMap<u64, u64, Memory> =
                StableBTreeMap::init(memory_manager.get(MemoryId::new(MEM_WITHDRAWAL_CREATED_AT)));
            let sp_unallocated_interest_blocks: StableBTreeMap<u64, u64, Memory> =
                StableBTreeMap::init(
                    memory_manager.get(MemoryId::new(MEM_SP_UNALLOCATED_INTEREST_BLOCKS)),
                );
            let sp_unallocated_interest_transfer_blocks: StableBTreeMap<u64, u64, Memory> =
                StableBTreeMap::init(
                    memory_manager.get(MemoryId::new(MEM_SP_UNALLOCATED_INTEREST_TRANSFER_BLOCKS)),
                );

            let icusd_deposit_blocks: StableBTreeMap<u64, u64, Memory> =
                StableBTreeMap::init(memory_manager.get(MemoryId::new(MEM_ICUSD_DEPOSIT_BLOCKS)));
            let mut icusd_deposit_block_backfill: StableCell<IcusdDepositBlockBackfill, Memory> =
                StableCell::init(
                    memory_manager.get(MemoryId::new(MEM_ICUSD_DEPOSIT_BLOCK_BACKFILL)),
                    IcusdDepositBlockBackfill::Uninitialized,
                )
                .unwrap();
            let icusd_deposit_indexed_through: StableCell<u64, Memory> = StableCell::init(
                memory_manager.get(MemoryId::new(MEM_ICUSD_DEPOSIT_INDEXED_THROUGH)),
                0,
            )
            .unwrap();
            if matches!(
                icusd_deposit_block_backfill.get(),
                IcusdDepositBlockBackfill::Uninitialized
            ) {
                let indexed_through = *icusd_deposit_indexed_through.get();
                let initial_status = if max_id <= indexed_through {
                    IcusdDepositBlockBackfill::Complete
                } else {
                    IcusdDepositBlockBackfill::InProgress {
                        next_deposit_id: indexed_through.checked_add(1).unwrap_or(indexed_through),
                        snapshot_max_id: max_id,
                    }
                };
                icusd_deposit_block_backfill
                    .set(initial_status)
                    .expect("persist initial ICUSD block backfill state");
            } else if matches!(
                icusd_deposit_block_backfill.get(),
                IcusdDepositBlockBackfill::Complete
            ) && max_id > *icusd_deposit_indexed_through.get()
            {
                // A downgraded version has no knowledge of the block index
                // watermark and may have appended unindexed deposits. Rebuild
                // only the bounded tail beginning immediately after the last
                // ID known to have been indexed.
                let next_deposit_id = icusd_deposit_indexed_through
                    .get()
                    .checked_add(1)
                    .unwrap_or(*icusd_deposit_indexed_through.get());
                icusd_deposit_block_backfill
                    .set(IcusdDepositBlockBackfill::InProgress {
                        next_deposit_id,
                        snapshot_max_id: max_id,
                    })
                    .expect("persist ICUSD rollback backfill state");
            } else if let IcusdDepositBlockBackfill::InProgress {
                next_deposit_id,
                snapshot_max_id,
            } = icusd_deposit_block_backfill.get().clone()
            {
                if max_id > snapshot_max_id {
                    icusd_deposit_block_backfill
                        .set(IcusdDepositBlockBackfill::InProgress {
                            next_deposit_id,
                            snapshot_max_id: max_id,
                        })
                        .expect("extend ICUSD backfill snapshot after rollback");
                }
            }

            *s.borrow_mut() = Some(TreasuryState {
                deposits,
                balances,
                balances_cell,
                config,
                next_deposit_id,
                events,
                next_event_id,
                withdrawal_created_at,
                sp_unallocated_interest_blocks,
                sp_unallocated_interest_transfer_blocks,
                icusd_deposit_blocks,
                icusd_deposit_block_backfill,
                icusd_deposit_indexed_through,
            });
        });
    });
}

/// Read treasury state.
pub fn with_state<R>(f: impl FnOnce(&TreasuryState) -> R) -> R {
    STATE.with(|s| {
        let state = s.borrow();
        let state = state.as_ref().expect("Treasury state not initialized");
        f(state)
    })
}

/// Mutate treasury state.
pub fn with_state_mut<R>(f: impl FnOnce(&mut TreasuryState) -> R) -> R {
    STATE.with(|s| {
        let mut state = s.borrow_mut();
        let state = state.as_mut().expect("Treasury state not initialized");
        f(state)
    })
}

#[cfg(test)]
mod memory_layout_tests {
    use super::MEMORY_LAYOUT;
    use std::collections::HashSet;

    /// No two stable stores may share a MemoryManager slot — a collision would
    /// make two stores read/write the same bytes and silently corrupt state.
    /// This fails fast in CI if a future store reuses an existing ID.
    #[test]
    fn memory_ids_unique() {
        let mut seen = HashSet::new();
        for (id, label) in MEMORY_LAYOUT {
            assert!(
                seen.insert(*id),
                "duplicate stable MemoryId {id} (store {label:?}) — pick an unused slot"
            );
        }
    }
}
