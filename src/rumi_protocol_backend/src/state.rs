use crate::numeric::{Ratio, UsdIcp, ICUSD, ICP};
use crate::vault::Vault;
use crate::{
    compute_collateral_ratio, InitArg, ProtocolError, UpgradeArg, MINIMUM_COLLATERAL_RATIO,
    RECOVERY_COLLATERAL_RATIO, INFO, SEC_NANOS,
};
use candid::Principal;
use ic_canister_log::log;
use rust_decimal::prelude::FromPrimitive;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Serialize;
use std::cell::RefCell;
use std::collections::btree_map::Entry::{Occupied, Vacant};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use crate::guard::OperationState;

// Like assert_eq, but returns an error instead of panicking.
macro_rules! ensure_eq {
    ($lhs:expr, $rhs:expr, $msg:expr $(, $args:expr)* $(,)*) => {
        if $lhs != $rhs {
            return Err(format!("{} ({:?}) != {} ({:?}): {}",
                               std::stringify!($lhs), $lhs,
                               std::stringify!($rhs), $rhs,
                               format!($msg $(,$args)*)));
        }
    }
}

macro_rules! ensure {
    ($cond:expr, $msg:expr $(, $args:expr)* $(,)*) => {
        if !$cond {
            return Err(format!("Condition {} is false: {}",
                               std::stringify!($cond),
                               format!($msg $(,$args)*)));
        }
    }
}

pub const ICP_TRANSFER_FEE: ICP = ICP::new(10_000); // 0.0001 ICP — standard ICP ledger fee
pub type VaultId = u64;
pub const DEFAULT_BORROW_FEE: Ratio = Ratio::new(dec!(0.005));
pub const DEFAULT_CKSTABLE_REPAY_FEE: Ratio = Ratio::new(dec!(0.0005)); // 0.05%
pub const DEFAULT_LIQUIDATION_BONUS: Ratio = Ratio::new(dec!(1.15)); // 115% (15% bonus)
pub const DEFAULT_MAX_PARTIAL_LIQUIDATION_RATIO: Ratio = Ratio::new(dec!(0.5)); // 50% max
pub const DEFAULT_REDEMPTION_FEE_FLOOR: Ratio = Ratio::new(dec!(0.005)); // 0.5%
pub const DEFAULT_REDEMPTION_FEE_CEILING: Ratio = Ratio::new(dec!(0.05)); // 5%
pub const DEFAULT_RECOVERY_TARGET_CR: Ratio = Ratio::new(dec!(1.55)); // 155% — target CR after recovery liquidation
pub const DEFAULT_INTEREST_RATE_APR: Ratio = Ratio::new(dec!(0.0)); // 0% — placeholder for future accrual

/// Collateral type identified by its ICRC-1 ledger canister principal.
pub type CollateralType = Principal;

/// Per-collateral status — graduated severity levels for risk management.
#[derive(candid::CandidType, Clone, Debug, PartialEq, Eq, serde::Deserialize, Serialize, Copy)]
pub enum CollateralStatus {
    /// Full functionality — all operations allowed
    Active,
    /// No new borrows/vaults or withdrawals; repay, add collateral, close (if empty) allowed
    Paused,
    /// HARD STOP — nothing works except admin actions. Emergency brake.
    Frozen,
    /// Winding down — repay and close only, no new activity
    Sunset,
    /// Fully wound down — read-only
    Deprecated,
}

impl Default for CollateralStatus {
    fn default() -> Self {
        Self::Active
    }
}

impl CollateralStatus {
    /// Whether opening new vaults is allowed for this collateral
    pub fn allows_open(&self) -> bool {
        matches!(self, CollateralStatus::Active)
    }

    /// Whether borrowing (minting) is allowed
    pub fn allows_borrow(&self) -> bool {
        matches!(self, CollateralStatus::Active)
    }

    /// Whether repaying debt is allowed
    pub fn allows_repay(&self) -> bool {
        matches!(self, CollateralStatus::Active | CollateralStatus::Paused | CollateralStatus::Sunset)
    }

    /// Whether adding collateral is allowed
    pub fn allows_add_collateral(&self) -> bool {
        matches!(self, CollateralStatus::Active | CollateralStatus::Paused)
    }

    /// Whether withdrawing collateral is allowed
    pub fn allows_withdraw(&self) -> bool {
        matches!(self, CollateralStatus::Active | CollateralStatus::Sunset)
    }

    /// Whether closing a vault is allowed (requires zero debt and zero collateral)
    pub fn allows_close(&self) -> bool {
        matches!(self, CollateralStatus::Active | CollateralStatus::Paused | CollateralStatus::Sunset)
    }

    /// Whether liquidations are allowed
    pub fn allows_liquidation(&self) -> bool {
        matches!(self, CollateralStatus::Active | CollateralStatus::Paused)
    }

    /// Whether redemptions are allowed
    pub fn allows_redemption(&self) -> bool {
        matches!(self, CollateralStatus::Active)
    }
}

/// Price source configuration for a collateral type.
#[derive(candid::CandidType, Clone, Debug, PartialEq, Eq, serde::Deserialize, Serialize)]
pub enum PriceSource {
    /// Use the ICP Exchange Rate Canister (XRC) with specified asset pair
    Xrc {
        base_asset: String,
        quote_asset: String,
    },
}

/// Per-collateral-type configuration. Each supported collateral token has one of these.
#[derive(candid::CandidType, Clone, Debug, serde::Deserialize, Serialize)]
pub struct CollateralConfig {
    /// ICRC-1 ledger canister ID for this token
    pub ledger_canister_id: Principal,
    /// Token decimal precision (e.g., 8 for ICP/ckBTC, 6 for ckUSDC)
    pub decimals: u8,
    /// Below this ratio, the vault can be liquidated (e.g., 1.33 = 133%)
    pub liquidation_ratio: Ratio,
    /// Below this ratio, recovery mode triggers for this collateral (e.g., 1.5 = 150%)
    pub borrow_threshold_ratio: Ratio,
    /// Bonus multiplier for liquidators (e.g., 1.15 = 15% bonus)
    pub liquidation_bonus: Ratio,
    /// One-time fee at borrow/mint time (e.g., 0.005 = 0.5%)
    pub borrowing_fee: Ratio,
    /// Ongoing interest rate — placeholder for future accrual (default 0.0)
    pub interest_rate_apr: Ratio,
    /// Maximum total debt across all vaults for this collateral (u64::MAX = no cap)
    pub debt_ceiling: u64,
    /// Minimum vault debt (dust threshold)
    pub min_vault_debt: ICUSD,
    /// Token transfer fee in native units
    pub ledger_fee: u64,
    /// How to fetch the USD price for this token
    pub price_source: PriceSource,
    /// Current operational status
    pub status: CollateralStatus,
    /// Last known USD price per 1 whole token (f64 for Candid compatibility)
    pub last_price: Option<f64>,
    /// Timestamp of last price update (nanoseconds)
    pub last_price_timestamp: Option<u64>,
    /// Minimum redemption fee (e.g., 0.005 = 0.5%)
    pub redemption_fee_floor: Ratio,
    /// Maximum redemption fee (e.g., 0.05 = 5%)
    pub redemption_fee_ceiling: Ratio,
    /// Dynamic base rate that spikes on redemptions and decays over time
    pub current_base_rate: Ratio,
    /// Timestamp of last redemption (nanoseconds)
    pub last_redemption_time: u64,
    /// Target CR to restore vaults to during recovery-mode liquidations (e.g., 1.55)
    pub recovery_target_cr: Ratio,
}

