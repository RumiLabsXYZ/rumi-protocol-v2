use crate::types::{
    AssetBalance, AssetType, BalancesSnapshot, DepositRecord, ExternalTokenDepositArgs,
    ExternalTokenDepositBackfill, TreasuryAction, TreasuryEvent, TreasuryInitArgs,
    WithdrawalIntent,
};
use candid::Principal;
use ic_stable_structures::memory_manager::{MemoryId, MemoryManager, VirtualMemory};
use ic_stable_structures::{DefaultMemoryImpl, StableBTreeMap, StableCell};
use std::borrow::Cow;
use std::cell::RefCell;
use std::cmp::Ordering;
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
const MEM_EXTERNAL_TOKEN_RECEIPTS: u8 = 10; // StableBTreeMap<(ledger, block), receipt>
const MEM_EXTERNAL_TOKEN_LEGACY_BLOCKS: u8 = 11; // StableBTreeMap<(asset, block), ()>
const MEM_EXTERNAL_TOKEN_BACKFILL: u8 = 12; // StableCell<ExternalTokenDepositBackfill>
const MEM_EXTERNAL_TOKEN_INDEXED_THROUGH: u8 = 13; // StableCell<u64>
const MEM_EXTERNAL_TOKEN_RECEIPTS_BY_ID: u8 = 14; // StableBTreeMap<deposit ID, receipt>
const MEM_WITHDRAWAL_INTENTS: u8 = 15; // StableBTreeMap<request ID, durable withdrawal intent>

const PRINCIPAL_KEY_MAX_BYTES: usize = 29;
const EXTERNAL_RECEIPT_KEY_BYTES: usize = 1 + PRINCIPAL_KEY_MAX_BYTES + 8;

/// Keep each migration update bounded regardless of the size of the historic
/// treasury deposit log.
pub const ICUSD_BLOCK_BACKFILL_BATCH_SIZE: usize = 100;
pub const EXTERNAL_TOKEN_BLOCK_BACKFILL_BATCH_SIZE: usize = 100;
/// Permanent withdrawal IDs cannot be pruned without losing at-most-once
/// protection. Stop accepting new identities before journal growth is unbounded.
const MAX_WITHDRAWAL_INTENTS: u64 = 10_000;

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
    (
        MEM_ICUSD_DEPOSIT_INDEXED_THROUGH,
        "icusd_deposit_indexed_through",
    ),
    (MEM_EXTERNAL_TOKEN_RECEIPTS, "external_token_receipts"),
    (
        MEM_EXTERNAL_TOKEN_LEGACY_BLOCKS,
        "external_token_legacy_blocks",
    ),
    (MEM_EXTERNAL_TOKEN_BACKFILL, "external_token_backfill"),
    (
        MEM_EXTERNAL_TOKEN_INDEXED_THROUGH,
        "external_token_indexed_through",
    ),
    (
        MEM_EXTERNAL_TOKEN_RECEIPTS_BY_ID,
        "external_token_receipts_by_id",
    ),
    (MEM_WITHDRAWAL_INTENTS, "withdrawal_intents"),
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

/// Stable key for a physical block scoped to the exact configured ledger.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalTokenReceiptKey {
    pub ledger: Principal,
    pub block_index: u64,
}

impl Ord for ExternalTokenReceiptKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.ledger
            .cmp(&other.ledger)
            .then_with(|| self.block_index.cmp(&other.block_index))
    }
}

