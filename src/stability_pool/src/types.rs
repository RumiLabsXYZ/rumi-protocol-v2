use candid::{CandidType, Deserialize, Nat, Principal};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

pub const BOB_COLLATERAL_PRINCIPAL: &str = "7pail-xaaaa-aaaas-aabmq-cai";

pub fn bob_collateral() -> Principal {
    Principal::from_text(BOB_COLLATERAL_PRINCIPAL)
        .expect("BOB collateral principal is a valid principal")
}

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
    /// Chain collateral sentinels this user has explicitly opted into.
    /// Chain-native collateral is default-out; normal collateral remains
    /// default-in via `opted_out_collateral`.
    #[serde(default)]
    pub opted_in_chain_collateral: Option<BTreeSet<Principal>>,
    /// First deposit timestamp (nanos).
    pub deposit_timestamp: u64,
    /// Lifetime claimed gains per collateral type.
    pub total_claimed_gains: BTreeMap<Principal, u64>,
    /// Lifetime interest earned by this depositor (e8s, for display).
    /// `Option` is required for Candid backward-compatible stable memory upgrades.
    #[serde(default)]
    pub total_interest_earned_e8s: Option<u64>,
    /// Claimable native-chain collateral keyed by deterministic chain sentinel,
    /// in native base units (wei for CFX). `Option` is required for Candid
    /// backward-compatible stable memory upgrades.
    #[serde(default)]
    pub cfx_claims: Option<BTreeMap<Principal, u128>>,
    /// Payout destinations for native/off-IC collateral types keyed by synthetic
    /// collateral principal. XRP uses this as its opt-in marker: no address means
    /// the depositor does not absorb or earn from XRP liquidations.
    #[serde(default)]
    pub native_payout_addresses: Option<BTreeMap<Principal, String>>,
    /// Optional XRP Ledger destination tags keyed by native collateral principal.
    /// Stored separately from `native_payout_addresses` so the original address-
    /// only field remains wire-compatible for older clients.
    #[serde(default)]
    pub native_payout_destination_tags: Option<BTreeMap<Principal, u32>>,
    /// Pending native-XRP payout reminders keyed by backend XrpClaim id. These
    /// are UI retry/cleanup records only; backend claim custody remains
    /// authoritative.
    #[serde(default)]
    pub pending_native_xrp_payouts: Option<BTreeMap<u64, NativeXrpPendingPayout>>,
}