impl PartialEq for CollateralConfig {
    fn eq(&self, other: &Self) -> bool {
        self.ledger_canister_id == other.ledger_canister_id
            && self.decimals == other.decimals
            && self.liquidation_ratio == other.liquidation_ratio
            && self.borrow_threshold_ratio == other.borrow_threshold_ratio
            && self.liquidation_bonus == other.liquidation_bonus
            && self.borrowing_fee == other.borrowing_fee
            && self.interest_rate_apr == other.interest_rate_apr
            && self.debt_ceiling == other.debt_ceiling
            && self.min_vault_debt == other.min_vault_debt
            && self.ledger_fee == other.ledger_fee
            && self.price_source == other.price_source
            && self.status == other.status
            && self.last_price.map(f64::to_bits) == other.last_price.map(f64::to_bits)
            && self.last_price_timestamp == other.last_price_timestamp
            && self.redemption_fee_floor == other.redemption_fee_floor
            && self.redemption_fee_ceiling == other.redemption_fee_ceiling
            && self.current_base_rate == other.current_base_rate
            && self.last_redemption_time == other.last_redemption_time
            && self.recovery_target_cr == other.recovery_target_cr
    }
}

impl Eq for CollateralConfig {}

/// Controls which operations the protocol can perform.
#[derive(candid::CandidType, Clone, Debug, PartialEq, Eq, serde::Deserialize, Serialize, Copy)]
pub enum Mode {
    /// Protocol's state is read-only.
    ReadOnly,
    /// No restrictions on the protocol interactions.
    GeneralAvailability,
    /// The protocols tries to get back to a total
    /// collateral ratio above 150%
    Recovery,
}


impl Mode {
    pub fn is_available(&self) -> bool {
        match self {
            Mode::ReadOnly => false,
            Mode::GeneralAvailability => true,
            Mode::Recovery => true,
        }
    }

    pub fn get_minimum_liquidation_collateral_ratio(&self) -> Ratio {
        match self {
            Mode::ReadOnly => MINIMUM_COLLATERAL_RATIO,
            Mode::GeneralAvailability => MINIMUM_COLLATERAL_RATIO,
            Mode::Recovery => RECOVERY_COLLATERAL_RATIO,
        }
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Mode::ReadOnly => write!(f, "Read-only"),
            Mode::GeneralAvailability => write!(f, "General availability"),
            Mode::Recovery => write!(f, "Recovery"),
        }
    }
}

impl Default for Mode {
    fn default() -> Self {
        Self::GeneralAvailability
    }
}



#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, Serialize, Copy)]
pub struct PendingMarginTransfer {
    pub owner: Principal,
    pub margin: ICP,
    /// Which collateral ledger to transfer on. Defaults to ICP (via Principal::anonymous()
    /// sentinel, fixed up to icp_ledger_principal during processing) for backward compat
    /// with in-flight pending transfers from before the multi-collateral refactor.
    #[serde(default = "crate::vault::default_collateral_type")]
    pub collateral_type: Principal,
}

thread_local! {
    static __STATE: RefCell<Option<State>> = RefCell::default();
}


pub struct State {
    pub vault_id_to_vaults: BTreeMap<u64, Vault>,
    pub principal_to_vault_ids: BTreeMap<Principal, BTreeSet<u64>>,
    pub pending_margin_transfers: BTreeMap<VaultId, PendingMarginTransfer>,
    pub pending_excess_transfers: BTreeMap<VaultId, PendingMarginTransfer>,
    pub pending_redemption_transfer: BTreeMap<u64, PendingMarginTransfer>,
    pub mode: Mode,
    pub fee: Ratio,
    pub developer_principal: Principal,
    pub next_available_vault_id: u64,
    pub total_collateral_ratio: Ratio,
    pub current_base_rate: Ratio,
    pub last_redemption_time: u64,
    pub liquidity_pool: BTreeMap<Principal, ICUSD>,
    pub liquidity_returns: BTreeMap<Principal, ICP>,
    pub xrc_principal: Principal,
    pub icusd_ledger_principal: Principal,
    pub icp_ledger_principal: Principal,
    pub icp_ledger_fee: ICP,
    pub last_icp_rate: Option<UsdIcp>,
    pub last_icp_timestamp: Option<u64>,
    pub principal_guards: BTreeSet<Principal>,
    pub principal_guard_timestamps: BTreeMap<Principal, u64>, // Add timestamps for guards
    pub operation_states: BTreeMap<Principal, OperationState>, // Track operation states
    pub operation_names: BTreeMap<Principal, String>, // Track operation names
    pub is_timer_running: bool,
    pub is_fetching_rate: bool,

    // Rate limiting for close_vault operations
    pub close_vault_requests: BTreeMap<Principal, Vec<u64>>,
    pub global_close_requests: Vec<u64>,
    pub concurrent_close_operations: u32,
    pub dust_forgiven_total: ICUSD,
    pub treasury_principal: Option<Principal>,
    pub stability_pool_canister: Option<Principal>,
    pub ckusdt_ledger_principal: Option<Principal>,
    pub ckusdc_ledger_principal: Option<Principal>,
    pub ckstable_repay_fee: Ratio,
    pub ckusdt_enabled: bool,
    pub ckusdc_enabled: bool,
    // Cached ckstable prices (from XRC, on-demand only)
    pub last_ckusdt_rate: Option<rust_decimal::Decimal>,  // USDT/USD price (should be ~1.0)
    pub last_ckusdt_timestamp: Option<u64>,                // nanos
    pub last_ckusdc_rate: Option<rust_decimal::Decimal>,  // USDC/USD price (should be ~1.0)
    pub last_ckusdc_timestamp: Option<u64>,                // nanos
    pub liquidation_bonus: Ratio,
    pub max_partial_liquidation_ratio: Ratio,
    pub redemption_fee_floor: Ratio,
    pub redemption_fee_ceiling: Ratio,
    pub recovery_target_cr: Ratio,

    /// Cached dynamic recovery mode threshold (debt-weighted average of per-collateral borrow thresholds).
    /// Updated alongside total_collateral_ratio on each price tick.
    pub recovery_mode_threshold: Ratio,

    // Multi-collateral support
    pub collateral_configs: BTreeMap<CollateralType, CollateralConfig>,
    pub collateral_to_vault_ids: BTreeMap<CollateralType, BTreeSet<u64>>,
}