impl PartialOrd for ExternalTokenReceiptKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl ic_stable_structures::Storable for ExternalTokenReceiptKey {
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        let principal = self.ledger.as_slice();
        assert!(principal.len() <= PRINCIPAL_KEY_MAX_BYTES);
        let mut bytes = vec![0; EXTERNAL_RECEIPT_KEY_BYTES];
        bytes[0] = principal.len() as u8;
        bytes[1..1 + principal.len()].copy_from_slice(principal);
        bytes[1 + PRINCIPAL_KEY_MAX_BYTES..].copy_from_slice(&self.block_index.to_be_bytes());
        Cow::Owned(bytes)
    }

    fn from_bytes(bytes: Cow<[u8]>) -> Self {
        assert_eq!(bytes.len(), EXTERNAL_RECEIPT_KEY_BYTES);
        let principal_len = bytes[0] as usize;
        assert!(principal_len <= PRINCIPAL_KEY_MAX_BYTES);
        let ledger = Principal::from_slice(&bytes[1..1 + principal_len]);
        let block_index = u64::from_be_bytes(
            bytes[1 + PRINCIPAL_KEY_MAX_BYTES..]
                .try_into()
                .expect("fixed external receipt key block index"),
        );
        Self {
            ledger,
            block_index,
        }
    }

    const BOUND: ic_stable_structures::storable::Bound =
        ic_stable_structures::storable::Bound::Bounded {
            max_size: EXTERNAL_RECEIPT_KEY_BYTES as u32,
            is_fixed_size: true,
        };
}

#[derive(candid::CandidType, serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct ExternalTokenDepositReceipt {
    pub deposit_id: u64,
    pub ledger: Principal,
    pub deposit_type: crate::types::DepositType,
    pub asset_type: AssetType,
    pub amount: u64,
    pub block_index: u64,
    pub memo: Option<String>,
}

impl ic_stable_structures::Storable for ExternalTokenDepositReceipt {
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        Cow::Owned(candid::encode_one(self).unwrap())
    }

    fn from_bytes(bytes: Cow<[u8]>) -> Self {
        candid::decode_one(&bytes).unwrap()
    }

    const BOUND: ic_stable_structures::storable::Bound =
        ic_stable_structures::storable::Bound::Unbounded;
}

/// Historical external-token deposits do not persist their ledger principal.
/// A block-only key conservatively prevents either CK asset from claiming the
/// physical block, including when both assets are configured to one ledger.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalTokenLegacyBlockKey {
    pub block_index: u64,
}

impl Ord for ExternalTokenLegacyBlockKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.block_index.cmp(&other.block_index)
    }
}

impl PartialOrd for ExternalTokenLegacyBlockKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl ic_stable_structures::Storable for ExternalTokenLegacyBlockKey {
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        Cow::Owned(self.block_index.to_be_bytes().to_vec())
    }

    fn from_bytes(bytes: Cow<[u8]>) -> Self {
        assert_eq!(bytes.len(), 8);
        Self {
            block_index: u64::from_be_bytes(bytes[..].try_into().unwrap()),
        }
    }

    const BOUND: ic_stable_structures::storable::Bound =
        ic_stable_structures::storable::Bound::Bounded {
            max_size: 8,
            is_fixed_size: true,
        };
}

#[derive(Clone, Copy, Debug, Default)]
pub struct UnitValue;

impl ic_stable_structures::Storable for UnitValue {
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        Cow::Owned(vec![0])
    }
    fn from_bytes(bytes: Cow<[u8]>) -> Self {
        assert_eq!(bytes.as_ref(), &[0]);
        Self
    }
    const BOUND: ic_stable_structures::storable::Bound =
        ic_stable_structures::storable::Bound::Bounded {
            max_size: 1,
            is_fixed_size: true,
        };
}

impl ic_stable_structures::Storable for ExternalTokenDepositBackfill {
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        Cow::Owned(candid::encode_one(self).unwrap())
    }
    fn from_bytes(bytes: Cow<[u8]>) -> Self {
        candid::decode_one(&bytes).unwrap()
    }
    const BOUND: ic_stable_structures::storable::Bound =
        ic_stable_structures::storable::Bound::Unbounded;
}

