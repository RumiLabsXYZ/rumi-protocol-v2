use candid::{CandidType, Deserialize, Nat, Principal};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

// ──────────────────────────────────────────────────────────────
// Registry types (dynamic, admin-configurable)
// ──────────────────────────────────────────────────────────────

/// Configuration for an accepted stablecoin deposit token.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StablecoinConfig {
    pub ledger_id: Principal,
    pub symbol: String,
    pub decimals: u8,
    /// Higher priority = consumed first during liquidations.
    /// e.g. ckstables = 2, icUSD = 1.
    pub priority: u8,
    /// false = no new deposits accepted, existing balances still withdrawable/consumable.
    pub is_active: bool,
}

/// Subset of backend CollateralConfig needed by the pool.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollateralInfo {
    pub ledger_id: Principal,
    pub symbol: String,
    pub decimals: u8,
    pub status: CollateralStatus,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CollateralStatus {
    Active,
    Paused,
    Frozen,
    Sunset,
    Deprecated,
}

// ──────────────────────────────────────────────────────────────
// Depositor types
// ──────────────────────────────────────────────────────────────

/// Per-user position in the stability pool.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DepositPosition {
    /// Stablecoin balances keyed by ledger principal, in native decimals.
    pub stablecoin_balances: BTreeMap<Principal, u64>,
    /// Claimable collateral gains keyed by collateral ledger principal.
    pub collateral_gains: BTreeMap<Principal, u64>,
    /// Collateral types this user has opted out of.
    pub opted_out_collateral: BTreeSet<Principal>,
    /// First deposit timestamp (nanos).
    pub deposit_timestamp: u64,
    /// Lifetime claimed gains per collateral type.
    pub total_claimed_gains: BTreeMap<Principal, u64>,
    /// Lifetime interest earned by this depositor (e8s, for display).
    #[serde(default)]
    pub total_interest_earned_e8s: u64,
}

impl DepositPosition {
    pub fn new(timestamp: u64) -> Self {
        Self {
            stablecoin_balances: BTreeMap::new(),
            collateral_gains: BTreeMap::new(),
            opted_out_collateral: BTreeSet::new(),
            deposit_timestamp: timestamp,
            total_claimed_gains: BTreeMap::new(),
            total_interest_earned_e8s: 0,
        }
    }

    /// Total stablecoin value in e8s (USD-equivalent).
    /// Converts each token to e8s using its decimal config.
    pub fn total_usd_value(&self, stablecoin_registry: &BTreeMap<Principal, StablecoinConfig>) -> u64 {
        self.stablecoin_balances.iter().map(|(ledger, &amount)| {
            match stablecoin_registry.get(ledger) {
                Some(config) => normalize_to_e8s(amount, config.decimals),
                None => 0,
            }
        }).sum()
    }

    /// Whether this user is opted in for a given collateral type.
    pub fn is_opted_in(&self, collateral_type: &Principal) -> bool {
        !self.opted_out_collateral.contains(collateral_type)
    }

    /// Whether the position is entirely empty (no balances, no gains).
    pub fn is_empty(&self) -> bool {
        self.stablecoin_balances.values().all(|&v| v == 0)
            && self.collateral_gains.values().all(|&v| v == 0)
    }
}

/// Convert a token amount from its native decimals to e8s (8 decimal places).
/// Uses saturating arithmetic to prevent overflow on large amounts.
pub fn normalize_to_e8s(amount: u64, decimals: u8) -> u64 {
    match decimals.cmp(&8) {
        std::cmp::Ordering::Equal => amount,
        std::cmp::Ordering::Less => amount.saturating_mul(10u64.pow((8 - decimals) as u32)),
        std::cmp::Ordering::Greater => amount / 10u64.pow((decimals - 8) as u32),
    }
}

/// Convert an e8s amount to a token's native decimals.
/// Uses saturating arithmetic to prevent overflow on large amounts.
pub fn normalize_from_e8s(amount_e8s: u64, decimals: u8) -> u64 {
    match decimals.cmp(&8) {
        std::cmp::Ordering::Equal => amount_e8s,
        std::cmp::Ordering::Less => amount_e8s / 10u64.pow((8 - decimals) as u32),
        std::cmp::Ordering::Greater => amount_e8s.saturating_mul(10u64.pow((decimals - 8) as u32)),
    }
}

// ──────────────────────────────────────────────────────────────
// Liquidation types
// ──────────────────────────────────────────────────────────────

/// Info pushed from backend to pool when vaults become liquidatable.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiquidatableVaultInfo {
    pub vault_id: u64,
    pub collateral_type: Principal,
    pub debt_amount: u64,         // icUSD e8s
    pub collateral_amount: u64,   // native decimals
}

/// Result of a single liquidation attempt.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiquidationResult {
    pub vault_id: u64,
    pub stables_consumed: BTreeMap<Principal, u64>, // ledger -> amount consumed (native decimals)
    pub collateral_gained: u64,                      // native decimals of collateral received
    pub collateral_type: Principal,
    pub success: bool,
    pub error_message: Option<String>,
}

/// Audit trail record for a completed liquidation.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolLiquidationRecord {
    pub vault_id: u64,
    pub timestamp: u64,
    pub stables_consumed: BTreeMap<Principal, u64>,
    pub collateral_gained: u64,
    pub collateral_type: Principal,
    pub depositors_count: u64,
}