impl From<InitArg> for State {
    fn from(args: InitArg) -> Self {
        let fee = Decimal::from_u64(args.fee_e8s).unwrap() / dec!(100_000_000);
        Self {
            last_redemption_time: 0,
            current_base_rate: Ratio::from(Decimal::ZERO),
            fee: Ratio::from(fee),
            developer_principal: args.developer_principal,
            principal_to_vault_ids: BTreeMap::new(),
            pending_redemption_transfer: BTreeMap::new(),
            vault_id_to_vaults: BTreeMap::new(),
            xrc_principal: args.xrc_principal,
            icusd_ledger_principal: args.icusd_ledger_principal,
            icp_ledger_principal: args.icp_ledger_principal,
            icp_ledger_fee: ICP_TRANSFER_FEE,
            mode: Mode::GeneralAvailability,
            total_collateral_ratio: Ratio::from(Decimal::MAX),
            last_icp_timestamp: None,
            last_icp_rate: None,
            next_available_vault_id: 1,
            principal_guards: BTreeSet::new(),
            principal_guard_timestamps: BTreeMap::new(), // Initialize empty timestamps map
            operation_states: BTreeMap::new(),
            operation_names: BTreeMap::new(),
            liquidity_pool: BTreeMap::new(),
            liquidity_returns: BTreeMap::new(),
            pending_margin_transfers: BTreeMap::new(),
            pending_excess_transfers: BTreeMap::new(),
            is_timer_running: false,
            is_fetching_rate: false,
            // Rate limiting initialization
            close_vault_requests: BTreeMap::new(),
            global_close_requests: Vec::new(),
            concurrent_close_operations: 0,
            dust_forgiven_total: ICUSD::new(0),

            // ckStable repayment initialization
            treasury_principal: args.treasury_principal,
            stability_pool_canister: args.stability_pool_principal,
            ckusdt_ledger_principal: args.ckusdt_ledger_principal,
            ckusdc_ledger_principal: args.ckusdc_ledger_principal,
            ckstable_repay_fee: DEFAULT_CKSTABLE_REPAY_FEE,
            ckusdt_enabled: true,
            ckusdc_enabled: true,
            last_ckusdt_rate: None,
            last_ckusdt_timestamp: None,
            last_ckusdc_rate: None,
            last_ckusdc_timestamp: None,
            liquidation_bonus: DEFAULT_LIQUIDATION_BONUS,
            max_partial_liquidation_ratio: DEFAULT_MAX_PARTIAL_LIQUIDATION_RATIO,
            redemption_fee_floor: DEFAULT_REDEMPTION_FEE_FLOOR,
            redemption_fee_ceiling: DEFAULT_REDEMPTION_FEE_CEILING,
            recovery_target_cr: DEFAULT_RECOVERY_TARGET_CR,
            recovery_mode_threshold: RECOVERY_COLLATERAL_RATIO,

            // Multi-collateral: initialize with ICP as the default collateral
            collateral_configs: {
                let mut configs = BTreeMap::new();
                configs.insert(args.icp_ledger_principal, CollateralConfig {
                    ledger_canister_id: args.icp_ledger_principal,
                    decimals: 8,
                    liquidation_ratio: MINIMUM_COLLATERAL_RATIO,
                    borrow_threshold_ratio: RECOVERY_COLLATERAL_RATIO,
                    liquidation_bonus: DEFAULT_LIQUIDATION_BONUS,
                    borrowing_fee: Ratio::from(fee),
                    interest_rate_apr: DEFAULT_INTEREST_RATE_APR,
                    debt_ceiling: u64::MAX,
                    min_vault_debt: ICUSD::new(1_000_000), // 0.01 icUSD
                    ledger_fee: ICP_TRANSFER_FEE.to_u64(),
                    price_source: PriceSource::Xrc {
                        base_asset: "ICP".to_string(),
                        quote_asset: "USD".to_string(),
                    },
                    status: CollateralStatus::Active,
                    last_price: None,
                    last_price_timestamp: None,
                    redemption_fee_floor: DEFAULT_REDEMPTION_FEE_FLOOR,
                    redemption_fee_ceiling: DEFAULT_REDEMPTION_FEE_CEILING,
                    current_base_rate: Ratio::from(Decimal::ZERO),
                    last_redemption_time: 0,
                    recovery_target_cr: DEFAULT_RECOVERY_TARGET_CR,
                });
                configs
            },
            collateral_to_vault_ids: BTreeMap::new(),
        }
    }
}

impl State {

    // Rate limiting functions for close_vault operations
    pub fn check_close_vault_rate_limit(&mut self, principal: Principal) -> Result<(), ProtocolError> {
        let current_time = ic_cdk::api::time();
        let minute_nanos = 60 * 1_000_000_000; // 1 minute in nanoseconds
        let day_nanos = 24 * 60 * minute_nanos; // 24 hours in nanoseconds
        
        // Clean old timestamps (older than 24 hours)
        let cutoff_time = current_time.saturating_sub(day_nanos);
        
        // Clean user's timestamps
        if let Some(user_requests) = self.close_vault_requests.get_mut(&principal) {
            user_requests.retain(|&timestamp| timestamp > cutoff_time);
        }
        
        // Clean global timestamps
        self.global_close_requests.retain(|&timestamp| timestamp > cutoff_time);
        
        // Check user rate limits (5 per minute, 60 per day)
        let user_recent_requests = self.close_vault_requests
            .get(&principal)
            .map(|requests| requests.iter().filter(|&&timestamp| timestamp > current_time - minute_nanos).count())
            .unwrap_or(0);
            
        let user_daily_requests = self.close_vault_requests
            .get(&principal)
            .map(|requests| requests.len())
            .unwrap_or(0);
            
        if user_recent_requests >= 5 {
            return Err(ProtocolError::GenericError(
                "Rate limit exceeded: Maximum 5 close_vault calls per minute per user".to_string()
            ));
        }
        
        if user_daily_requests >= 60 {
            return Err(ProtocolError::GenericError(
                "Rate limit exceeded: Maximum 60 close_vault calls per day per user".to_string()
            ));
        }
        
        // Check global rate limits (300 per minute, 30,000 per day)
        let global_recent_requests = self.global_close_requests
            .iter()
            .filter(|&&timestamp| timestamp > current_time - minute_nanos)
            .count();
            
        let global_daily_requests = self.global_close_requests.len();
        
        if global_recent_requests >= 300 {
            return Err(ProtocolError::GenericError(
                "Rate limit exceeded: Maximum 300 close_vault calls per minute globally".to_string()
            ));
        }
        
        if global_daily_requests >= 30_000 {
            return Err(ProtocolError::GenericError(
                "Rate limit exceeded: Maximum 30,000 close_vault calls per day globally".to_string()
            ));
        }
        
        // Check concurrent operations limit (200)
        if self.concurrent_close_operations >= 200 {
            return Err(ProtocolError::GenericError(
                "Rate limit exceeded: Maximum 200 concurrent close_vault operations".to_string()
            ));
        }
        
        Ok(())
    }
    
    pub fn record_close_vault_request(&mut self, principal: Principal) {
        let current_time = ic_cdk::api::time();
        
        // Record user request
        self.close_vault_requests
            .entry(principal)
            .or_insert_with(Vec::new)
            .push(current_time);
            
        // Record global request
        self.global_close_requests.push(current_time);
        
        // Increment concurrent operations
        self.concurrent_close_operations += 1;
    }
    
    pub fn complete_close_vault_request(&mut self) {
        // Decrement concurrent operations
        if self.concurrent_close_operations > 0 {
            self.concurrent_close_operations -= 1;
        }
    }

    pub fn check_price_not_too_old(&self) -> Result<(), ProtocolError> {
        let current_time = ic_cdk::api::time();
        const TEN_MINS_NANOS: u64 = 10 * 60 * 1_000_000_000;
        let last_icp_timestamp = match self.last_icp_timestamp {
            Some(last_icp_timestamp) => last_icp_timestamp,
            None => {
                return Err(ProtocolError::TemporarilyUnavailable(
                    "No ICP price fetched".to_string(),
                ))
            }
        };
        if current_time.saturating_sub(last_icp_timestamp) > TEN_MINS_NANOS {
            return Err(ProtocolError::TemporarilyUnavailable(
                "Last known ICP price too old".to_string(),
            ));
        }
        Ok(())
    }

    pub fn increment_vault_id(&mut self) -> u64 {
        let vault_id = self.next_available_vault_id;
        self.next_available_vault_id += 1;
        vault_id
    }

    pub fn upgrade(&mut self, args: UpgradeArg) {
        if let Some(mode) = args.mode {
            self.mode = mode;
        }
    }

    pub fn total_borrowed_icusd_amount(&self) -> ICUSD {
        self.vault_id_to_vaults
            .values()
            .map(|vault| vault.borrowed_icusd_amount)
            .sum()
    }

    /// Deprecated: use `total_collateral_for(&icp_ledger)` for ICP specifically,
    /// or sum across all collateral types for cross-collateral totals.
    /// Kept for backward compat with dashboard and metrics endpoints.
    pub fn total_icp_margin_amount(&self) -> ICP {
        ICP::from(self.total_collateral_for(&self.icp_ledger_principal))
    }

