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
    /// ICRC-1 transfer/approve fee in native token units.
    /// Used to deduct approve fees from tracked balances so accounting stays accurate.
    #[serde(default)]
    pub transfer_fee: Option<u64>,
    /// True if this token is an LP token requiring special liquidation handling.
    #[serde(default)]
    pub is_lp_token: Option<bool>,
    /// Pool canister to call for LP token burn operations (e.g., 3pool canister).
    #[serde(default)]
    pub underlying_pool: Option<Principal>,
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
    /// `Option` is required for Candid backward-compatible stable memory upgrades.
    #[serde(default)]
    pub total_interest_earned_e8s: Option<u64>,
}

impl DepositPosition {
    pub fn new(timestamp: u64) -> Self {
        Self {
            stablecoin_balances: BTreeMap::new(),
            collateral_gains: BTreeMap::new(),
            opted_out_collateral: BTreeSet::new(),
            deposit_timestamp: timestamp,
            total_claimed_gains: BTreeMap::new(),
            total_interest_earned_e8s: Some(0),
        }
    }

    /// Total stablecoin value in e8s (USD-equivalent).
    /// Converts each token to e8s using its decimal config.
    /// LP tokens are valued using their virtual price instead of 1:1 normalization.
    pub fn total_usd_value(
        &self,
        stablecoin_registry: &BTreeMap<Principal, StablecoinConfig>,
        virtual_prices: &BTreeMap<Principal, u128>,
    ) -> u64 {
        self.stablecoin_balances.iter().map(|(ledger, &amount)| {
            match stablecoin_registry.get(ledger) {
                Some(config) if config.is_lp_token.unwrap_or(false) => {
                    virtual_prices.get(ledger)
                        .map(|&vp| lp_to_usd_e8s(amount, vp))
                        .unwrap_or(0)
                }
                Some(config) => normalize_to_e8s(amount, config.decimals),
                None => 0,
            }
        }).sum()
    }

