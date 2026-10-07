use crate::types::{
    AssetBalance, AssetType, BalancesSnapshot, DepositRecord, DepositRecordV1, DepositType,
    PendingWithdrawalV2, PendingWithdrawalV3, PendingWithdrawalsPageV2, PendingWithdrawalsPageV3,
    TreasuryAction, TreasuryEvent, TreasuryEventV1, TreasuryInitArgs,
    UnknownTreasuryEvidencePageV2, UnknownTreasuryEvidenceV2,
};
use candid::Principal;
use ic_stable_structures::memory_manager::{MemoryId, MemoryManager, VirtualMemory};
use ic_stable_structures::{DefaultMemoryImpl, StableBTreeMap, StableCell};
use std::cell::RefCell;
use std::collections::HashMap;

type Memory = VirtualMemory<DefaultMemoryImpl>;

fn event_has_other_asset(event: &TreasuryEvent) -> bool {
    match &event.action {
        TreasuryAction::Deposit { asset_type, .. }
        | TreasuryAction::Withdraw { asset_type, .. } => {
            matches!(asset_type, crate::types::AssetType::Other(_))
        }
        TreasuryAction::SetPaused { .. } | TreasuryAction::LegacyUnknown { .. } => false,
    }
}

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
const MEM_DEPOSIT_RECEIPTS: u8 = 7; // StableBTreeMap<DepositReceiptKey, DepositReceipt> (asset + ledger block → deposit)
const MEM_DEPOSIT_RECEIPT_MIGRATION: u8 = 8; // StableCell<u8> (deposit receipt index migration version)
const MEM_WITHDRAWAL_REQUESTS: u8 = 9; // StableBTreeMap<u64, WithdrawalRequestRecord>
const MEM_WITHDRAWAL_REQUEST_MIGRATION: u8 = 10; // StableCell<u8> (legacy request quarantine version)

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
    (MEM_DEPOSIT_RECEIPTS, "deposit_receipts"),
    (MEM_DEPOSIT_RECEIPT_MIGRATION, "deposit_receipt_migration"),
    (MEM_WITHDRAWAL_REQUESTS, "withdrawal_requests"),
    (
        MEM_WITHDRAWAL_REQUEST_MIGRATION,
        "withdrawal_request_migration",
    ),
];

/// Stable key for the physical receipt associated with a treasury deposit.
/// Asset tags are explicit and permanent; do not reorder or reuse them.
#[derive(
    candid::CandidType,
    serde::Serialize,
    serde::Deserialize,
    Clone,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
)]
struct DepositReceiptKey {
    asset_tag: u8,
    block_index: u64,
    /// Present only for `Other` assets. Optional so keys written before this
    /// field was introduced continue to decode from stable memory.
    #[serde(default)]
    other_ledger: Option<Principal>,
}

impl DepositReceiptKey {
    fn new(asset_type: &AssetType, block_index: u64) -> Self {
        let (asset_tag, other_ledger) = match asset_type {
            AssetType::ICUSD => (0, None),
            AssetType::ICP => (1, None),
            AssetType::CKBTC => (2, None),
            AssetType::CKUSDT => (3, None),
            AssetType::CKUSDC => (4, None),
            AssetType::Other(ledger) => (5, Some(*ledger)),
        };
        Self {
            asset_tag,
            block_index,
            other_ledger,
        }
    }
}

#[cfg(test)]
mod deposit_receipt_key_compat_tests {
    use super::DepositReceiptKey;
    use candid::{CandidType, Deserialize};

    #[derive(CandidType, Deserialize)]
    struct PreviousDepositReceiptKey {
        asset_tag: u8,
        block_index: u64,
    }

    #[test]
    fn previous_receipt_key_decodes_with_no_other_ledger() {
        let old = PreviousDepositReceiptKey {
            asset_tag: 4,
            block_index: 99,
        };
        let bytes = candid::encode_one(old).unwrap();
        let decoded: DepositReceiptKey = candid::decode_one(&bytes).unwrap();
        assert_eq!(decoded.asset_tag, 4);
        assert_eq!(decoded.block_index, 99);
        assert_eq!(decoded.other_ledger, None);
    }
}

#[derive(candid::CandidType, serde::Serialize, serde::Deserialize, Clone, Debug)]
struct DepositReceipt {
    deposit_id: u64,
    amount: u64,
    deposit_type: crate::types::DepositType,
    memo: Option<String>,
    /// Set when multiple records share this physical receipt. Such a receipt
    /// cannot safely be treated as an idempotent replay.
    #[serde(default)]
    ambiguous_conflict: bool,
}

#[derive(candid::CandidType, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum WithdrawalRequestStatus {
    Pending,
    Complete {
        block_index: u64,
    },
    /// Pre-upgrade timestamp-only rows cannot be bound to a transfer tuple or
    /// settlement result. Keep their request IDs quarantined for reconciliation.
    LegacyUnknown,
}

#[derive(candid::CandidType, serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct WithdrawalRequestRecord {
    pub caller: Principal,
    pub asset_type: AssetType,
    pub ledger: Principal,
    pub amount: u64,
    pub to: Principal,
    pub memo: Option<String>,
    pub created_at_time: u64,
    pub send_amount: u64,
    pub fee: u64,
    pub status: WithdrawalRequestStatus,
    /// Durable count of known dispatch attempts. None means an older schema
    /// or interrupted migration leaves prior dispatch history uncertain.
    #[serde(default)]
    pub dispatch_attempts: Option<u32>,
}