    pub fn compute_total_collateral_ratio(&self, _rate: UsdIcp) -> Ratio {
        let total_debt = self.total_borrowed_icusd_amount();
        if total_debt == ICUSD::new(0) {
            return Ratio::from(Decimal::MAX);
        }
        // Sum USD value across ALL vaults using per-collateral pricing.
        // Iterates vaults directly (not via collateral_to_vault_ids index) for robustness
        // against legacy vaults with Principal::anonymous() collateral_type.
        let mut total_value = ICUSD::new(0);
        for vault in self.vault_id_to_vaults.values() {
            if let Some(config) = self.get_collateral_config(&vault.collateral_type) {
                if let Some(price) = config.last_price {
                    let price_dec = Decimal::from_f64(price).unwrap_or(Decimal::ZERO);
                    total_value += crate::numeric::collateral_usd_value(
                        vault.collateral_amount,
                        price_dec,
                        config.decimals,
                    );
                }
                // No price → contributes 0 value (conservative)
            }
            // No config → contributes 0 value (conservative)
        }
        total_value / total_debt
    }

    /// Compute the dynamic recovery mode threshold as a debt-weighted average
    /// of per-collateral borrow_threshold_ratio values.
    /// Falls back to RECOVERY_COLLATERAL_RATIO when total debt is zero.
    ///
    /// Formula: recovery_threshold = Σ (debt_i / total_debt) × borrow_threshold_i
    ///
    /// Mathematical guarantee: the result can never be lower than the lowest
    /// individual borrow_threshold_ratio, ensuring no collateral type's users
    /// get surprise-liquidated below their own threshold.
    pub fn compute_dynamic_recovery_threshold(&self) -> Ratio {
        let total_debt = self.total_borrowed_icusd_amount();
        if total_debt == ICUSD::new(0) {
            return RECOVERY_COLLATERAL_RATIO;
        }
        let total_debt_dec = Decimal::from_u64(total_debt.to_u64())
            .unwrap_or(Decimal::ZERO);

        let mut weighted_sum = Decimal::ZERO;
        for (ct, config) in &self.collateral_configs {
            let debt_i = self.total_debt_for_collateral(ct);
            if debt_i == ICUSD::new(0) {
                continue;
            }
            let debt_i_dec = Decimal::from_u64(debt_i.to_u64())
                .unwrap_or(Decimal::ZERO);
            weighted_sum += (debt_i_dec / total_debt_dec) * config.borrow_threshold_ratio.0;
        }

        if weighted_sum == Decimal::ZERO {
            // Safety fallback: no configs matched (shouldn't happen if total_debt > 0)
            return RECOVERY_COLLATERAL_RATIO;
        }
        Ratio::from(weighted_sum)
    }

    pub fn get_redemption_fee(&self, redeemed_amount: ICUSD) -> Ratio {
        let current_time = ic_cdk::api::time();
        let last_redemption_time = self.last_redemption_time;
        let elapsed_hours = (current_time - last_redemption_time) / 1_000_000_000 / 3600;
        compute_redemption_fee(
            elapsed_hours,
            redeemed_amount,
            self.total_borrowed_icusd_amount(),
            self.current_base_rate,
            self.redemption_fee_floor,
            self.redemption_fee_ceiling,
        )
    }

    pub fn get_borrowing_fee(&self) -> Ratio {
        match self.mode {
            Mode::Recovery => Ratio::from(Decimal::ZERO),
            Mode::GeneralAvailability => self.fee,
            Mode::ReadOnly => self.fee,
        }
    }

    // --- Multi-collateral helper methods ---

    /// Get the collateral config for a given collateral type.
    /// Resolves `Principal::anonymous()` (serde default for legacy vaults) to the ICP ledger.
    pub fn get_collateral_config(&self, ct: &CollateralType) -> Option<&CollateralConfig> {
        let resolved = if ct == &Principal::anonymous() {
            &self.icp_ledger_principal
        } else {
            ct
        };
        self.collateral_configs.get(resolved)
    }

    /// Get a mutable reference to the collateral config.
    /// Resolves `Principal::anonymous()` to the ICP ledger.
    pub fn get_collateral_config_mut(&mut self, ct: &CollateralType) -> Option<&mut CollateralConfig> {
        let resolved = if ct == &Principal::anonymous() {
            self.icp_ledger_principal
        } else {
            *ct
        };
        self.collateral_configs.get_mut(&resolved)
    }

    /// Get the ICP collateral type (convenience)
    pub fn icp_collateral_type(&self) -> CollateralType {
        self.icp_ledger_principal
    }

    /// Set the ICP rate on both the global field AND the ICP CollateralConfig's `last_price`.
    /// This is the ONLY correct way to update the ICP price.
    pub fn set_icp_rate(&mut self, rate: crate::numeric::UsdIcp, timestamp_nanos: Option<u64>) {
        use rust_decimal::prelude::ToPrimitive;
        self.last_icp_rate = Some(rate);
        if let Some(ts) = timestamp_nanos {
            self.last_icp_timestamp = Some(ts);
        }
        let icp = self.icp_collateral_type();
        if let Some(config) = self.collateral_configs.get_mut(&icp) {
            config.last_price = Some(rate.to_f64());
            if let Some(ts) = timestamp_nanos {
                config.last_price_timestamp = Some(ts);
            }
        }
    }

    /// Get borrowing fee for a specific collateral type
    pub fn get_borrowing_fee_for(&self, ct: &CollateralType) -> Ratio {
        if self.mode == Mode::Recovery {
            return Ratio::from(Decimal::ZERO);
        }
        self.collateral_configs
            .get(ct)
            .map(|c| c.borrowing_fee)
            .unwrap_or(self.fee)
    }

    /// Get liquidation bonus for a specific collateral type
    pub fn get_liquidation_bonus_for(&self, ct: &CollateralType) -> Ratio {
        self.collateral_configs
            .get(ct)
            .map(|c| c.liquidation_bonus)
            .unwrap_or(self.liquidation_bonus)
    }

    /// Get the liquidation ratio (below this, vault is liquidatable) for a specific collateral type
    pub fn get_liquidation_ratio_for(&self, ct: &CollateralType) -> Ratio {
        self.collateral_configs
            .get(ct)
            .map(|c| c.liquidation_ratio)
            .unwrap_or(MINIMUM_COLLATERAL_RATIO)
    }

    /// Get the minimum collateral ratio (below this, recovery mode triggers) for a collateral type
    pub fn get_min_collateral_ratio_for(&self, ct: &CollateralType) -> Ratio {
        self.collateral_configs
            .get(ct)
            .map(|c| c.borrow_threshold_ratio)
            .unwrap_or(RECOVERY_COLLATERAL_RATIO)
    }

    /// Get the last known price for a collateral type (USD per 1 whole token)
    pub fn get_price_for(&self, ct: &CollateralType) -> Option<f64> {
        self.get_collateral_config(ct)
            .and_then(|c| c.last_price)
    }

    /// Get the collateral's USD price as Decimal, or None.
    /// Uses per-collateral config (resolves Principal::anonymous() → ICP).
    pub fn get_collateral_price_decimal(&self, ct: &CollateralType) -> Option<Decimal> {
        self.get_collateral_config(ct)
            .and_then(|c| c.last_price)
            .and_then(|p| Decimal::from_f64(p))
    }

    /// Get the recovery target CR for a specific collateral type
    pub fn get_recovery_target_cr_for(&self, ct: &CollateralType) -> Ratio {
        self.collateral_configs
            .get(ct)
            .map(|c| c.recovery_target_cr)
            .unwrap_or(self.recovery_target_cr)
    }

    /// Get the minimum liquidation collateral ratio for a specific collateral type,
    /// accounting for the current protocol mode.
    /// - Normal/ReadOnly: `config.liquidation_ratio` (e.g., 1.33)
    /// - Recovery: `config.borrow_threshold_ratio` (e.g., 1.50) — recovery mode liquidates more aggressively
    pub fn get_min_liquidation_ratio_for(&self, ct: &CollateralType) -> Ratio {
        match self.mode {
            Mode::Recovery => self.get_min_collateral_ratio_for(ct),     // borrow_threshold_ratio
            _ => self.get_liquidation_ratio_for(ct),                      // liquidation_ratio
        }
    }