impl ic_stable_structures::Storable for WithdrawalIntent {
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        Cow::Owned(candid::encode_one(self).expect("withdrawal intent Candid encoding"))
    }
    fn from_bytes(bytes: Cow<[u8]>) -> Self {
        candid::decode_one(&bytes).expect("withdrawal intent Candid decoding")
    }
    const BOUND: ic_stable_structures::storable::Bound =
        ic_stable_structures::storable::Bound::Unbounded;
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
    /// Legacy timestamp-only withdrawal records. They are retained for
    /// compatibility and block ID reuse; presence does not prove payment state.
    pub withdrawal_created_at: StableBTreeMap<u64, u64, Memory>,
    /// Permanent request identities and transfer tuples. Never pruned: an
    /// expired ambiguous request must not authorize a second ledger payment.
    pub withdrawal_intents: StableBTreeMap<u64, WithdrawalIntent, Memory>,
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
    pub external_token_receipts:
        StableBTreeMap<ExternalTokenReceiptKey, ExternalTokenDepositReceipt, Memory>,
    pub external_token_receipts_by_id: StableBTreeMap<u64, ExternalTokenDepositReceipt, Memory>,
    pub external_token_legacy_blocks:
        StableBTreeMap<ExternalTokenLegacyBlockKey, UnitValue, Memory>,
    pub external_token_backfill: StableCell<ExternalTokenDepositBackfill, Memory>,
    pub external_token_indexed_through: StableCell<u64, Memory>,
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