#[derive(Debug)]
pub enum WithdrawalStart {
    Transfer(WithdrawalRequestRecord),
    Retry(WithdrawalRequestRecord),
    Complete {
        record: WithdrawalRequestRecord,
        block_index: u64,
    },
}

impl ic_stable_structures::Storable for DepositReceiptKey {
    fn to_bytes(&self) -> std::borrow::Cow<'_, [u8]> {
        std::borrow::Cow::Owned(candid::encode_one(self).unwrap())
    }

    fn from_bytes(bytes: std::borrow::Cow<[u8]>) -> Self {
        candid::decode_one(&bytes).unwrap()
    }

    const BOUND: ic_stable_structures::storable::Bound =
        ic_stable_structures::storable::Bound::Unbounded;
}

impl ic_stable_structures::Storable for DepositReceipt {
    fn to_bytes(&self) -> std::borrow::Cow<'_, [u8]> {
        std::borrow::Cow::Owned(candid::encode_one(self).unwrap())
    }

    fn from_bytes(bytes: std::borrow::Cow<[u8]>) -> Self {
        candid::decode_one(&bytes).unwrap()
    }

    const BOUND: ic_stable_structures::storable::Bound =
        ic_stable_structures::storable::Bound::Unbounded;
}

impl ic_stable_structures::Storable for WithdrawalRequestRecord {
    fn to_bytes(&self) -> std::borrow::Cow<'_, [u8]> {
        std::borrow::Cow::Owned(candid::encode_one(self).unwrap())
    }

    fn from_bytes(bytes: std::borrow::Cow<[u8]>) -> Self {
        candid::decode_one(&bytes).unwrap()
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
    /// Physical deposit receipt to original deposit ID and amount.
    deposit_receipts: StableBTreeMap<DepositReceiptKey, DepositReceipt, Memory>,
    /// Exact withdrawal request identity and terminal/pending state. Retained
    /// permanently so an old request ID can never debit a second time.
    pub withdrawal_requests: StableBTreeMap<u64, WithdrawalRequestRecord, Memory>,
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
        candid::decode_one(&bytes).unwrap_or_else(|_| {
            candid::decode_one::<LegacyDepositRecord>(&bytes)
                .map(Into::into)
                .unwrap_or_else(|_| {
                    // Candid 0.10's dynamic variant decoder rejects some
                    // future variant tables. Preserve the complete original
                    // bytes as evidence and decode to a zero-value sentinel;
                    // withdrawals are globally held while this sentinel exists.
                    let candid_hex = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
                    DepositRecord {
                        id: 0,
                        deposit_type: crate::types::DepositType::LegacyUnknown { candid_hex },
                        asset_type: AssetType::ICUSD,
                        amount: 0,
                        block_index: 0,
                        timestamp: 0,
                        memo: Some(
                            "Unrecognized stable deposit; original bytes retained in deposit_type"
                                .into(),
                        ),
                    }
                })
        })
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
        candid::decode_one(&bytes).unwrap_or_else(|_| {
            match candid::decode_one::<LegacyTreasuryEvent>(&bytes) {
                Ok(legacy) => legacy.into(),
                Err(_) => TreasuryEvent {
                    id: 0,
                    timestamp: 0,
                    caller: Principal::anonymous(),
                    action: TreasuryAction::LegacyUnknown {
                        candid_value: bytes.iter().map(|byte| format!("{byte:02x}")).collect(),
                    },
                },
            }
        })
    }

    const BOUND: ic_stable_structures::storable::Bound =
        ic_stable_structures::storable::Bound::Unbounded;
}

/// Candid schema used before the DepositType variant rename. Serde aliases do
/// not preserve Candid variant field IDs, so stable values need this explicit
/// wire decoder as long as pre-rename records can remain in stable memory.
#[derive(candid::CandidType, serde::Serialize, serde::Deserialize)]
enum LegacyDepositType {
    MintingFee,
    RedemptionFee,
    LiquidationSurplus,
    StabilityFee,
}

impl From<LegacyDepositType> for crate::types::DepositType {
    fn from(value: LegacyDepositType) -> Self {
        match value {
            LegacyDepositType::MintingFee => Self::BorrowingFee,
            LegacyDepositType::RedemptionFee => Self::RedemptionFee,
            LegacyDepositType::LiquidationSurplus => Self::LiquidationFee,
            LegacyDepositType::StabilityFee => Self::InterestRevenue,
        }
    }
}

#[derive(candid::CandidType, serde::Serialize, serde::Deserialize)]
struct LegacyDepositRecord {
    id: u64,
    deposit_type: LegacyDepositType,
    asset_type: AssetType,
    amount: u64,
    block_index: u64,
    timestamp: u64,
    memo: Option<String>,
}

impl From<LegacyDepositRecord> for DepositRecord {
    fn from(value: LegacyDepositRecord) -> Self {
        Self {
            id: value.id,
            deposit_type: value.deposit_type.into(),
            asset_type: value.asset_type,
            amount: value.amount,
            block_index: value.block_index,
            timestamp: value.timestamp,
            memo: value.memo,
        }
    }
}