    /// Get the collateral status for a given collateral type
    pub fn get_collateral_status(&self, ct: &CollateralType) -> Option<CollateralStatus> {
        self.collateral_configs.get(ct).map(|c| c.status)
    }

    /// Get the redemption fee for a specific collateral type
    pub fn get_redemption_fee_for(&self, ct: &CollateralType, redeemed_amount: ICUSD) -> Ratio {
        if let Some(config) = self.collateral_configs.get(ct) {
            let current_time = ic_cdk::api::time();
            let elapsed_hours = (current_time - config.last_redemption_time) / 1_000_000_000 / 3600;
            let total_borrowed = self.total_debt_for_collateral(ct);
            compute_redemption_fee(
                elapsed_hours,
                redeemed_amount,
                total_borrowed,
                config.current_base_rate,
                config.redemption_fee_floor,
                config.redemption_fee_ceiling,
            )
        } else {
            self.get_redemption_fee(redeemed_amount)
        }
    }

    /// Total borrowed icUSD for a specific collateral type
    pub fn total_debt_for_collateral(&self, ct: &CollateralType) -> ICUSD {
        match self.collateral_to_vault_ids.get(ct) {
            Some(vault_ids) => vault_ids
                .iter()
                .filter_map(|id| self.vault_id_to_vaults.get(id))
                .map(|v| v.borrowed_icusd_amount)
                .sum(),
            None => ICUSD::new(0),
        }
    }

    /// Total raw collateral amount for a specific collateral type
    pub fn total_collateral_for(&self, ct: &CollateralType) -> u64 {
        match self.collateral_to_vault_ids.get(ct) {
            Some(vault_ids) => vault_ids
                .iter()
                .filter_map(|id| self.vault_id_to_vaults.get(id))
                .map(|v| v.collateral_amount)
                .sum(),
            None => 0,
        }
    }

    /// Total USD value of collateral for a specific collateral type (normalized by decimals).
    /// Returns ICUSD value in e8s.
    pub fn total_collateral_value_for(&self, ct: &CollateralType) -> ICUSD {
        let raw_amount = self.total_collateral_for(ct);
        if let Some(config) = self.collateral_configs.get(ct) {
            if let Some(price) = config.last_price {
                let price_dec = Decimal::from_f64(price).unwrap_or(Decimal::ZERO);
                crate::numeric::collateral_usd_value(raw_amount, price_dec, config.decimals)
            } else {
                ICUSD::new(0)
            }
        } else {
            ICUSD::new(0)
        }
    }

    /// Get all supported collateral types and their statuses
    pub fn supported_collateral_types(&self) -> Vec<(CollateralType, CollateralStatus)> {
        self.collateral_configs
            .iter()
            .map(|(ct, config)| (*ct, config.status))
            .collect()
    }

    /// Register a vault ID under its collateral type index
    pub fn index_vault_by_collateral(&mut self, collateral_type: CollateralType, vault_id: u64) {
        self.collateral_to_vault_ids
            .entry(collateral_type)
            .or_insert_with(BTreeSet::new)
            .insert(vault_id);
    }

    /// Remove a vault ID from its collateral type index
    pub fn unindex_vault_by_collateral(&mut self, collateral_type: &CollateralType, vault_id: u64) {
        if let Some(ids) = self.collateral_to_vault_ids.get_mut(collateral_type) {
            ids.remove(&vault_id);
            if ids.is_empty() {
                self.collateral_to_vault_ids.remove(collateral_type);
            }
        }
    }

    /// Sync a global fee-setting event to the ICP CollateralConfig (for backward compat during replay)
    pub fn sync_icp_collateral_config(&mut self) {
        let icp = self.icp_ledger_principal;
        if let Some(config) = self.collateral_configs.get_mut(&icp) {
            config.borrowing_fee = self.fee;
            config.liquidation_bonus = self.liquidation_bonus;
            config.redemption_fee_floor = self.redemption_fee_floor;
            config.redemption_fee_ceiling = self.redemption_fee_ceiling;
            config.recovery_target_cr = self.recovery_target_cr;
            config.ledger_fee = self.icp_ledger_fee.to_u64();
        }
    }

    pub fn update_total_collateral_ratio_and_mode(&mut self, rate: UsdIcp) {
        let previous_mode = self.mode;
        let new_total_collateral_ratio = self.compute_total_collateral_ratio(rate);
        self.total_collateral_ratio = new_total_collateral_ratio;

        // Compute the debt-weighted recovery threshold and cache it
        let dynamic_threshold = self.compute_dynamic_recovery_threshold();
        self.recovery_mode_threshold = dynamic_threshold;

        if new_total_collateral_ratio < dynamic_threshold {
            self.mode = Mode::Recovery;
        } else {
            self.mode = Mode::GeneralAvailability;
        }

        if new_total_collateral_ratio < Ratio::from(dec!(1.0)) {
            self.mode = Mode::ReadOnly;
        }

        if previous_mode != self.mode {
            log!(
                crate::DEBUG,
                "[update_mode] switched to {}, ratio: {}, recovery threshold: {}",
                self.mode,
                new_total_collateral_ratio.to_f64(),
                dynamic_threshold.to_f64()
            );
        }
    }

    pub fn open_vault(&mut self, vault: Vault) {
        let vault_id = vault.vault_id;
        let collateral_type = vault.collateral_type;
        self.vault_id_to_vaults.insert(vault_id, vault.clone());
        match self.principal_to_vault_ids.get_mut(&vault.owner) {
            Some(vault_ids) => {
                vault_ids.insert(vault_id);
            }
            None => {
                let mut vault_ids: BTreeSet<u64> = BTreeSet::new();
                vault_ids.insert(vault_id);
                self.principal_to_vault_ids.insert(vault.owner, vault_ids);
            }
        }
        // Index by collateral type
        self.index_vault_by_collateral(collateral_type, vault_id);
    }

    pub fn close_vault(&mut self, vault_id: u64) {
        if let Some(vault) = self.vault_id_to_vaults.remove(&vault_id) {
            let owner = vault.owner;
            self.pending_margin_transfers.insert(
                vault_id,
                PendingMarginTransfer {
                    owner,
                    margin: ICP::from(vault.collateral_amount),
                    collateral_type: vault.collateral_type,
                },
            );
            if let Some(vault_ids) = self.principal_to_vault_ids.get_mut(&owner) {
                vault_ids.remove(&vault_id);
            } else {
                ic_cdk::trap("BUG: tried to close vault with no owner");
            }
        } else {
            ic_cdk::trap("BUG: tried to close unknown vault");
        }
    }

    pub fn borrow_from_vault(&mut self, vault_id: u64, borrowed_amount: ICUSD) {
        match self.vault_id_to_vaults.get_mut(&vault_id) {
            Some(vault) => {
                vault.borrowed_icusd_amount += borrowed_amount;
            }
            None => ic_cdk::trap("borrowing from unknown vault"),
        }
    }

    pub fn add_margin_to_vault(&mut self, vault_id: u64, add_margin: ICP) {
        match self.vault_id_to_vaults.get_mut(&vault_id) {
            Some(vault) => {
                vault.collateral_amount += add_margin.to_u64();
            }
            None => ic_cdk::trap("adding margin to unknown vault"),
        }
    }

    pub fn remove_margin_from_vault(&mut self, vault_id: u64, amount: ICP) {
        match self.vault_id_to_vaults.get_mut(&vault_id) {
            Some(vault) => {
                assert!(amount.to_u64() <= vault.collateral_amount);
                vault.collateral_amount -= amount.to_u64();
            }
            None => ic_cdk::trap("removing margin from unknown vault"),
        }
    }

    pub fn repay_to_vault(&mut self, vault_id: u64, repayed_amount: ICUSD) {
        match self.vault_id_to_vaults.get_mut(&vault_id) {
            Some(vault) => {
                assert!(repayed_amount <= vault.borrowed_icusd_amount);
                vault.borrowed_icusd_amount -= repayed_amount;
            }
            None => ic_cdk::trap("repaying to unknown vault"),
        }
    }

