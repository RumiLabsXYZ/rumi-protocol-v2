use crate::types::{AssetBalance, AssetType, BalancesSnapshot, DepositRecord, TreasuryInitArgs};
use candid::Principal;
use ic_stable_structures::memory_manager::{MemoryId, MemoryManager, VirtualMemory};
use ic_stable_structures::{DefaultMemoryImpl, StableBTreeMap, StableCell};
use std::cell::RefCell;
use std::collections::HashMap;

type Memory = VirtualMemory<DefaultMemoryImpl>;

// Stable memory layout:
//   MemoryId 0 → StableBTreeMap<u64, DepositRecord>  (deposit log)
//   MemoryId 1 → StableCell<TreasuryConfig>           (ledger principals, paused flag)
//   MemoryId 2 → StableCell<BalancesSnapshot>          (asset balances — survives upgrades)

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
            };

            let balances = empty_balances();

            Self {
                deposits: StableBTreeMap::init(memory_manager.get(MemoryId::new(0))),
                balances_cell: StableCell::init(
                    memory_manager.get(MemoryId::new(2)),
                    BalancesSnapshot::default(),
                )
                .unwrap(),
                balances,
                config: StableCell::init(memory_manager.get(MemoryId::new(1)), config).unwrap(),
                next_deposit_id: 1,
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
        self.deposits.insert(deposit_id, final_record);

        self.persist_balances();
        deposit_id
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
                StableBTreeMap::init(memory_manager.get(MemoryId::new(0)));

            // Dummy default for StableCell::init — real value is read from stable memory
            let dummy_config = TreasuryConfig {
                icusd_ledger: Principal::anonymous(),
                icp_ledger: Principal::anonymous(),
                ckbtc_ledger: None,
                ckusdt_ledger: None,
                ckusdc_ledger: None,
                is_paused: true,
            };
            let config =
                StableCell::init(memory_manager.get(MemoryId::new(1)), dummy_config).unwrap();

            // Read persisted balances (MemoryId 2).
            // On first upgrade from old code the cell won't exist yet, so the
            // default is an empty snapshot — we fall back to deposit replay.
            let balances_cell: StableCell<BalancesSnapshot, Memory> =
                StableCell::init(
                    memory_manager.get(MemoryId::new(2)),
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
            let max_id = deposits.iter().map(|(id, _)| id).last().unwrap_or(0);
            let next_deposit_id = if max_id > 0 { max_id + 1 } else { 1 };

            *s.borrow_mut() = Some(TreasuryState {
                deposits,
                balances,
                balances_cell,
                config,
                next_deposit_id,
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