#[derive(candid::CandidType, serde::Serialize, serde::Deserialize)]
enum LegacyTreasuryAction {
    Deposit {
        deposit_type: LegacyDepositType,
        asset_type: AssetType,
        amount: u64,
    },
    Withdraw {
        asset_type: AssetType,
        amount: u64,
        to: Principal,
    },
    SetPaused {
        paused: bool,
    },
}

#[derive(candid::CandidType, serde::Serialize, serde::Deserialize)]
struct LegacyTreasuryEvent {
    id: u64,
    timestamp: u64,
    caller: Principal,
    action: LegacyTreasuryAction,
}

impl From<LegacyTreasuryEvent> for TreasuryEvent {
    fn from(value: LegacyTreasuryEvent) -> Self {
        let action = match value.action {
            LegacyTreasuryAction::Deposit {
                deposit_type,
                asset_type,
                amount,
            } => TreasuryAction::Deposit {
                deposit_type: deposit_type.into(),
                asset_type,
                amount,
            },
            LegacyTreasuryAction::Withdraw {
                asset_type,
                amount,
                to,
            } => TreasuryAction::Withdraw {
                asset_type,
                amount,
                to,
            },
            LegacyTreasuryAction::SetPaused { paused } => TreasuryAction::SetPaused { paused },
        };
        Self {
            id: value.id,
            timestamp: value.timestamp,
            caller: value.caller,
            action,
        }
    }
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
            let _deposit_receipt_migration: StableCell<u8, Memory> = StableCell::init(
                memory_manager.get(MemoryId::new(MEM_DEPOSIT_RECEIPT_MIGRATION)),
                1,
            )
            .unwrap();
            let _withdrawal_request_migration: StableCell<u8, Memory> = StableCell::init(
                memory_manager.get(MemoryId::new(MEM_WITHDRAWAL_REQUEST_MIGRATION)),
                1,
            )
            .unwrap();

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
                deposit_receipts: StableBTreeMap::init(
                    memory_manager.get(MemoryId::new(MEM_DEPOSIT_RECEIPTS)),
                ),
                withdrawal_requests: StableBTreeMap::init(
                    memory_manager.get(MemoryId::new(MEM_WITHDRAWAL_REQUESTS)),
                ),
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
        let event_id = self.next_event_id;
        self.next_event_id += 1;
        self.events.insert(
            event_id,
            TreasuryEvent {
                id: event_id,
                timestamp: ic_cdk::api::time(),
                caller,
                action,
            },
        );
    }

    /// Get events (paginated).
    pub fn get_events(&self, start: Option<u64>, limit: usize) -> Vec<TreasuryEventV1> {
        let start_key = start.unwrap_or(0);
        self.events
            .range(start_key..)
            .filter(|(_, event)| {
                !matches!(event.action, TreasuryAction::LegacyUnknown { .. })
                    && !event_has_other_asset(event)
            })
            .take(limit)
            .map(|(_, event)| TreasuryEventV1::from(event))
            .collect()
    }

    /// Full event projection for callers that support address-bound assets.
    pub fn get_events_v2(&self, start: Option<u64>, limit: usize) -> Vec<TreasuryEvent> {
        let start_key = start.unwrap_or(0);
        self.events
            .range(start_key..)
            .filter(|(_, event)| !matches!(event.action, TreasuryAction::LegacyUnknown { .. }))
            .take(limit)
            .map(|(_, event)| event)
            .collect()
    }

    /// Get total events count.
    pub fn get_events_count(&self) -> u64 {
        self.events
            .iter()
            .filter(|(_, event)| !matches!(event.action, TreasuryAction::LegacyUnknown { .. }))
            .count() as u64
    }

    // ------------------------------------------------------------------
    // Deposits / withdrawals
    // ------------------------------------------------------------------

    /// Add a new deposit record and update balances.
    pub fn add_deposit(&mut self, record: DepositRecord) -> u64 {
        let deposit_id = self.next_deposit_id;
        self.next_deposit_id += 1;
        let receipt_key = DepositReceiptKey::new(&record.asset_type, record.block_index);
        let receipt = DepositReceipt {
            deposit_id,
            amount: record.amount,
            deposit_type: record.deposit_type.clone(),
            memo: record.memo.clone(),
            ambiguous_conflict: false,
        };

        // Update balance for this asset type
        let balance = self.balances.entry(record.asset_type.clone()).or_default();
        balance.total += record.amount;
        balance.available += record.amount;

        // Store the deposit record
        let mut final_record = record;
        final_record.id = deposit_id;
        self.deposits.insert(deposit_id, final_record);

        match self.deposit_receipts.get(&receipt_key) {
            Some(mut existing) => {
                existing.ambiguous_conflict = true;
                self.deposit_receipts.insert(receipt_key, existing);
            }
            None => {
                self.deposit_receipts.insert(receipt_key, receipt);
            }
        }

        self.persist_balances();
        deposit_id
    }

    /// Record a deposit exactly once for an asset ledger receipt. A replay
    /// must preserve amount, type, and memo; timestamp is observational only.
    pub fn add_deposit_once(&mut self, record: DepositRecord) -> Result<(u64, bool), String> {
        if matches!(record.deposit_type, DepositType::LegacyUnknown { .. }) {
            return Err("unrecognized stable deposit evidence cannot be credited".into());
        }
        let key = DepositReceiptKey::new(&record.asset_type, record.block_index);
        if let Some(receipt) = self.deposit_receipts.get(&key) {
            if receipt.ambiguous_conflict {
                return Err(format!(
                    "deposit receipt ({:?}, {}) is ambiguous in legacy records",
                    record.asset_type, record.block_index
                ));
            }
            if receipt.amount != record.amount
                || receipt.deposit_type != record.deposit_type
                || receipt.memo != record.memo
            {
                return Err(format!(
                    "deposit receipt ({:?}, {}) conflicts with its recorded amount, type, or memo",
                    record.asset_type, record.block_index
                ));
            }
            return Ok((receipt.deposit_id, false));
        }

        let deposit_id = self.add_deposit(record);
        Ok((deposit_id, true))
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
        let existing: Vec<u64> = source_mint_blocks
            .iter()
            .filter_map(|block| self.sp_unallocated_interest_blocks.get(block))
            .collect();
        if !existing.is_empty() {
            if existing.len() == source_mint_blocks.len()
                && existing.iter().all(|id| *id == existing[0])
            {
                return Ok((existing[0], false));
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

        let deposit_id = self.add_deposit(DepositRecord {
            id: 0,
            deposit_type: crate::types::DepositType::InterestRevenue,
            asset_type: crate::types::AssetType::ICUSD,
            amount,
            block_index: transfer_block_index,
            timestamp: now,
            memo: Some("stability-pool unallocated interest".to_string()),
        });
        for source_mint_block in source_mint_blocks {
            self.sp_unallocated_interest_blocks
                .insert(*source_mint_block, deposit_id);
        }
        self.sp_unallocated_interest_transfer_blocks
            .insert(transfer_block_index, deposit_id);
        Ok((deposit_id, true))
    }

    fn has_ambiguous_deposit_receipt(&self, asset_type: &AssetType) -> bool {
        let key = DepositReceiptKey::new(asset_type, 0);
        self.deposit_receipts.iter().any(|(candidate, receipt)| {
            candidate.asset_tag == key.asset_tag
                && candidate.other_ledger == key.other_ledger
                && receipt.ambiguous_conflict
        })
    }

    fn has_unrecognized_deposit(&self) -> bool {
        self.deposits.iter().any(|(_, record)| {
            matches!(
                record.deposit_type,
                crate::types::DepositType::LegacyUnknown { .. }
            )
        })
    }

    /// Reserve `amount` from bookkeeping before attempting a withdrawal transfer.
    pub fn withdraw(&mut self, asset_type: AssetType, amount: u64) -> Result<(), String> {
        if self.has_unrecognized_deposit() || self.has_ambiguous_deposit_receipt(&asset_type) {
            return Err(format!(
                "Withdrawals are held because a deposit record or ledger receipt is unrecognized, duplicate, or conflicting; reconcile stable history before clearing the hold (requested asset: {:?})",
                asset_type
            ));
        }
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

    /// Reserve a withdrawal exactly once for its durable request ID. Exact
    /// pending retries reuse the original wire tuple without another debit;
    /// completed retries return the original ledger block. Reusing an ID for
    /// a different tuple is rejected.
    pub fn begin_withdrawal(
        &mut self,
        request_id: u64,
        mut requested: WithdrawalRequestRecord,
    ) -> Result<WithdrawalStart, String> {
        match self.withdrawal_requests.get(&request_id) {
            Some(existing) => {
                if existing.status == WithdrawalRequestStatus::LegacyUnknown {
                    return Err(format!(
                        "Withdrawal request {} predates exact request tracking and requires reconciliation",
                        request_id
                    ));
                }
                if existing.caller != requested.caller
                    || existing.asset_type != requested.asset_type
                    || existing.ledger != requested.ledger
                    || existing.amount != requested.amount
                    || existing.to != requested.to
                    || existing.memo != requested.memo
                {
                    return Err(format!(
                        "Withdrawal request_id {} is already bound to a different transfer",
                        request_id
                    ));
                }
                return Ok(match existing.status.clone() {
                    WithdrawalRequestStatus::Complete { block_index } => {
                        WithdrawalStart::Complete {
                            record: existing,
                            block_index,
                        }
                    }
                    WithdrawalRequestStatus::Pending => WithdrawalStart::Retry(existing),
                    WithdrawalRequestStatus::LegacyUnknown => unreachable!(),
                });
            }
            None => {}
        }

        // Existing exact pending retries are returned above for reconciliation.
        // New reservations must obey the same migrated-history holds as the
        // legacy withdraw helper so ambiguous credits cannot fund fresh sends.
        if self.has_unrecognized_deposit()
            || self.has_ambiguous_deposit_receipt(&requested.asset_type)
        {
            return Err(format!(
                "New withdrawals for {:?} are held because stable deposit history is unrecognized, duplicate, or conflicting",
                requested.asset_type
            ));
        }

        let balance = self
            .balances
            .get_mut(&requested.asset_type)
            .ok_or_else(|| format!("Unknown asset type: {:?}", requested.asset_type))?;
        if balance.available < requested.amount {
            return Err(format!(
                "Insufficient balance. Available: {}, requested: {}",
                balance.available, requested.amount
            ));
        }
        balance.total -= requested.amount;
        balance.available -= requested.amount;
        self.persist_balances();

        requested.status = WithdrawalRequestStatus::Pending;
        requested.dispatch_attempts = Some(0);
        self.withdrawal_created_at
            .insert(request_id, requested.created_at_time);
        self.withdrawal_requests
            .insert(request_id, requested.clone());
        Ok(WithdrawalStart::Transfer(requested))
    }

    /// Persist dispatch intent before awaiting the ledger. `None` stays
    /// uncertain across retries and upgrades; only a proven first dispatch may
    /// safely release its reservation after a typed ledger rejection.
    pub fn mark_withdrawal_dispatch_attempt(
        &mut self,
        request_id: u64,
    ) -> Result<Option<u32>, String> {
        let mut record = self
            .withdrawal_requests
            .get(&request_id)
            .ok_or_else(|| format!("Withdrawal request {} is not recorded", request_id))?;
        if record.status != WithdrawalRequestStatus::Pending {
            return Err(format!("Withdrawal request {} is not pending", request_id));
        }
        record.dispatch_attempts = record
            .dispatch_attempts
            .and_then(|attempts| attempts.checked_add(1));
        let attempts = record.dispatch_attempts;
        self.withdrawal_requests.insert(request_id, record);
        Ok(attempts)
    }

    /// Settle a pending withdrawal once. Returns false for a matching terminal
    /// replay so callers can avoid emitting duplicate audit events.
    pub fn complete_withdrawal(
        &mut self,
        request_id: u64,
        block_index: u64,
    ) -> Result<bool, String> {
        let mut record = self
            .withdrawal_requests
            .get(&request_id)
            .ok_or_else(|| format!("Withdrawal request {} is not recorded", request_id))?;
        match &record.status {
            WithdrawalRequestStatus::Complete { block_index: prior } => {
                if *prior != block_index {
                    return Err(format!(
                        "Withdrawal request {} has conflicting completion blocks",
                        request_id
                    ));
                }
                Ok(false)
            }
            WithdrawalRequestStatus::Pending => {
                record.status = WithdrawalRequestStatus::Complete { block_index };
                self.withdrawal_requests.insert(request_id, record);
                Ok(true)
            }
            WithdrawalRequestStatus::LegacyUnknown => Err(format!(
                "Withdrawal request {} requires legacy reconciliation",
                request_id
            )),
        }
    }

    /// A definitive ledger rejection proves no transfer occurred, so restore
    /// the one reservation and allow a corrected later attempt.
    pub fn abort_withdrawal(&mut self, request_id: u64) -> Result<(), String> {
        let record = self
            .withdrawal_requests
            .get(&request_id)
            .ok_or_else(|| format!("Withdrawal request {} is not recorded", request_id))?;
        if record.status != WithdrawalRequestStatus::Pending {
            return Err(format!("Withdrawal request {} is not pending", request_id));
        }
        if record.dispatch_attempts != Some(1) {
            return Err(format!(
                "Withdrawal request {} may have an earlier dispatched transfer; reservation remains held for reconciliation",
                request_id
            ));
        }
        if let Some(balance) = self.balances.get_mut(&record.asset_type) {
            balance.total = balance.total.saturating_add(record.amount);
            balance.available = balance.available.saturating_add(record.amount);
        }
        self.withdrawal_requests.remove(&request_id);
        self.withdrawal_created_at.remove(&request_id);
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
    pub fn get_deposits(&self, start: Option<u64>, limit: usize) -> Vec<DepositRecordV1> {
        let start_key = start.unwrap_or(0);
        self.deposits
            .range(start_key..)
            .filter(|(_, record)| {
                !matches!(record.deposit_type, DepositType::LegacyUnknown { .. })
                    && !matches!(record.asset_type, crate::types::AssetType::Other(_))
            })
            .take(limit)
            .map(|(_, record)| DepositRecordV1::from(record))
            .collect()
    }

    /// Full deposit projection for callers that support address-bound assets.
    pub fn get_deposits_v2(&self, start: Option<u64>, limit: usize) -> Vec<DepositRecord> {
        let start_key = start.unwrap_or(0);
        self.deposits
            .range(start_key..)
            .filter(|(_, record)| !matches!(record.deposit_type, DepositType::LegacyUnknown { .. }))
            .take(limit)
            .map(|(_, record)| record)
            .collect()
    }

    pub fn get_unknown_evidence_v2(
        &self,
        start: Option<u64>,
        limit: usize,
    ) -> UnknownTreasuryEvidencePageV2 {
        let start_key = start.unwrap_or(0);
        let deposits = self
            .deposits
            .range(start_key..)
            .filter_map(|(id, record)| {
                if let DepositType::LegacyUnknown { candid_hex } = record.deposit_type {
                    Some(UnknownTreasuryEvidenceV2 {
                        record_kind: "deposit".into(),
                        id,
                        raw_candid_hex: candid_hex,
                    })
                } else {
                    None
                }
            })
            .take(limit)
            .collect();
        let events = self
            .events
            .range(start_key..)
            .filter_map(|(id, event)| {
                if let TreasuryAction::LegacyUnknown { candid_value } = event.action {
                    Some(UnknownTreasuryEvidenceV2 {
                        record_kind: "event".into(),
                        id,
                        raw_candid_hex: candid_value,
                    })
                } else {
                    None
                }
            })
            .take(limit)
            .collect();
        UnknownTreasuryEvidencePageV2 { deposits, events }
    }

    /// Scan at most `limit` stable request rows. Pagination advances by the
    /// final raw key scanned, including completed rows, so the query never
    /// needs an unbounded search to fill a page of pending requests.
    pub fn get_pending_withdrawals_v2(
        &self,
        start: Option<u64>,
        limit: usize,
    ) -> PendingWithdrawalsPageV2 {
        let start_key = start.unwrap_or(0);
        let scanned: Vec<_> = self
            .withdrawal_requests
            .range(start_key..)
            .take(limit)
            .collect();
        let next_start = scanned.last().and_then(|(key, _)| key.checked_add(1));
        let withdrawals = scanned
            .into_iter()
            .filter_map(|(request_id, record)| {
                let status = match record.status {
                    WithdrawalRequestStatus::Pending => "pending",
                    WithdrawalRequestStatus::LegacyUnknown => "legacy_unknown",
                    WithdrawalRequestStatus::Complete { .. } => return None,
                };
                Some(PendingWithdrawalV2 {
                    request_id,
                    caller: record.caller,
                    asset_type: crate::types::AssetTypeV1::try_from(record.asset_type).ok()?,
                    ledger: record.ledger,
                    amount: record.amount,
                    to: record.to,
                    memo: record.memo,
                    created_at_time: record.created_at_time,
                    send_amount: record.send_amount,
                    fee: record.fee,
                    dispatch_attempts: record.dispatch_attempts,
                    status: status.into(),
                })
            })
            .collect();
        PendingWithdrawalsPageV2 {
            withdrawals,
            next_start,
        }
    }

    pub fn get_pending_withdrawals_v3(
        &self,
        start: Option<u64>,
        limit: usize,
    ) -> PendingWithdrawalsPageV3 {
        let start_key = start.unwrap_or(0);
        let scanned: Vec<_> = self
            .withdrawal_requests
            .range(start_key..)
            .take(limit)
            .collect();
        let next_start = scanned.last().and_then(|(key, _)| key.checked_add(1));
        let withdrawals = scanned
            .into_iter()
            .filter_map(|(request_id, record)| {
                let status = match record.status {
                    WithdrawalRequestStatus::Pending => "pending",
                    WithdrawalRequestStatus::LegacyUnknown => "legacy_unknown",
                    WithdrawalRequestStatus::Complete { .. } => return None,
                };
                Some(PendingWithdrawalV3 {
                    request_id,
                    caller: record.caller,
                    asset_type: record.asset_type,
                    ledger: record.ledger,
                    amount: record.amount,
                    to: record.to,
                    memo: record.memo,
                    created_at_time: record.created_at_time,
                    send_amount: record.send_amount,
                    fee: record.fee,
                    dispatch_attempts: record.dispatch_attempts,
                    status: status.into(),
                })
            })
            .collect();
        PendingWithdrawalsPageV3 {
            withdrawals,
            next_start,
        }
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

/// Test fixture for upgrades from a version that predates the receipt index.
#[cfg(test)]
pub fn clear_receipt_index_for_legacy_test() {
    MEMORY_MANAGER.with(|mm| {
        let memory_manager = mm.borrow();
        let mut receipts: StableBTreeMap<DepositReceiptKey, DepositReceipt, Memory> =
            StableBTreeMap::init(memory_manager.get(MemoryId::new(MEM_DEPOSIT_RECEIPTS)));
        receipts.clear_new();
        let mut migration: StableCell<u8, Memory> = StableCell::init(
            memory_manager.get(MemoryId::new(MEM_DEPOSIT_RECEIPT_MIGRATION)),
            0,
        )
        .unwrap();
        migration.set(0).unwrap();
    });
}

#[cfg(test)]
pub fn reset_withdrawal_request_migration_for_legacy_test() {
    MEMORY_MANAGER.with(|mm| {
        let memory_manager = mm.borrow();
        let mut requests: StableBTreeMap<u64, WithdrawalRequestRecord, Memory> =
            StableBTreeMap::init(memory_manager.get(MemoryId::new(MEM_WITHDRAWAL_REQUESTS)));
        requests.clear_new();
        let mut migration: StableCell<u8, Memory> = StableCell::init(
            memory_manager.get(MemoryId::new(MEM_WITHDRAWAL_REQUEST_MIGRATION)),
            0,
        )
        .unwrap();
        migration.set(0).unwrap();
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
            let mut deposits: StableBTreeMap<u64, DepositRecord, Memory> =
                StableBTreeMap::init(memory_manager.get(MemoryId::new(MEM_DEPOSITS)));
            // The stable map key is the authoritative historical deposit ID.
            // A future DepositType cannot be decoded into its original record
            // fields, so repair the zero-value evidence sentinel's display ID
            // from that key without changing balances or its raw wire proof.
            let unknown_records: Vec<_> = deposits
                .iter()
                .filter(|(_, record)| {
                    matches!(
                        record.deposit_type,
                        crate::types::DepositType::LegacyUnknown { .. }
                    )
                })
                .collect();
            for (deposit_id, mut record) in unknown_records {
                if matches!(
                    record.deposit_type,
                    crate::types::DepositType::LegacyUnknown { .. }
                ) && record.id != deposit_id
                {
                    record.id = deposit_id;
                    deposits.insert(deposit_id, record);
                }
            }

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
                    let balance = b.entry(record.asset_type).or_default();
                    balance.total += record.amount;
                    balance.available += record.amount;
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
            let max_id = deposits.iter().map(|(id, _)| id).last().unwrap_or(0);
            let next_deposit_id = if max_id > 0 { max_id + 1 } else { 1 };

            // Re-open events stable map
            let events: StableBTreeMap<u64, TreasuryEvent, Memory> =
                StableBTreeMap::init(memory_manager.get(MemoryId::new(MEM_EVENTS)));
            let max_event_id = events.iter().map(|(id, _)| id).last().unwrap_or(0);
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
            let mut deposit_receipts: StableBTreeMap<DepositReceiptKey, DepositReceipt, Memory> =
                StableBTreeMap::init(memory_manager.get(MemoryId::new(MEM_DEPOSIT_RECEIPTS)));

            // Additive migration: older versions had no receipt index. Rebuild
            // it from the durable deposit log without changing old records or
            // balances. Conflicting legacy duplicates remain marked ambiguous
            // so a later retry cannot silently credit them again.
            let mut deposit_receipt_migration: StableCell<u8, Memory> = StableCell::init(
                memory_manager.get(MemoryId::new(MEM_DEPOSIT_RECEIPT_MIGRATION)),
                0,
            )
            .unwrap();
            if *deposit_receipt_migration.get() < 1 {
                for (deposit_id, record) in deposits.iter() {
                    let key = DepositReceiptKey::new(&record.asset_type, record.block_index);
                    match deposit_receipts.get(&key) {
                        Some(mut receipt)
                            if receipt.deposit_id != deposit_id
                                || receipt.amount != record.amount
                                || receipt.deposit_type != record.deposit_type
                                || receipt.memo != record.memo =>
                        {
                            // Any second historical record for this physical
                            // receipt is ambiguous, even if every field matches:
                            // the old log may already have credited it twice.
                            receipt.ambiguous_conflict = true;
                            deposit_receipts.insert(key, receipt);
                        }
                        Some(_) => {}
                        None => {
                            let unknown_type = matches!(
                                record.deposit_type,
                                crate::types::DepositType::LegacyUnknown { .. }
                            );
                            deposit_receipts.insert(
                                key,
                                DepositReceipt {
                                    deposit_id,
                                    amount: record.amount,
                                    deposit_type: record.deposit_type.clone(),
                                    memo: record.memo.clone(),
                                    ambiguous_conflict: unknown_type,
                                },
                            );
                        }
                    }
                }
                deposit_receipt_migration
                    .set(1)
                    .expect("deposit receipt migration version must persist");
            }

            let mut withdrawal_requests: StableBTreeMap<u64, WithdrawalRequestRecord, Memory> =
                StableBTreeMap::init(memory_manager.get(MemoryId::new(MEM_WITHDRAWAL_REQUESTS)));
            let mut withdrawal_request_migration: StableCell<u8, Memory> = StableCell::init(
                memory_manager.get(MemoryId::new(MEM_WITHDRAWAL_REQUEST_MIGRATION)),
                0,
            )
            .unwrap();
            if *withdrawal_request_migration.get() < 1 {
                // Old code persisted only request_id -> created_at_time. It
                // cannot tell whether a transfer landed or which tuple it
                // represented, so quarantine each ID rather than reusing it.
                for (request_id, created_at_time) in withdrawal_created_at.iter() {
                    if withdrawal_requests.get(&request_id).is_none() {
                        withdrawal_requests.insert(
                            request_id,
                            WithdrawalRequestRecord {
                                caller: Principal::anonymous(),
                                asset_type: AssetType::ICUSD,
                                ledger: Principal::anonymous(),
                                amount: 0,
                                to: Principal::anonymous(),
                                memo: None,
                                created_at_time,
                                send_amount: 0,
                                fee: 0,
                                status: WithdrawalRequestStatus::LegacyUnknown,
                                dispatch_attempts: None,
                            },
                        );
                    }
                }
                withdrawal_request_migration
                    .set(1)
                    .expect("withdrawal request migration version must persist");
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
                deposit_receipts,
                withdrawal_requests,
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
mod candid_legacy_migration_tests {
    use super::*;
    use ic_stable_structures::Storable;

    fn old_record(deposit_type: LegacyDepositType) -> LegacyDepositRecord {
        LegacyDepositRecord {
            id: 7,
            deposit_type,
            asset_type: AssetType::ICUSD,
            amount: 123,
            block_index: 456,
            timestamp: 789,
            memo: Some("legacy source memo".into()),
        }
    }

    #[test]
    fn missing_dispatch_history_decodes_as_uncertain_for_old_rows() {
        #[derive(candid::CandidType, serde::Serialize)]
        struct PriorWithdrawalRequestRecord {
            caller: Principal,
            asset_type: AssetType,
            ledger: Principal,
            amount: u64,
            to: Principal,
            memo: Option<String>,
            created_at_time: u64,
            send_amount: u64,
            fee: u64,
            status: WithdrawalRequestStatus,
        }
        let old_wire = candid::encode_one(PriorWithdrawalRequestRecord {
            caller: Principal::from_slice(&[1]),
            asset_type: AssetType::ICP,
            ledger: Principal::from_slice(&[2]),
            amount: 3,
            to: Principal::from_slice(&[4]),
            memo: None,
            created_at_time: 5,
            send_amount: 2,
            fee: 1,
            status: WithdrawalRequestStatus::Pending,
        })
        .unwrap();
        let decoded = WithdrawalRequestRecord::from_bytes(std::borrow::Cow::Owned(old_wire));
        assert_eq!(decoded.dispatch_attempts, None);
    }

    #[test]
    fn decodes_each_pre_rename_candid_deposit_variant() {
        let cases = [
            (
                LegacyDepositType::MintingFee,
                crate::types::DepositType::BorrowingFee,
            ),
            (
                LegacyDepositType::RedemptionFee,
                crate::types::DepositType::RedemptionFee,
            ),
            (
                LegacyDepositType::LiquidationSurplus,
                crate::types::DepositType::LiquidationFee,
            ),
            (
                LegacyDepositType::StabilityFee,
                crate::types::DepositType::InterestRevenue,
            ),
        ];
        for (legacy_type, expected) in cases {
            let old_wire = candid::encode_one(old_record(legacy_type))
                .expect("encode independent pre-rename Candid schema");
            let decoded = DepositRecord::from_bytes(std::borrow::Cow::Owned(old_wire));
            assert_eq!(decoded.deposit_type, expected);
            assert_eq!(decoded.memo.as_deref(), Some("legacy source memo"));
            assert_eq!(decoded.block_index, 456);
        }
    }

    #[test]
    fn unknown_deposit_variant_decodes_as_evidence_and_is_marked_non_spendable() {
        #[derive(candid::CandidType, serde::Serialize)]
        enum FutureDepositType {
            EmergencyRecoveryFee,
        }
        #[derive(candid::CandidType, serde::Serialize)]
        struct FutureDepositRecord {
            id: u64,
            deposit_type: FutureDepositType,
            asset_type: AssetType,
            amount: u64,
            block_index: u64,
            timestamp: u64,
            memo: Option<String>,
        }
        let wire = candid::encode_one(FutureDepositRecord {
            id: 8,
            deposit_type: FutureDepositType::EmergencyRecoveryFee,
            asset_type: AssetType::ICP,
            amount: 90,
            block_index: 12,
            timestamp: 13,
            memo: Some("future memo".into()),
        })
        .expect("encode future Candid variant");
        let decoded = DepositRecord::from_bytes(std::borrow::Cow::Owned(wire.clone()));
        let crate::types::DepositType::LegacyUnknown { candid_hex } = &decoded.deposit_type else {
            panic!("unknown type was misclassified as a known financial event");
        };
        assert_eq!(candid_hex.len(), wire.len() * 2);
        let second_upgrade = DepositRecord::from_bytes(decoded.to_bytes());
        assert_eq!(second_upgrade.deposit_type, decoded.deposit_type);

        let args = TreasuryInitArgs {
            controller: Principal::from_slice(&[1]),
            icusd_ledger: Principal::from_slice(&[2]),
            icp_ledger: Principal::from_slice(&[3]),
            ckbtc_ledger: None,
            ckusdt_ledger: None,
            ckusdc_ledger: None,
        };
        init_state(args);
        with_state_mut(|state| {
            state.deposits.insert(88, second_upgrade);
        });
        restore_state();
        with_state_mut(|state| {
            let persisted = state
                .deposits
                .get(&88)
                .expect("original stable key retained");
            assert_eq!(persisted.id, 88);
            assert!(matches!(
                persisted.deposit_type,
                crate::types::DepositType::LegacyUnknown { .. }
            ));
            assert!(state.get_deposits(None, 10).is_empty());
            let evidence = state.get_unknown_evidence_v2(Some(88), 10);
            assert_eq!(evidence.deposits.len(), 1);
            assert_eq!(evidence.deposits[0].record_kind, "deposit");
            assert_eq!(evidence.deposits[0].id, 88);
            assert!(state.withdraw(crate::types::AssetType::ICP, 1).is_err());
            let new_request = WithdrawalRequestRecord {
                caller: Principal::from_slice(&[7]),
                asset_type: crate::types::AssetType::ICP,
                ledger: Principal::from_slice(&[3]),
                amount: 1,
                to: Principal::from_slice(&[9]),
                memo: None,
                created_at_time: 1,
                send_amount: 1,
                fee: 0,
                status: WithdrawalRequestStatus::Pending,
                dispatch_attempts: Some(0),
            };
            assert!(state.begin_withdrawal(888, new_request).is_err());
            assert_eq!(state.balances[&crate::types::AssetType::ICP].total, 0);
        });
    }

    #[test]
    fn old_treasury_event_variant_uses_the_same_candid_migration() {
        let event = LegacyTreasuryEvent {
            id: 1,
            timestamp: 2,
            caller: Principal::from_slice(&[1]),
            action: LegacyTreasuryAction::Deposit {
                deposit_type: LegacyDepositType::MintingFee,
                asset_type: AssetType::ICUSD,
                amount: 3,
            },
        };
        let bytes = candid::encode_one(event).expect("encode old event schema");
        let decoded = TreasuryEvent::from_bytes(std::borrow::Cow::Owned(bytes));
        assert!(matches!(
            decoded.action,
            TreasuryAction::Deposit {
                deposit_type: crate::types::DepositType::BorrowingFee,
                amount: 3,
                ..
            }
        ));
    }
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