    pub fn provide_liquidity(&mut self, amount: ICUSD, caller: Principal) {
        if amount == 0 {
            return;
        }
        self.liquidity_pool
            .entry(caller)
            .and_modify(|curr| *curr += amount)
            .or_insert(amount);
    }

    pub fn withdraw_liquidity(&mut self, amount: ICUSD, caller: Principal) {
        match self.liquidity_pool.entry(caller) {
            Occupied(mut entry) => {
                assert!(*entry.get() >= amount);
                *entry.get_mut() -= amount;
                if *entry.get() == 0 {
                    entry.remove_entry();
                }
            }
            Vacant(_) => ic_cdk::trap("cannot remove liquidity from unknown principal"),
        }
    }

    pub fn claim_liquidity_returns(&mut self, amount: ICP, caller: Principal) {
        match self.liquidity_returns.entry(caller) {
            Occupied(mut entry) => {
                assert!(*entry.get() >= amount);
                *entry.get_mut() -= amount;
                if *entry.get() == 0 {
                    entry.remove_entry();
                }
            }
            Vacant(_) => ic_cdk::trap("cannot claim returns from unknown principal"),
        }
    }

    pub fn get_liquidity_returns_of(&self, principal: Principal) -> ICP {
        *self.liquidity_returns.get(&principal).unwrap_or(&0.into())
    }

    pub fn total_provided_liquidity_amount(&self) -> ICUSD {
        self.liquidity_pool.values().cloned().sum()
    }

    pub fn total_available_returns(&self) -> ICP {
        self.liquidity_returns.values().cloned().sum()
    }

    pub fn get_provided_liquidity(&self, principal: Principal) -> ICUSD {
        *self.liquidity_pool.get(&principal).unwrap_or(&ICUSD::from(0))
    }

    /// Compute the icUSD repayment needed to restore a vault's CR to recovery_target_cr.
    /// Returns None if not applicable (not in recovery, or vault CR outside the per-collateral
    /// liquidation_ratio..borrow_threshold_ratio range).
    pub fn compute_recovery_repay_cap(&self, vault: &Vault, collateral_price: UsdIcp) -> Option<ICUSD> {
        if self.mode != Mode::Recovery {
            return None;
        }
        let vault_cr = compute_collateral_ratio(vault, collateral_price, self);
        let per_collateral_liq_ratio = self.get_liquidation_ratio_for(&vault.collateral_type);
        let per_collateral_borrow_threshold = self.get_min_collateral_ratio_for(&vault.collateral_type);
        if vault_cr <= per_collateral_liq_ratio || vault_cr >= per_collateral_borrow_threshold {
            return None;
        }
        let ct = &vault.collateral_type;
        let config = self.get_collateral_config(ct)?;
        let price = Decimal::from_f64(config.last_price?)?;
        let collateral_value: ICUSD = crate::numeric::collateral_usd_value(
            vault.collateral_amount,
            price,
            config.decimals,
        );
        let recovery_target = self.get_recovery_target_cr_for(ct);
        let liq_bonus = self.get_liquidation_bonus_for(ct);
        let numerator_icusd = vault.borrowed_icusd_amount * recovery_target;
        if numerator_icusd <= collateral_value {
            return None; // already at or above target
        }
        let deficit = numerator_icusd - collateral_value;
        let denominator = recovery_target - liq_bonus;
        let repay_amount = deficit / denominator;
        Some(repay_amount.min(vault.borrowed_icusd_amount))
    }

    /// Compute the max partial liquidation amount: enough to restore a vault's CR to
    /// recovery_target_cr. Works in all modes. Returns the full debt if the vault is
    /// so deeply undercollateralized that the formula exceeds 100%.
    pub fn compute_partial_liquidation_cap(&self, vault: &Vault, _collateral_price: UsdIcp) -> ICUSD {
        let ct = &vault.collateral_type;
        let collateral_value: ICUSD = if let Some(config) = self.get_collateral_config(ct) {
            if let Some(price) = config.last_price.and_then(Decimal::from_f64) {
                crate::numeric::collateral_usd_value(vault.collateral_amount, price, config.decimals)
            } else {
                // No price — conservatively return full debt (allows full liquidation)
                return vault.borrowed_icusd_amount;
            }
        } else {
            return vault.borrowed_icusd_amount;
        };
        let recovery_target = self.get_recovery_target_cr_for(ct);
        let liq_bonus = self.get_liquidation_bonus_for(ct);
        let numerator_icusd = vault.borrowed_icusd_amount * recovery_target;
        if numerator_icusd <= collateral_value {
            // Already at or above target — shouldn't be liquidatable, but return 0
            return ICUSD::new(0);
        }
        let deficit = numerator_icusd - collateral_value;
        let denominator = recovery_target - liq_bonus;
        let repay_amount = deficit / denominator;
        repay_amount.min(vault.borrowed_icusd_amount)
    }

    pub fn liquidate_vault(&mut self, vault_id: u64, mode: Mode, collateral_price: UsdIcp) {
        let vault = self
            .vault_id_to_vaults
            .get(&vault_id)
            .cloned()
            .expect("bug: vault not found");

        let ct = vault.collateral_type;
        let vault_collateral_ratio = compute_collateral_ratio(&vault, collateral_price, self);

        if mode == Mode::Recovery && vault_collateral_ratio > MINIMUM_COLLATERAL_RATIO {
            // Recovery mode: liquidate only enough to restore CR to recovery_target_cr
            let config = match self.get_collateral_config(&ct) {
                Some(c) => c,
                None => return, // unknown collateral — cannot liquidate
            };
            let price = match config.last_price.and_then(Decimal::from_f64) {
                Some(p) => p,
                None => return, // no price — cannot compute
            };
            let decimals = config.decimals;

            let collateral_value: ICUSD = crate::numeric::collateral_usd_value(
                vault.collateral_amount,
                price,
                decimals,
            );
            let recovery_target = self.get_recovery_target_cr_for(&ct);
            let liq_bonus = self.get_liquidation_bonus_for(&ct);
            let numerator_icusd = vault.borrowed_icusd_amount * recovery_target;

            if numerator_icusd <= collateral_value {
                return; // already at/above target
            }

            let deficit = numerator_icusd - collateral_value;
            let denominator = recovery_target - liq_bonus;
            let repay_amount = (deficit / denominator).min(vault.borrowed_icusd_amount);

            // Collateral seized = icusd_to_collateral_amount(repay_amount * bonus)
            let repay_with_bonus: ICUSD = repay_amount * liq_bonus;
            let collateral_seized = crate::numeric::icusd_to_collateral_amount(
                repay_with_bonus,
                price,
                decimals,
            ).min(vault.collateral_amount);

            match self.vault_id_to_vaults.get_mut(&vault_id) {
                Some(vault) => {
                    vault.borrowed_icusd_amount -= repay_amount;
                    vault.collateral_amount -= collateral_seized;
                }
                None => ic_cdk::trap("liquidating unknown vault"),
            }
        } else {
            // Full liquidation — removes vault entirely
            if let Some(vault) = self.vault_id_to_vaults.remove(&vault_id) {
                if let Some(vault_ids) = self.principal_to_vault_ids.get_mut(&vault.owner) {
                    vault_ids.remove(&vault_id);
                }
            }
        }
    }

        
    pub fn redistribute_vault(&mut self, vault_id: u64) {
        let vault = self
            .vault_id_to_vaults
            .get(&vault_id)
            .expect("bug: vault not found");
        let entries = distribute_across_vaults(&self.vault_id_to_vaults, vault.clone());
        for entry in entries {
            match self.vault_id_to_vaults.entry(entry.vault_id) {
                Occupied(mut vault_entry) => {
                    vault_entry.get_mut().collateral_amount += entry.icp_share_amount.to_u64();
                    vault_entry.get_mut().borrowed_icusd_amount += entry.icusd_share_amount;
                }
                Vacant(_) => panic!("bug: vault not found"),
            }
        }
        if let Some(vault) = self.vault_id_to_vaults.remove(&vault_id) {
            let owner = vault.owner;
            if let Some(vault_ids) = self.principal_to_vault_ids.get_mut(&owner) {
                vault_ids.remove(&vault_id);
            }
        }
    }
    