// ──────────────────────────────────────────────────────────────
// Init / Config / API types
// ──────────────────────────────────────────────────────────────

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StabilityPoolInitArgs {
    pub protocol_canister_id: Principal,
    pub authorized_admins: Vec<Principal>,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolConfiguration {
    pub min_deposit_e8s: u64,
    pub max_liquidations_per_batch: u64,
    pub emergency_pause: bool,
    pub authorized_admins: Vec<Principal>,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StabilityPoolStatus {
    pub total_deposits_e8s: u64,
    pub total_depositors: u64,
    pub total_liquidations_executed: u64,
    pub stablecoin_balances: BTreeMap<Principal, u64>,
    pub collateral_gains: BTreeMap<Principal, u64>,
    pub stablecoin_registry: Vec<StablecoinConfig>,
    pub collateral_registry: Vec<CollateralInfo>,
    pub emergency_paused: bool,
    pub total_interest_received_e8s: u64,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserStabilityPosition {
    pub stablecoin_balances: BTreeMap<Principal, u64>,
    pub collateral_gains: BTreeMap<Principal, u64>,
    pub opted_out_collateral: Vec<Principal>,
    pub deposit_timestamp: u64,
    pub total_claimed_gains: BTreeMap<Principal, u64>,
    pub total_usd_value_e8s: u64,
    pub total_interest_earned_e8s: u64,
}

// ──────────────────────────────────────────────────────────────
// Error types
// ──────────────────────────────────────────────────────────────

#[derive(CandidType, Debug, Clone, Deserialize)]
pub enum StabilityPoolError {
    InsufficientBalance { token: Principal, required: u64, available: u64 },
    AmountTooLow { minimum_e8s: u64 },
    NoPositionFound,
    InsufficientPoolBalance,
    Unauthorized,
    TokenNotAccepted { ledger: Principal },
    TokenNotActive { ledger: Principal },
    CollateralNotFound { ledger: Principal },
    LedgerTransferFailed { reason: String },
    InterCanisterCallFailed { target: String, method: String },
    LiquidationFailed { vault_id: u64, reason: String },
    EmergencyPaused,
    SystemBusy,
    AlreadyOptedOut { collateral: Principal },
    AlreadyOptedIn { collateral: Principal },
}

// ──────────────────────────────────────────────────────────────
// ICRC-21: Canister Call Consent Messages
// ──────────────────────────────────────────────────────────────

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct Icrc21ConsentMessageRequest {
    pub method: String,
    pub arg: Vec<u8>,
    pub user_preferences: Icrc21ConsentMessageSpec,
}

/// Per the ICRC-21 spec, `user_preferences` is a `consent_message_spec`
/// containing nested metadata + an optional device_spec.
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct Icrc21ConsentMessageSpec {
    pub metadata: Icrc21ConsentMessageMetadata,
    pub device_spec: Option<Icrc21DeviceSpec>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum Icrc21DeviceSpec {
    GenericDisplay,
    LineDisplay {
        characters_per_line: u16,
        lines_per_page: u16,
    },
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct Icrc21ConsentMessageMetadata {
    pub language: String,
    pub utc_offset_minutes: Option<i16>,
}

#[derive(CandidType, Serialize, Clone, Debug)]
pub enum Icrc21ConsentMessageResponse {
    #[serde(rename = "Ok")]
    Ok(Icrc21ConsentInfo),
    #[serde(rename = "Err")]
    Err(Icrc21Error),
}

#[derive(CandidType, Serialize, Clone, Debug)]
pub struct Icrc21ConsentInfo {
    pub consent_message: Icrc21ConsentMessage,
    pub metadata: Icrc21ConsentMessageResponseMetadata,
}

#[derive(CandidType, Serialize, Clone, Debug)]
pub struct Icrc21ConsentMessageResponseMetadata {
    pub language: String,
    pub utc_offset_minutes: Option<i16>,
}

#[derive(CandidType, Serialize, Clone, Debug)]
pub enum Icrc21ConsentMessage {
    GenericDisplayMessage(String),
    LineDisplayMessage { pages: Vec<Icrc21LineDisplayPage> },
}

#[derive(CandidType, Serialize, Clone, Debug)]
pub struct Icrc21LineDisplayPage {
    pub lines: Vec<Icrc21LineDisplayLine>,
}

#[derive(CandidType, Serialize, Clone, Debug)]
pub struct Icrc21LineDisplayLine {
    pub line: String,
}

#[derive(CandidType, Serialize, Clone, Debug)]
pub enum Icrc21Error {
    UnsupportedCanisterCall(Icrc21ErrorInfo),
    ConsentMessageUnavailable(Icrc21ErrorInfo),
    GenericError { error_code: Nat, description: String },
}

#[derive(CandidType, Serialize, Clone, Debug)]
pub struct Icrc21ErrorInfo {
    pub description: String,
}

// ──────────────────────────────────────────────────────────────
// ICRC-10: Supported Standards
// ──────────────────────────────────────────────────────────────

#[derive(CandidType, Serialize, Clone, Debug)]
pub struct Icrc10SupportedStandard {
    pub name: String,
    pub url: String,
}