impl DepositPosition {
    pub fn new(timestamp: u64) -> Self {
        // BOB is being sunset. Existing depositors retain their current
        // exposure until they opt out, while every new position is default-out.
        Self {
            stablecoin_balances: BTreeMap::new(),
            collateral_gains: BTreeMap::new(),
            opted_out_collateral: BTreeSet::from([bob_collateral()]),
            opted_in_chain_collateral: Some(BTreeSet::new()),
            deposit_timestamp: timestamp,
            total_claimed_gains: BTreeMap::new(),
            total_interest_earned_e8s: Some(0),
            cfx_claims: Some(BTreeMap::new()),
            native_payout_addresses: Some(BTreeMap::new()),
            native_payout_destination_tags: Some(BTreeMap::new()),
            pending_native_xrp_payouts: Some(BTreeMap::new()),
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
        self.stablecoin_balances
            .iter()
            .map(|(ledger, &amount)| match stablecoin_registry.get(ledger) {
                Some(config) if config.is_lp_token.unwrap_or(false) => virtual_prices
                    .get(ledger)
                    .map(|&vp| lp_to_usd_e8s(amount, vp))
                    .unwrap_or(0),
                Some(config) => normalize_to_e8s(amount, config.decimals),
                None => 0,
            })
            .sum()
    }

    /// icUSD-only stablecoin value in e8s.
    /// Identifies the icUSD ledger by symbol from the registry. Returns 0 if
    /// no icUSD ledger is registered or the depositor holds no icUSD.
    /// icUSD is 8 decimals, so the raw balance is the e8s value directly.
    pub fn icusd_value(&self, stablecoin_registry: &BTreeMap<Principal, StablecoinConfig>) -> u64 {
        let icusd_ledger = stablecoin_registry
            .iter()
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

    /// Whether this user has explicitly opted in for a chain-native sentinel.
    pub fn is_opted_in_for_chain(&self, sentinel: &Principal) -> bool {
        self.opted_in_chain_collateral
            .as_ref()
            .map(|s| s.contains(sentinel))
            .unwrap_or(false)
    }

    /// Whether the position is entirely empty (no balances, no gains).
    pub fn is_empty(&self) -> bool {
        self.stablecoin_balances.values().all(|&v| v == 0)
            && self.collateral_gains.values().all(|&v| v == 0)
            && self
                .cfx_claims
                .as_ref()
                .map(|m| m.values().all(|&v| v == 0))
                .unwrap_or(true)
            && self
                .pending_native_xrp_payouts
                .as_ref()
                .map(|m| m.is_empty())
                .unwrap_or(true)
    }
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeXrpPendingPayout {
    pub claim_id: u64,
    pub collateral_type: Principal,
    pub vault_id: u64,
    pub drops: u64,
    pub payout_address: String,
    pub destination_tag: Option<u32>,
    pub created_at_ns: u64,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeXrpPayoutAllocation {
    pub claimant: Principal,
    pub payout_address: String,
    pub destination_tag: Option<u32>,
    pub drops: u64,
}

pub type XrpSpAbsorbPreflight = rumi_protocol_backend::XrpSpAbsorbPreflight;
pub type XrpSpPayoutAllocation = rumi_protocol_backend::XrpSpPayoutAllocation;
pub type XrpSpAbsorbRequest = rumi_protocol_backend::XrpSpAbsorbRequest;
pub type XrpSpPayoutClaim = rumi_protocol_backend::XrpSpPayoutClaim;
pub type XrpSpAbsorbResult = rumi_protocol_backend::XrpSpAbsorbResult;

#[derive(CandidType, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NativeXrpAbsorbIntentStatus {
    Prepared,
    Burned,
    BackendAccepted,
    LocalApplied,
    BackendRejected,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeXrpAbsorbIntent {
    pub vault_id: u64,
    pub collateral_type: Principal,
    pub icusd_ledger: Principal,
    pub icusd_minting_account: icrc_ledger_types::icrc1::account::Account,
    pub icusd_to_burn_e8s: u64,
    pub stables_consumed: BTreeMap<Principal, u64>,
    pub collateral_received_drops: u64,
    pub collateral_price_e8s: u64,
    pub allocations: Vec<XrpSpPayoutAllocation>,
    pub burn_created_at_time_ns: u64,
    pub status: NativeXrpAbsorbIntentStatus,
    pub burn_proof: Option<rumi_protocol_backend::icrc3_proof::SpWritedownProof>,
    pub backend_result: Option<XrpSpAbsorbResult>,
    pub last_error: Option<String>,
    pub created_at_ns: u64,
    pub updated_at_ns: u64,
}

impl From<NativeXrpPayoutAllocation> for XrpSpPayoutAllocation {
    fn from(allocation: NativeXrpPayoutAllocation) -> Self {
        Self {
            claimant: allocation.claimant,
            payout_address: allocation.payout_address,
            destination_tag: allocation.destination_tag,
            drops: allocation.drops,
        }
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
    if virtual_price == 0 {
        return 0;
    }
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
    pub debt_amount: u64,       // icUSD e8s
    pub collateral_amount: u64, // native decimals
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
    pub collateral_gained: u64,                     // native decimals of collateral received
    pub collateral_type: Principal,
    pub success: bool,
    pub error_message: Option<String>,
}

/// Mirror of the backend's `ChainLiquidatableVault` Candid record. Kept local
/// because the backend exports that type from its canister binary, not its lib.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainLiquidatableVaultInfo {
    pub vault_id: u64,
    pub chain_id: rumi_protocol_backend::chains::config::ChainId,
    pub chain_collateral_sentinel: Principal,
    pub sp_attempted: bool,
    pub debt_e8s: u128,
    pub effective_debt_e8s: u128,
    pub collateral_native: u128,
    pub cr_e4: u64,
    pub liquidation_threshold_e4: u64,
    pub sized_repay_e8s: u128,
}

/// Mirror of the backend's `ChainStabilityPoolLiquidationResult`.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainStabilityPoolLiquidationResult {
    pub success: bool,
    pub vault_id: u64,
    pub chain_id: rumi_protocol_backend::chains::config::ChainId,
    pub liquidated_debt_e8s: u128,
    pub collateral_received_native: u128,
    pub claim_id: u64,
    pub custody_address: String,
    pub block_index: u64,
    pub collateral_price_e8s: u64,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainSpAbsorbResult {
    pub success: bool,
    pub vault_id: u64,
    pub chain_id: rumi_protocol_backend::chains::config::ChainId,
    pub icusd_burned_e8s: u64,
    pub liquidated_debt_e8s: u128,
    pub collateral_received_native: u128,
    pub claim_id: u64,
    pub custody_address: String,
    pub block_index: u64,
    pub collateral_price_e8s: u64,
}

#[derive(CandidType, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChainSpAbsorbIntentStatus {
    Prepared,
    Burned,
    BackendAccepted,
    LocalApplied,
    BackendRejected,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainSpAbsorbCandidate {
    pub vault: ChainLiquidatableVaultInfo,
    pub icusd_to_burn_e8s: u64,
    pub pending_status: Option<ChainSpAbsorbIntentStatus>,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainSpAbsorbIntent {
    pub vault_id: u64,
    pub chain_id: rumi_protocol_backend::chains::config::ChainId,
    pub chain_sentinel: Principal,
    pub icusd_ledger: Principal,
    pub icusd_minting_account: icrc_ledger_types::icrc1::account::Account,
    pub icusd_to_burn_e8s: u64,
    pub stables_consumed: BTreeMap<Principal, u64>,
    pub burn_created_at_time_ns: u64,
    pub status: ChainSpAbsorbIntentStatus,
    pub burn_proof: Option<rumi_protocol_backend::icrc3_proof::SpWritedownProof>,
    pub backend_result: Option<ChainStabilityPoolLiquidationResult>,
    pub last_error: Option<String>,
    pub created_at_ns: u64,
    pub updated_at_ns: u64,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainSpAbsorbCompletion {
    pub vault_id: u64,
    pub result: ChainSpAbsorbResult,
    pub completed_at_ns: u64,
}

pub const MIN_CHAIN_ABSORB_AUTO_INTERVAL_SECONDS: u64 = 60;
pub const DEFAULT_CHAIN_ABSORB_AUTO_INTERVAL_SECONDS: u64 = 300;
pub const DEFAULT_CHAIN_ABSORB_AUTO_MAX_SCAN_PER_CHAIN: u64 = 1;
pub const MAX_CHAIN_ABSORB_AUTO_SCAN_PER_CHAIN: u64 = 500;

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainAbsorbAutoConfig {
    pub enabled: bool,
    pub interval_seconds: u64,
    pub max_scan_per_chain: u64,
}

impl Default for ChainAbsorbAutoConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_seconds: DEFAULT_CHAIN_ABSORB_AUTO_INTERVAL_SECONDS,
            max_scan_per_chain: DEFAULT_CHAIN_ABSORB_AUTO_MAX_SCAN_PER_CHAIN,
        }
    }
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainAbsorbAutoTickRecord {
    pub started_at_ns: u64,
    pub completed_at_ns: u64,
    pub attempted_vault_id: Option<u64>,
    pub candidates_scanned: u64,
    pub absorbed: Option<ChainSpAbsorbResult>,
    pub error: Option<String>,
    pub skipped_reason: Option<String>,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainAbsorbAutoStatus {
    pub config: ChainAbsorbAutoConfig,
    pub tick_in_flight: bool,
    pub last_tick: Option<ChainAbsorbAutoTickRecord>,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainClaimSource {
    pub claim_id: u64,
    pub remaining_native: u128,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CfxClaimPayoutRecoveryKey {
    pub chain_sentinel: Principal,
    pub op_id: u64,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CfxClaimPayoutRecovery {
    pub chain_sentinel: Principal,
    pub op_id: u64,
    pub claim_id: u64,
    pub claimant: Principal,
    pub amount_wei: u128,
    pub reason: String,
    pub failed_at_ns: u64,
}

impl CfxClaimPayoutRecovery {
    pub fn key(&self) -> CfxClaimPayoutRecoveryKey {
        CfxClaimPayoutRecoveryKey {
            chain_sentinel: self.chain_sentinel,
            op_id: self.op_id,
        }
    }
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CfxClaimPayoutRecoveryRecord {
    pub key: CfxClaimPayoutRecoveryKey,
    pub claim_id: u64,
    pub claimant: Principal,
    pub amount_wei: u128,
    pub reason: String,
    pub failed_at_ns: u64,
    pub recovered_at_ns: u64,
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
    pub cfx_claims: Option<BTreeMap<Principal, u128>>,
    pub opted_out_collateral: Vec<Principal>,
    /// `Option` (candid `opt`) so a frontend built with this declaration can
    /// still decode a `get_user_position` response from an older SP canister
    /// that predates native collateral (the field simply decodes to `None`).
    /// A required field here silently breaks position decoding whenever the
    /// frontend is deployed ahead of the canister.
    pub native_payout_addresses: Option<BTreeMap<Principal, String>>,
    pub native_payout_destination_tags: Option<BTreeMap<Principal, u32>>,
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
    InsufficientBalance {
        token: Principal,
        required: u64,
        available: u64,
    },
    AmountTooLow {
        minimum_e8s: u64,
    },
    NoPositionFound,
    InsufficientPoolBalance,
    Unauthorized,
    TokenNotAccepted {
        ledger: Principal,
    },
    TokenNotActive {
        ledger: Principal,
    },
    CollateralNotFound {
        ledger: Principal,
    },
    LedgerTransferFailed {
        reason: String,
    },
    InterCanisterCallFailed {
        target: String,
        method: String,
    },
    LiquidationFailed {
        vault_id: u64,
        reason: String,
    },
    EmergencyPaused,
    SystemBusy,
    AlreadyOptedOut {
        collateral: Principal,
    },
    AlreadyOptedIn {
        collateral: Principal,
    },
    PayoutAddressRequired {
        collateral: Principal,
    },
    InvalidPayoutAddress {
        reason: String,
    },
    XrpClaimStillOutstanding {
        claim_id: u64,
    },
    XrpClaimStatusCheckFailed {
        reason: String,
    },
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
    GenericError {
        error_code: Nat,
        description: String,
    },
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
    InsufficientOutput {
        expected_min: u128,
        actual: u128,
    },
    InsufficientLiquidity,
    InvalidCoinIndex,
    ZeroAmount,
    PoolEmpty,
    SlippageExceeded,
    TransferFailed {
        token: String,
        reason: String,
    },
    Unauthorized,
    MathOverflow,
    InvariantNotConverged,
    PoolPaused,
    NotAuthorizedBurnCaller,
    BurnSlippageExceeded {
        max_bps: u16,
        actual_bps: u16,
    },
    InsufficientPoolBalance {
        token: String,
        required: u128,
        available: u128,
    },
    InsufficientLpBalance {
        required: u128,
        available: u128,
    },
    BurnFailed {
        token: String,
        reason: String,
    },
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

    fn config(
        ledger: Principal,
        symbol: &str,
        decimals: u8,
        priority: u8,
        is_lp: bool,
    ) -> StablecoinConfig {
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
        pos.stablecoin_balances.insert(ckusdc, 200_000_000); // 200 ckUSDC (e6s)
        pos.stablecoin_balances.insert(three_usd, 50_00_000_000); // 50 3USD (e8s)

        let v = pos.icusd_value(&registry);
        assert_eq!(
            v, 100_00_000_000,
            "should return only the icUSD balance in e8s"
        );
    }

    #[test]
    fn new_depositor_is_opted_out_of_sunset_bob_liquidations() {
        let bob = bob_collateral();
        let position = DepositPosition::new(0);

        assert!(position.opted_out_collateral.contains(&bob));
        assert!(!position.is_opted_in(&bob));
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

        assert_eq!(
            pos.icusd_value(&registry),
            0,
            "registry without icUSD config means we can't identify which ledger is icUSD"
        );
    }
}