    pub fn redeem_on_vaults(
        &mut self,
        icusd_amount: ICUSD,
        collateral_price: UsdIcp,
        collateral_type: &CollateralType,
    ) {
        // Resolve config for price & decimals
        let (price, decimals) = match self.get_collateral_config(collateral_type) {
            Some(config) => {
                let p = config.last_price
                    .and_then(Decimal::from_f64)
                    .expect("bug: redeem_on_vaults called without price");
                (p, config.decimals)
            }
            None => panic!("bug: redeem_on_vaults called with unknown collateral type"),
        };

        let resolved_ct = if collateral_type == &Principal::anonymous() {
            self.icp_ledger_principal
        } else {
            *collateral_type
        };

        let mut icusd_amount_to_convert = icusd_amount;
        let mut vaults: BTreeSet<(Ratio, VaultId)> = BTreeSet::new();

        // SECURITY: Only include vaults matching the target collateral type
        for vault in self.vault_id_to_vaults.values() {
            let vault_ct = if vault.collateral_type == Principal::anonymous() {
                self.icp_ledger_principal
            } else {
                vault.collateral_type
            };
            if vault_ct != resolved_ct {
                continue;
            }
            vaults.insert((
                crate::compute_collateral_ratio(vault, collateral_price, self),
                vault.vault_id,
            ));
        }

        let vault_ids: Vec<VaultId> = vaults.iter().map(|(_cr, vault_id)| *vault_id).collect();
        let mut index: usize = 0;

        while icusd_amount_to_convert > 0 && index < vault_ids.len() {
            let vault = self.vault_id_to_vaults.get(&vault_ids[index]).unwrap();

            if vault.borrowed_icusd_amount >= icusd_amount_to_convert {
                // Convert everything on this vault
                let redeemable_collateral = crate::numeric::icusd_to_collateral_amount(
                    icusd_amount_to_convert,
                    price,
                    decimals,
                );
                self.deduct_amount_from_vault(
                    redeemable_collateral,
                    icusd_amount_to_convert,
                    vault_ids[index],
                );
                icusd_amount_to_convert = ICUSD::from(0);
                break;
            } else {
                // Convert what we can on this vault
                let redeemable_icusd_amount = vault.borrowed_icusd_amount;
                let redeemable_collateral = crate::numeric::icusd_to_collateral_amount(
                    redeemable_icusd_amount,
                    price,
                    decimals,
                );
                self.deduct_amount_from_vault(
                    redeemable_collateral,
                    redeemable_icusd_amount,
                    vault_ids[index],
                );
                icusd_amount_to_convert -= redeemable_icusd_amount;
                index += 1;
            }
        }
        debug_assert!(icusd_amount_to_convert == 0);
    }

    fn deduct_amount_from_vault(
        &mut self,
        collateral_to_deduct: u64,
        icusd_amount_to_deduct: ICUSD,
        vault_id: VaultId,
    ) {
        match self.vault_id_to_vaults.get_mut(&vault_id) {
            Some(vault) => {
                assert!(vault.borrowed_icusd_amount >= icusd_amount_to_deduct);
                vault.borrowed_icusd_amount -= icusd_amount_to_deduct;
                assert!(vault.collateral_amount >= collateral_to_deduct);
                vault.collateral_amount -= collateral_to_deduct;
            }
            None => ic_cdk::trap("cannot deduct from unknown vault"),
        }
    }

    pub fn check_semantically_eq(&self, other: &Self) -> Result<(), String> {
        ensure_eq!(
            self.vault_id_to_vaults,
            other.vault_id_to_vaults,
            "vault_id_to_vaults does not match"
        );
        ensure_eq!(
            self.pending_margin_transfers,
            other.pending_margin_transfers,
            "pending_margin_transfers does not match"
        );
        ensure_eq!(
            self.pending_excess_transfers,
            other.pending_excess_transfers,
            "pending_excess_transfers does not match"
        );
        ensure_eq!(
            self.principal_to_vault_ids,
            other.principal_to_vault_ids,
            "principal_to_vault_ids does not match"
        );
        ensure_eq!(
            self.xrc_principal,
            other.xrc_principal,
            "xrc_principal does not match"
        );
        ensure_eq!(
            self.icusd_ledger_principal,
            other.icusd_ledger_principal,
            "icusd_ledger_principal does not match"
        );
        ensure_eq!(
            self.icp_ledger_principal,
            other.icp_ledger_principal,
            "icp_ledger_principal does not match"
        );

        Ok(())
    }

    pub fn check_invariants(&self) -> Result<(), String> {
        ensure!(
            self.vault_id_to_vaults.len()
                <= self
                    .principal_to_vault_ids
                    .values()
                    .map(|set| set.len())
                    .sum::<usize>(),
            "Inconsistent vault count: {} vaults, {} vault ids",
            self.vault_id_to_vaults.len(),
            self.principal_to_vault_ids
                .values()
                .map(|set| set.len())
                .sum::<usize>(),
        );

        for vault_ids in self.principal_to_vault_ids.values() {
            for vault_id in vault_ids {
                if self.vault_id_to_vaults.get(vault_id).is_none() {
                    panic!("Not all vault ids are in the id -> Vault map.")
                }
            }
        }

        Ok(())
    }

    pub fn mark_operation_failed(&mut self, principal: &Principal) {
        if let Some(state) = self.operation_states.get_mut(principal) {
            *state = OperationState::Failed;
        }
    }
    
    // Add method to clean up stale operations regularly
    pub fn clean_stale_operations(&mut self) {
        // Get the current time
        let now = ic_cdk::api::time();
        
        // Find any operations that are stale (older than 3 minutes)
        const STALE_OPERATION_NANOS: u64 = 3 * 60 * SEC_NANOS;
        
        // Check for stale processing state based on actual Mode variants
        // Mode is likely either GeneralAvailability, Recovery, or ReadOnly
        if let Mode::Recovery = self.mode {
            // If in recovery mode for too long, consider resetting
            if let Some(last_timestamp) = self.last_icp_timestamp {
                let age = now - last_timestamp;
                
                // If operation has been in processing mode for too long, reset it
                if age > STALE_OPERATION_NANOS {
                    log!(INFO, "[clean_stale_operations] Found stale recovery state, resetting mode to GeneralAvailability");
                    self.mode = Mode::GeneralAvailability;
                }
            }
        }
    }
}

#[derive(Debug)]
pub(crate) struct DistributeEntry {
    pub owner: Principal,
    pub icp_share: ICP,
    pub icusd_to_debit: ICUSD,
}

pub(crate) struct DistributeToVaultEntry {
    pub vault_id: u64,
    pub icp_share_amount: ICP,
    pub icusd_share_amount: ICUSD,
}