fn is_external_token_asset(asset_type: &AssetType) -> bool {
    matches!(asset_type, AssetType::CKUSDT | AssetType::CKUSDC)
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
                withdrawal_intents: StableBTreeMap::init(
                    memory_manager.get(MemoryId::new(MEM_WITHDRAWAL_INTENTS)),
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
                external_token_receipts: StableBTreeMap::init(
                    memory_manager.get(MemoryId::new(MEM_EXTERNAL_TOKEN_RECEIPTS)),
                ),
                external_token_receipts_by_id: StableBTreeMap::init(
                    memory_manager.get(MemoryId::new(MEM_EXTERNAL_TOKEN_RECEIPTS_BY_ID)),
                ),
                external_token_legacy_blocks: StableBTreeMap::init(
                    memory_manager.get(MemoryId::new(MEM_EXTERNAL_TOKEN_LEGACY_BLOCKS)),
                ),
                external_token_backfill: StableCell::init(
                    memory_manager.get(MemoryId::new(MEM_EXTERNAL_TOKEN_BACKFILL)),
                    ExternalTokenDepositBackfill::Complete,
                )
                .unwrap(),
                external_token_indexed_through: StableCell::init(
                    memory_manager.get(MemoryId::new(MEM_EXTERNAL_TOKEN_INDEXED_THROUGH)),
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
    fn balance_snapshot(balances: &HashMap<AssetType, AssetBalance>) -> BalancesSnapshot {
        BalancesSnapshot {
            entries: balances
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        }
    }

    fn persist_balances(&mut self) {
        let snapshot = Self::balance_snapshot(&self.balances);
        self.balances_cell
            .set(snapshot)
            .expect("persist treasury balances");
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
        self.next_event_id = event_id.checked_add(1).expect("treasury event ID overflow");
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

    /// Compatibility helper for unit fixtures. Production callers use the
    /// fallible form so overflow is rejected without partially changing state.
    pub fn add_deposit(&mut self, record: DepositRecord) -> u64 {
        self.try_add_deposit(record, true)
            .expect("test fixture deposit must fit treasury counters and balances")
    }

    /// Add one deposit only after all arithmetic preconditions pass. Legacy
    /// CKUSDT/CKUSDC rows are indexed as unknown-provenance block claims.
    pub fn try_add_deposit(
        &mut self,
        mut record: DepositRecord,
        legacy_external_receipt: bool,
    ) -> Result<u64, String> {
        let deposit_id = self.next_deposit_id;
        let next_deposit_id = deposit_id
            .checked_add(1)
            .ok_or_else(|| "treasury deposit ID overflow".to_string())?;
        let balance = self
            .balances
            .get(&record.asset_type)
            .ok_or_else(|| format!("treasury balance entry missing for {:?}", record.asset_type))?;
        let next_balance = AssetBalance {
            total: balance
                .total
                .checked_add(record.amount)
                .ok_or_else(|| format!("{:?} total balance overflow", record.asset_type))?,
            reserved: balance.reserved,
            available: balance
                .available
                .checked_add(record.amount)
                .ok_or_else(|| format!("{:?} available balance overflow", record.asset_type))?,
        };

        let mut next_balances = self.balances.clone();
        next_balances.insert(record.asset_type.clone(), next_balance.clone());
        self.balances_cell
            .set(Self::balance_snapshot(&next_balances))
            .map_err(|e| format!("failed to persist deposit balance: {:?}", e))?;

        let backfill_update = match self.external_token_backfill.get().clone() {
            ExternalTokenDepositBackfill::Complete => {
                if let Err(error) = self.external_token_indexed_through.set(deposit_id) {
                    panic!("failed to persist external token index watermark after balance write: {error:?}");
                }
                None
            }
            ExternalTokenDepositBackfill::InProgress {
                next_deposit_id: cursor,
                ..
            } => Some(ExternalTokenDepositBackfill::InProgress {
                next_deposit_id: cursor,
                snapshot_max_id: deposit_id,
            }),
            ExternalTokenDepositBackfill::Uninitialized => {
                let old_indexed = *self.external_token_indexed_through.get();
                Some(ExternalTokenDepositBackfill::InProgress {
                    next_deposit_id: old_indexed.checked_add(1).unwrap_or(old_indexed),
                    snapshot_max_id: deposit_id,
                })
            }
        };
        if let Some(status) = backfill_update {
            if let Err(error) = self.external_token_backfill.set(status) {
                panic!("failed to persist external token backfill state after balance write: {error:?}");
            }
        }

        record.id = deposit_id;
        let external_asset = is_external_token_asset(&record.asset_type);
        let asset_type = record.asset_type.clone();
        let block_index = record.block_index;

        self.deposits.insert(deposit_id, record);
        self.next_deposit_id = next_deposit_id;
        self.balances.insert(asset_type.clone(), next_balance);
        if external_asset && legacy_external_receipt {
            self.external_token_legacy_blocks
                .insert(ExternalTokenLegacyBlockKey { block_index }, UnitValue);
        }

        if matches!(
            self.icusd_deposit_block_backfill.get(),
            IcusdDepositBlockBackfill::Complete
        ) {
            let _ = self.icusd_deposit_indexed_through.set(deposit_id);
        }
        Ok(deposit_id)
    }

    pub fn record_external_token_deposit_once(
        &mut self,
        args: ExternalTokenDepositArgs,
        timestamp: u64,
    ) -> Result<(u64, bool), String> {
        match &args.asset_type {
            AssetType::CKUSDT | AssetType::CKUSDC => {}
            _ => return Err("external receipt accepts only CKUSDT or CKUSDC".to_string()),
        }
        if !matches!(
            &args.deposit_type,
            crate::types::DepositType::InterestRevenue
                | crate::types::DepositType::LiquidationFee
                | crate::types::DepositType::RedemptionFee
        ) {
            return Err("CKUSDT/CKUSDC receipts accept only InterestRevenue, LiquidationFee, or RedemptionFee".to_string());
        }
        if args.amount == 0 {
            return Err("deposit amount must be greater than zero".to_string());
        }
        if !matches!(
            self.external_token_backfill.get(),
            ExternalTokenDepositBackfill::Complete
        ) {
            return Err("external token deposit block backfill is in progress".to_string());
        }
        if self
            .external_token_legacy_blocks
            .contains_key(&ExternalTokenLegacyBlockKey {
                block_index: args.block_index,
            })
        {
            return Err(format!(
                "external token block {} has unknown legacy ledger provenance",
                args.block_index
            ));
        }

        let key = ExternalTokenReceiptKey {
            ledger: args.ledger.clone(),
            block_index: args.block_index,
        };
        if let Some(existing) = self.external_token_receipts.get(&key) {
            if existing.deposit_id == 0 {
                return Err("external token receipt index is marked ambiguous".to_string());
            }
            if existing.ledger != args.ledger
                || existing.deposit_type != args.deposit_type
                || existing.asset_type != args.asset_type
                || existing.amount != args.amount
                || existing.block_index != args.block_index
                || existing.memo != args.memo
            {
                return Err(format!(
                    "external token block {} conflicts with its existing receipt",
                    args.block_index
                ));
            }
            let deposit = self.deposits.get(&existing.deposit_id).ok_or_else(|| {
                format!(
                    "external token receipt points to missing deposit {}",
                    existing.deposit_id
                )
            })?;
            if deposit.asset_type != existing.asset_type
                || deposit.deposit_type != existing.deposit_type
                || deposit.amount != existing.amount
                || deposit.block_index != existing.block_index
                || deposit.memo != existing.memo
            {
                return Err(
                    "external token receipt index conflicts with deposit record".to_string()
                );
            }
            return Ok((existing.deposit_id, false));
        }

        let configured_ledger = match &args.asset_type {
            AssetType::CKUSDT => self.get_config().ckusdt_ledger,
            AssetType::CKUSDC => self.get_config().ckusdc_ledger,
            _ => unreachable!("external asset was validated above"),
        }
        .ok_or_else(|| format!("configured ledger missing for {:?}", args.asset_type))?;
        if args.ledger != configured_ledger {
            return Err(format!(
                "ledger does not match configured {:?} ledger",
                args.asset_type
            ));
        }

        // The endpoint emits exactly one event for a new receipt. Preflight
        // its ID before any balance or receipt state is written.
        self.next_event_id
            .checked_add(1)
            .ok_or_else(|| "treasury event ID overflow".to_string())?;

        let record = DepositRecord {
            id: 0,
            deposit_type: args.deposit_type.clone(),
            asset_type: args.asset_type.clone(),
            amount: args.amount,
            block_index: args.block_index,
            timestamp,
            memo: args.memo.clone(),
        };
        let deposit_id = self.try_add_deposit(record, false)?;
        let receipt = ExternalTokenDepositReceipt {
            deposit_id,
            ledger: args.ledger,
            deposit_type: args.deposit_type,
            asset_type: args.asset_type,
            amount: args.amount,
            block_index: args.block_index,
            memo: args.memo,
        };
        self.external_token_receipts.insert(key, receipt.clone());
        self.external_token_receipts_by_id
            .insert(deposit_id, receipt);
        Ok((deposit_id, true))
    }

    pub fn continue_external_token_deposit_backfill(&mut self) -> Result<(u64, u64, bool), String> {
        let (mut cursor, snapshot_max_id) = match self.external_token_backfill.get().clone() {
            ExternalTokenDepositBackfill::Uninitialized => {
                return Err("external token backfill was not initialized".to_string())
            }
            ExternalTokenDepositBackfill::InProgress {
                next_deposit_id,
                snapshot_max_id,
            } => (next_deposit_id, snapshot_max_id),
            ExternalTokenDepositBackfill::Complete => return Ok((0, 0, true)),
        };
        let rows: Vec<(u64, DepositRecord)> = self
            .deposits
            .range(cursor..=snapshot_max_id)
            .take(EXTERNAL_TOKEN_BLOCK_BACKFILL_BATCH_SIZE)
            .collect();
        let processed = rows.len() as u64;
        let last_id = rows.last().map(|(id, _)| *id);
        for (deposit_id, record) in &rows {
            if is_external_token_asset(&record.asset_type) {
                let receipt_matches = self
                    .external_token_receipts_by_id
                    .get(deposit_id)
                    .map(|receipt| {
                        receipt.deposit_id == *deposit_id
                            && receipt.asset_type == record.asset_type
                            && receipt.deposit_type == record.deposit_type
                            && receipt.amount == record.amount
                            && receipt.block_index == record.block_index
                            && receipt.memo == record.memo
                    })
                    .unwrap_or(false);
                // Historical DepositRecord has no ledger principal. Preserve
                // the fact that provenance is unknown instead of guessing the
                // current configured ledger. Exact receipts from this version
                // retain their stronger ledger-scoped provenance.
                if !receipt_matches {
                    self.external_token_legacy_blocks.insert(
                        ExternalTokenLegacyBlockKey {
                            block_index: record.block_index,
                        },
                        UnitValue,
                    );
                }
            }
        }
        if let Some(id) = last_id {
            cursor = id.checked_add(1).unwrap_or(id);
        }
        let batch_complete =
            processed == 0 || last_id.map(|id| id >= snapshot_max_id).unwrap_or(false);
        let current_max_id = self
            .deposits
            .last_key_value()
            .map(|(id, _)| id)
            .unwrap_or(0);
        let status = if batch_complete && current_max_id <= snapshot_max_id {
            self.external_token_indexed_through
                .set(snapshot_max_id)
                .expect("persist external token index watermark");
            ExternalTokenDepositBackfill::Complete
        } else if batch_complete {
            ExternalTokenDepositBackfill::InProgress {
                next_deposit_id: snapshot_max_id.checked_add(1).unwrap_or(snapshot_max_id),
                snapshot_max_id: current_max_id,
            }
        } else {
            ExternalTokenDepositBackfill::InProgress {
                next_deposit_id: cursor,
                snapshot_max_id,
            }
        };
        let complete = matches!(status, ExternalTokenDepositBackfill::Complete);
        self.external_token_backfill
            .set(status)
            .expect("persist external token block backfill state");
        Ok((processed, cursor, complete))
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
        let deposit_id = self.try_add_deposit(record, false)?;
        self.icusd_deposit_blocks.insert(block_index, deposit_id);
        let _ = self.icusd_deposit_indexed_through.set(deposit_id);
        Ok((deposit_id, true))
    }

    /// Record an InterestRevenue/ICUSD ledger deposit with exact-block replay
    /// protection. This is intentionally separate from the legacy deposit
    /// path and from BorrowingFee idempotency: only the same physical block,
    /// amount, and memo can return the original deposit ID.
    pub fn record_interest_revenue_once(
        &mut self,
        record: DepositRecord,
    ) -> Result<(u64, bool), String> {
        if record.deposit_type != crate::types::DepositType::InterestRevenue
            || record.asset_type != AssetType::ICUSD
        {
            return Err("only InterestRevenue/ICUSD deposits are accepted".to_string());
        }
        if !matches!(
            self.icusd_deposit_block_backfill.get(),
            IcusdDepositBlockBackfill::Complete
        ) {
            return Err("ICUSD deposit block index backfill is in progress".to_string());
        }

        if let Some(existing_id) = self.icusd_deposit_blocks.get(&record.block_index) {
            if existing_id == AMBIGUOUS_BLOCK_INDEX {
                return Err(format!(
                    "ICUSD block {} is ambiguous in treasury history",
                    record.block_index
                ));
            }
            let existing = self.deposits.get(&existing_id).ok_or_else(|| {
                format!(
                    "ICUSD block {} points to missing treasury deposit {}",
                    record.block_index, existing_id
                )
            })?;
            if existing.deposit_type == crate::types::DepositType::InterestRevenue
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

        // The legacy Stability Pool index must not be silently bypassed if
        // the shared physical-block index is inconsistent.
        if self
            .sp_unallocated_interest_transfer_blocks
            .contains_key(&record.block_index)
        {
            return Err(format!(
                "ICUSD block {} is already recorded as a Stability Pool deposit",
                record.block_index
            ));
        }

        let block_index = record.block_index;
        let deposit_id = self.try_add_deposit(record, false)?;
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
            cursor = last_processed_id
                .checked_add(1)
                .unwrap_or(last_processed_id);
        }
        let complete = processed == 0
            || last_processed_id
                .map(|last_id| last_id >= snapshot_max_id)
                .unwrap_or(false);
        let current_max_id = self
            .deposits
            .last_key_value()
            .map(|(id, _)| id)
            .unwrap_or(0);
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

    /// Atomically verifies request identity, reserves the balance once, and
    /// installs its permanent journal row. Existing rows are returned without
    /// another debit; timestamp-only legacy rows cannot prove no payment.
    pub fn reserve_withdrawal_intent(
        &mut self,
        intent: WithdrawalIntent,
    ) -> Result<(WithdrawalIntent, bool), String> {
        if let Some(existing) = self.withdrawal_intents.get(&intent.request_id) {
            if existing.caller != intent.caller
                || existing.asset_type != intent.asset_type
                || existing.amount != intent.amount
                || existing.to != intent.to
                || existing.requested_memo != intent.requested_memo
            {
                return Err("Withdrawal request_id is already bound to a different request".into());
            }
            return Ok((existing, false));
        }
        if self.active_withdrawal_intent().is_some() {
            return Err("An unresolved withdrawal already exists; operator reconciliation is required before another withdrawal".into());
        }
        if self.withdrawal_created_at.get(&intent.request_id).is_some() {
            return Err(
                "Legacy withdrawal timestamp exists without payment proof; request is held".into(),
            );
        }
        if self.withdrawal_intents.len() >= MAX_WITHDRAWAL_INTENTS {
            return Err("Withdrawal intent journal is full; operator recovery is required".into());
        }

        self.withdraw(intent.asset_type.clone(), intent.amount)?;
        self.withdrawal_created_at
            .insert(intent.request_id, intent.created_at_time);
        self.withdrawal_intents
            .insert(intent.request_id, intent.clone());
        Ok((intent, true))
    }

    pub fn withdrawal_intent(&self, request_id: u64) -> Option<WithdrawalIntent> {
        self.withdrawal_intents.get(&request_id)
    }

    /// At most one nonterminal withdrawal may exist. New IDs are blocked until
    /// an unresolved payment is positively reconciled by an operator.
    pub fn active_withdrawal_intent(&self) -> Option<WithdrawalIntent> {
        self.withdrawal_intents.iter().find_map(|(_, intent)| {
            (!matches!(
                intent.status,
                crate::types::WithdrawalIntentStatus::Completed(_)
                    | crate::types::WithdrawalIntentStatus::Rejected { .. }
            ))
            .then_some(intent)
        })
    }

    pub fn set_withdrawal_intent(&mut self, intent: WithdrawalIntent) {
        self.withdrawal_intents.insert(intent.request_id, intent);
    }

    /// Record confirmed completion and append exactly one audit event.
    pub fn complete_withdrawal_intent(
        &mut self,
        request_id: u64,
        result: crate::types::WithdrawResult,
        caller: Principal,
        now: u64,
    ) -> Result<crate::types::WithdrawResult, String> {
        let mut intent = self
            .withdrawal_intents
            .get(&request_id)
            .ok_or_else(|| "Withdrawal intent disappeared".to_string())?;
        if intent.caller != caller {
            return Err("Withdrawal completion caller does not match the intent".into());
        }
        if let crate::types::WithdrawalIntentStatus::Completed(ref existing) = intent.status {
            return Ok(existing.clone());
        }
        if !matches!(
            intent.status,
            crate::types::WithdrawalIntentStatus::InFlight
                | crate::types::WithdrawalIntentStatus::TransportAmbiguous
        ) {
            return Err(
                "Withdrawal intent cannot transition to completed from its current state".into(),
            );
        }
        intent.may_have_committed = true;
        intent.status = crate::types::WithdrawalIntentStatus::Completed(result.clone());
        self.withdrawal_intents.insert(request_id, intent.clone());
        self.push_event_at(
            caller,
            TreasuryAction::Withdraw {
                asset_type: intent.asset_type,
                amount: intent.amount,
                to: intent.to,
            },
            now,
        );
        Ok(result)
    }

    /// A typed ledger rejection proves the submitted transfer did not apply.
    /// Restore only once, and make this request ID terminal; a caller wishing
    /// to try again must submit a fresh ID. Never clear a prior ambiguity.
    pub fn reject_withdrawal_intent(&mut self, request_id: u64, error: String) {
        if let Some(mut intent) = self.withdrawal_intents.get(&request_id) {
            if !intent.may_have_committed
                && matches!(
                    intent.status,
                    crate::types::WithdrawalIntentStatus::InFlight
                )
            {
                self.restore_balance(&intent.asset_type, intent.amount);
                intent.status = crate::types::WithdrawalIntentStatus::Rejected { error };
                self.withdrawal_intents.insert(request_id, intent);
            }
        }
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
            let withdrawal_intents: StableBTreeMap<u64, WithdrawalIntent, Memory> =
                StableBTreeMap::init(memory_manager.get(MemoryId::new(MEM_WITHDRAWAL_INTENTS)));
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
            let external_token_receipts = StableBTreeMap::init(
                memory_manager.get(MemoryId::new(MEM_EXTERNAL_TOKEN_RECEIPTS)),
            );
            let external_token_receipts_by_id = StableBTreeMap::init(
                memory_manager.get(MemoryId::new(MEM_EXTERNAL_TOKEN_RECEIPTS_BY_ID)),
            );
            let external_token_legacy_blocks = StableBTreeMap::init(
                memory_manager.get(MemoryId::new(MEM_EXTERNAL_TOKEN_LEGACY_BLOCKS)),
            );
            let mut external_token_backfill: StableCell<ExternalTokenDepositBackfill, Memory> =
                StableCell::init(
                    memory_manager.get(MemoryId::new(MEM_EXTERNAL_TOKEN_BACKFILL)),
                    ExternalTokenDepositBackfill::Uninitialized,
                )
                .unwrap();
            let external_token_indexed_through: StableCell<u64, Memory> = StableCell::init(
                memory_manager.get(MemoryId::new(MEM_EXTERNAL_TOKEN_INDEXED_THROUGH)),
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

            if matches!(
                external_token_backfill.get(),
                ExternalTokenDepositBackfill::Uninitialized
            ) {
                let indexed_through = *external_token_indexed_through.get();
                let initial_status = if max_id <= indexed_through {
                    ExternalTokenDepositBackfill::Complete
                } else {
                    ExternalTokenDepositBackfill::InProgress {
                        next_deposit_id: indexed_through.checked_add(1).unwrap_or(indexed_through),
                        snapshot_max_id: max_id,
                    }
                };
                external_token_backfill
                    .set(initial_status)
                    .expect("persist initial external token block backfill state");
            } else if matches!(
                external_token_backfill.get(),
                ExternalTokenDepositBackfill::Complete
            ) && max_id > *external_token_indexed_through.get()
            {
                // A downgrade to code without this index may have appended
                // deposits. Re-scan from the last known complete watermark.
                let indexed = *external_token_indexed_through.get();
                external_token_backfill
                    .set(ExternalTokenDepositBackfill::InProgress {
                        next_deposit_id: indexed.checked_add(1).unwrap_or(indexed),
                        snapshot_max_id: max_id,
                    })
                    .expect("persist external token rollback backfill state");
            } else if let ExternalTokenDepositBackfill::InProgress {
                next_deposit_id,
                snapshot_max_id,
            } = external_token_backfill.get().clone()
            {
                if max_id > snapshot_max_id {
                    external_token_backfill
                        .set(ExternalTokenDepositBackfill::InProgress {
                            next_deposit_id,
                            snapshot_max_id: max_id,
                        })
                        .expect("extend external token backfill snapshot after rollback");
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
                withdrawal_intents,
                sp_unallocated_interest_blocks,
                sp_unallocated_interest_transfer_blocks,
                icusd_deposit_blocks,
                icusd_deposit_block_backfill,
                icusd_deposit_indexed_through,
                external_token_receipts,
                external_token_receipts_by_id,
                external_token_legacy_blocks,
                external_token_backfill,
                external_token_indexed_through,
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