    /// icUSD-only stablecoin value in e8s.
    /// Identifies the icUSD ledger by symbol from the registry. Returns 0 if
    /// no icUSD ledger is registered or the depositor holds no icUSD.
    /// icUSD is 8 decimals, so the raw balance is the e8s value directly.
    pub fn icusd_value(
        &self,
        stablecoin_registry: &BTreeMap<Principal, StablecoinConfig>,
    ) -> u64 {
        let icusd_ledger = stablecoin_registry.iter()
            .find(|(_, c)| c.symbol == "icUSD")
            .map(|(id, _)| *id);

        match icusd_ledger {
            Some(ledger) => self.stablecoin_balances.get(&ledger).copied().unwrap_or(0),
            None => 0,
        }
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

/// Inverse of `normalize_to_e8s`: convert an e8s (8-decimal) amount back to a
/// token's native decimals. SP-101 uses this to express the backend's realized
/// icUSD-denominated debt (always e8s) in the drawn stable token's native units
/// (e.g. ckUSDT/ckUSDC at 6 decimals).
pub fn denormalize_from_e8s(amount_e8s: u64, decimals: u8) -> u64 {
    match decimals.cmp(&8) {
        std::cmp::Ordering::Equal => amount_e8s,
        std::cmp::Ordering::Less => amount_e8s / 10u64.pow((8 - decimals) as u32),
        std::cmp::Ordering::Greater => amount_e8s.saturating_mul(10u64.pow((decimals - 8) as u32)),
    }
}

/// Convert a 3USD (LP token) amount to its USD value in e8s using virtual price.
/// `virtual_price` is scaled by 1e18, LP token has 8 decimals.
/// Result: amount_e8s = lp_amount * virtual_price / 1e18
pub fn lp_to_usd_e8s(lp_amount: u64, virtual_price: u128) -> u64 {
    (lp_amount as u128 * virtual_price / 1_000_000_000_000_000_000u128) as u64
}

/// Convert a USD e8s amount to the equivalent 3USD LP token amount.
pub fn usd_e8s_to_lp(usd_e8s: u64, virtual_price: u128) -> u64 {
    if virtual_price == 0 { return 0; }
    (usd_e8s as u128 * 1_000_000_000_000_000_000u128 / virtual_price) as u64
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
    /// Recommended partial liquidation amount (e8s). Sent by backend, 0 if unknown.
    #[serde(default)]
    pub recommended_liquidation_amount: u64,
    /// Collateral price in e8s (USD). Sent by backend, 0 if unknown.
    #[serde(default)]
    pub collateral_price_e8s: u64,
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
    /// USD price of the collateral at liquidation time (e8s), for future ROI calculations.
    /// `Option` is required for Candid backward-compatible stable memory upgrades.
    #[serde(default)]
    pub collateral_price_e8s: Option<u64>,
}

/// Tokens the pool still owes a user after a failed `deposit_as_3usd` refund,
/// recoverable via `claim_pending_refund` instead of being silently stranded
/// (audit IC-S-001). Mirrors rumi_3pool's `ThreePoolPendingClaim` pattern.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingRefund {
    pub id: u64,
    pub user: Principal,
    pub token_ledger: Principal,
    /// Gross amount still held by the pool (native decimals). The payout
    /// sends this minus the ledger transfer fee.
    pub amount: u64,
    pub reason: String,
    pub created_at: u64,
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
    /// Per-collateral eligible icUSD (e8s): for each collateral type, the total
    /// icUSD deposited by users who are opted in to that collateral.
    /// Used by the frontend to compute per-collateral APR.
    pub eligible_icusd_per_collateral: Vec<(Principal, u64)>,
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
    RefundClaimNotFound,
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

// ──────────────────────────────────────────────────────────────
// 3Pool Interop Types (for virtual price queries and deposits)
// ──────────────────────────────────────────────────────────────

/// Minimal subset of the 3pool's PoolStatus for virtual price queries.
/// Fields we don't need are omitted — Candid deserialization skips unknown fields.
#[derive(CandidType, Clone, Debug, Deserialize)]
pub struct ThreePoolStatus {
    pub virtual_price: u128,
    pub tokens: Vec<ThreePoolTokenInfo>,
}

#[derive(CandidType, Clone, Debug, Deserialize)]
pub struct ThreePoolTokenInfo {
    pub ledger_id: Principal,
    pub symbol: String,
    pub decimals: u8,
}

/// Remote 3pool error (for deserialization only).
#[derive(CandidType, Clone, Debug, Deserialize)]
pub enum ThreePoolErrorRemote {
    InsufficientOutput { expected_min: u128, actual: u128 },
    InsufficientLiquidity,
    InvalidCoinIndex,
    ZeroAmount,
    PoolEmpty,
    SlippageExceeded,
    TransferFailed { token: String, reason: String },
    Unauthorized,
    MathOverflow,
    InvariantNotConverged,
    PoolPaused,
    NotAuthorizedBurnCaller,
    BurnSlippageExceeded { max_bps: u16, actual_bps: u16 },
    InsufficientPoolBalance { token: String, required: u128, available: u128 },
    InsufficientLpBalance { required: u128, available: u128 },
    BurnFailed { token: String, reason: String },
    /// Audit fence B-01 (Wave 14a): rumi_3pool returns this when another
    /// concurrent operation holds the pool lock. The SP `add_liquidity`
    /// path will refund the user and surface a transient call failure.
    PoolLocked,
}

// ──────────────────────────────────────────────────────────────
// Pool Events (audit trail for deposits, withdrawals, claims, interest)
// ──────────────────────────────────────────────────────────────

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct PoolEvent {
    pub id: u64,
    pub timestamp: u64,
    pub caller: Principal,
    pub event_type: PoolEventType,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub enum PoolEventType {
    Deposit {
        token_ledger: Principal,
        amount: u64,
    },
    Withdraw {
        token_ledger: Principal,
        amount: u64,
    },
    ClaimCollateral {
        collateral_ledger: Principal,
        amount: u64,
    },
    DepositAs3USD {
        token_ledger: Principal,
        amount_in: u64,
        lp_minted: u64,
    },
    InterestReceived {
        token_ledger: Principal,
        amount: u64,
    },
    // ─── Collateral Opt-in/Opt-out ───
    OptOutCollateral {
        collateral_type: Principal,
    },
    OptInCollateral {
        collateral_type: Principal,
    },
    // ─── Liquidation Events ───
    LiquidationNotification {
        vault_count: u64,
    },
    LiquidationExecuted {
        vault_id: u64,
        stables_consumed_e8s: u64,
        collateral_gained: u64,
        collateral_type: Principal,
        success: bool,
    },
    // ─── Admin: Registry ───
    StablecoinRegistered {
        ledger: Principal,
        symbol: String,
    },
    CollateralRegistered {
        ledger: Principal,
        symbol: String,
    },
    // ─── Admin: Configuration ───
    ConfigurationUpdated,
    EmergencyPauseActivated,
    OperationsResumed,
    // ─── Admin: Balance Corrections ───
    BalanceCorrected {
        user: Principal,
        token_ledger: Principal,
        new_amount: u64,
    },
    CollateralGainCorrected {
        user: Principal,
        collateral_ledger: Principal,
        new_amount: u64,
    },
}

/// Arguments for the 3pool's authorized redeem-and-burn operation.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct AuthorizedRedeemAndBurnArgs {
    pub token_ledger: Principal,
    pub token_amount: u128,
    pub lp_amount: u128,
    pub max_slippage_bps: u16,
}

/// Result of a successful 3pool redeem-and-burn operation.
#[derive(CandidType, Clone, Debug, Deserialize)]
pub struct RedeemAndBurnResult {
    pub token_amount_burned: u128,
    pub lp_amount_burned: u128,
    pub burn_block_index: u64,
}

#[cfg(test)]
mod icusd_value_tests {
    use super::*;
    use candid::Principal;
    use std::collections::BTreeMap;

    fn config(ledger: Principal, symbol: &str, decimals: u8, priority: u8, is_lp: bool) -> StablecoinConfig {
        StablecoinConfig {
            ledger_id: ledger,
            symbol: symbol.to_string(),
            decimals,
            priority,
            is_active: true,
            transfer_fee: None,
            is_lp_token: if is_lp { Some(true) } else { None },
            underlying_pool: None,
        }
    }

    #[test]
    fn icusd_value_returns_only_icusd_balance() {
        let icusd = Principal::from_text("t6bor-paaaa-aaaap-qrd5q-cai").unwrap();
        let ckusdc = Principal::from_text("xevnm-gaaaa-aaaar-qafnq-cai").unwrap();
        let three_usd = Principal::from_text("fohh4-yyaaa-aaaap-qtkpa-cai").unwrap();

        let mut registry = BTreeMap::new();
        registry.insert(icusd, config(icusd, "icUSD", 8, 1, false));
        registry.insert(ckusdc, config(ckusdc, "ckUSDC", 6, 1, false));
        registry.insert(three_usd, config(three_usd, "3USD", 8, 1, true));

        let mut pos = DepositPosition::new(0);
        pos.stablecoin_balances.insert(icusd, 100_00_000_000); // 100 icUSD (e8s)
        pos.stablecoin_balances.insert(ckusdc, 200_000_000);   // 200 ckUSDC (e6s)
        pos.stablecoin_balances.insert(three_usd, 50_00_000_000); // 50 3USD (e8s)

        let v = pos.icusd_value(&registry);
        assert_eq!(v, 100_00_000_000, "should return only the icUSD balance in e8s");
    }

    #[test]
    fn icusd_value_zero_when_no_icusd_balance() {
        let icusd = Principal::from_text("t6bor-paaaa-aaaap-qrd5q-cai").unwrap();
        let ckusdc = Principal::from_text("xevnm-gaaaa-aaaar-qafnq-cai").unwrap();

        let mut registry = BTreeMap::new();
        registry.insert(icusd, config(icusd, "icUSD", 8, 1, false));
        registry.insert(ckusdc, config(ckusdc, "ckUSDC", 6, 1, false));

        let mut pos = DepositPosition::new(0);
        pos.stablecoin_balances.insert(ckusdc, 500_000_000);

        assert_eq!(pos.icusd_value(&registry), 0);
    }

    #[test]
    fn icusd_value_zero_when_registry_has_no_icusd() {
        let icusd = Principal::from_text("t6bor-paaaa-aaaap-qrd5q-cai").unwrap();
        let registry: BTreeMap<Principal, StablecoinConfig> = BTreeMap::new();

        let mut pos = DepositPosition::new(0);
        pos.stablecoin_balances.insert(icusd, 100_00_000_000);

        assert_eq!(pos.icusd_value(&registry), 0,
            "registry without icUSD config means we can't identify which ledger is icUSD");
    }
}