pub(crate) fn distribute_across_vaults(
    vaults: &BTreeMap<u64, Vault>,
    target_vault: Vault,
) -> Vec<DistributeToVaultEntry> {
    assert!(!vaults.is_empty());

    let target_vault_id = target_vault.vault_id;
    let total_icp_margin: ICP = ICP::from(
        vaults
            .iter()
            .filter(|&(&vault_id, _vault)| vault_id != target_vault_id)
            .map(|(_vault_id, vault)| vault.collateral_amount)
            .sum::<u64>()
    );
    assert_ne!(total_icp_margin, ICP::new(0));

    let target_collateral = ICP::from(target_vault.collateral_amount);
    let mut result = vec![];
    let mut distributed_icp: ICP = ICP::new(0);
    let mut distributed_icusd: ICUSD = ICUSD::new(0);

    for (vault_id, vault) in vaults {
        if *vault_id != target_vault_id {
            let share: Ratio = ICP::from(vault.collateral_amount) / total_icp_margin;
            let icp_share = target_collateral * share;
            let icusd_share = target_vault.borrowed_icusd_amount * share;
            distributed_icp += icp_share;
            distributed_icusd += icusd_share;
            result.push(DistributeToVaultEntry {
                vault_id: *vault_id,
                icp_share_amount: icp_share,
                icusd_share_amount: icusd_share,
            })
        }
    }

    if !result.is_empty() {
        result[0].icusd_share_amount += target_vault.borrowed_icusd_amount - distributed_icusd;
        result[0].icp_share_amount += target_collateral - distributed_icp;
    }

    result
}


fn compute_redemption_fee(
    elapsed_hours: u64,
    redeemed_amount: ICUSD,
    total_borrowed_icusd_amount: ICUSD,
    current_base_rate: Ratio,
    fee_floor: Ratio,
    fee_ceiling: Ratio,
) -> Ratio {
    if total_borrowed_icusd_amount == 0 {
        return Ratio::from(Decimal::ZERO);
    }
    const REEDEMED_PROPORTION: Ratio = Ratio::new(dec!(0.5)); // 0.5
    const DECAY_FACTOR: Ratio = Ratio::new(dec!(0.94));

    log!(
        crate::INFO,
        "current_base_rate: {current_base_rate}, elapsed_hours: {elapsed_hours}"
    );

    let rate = current_base_rate * DECAY_FACTOR.pow(elapsed_hours);
    let total_rate = rate + redeemed_amount / total_borrowed_icusd_amount * REEDEMED_PROPORTION;
    debug_assert!(total_rate < Ratio::from(dec!(1.0)));
    total_rate
        .max(fee_floor)
        .min(fee_ceiling)
}



pub fn mutate_state<F, R>(f: F) -> R
where
    F: FnOnce(&mut State) -> R,
{
    __STATE.with(|s| f(s.borrow_mut().as_mut().expect("State not initialized!")))
}

/// Read (part of) the current state using `f`.
///
/// Panics if there is no state.
pub fn read_state<F, R>(f: F) -> R
where
    F: FnOnce(&State) -> R,
{
    __STATE.with(|s| f(s.borrow().as_ref().expect("State not initialized!")))
}

/// Replaces the current state.
pub fn replace_state(state: State) {
    __STATE.with(|s| {
        *s.borrow_mut() = Some(state);
    });
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_distribute_across_vaults() {
        let mut vaults = BTreeMap::new();
        let vault1 = Vault {
            owner: Principal::anonymous(),
            vault_id: 1,
            collateral_amount: 500_000,
            borrowed_icusd_amount: ICUSD::new(300_000),
            collateral_type: Principal::anonymous(),
        };

        let vault2 = Vault {
            owner: Principal::anonymous(),
            vault_id: 2,
            collateral_amount: 300_000,
            borrowed_icusd_amount: ICUSD::new(200_000),
            collateral_type: Principal::anonymous(),
        };

        vaults.insert(1, vault1);
        vaults.insert(2, vault2);

        let target_vault = Vault {
            owner: Principal::anonymous(),
            vault_id: 3,
            collateral_amount: 700_000,
            borrowed_icusd_amount: ICUSD::new(400_000),
            collateral_type: Principal::anonymous(),
        };

        let result = distribute_across_vaults(&vaults, target_vault);

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].icp_share_amount, ICP::new(437_500));
        assert_eq!(result[0].icusd_share_amount, ICUSD::new(250_000));
        assert_eq!(result[1].icp_share_amount, ICP::new(262_500));
        assert_eq!(result[1].icusd_share_amount, ICUSD::new(150_000));
    }

    #[test]
    fn test_partial_repay_reduces_debt() {
        // Initialize a minimal state
        let mut state = State::from(InitArg {
            xrc_principal: Principal::anonymous(),
            icusd_ledger_principal: Principal::anonymous(),
            icp_ledger_principal: Principal::anonymous(),
            fee_e8s: 0,
            developer_principal: Principal::anonymous(),
            treasury_principal: None,
            stability_pool_principal: None,
            ckusdt_ledger_principal: None,
            ckusdc_ledger_principal: None,
        });

        // Create a vault with some debt
        let owner = Principal::anonymous();
        let vault_id = 1u64;
        state.open_vault(Vault {
            owner,
            vault_id,
            collateral_amount: 1_000_000, // 0.01 ICP
            borrowed_icusd_amount: ICUSD::new(200_000_000), // 2 icUSD
            collateral_type: Principal::anonymous(),
        });

        // Repay 0.01 icUSD (minimum partial repay in e8s is 1_000_000)
        let repay_amount = ICUSD::new(1_000_000);
        state.repay_to_vault(vault_id, repay_amount);

        // Assert debt reduced correctly
        let vault = state.vault_id_to_vaults.get(&vault_id).unwrap();
        assert_eq!(vault.borrowed_icusd_amount, ICUSD::new(199_000_000));
    }

    #[test]
    fn test_recovery_mode_partial_liquidation_path() {
        // Initialize state with Recovery mode
        let mut state = State::from(InitArg {
            xrc_principal: Principal::anonymous(),
            icusd_ledger_principal: Principal::anonymous(),
            icp_ledger_principal: Principal::anonymous(),
            fee_e8s: 0,
            developer_principal: Principal::anonymous(),
            treasury_principal: None,
            stability_pool_principal: None,
            ckusdt_ledger_principal: None,
            ckusdc_ledger_principal: None,
        });
        state.mode = Mode::Recovery;

        // Set a price — must use set_icp_rate to sync CollateralConfig.last_price
        let collateral_price = UsdIcp::from(dec!(5)); // $5 per ICP
        state.set_icp_rate(collateral_price, None);

        // Vault at 140% CR (between 133% and 150%) — should get targeted liquidation
        // borrowed = 10 icUSD, margin = 2.8 ICP ⇒ collateral value = $14 ⇒ ratio = 1.4
        let owner = Principal::anonymous();
        let vault_id = 42u64;
        state.open_vault(Vault {
            owner,
            vault_id,
            collateral_amount: 280_000_000, // 2.8 ICP
            borrowed_icusd_amount: ICUSD::new(1_000_000_000), // 10 icUSD
            collateral_type: Principal::anonymous(),
        });

        // Move state into global so mutate_state/read_state work in this test.
        replace_state(state);

        // Verify CR before
        let cr_before = read_state(|s| {
            let vault = s.vault_id_to_vaults.get(&vault_id).unwrap();
            compute_collateral_ratio(vault, collateral_price, s)
        });
        assert!(cr_before.to_f64() > 1.33 && cr_before.to_f64() < 1.50,
            "CR before should be between 133% and 150%, got {}", cr_before.to_f64());

        // Execute protocol's recovery liquidation logic
        mutate_state(|s| s.liquidate_vault(vault_id, s.mode, collateral_price));

        // After recovery-mode targeted liquidation:
        // - Vault should still exist (not fully liquidated)
        // - Debt should NOT be zero
        // - CR should be approximately 1.55 (recovery_target_cr)
        let (borrowed_amount, cr_after) = read_state(|s| {
            let vault = s.vault_id_to_vaults.get(&vault_id).unwrap();
            (vault.borrowed_icusd_amount, compute_collateral_ratio(vault, collateral_price, s))
        });
        assert!(borrowed_amount > ICUSD::new(0),
            "Debt should not be zero after targeted recovery liquidation");

        let cr_f64 = cr_after.to_f64();
        assert!(cr_f64 > 1.54 && cr_f64 < 1.56,
            "CR after should be approximately 1.55 (155%), got {:.4}", cr_f64);
    }
}