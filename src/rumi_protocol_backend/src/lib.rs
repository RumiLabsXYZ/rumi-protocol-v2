use serde::Serialize;
use icrc_ledger_types::icrc1::transfer::TransferError;
use icrc_ledger_types::icrc2::transfer_from::TransferFromError;
use crate::state::PendingMarginTransfer;

use crate::guard::GuardError;
use crate::logs::{DEBUG, INFO};
use crate::numeric::{Ratio, ICUSD, ICP, UsdIcp};
use crate::state::{mutate_state, read_state, Mode};
use crate::vault::Vault;
use candid::{CandidType, Deserialize, Principal};
use ic_canister_log::log;
use num_traits::ToPrimitive;
use rust_decimal::prelude::FromPrimitive;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;


/// Maximum number of retries for a pending transfer before it is abandoned.
/// At 5-second intervals, 60 retries = 5 minutes of attempts.
const MAX_PENDING_RETRIES: u8 = 60;

pub mod dashboard;
pub mod event;
pub mod guard;
pub mod icrc21;
pub mod icrc3_proof;
pub mod liquidity_pool;
pub mod logs;
pub mod management;
pub mod numeric;
pub mod state;
pub mod storage;
pub mod treasury;
pub mod vault;
pub mod xrc;

#[cfg(any(test, feature = "test_endpoints"))]
pub mod test_helpers; 

#[cfg(test)]
mod tests;

pub const SEC_NANOS: u64 = 1_000_000_000;
pub const E8S: u64 = 100_000_000;

pub const MIN_LIQUIDITY_AMOUNT: ICUSD = ICUSD::new(1_000_000_000);
pub const MIN_ICP_AMOUNT: ICP = ICP::new(100_000);  // Instead of MIN_CKBTC_AMOUNT
pub const MIN_ICUSD_AMOUNT: ICUSD = ICUSD::new(10_000_000); // 0.1 icUSD minimum for all stablecoin operations
pub const DUST_THRESHOLD: ICUSD = ICUSD::new(100); // 0.000001 icUSD - dust threshold for vault closing

// Update collateral ratios per whitepaper
pub const RECOVERY_COLLATERAL_RATIO: Ratio = Ratio::new(dec!(1.5));  // 150%
pub const MINIMUM_COLLATERAL_RATIO: Ratio = Ratio::new(dec!(1.33));  // 133%
/// Default protocol share of liquidator's bonus profit (3%).
pub const DEFAULT_LIQUIDATION_PROTOCOL_SHARE: Ratio = Ratio::new(dec!(0.03));

/// Wave-9c DOS-005: default alert band (in basis points) above each
/// collateral's `min_liquidation_ratio` within which `check_vaults`
/// walks the sorted-troves index. 1000 bps = 10% headroom. Tuned via
/// `set_check_vaults_alert_band_bps`.
pub const DEFAULT_CHECK_VAULTS_ALERT_BAND_BPS: u64 = 1000;

/// Wave-9c DOS-005: default cadence (in 5-minute XRC ticks) for the
/// safety-belt full sweep that walks every vault regardless of CR
/// band. 12 = once per hour. 0 or 1 means full sweep every tick
/// (effectively reverts to pre-Wave-9c behavior). Tuned via
/// `set_check_vaults_full_sweep_every_n_ticks`.
pub const DEFAULT_CHECK_VAULTS_FULL_SWEEP_EVERY_N_TICKS: u64 = 12;

/// Stable token types accepted for vault repayment (1:1 with icUSD)
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StableTokenType {
    /// ckUSDT stablecoin
    CKUSDT,
    /// ckUSDC stablecoin
    CKUSDC,
}

/// Arguments for repaying vault with a stable token
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultArgWithToken {
    pub vault_id: u64,
    pub amount: u64,
    pub token_type: StableTokenType,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProtocolArg {
    Init(InitArg),
    Upgrade(UpgradeArg),
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitArg {
    pub xrc_principal: Principal,
    pub icusd_ledger_principal: Principal,
    pub icp_ledger_principal: Principal,
    pub fee_e8s: u64,
    pub developer_principal: Principal,
    pub treasury_principal: Option<Principal>,
    pub stability_pool_principal: Option<Principal>,
    pub ckusdt_ledger_principal: Option<Principal>,
    pub ckusdc_ledger_principal: Option<Principal>,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpgradeArg {
    pub mode: Option<Mode>,
    /// Human-readable description of what changed in this upgrade.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(CandidType, Deserialize, Debug)]
pub struct ProtocolStatus {
    pub last_icp_rate: f64,
    pub last_icp_timestamp: u64,
    pub total_icp_margin: u64,
    pub total_icusd_borrowed: u64,
    pub total_collateral_ratio: f64,
    pub mode: Mode,
    pub liquidation_bonus: f64,
    pub recovery_target_cr: f64,
    pub recovery_mode_threshold: f64,
    pub recovery_cr_multiplier: f64,
    pub reserve_redemptions_enabled: bool,
    pub reserve_redemption_fee: f64,
    pub ckstable_repay_fee: f64,
    pub min_icusd_amount: u64,
    pub global_icusd_mint_cap: u64,
    pub frozen: bool,
    pub manual_mode_override: bool,
    pub interest_pool_share: f64,
    pub weighted_average_interest_rate: f64,
    pub borrowing_fee_curve_resolved: Vec<(f64, f64)>,
    pub per_collateral_interest: Vec<CollateralInterestInfo>,
    pub per_collateral_rate_curves: Vec<PerCollateralRateCurve>,
    pub interest_split: Vec<InterestSplitArg>,
    /// Wave-8e LIQ-005: cumulative bad debt absorbed from underwater
    /// liquidations, awaiting fee-driven repayment.
    pub protocol_deficit_icusd: u64,
    /// Wave-8e LIQ-005: lifetime sum of icUSD applied as deficit repayment.
    pub total_deficit_repaid_icusd: u64,
    /// Wave-8e LIQ-005: fraction of each fee routed to deficit repayment.
    pub deficit_repayment_fraction: f64,
    /// Wave-8e LIQ-005: e8s threshold above which the protocol auto-latches
    /// to ReadOnly. 0 disables the latch.
    pub deficit_readonly_threshold_e8s: u64,
    /// Wave-10 LIQ-008: rolling window length for the mass-liquidation
    /// circuit breaker, in nanoseconds. 0 disables the breaker.
    pub breaker_window_ns: u64,
    /// Wave-10 LIQ-008: cumulative-debt ceiling within the window, in icUSD
    /// e8s. 0 disables tripping.
    pub breaker_window_debt_ceiling_e8s: u64,
    /// Wave-10 LIQ-008: live windowed sum of debt cleared (icUSD e8s).
    /// Compares to `breaker_window_debt_ceiling_e8s` to project breaker headroom.
    pub windowed_liquidation_total_e8s: u64,
    /// Wave-10 LIQ-008: true once the breaker has tripped on the current
    /// window total. Cleared by admin via `clear_liquidation_breaker`.
    pub liquidation_breaker_tripped: bool,
    /// Wave-9b DOS-006: nanosecond timestamp at which the cached heavy
    /// aggregates (totals, weighted rates, per-collateral rollups) were
    /// last computed. Two calls within `PROTOCOL_STATUS_SNAPSHOT_TTL_NANOS`
    /// observe the same value, proving cache hit. Live fields elsewhere
    /// in this struct still reflect current state on every call.
    pub snapshot_ts_ns: u64,
}

/// Per-collateral debt and weighted interest rate for APR calculations.
#[derive(CandidType, Deserialize, Debug)]
pub struct CollateralInterestInfo {
    pub collateral_type: Principal,
    pub total_debt_e8s: u64,
    pub weighted_interest_rate: f64,
}

/// Per-collateral Layer 1 interest rate curve for frontend interpolation.
#[derive(CandidType, Deserialize, Debug)]
pub struct PerCollateralRateCurve {
    pub collateral_type: Principal,
    pub base_rate: f64,
    pub markers: Vec<(f64, f64)>,
}

/// Candid-compatible representation of an interest split entry for the API.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterestSplitArg {
    pub destination: String, // "stability_pool", "treasury", "three_pool"
    pub bps: u64,
}

#[derive(CandidType, Deserialize, Debug)]
pub struct ReserveRedemptionResult {
    pub icusd_block_index: u64,
    pub stable_amount_sent: u64,
    pub fee_amount: u64,
    pub stable_token_used: Principal,
    pub vault_spillover_amount: u64,
}

#[derive(CandidType, Deserialize, Debug)]
pub struct ReserveBalance {
    pub ledger: Principal,
    pub balance: u64,
    pub symbol: String,
}

#[derive(CandidType, Deserialize, Debug)]
pub struct Fees {
    pub borrowing_fee: f64,
    pub redemption_fee: f64,
}

#[derive(CandidType, Deserialize, Debug)]
pub struct SuccessWithFee {
    pub block_index: u64,
    pub fee_amount_paid: u64,
    /// Total collateral (native units) awarded to the liquidator / stability pool.
    /// Added so the stability pool can correctly credit depositors with their
    /// proportional share of the actual collateral received, rather than only
    /// the liquidator bonus (`fee_amount_paid`).
    pub collateral_amount_received: Option<u64>,
}

/// Result from stability pool liquidation (both standard and debt-already-burned paths).
#[derive(CandidType, Deserialize, Debug)]
pub struct StabilityPoolLiquidationResult {
    pub success: bool,
    pub vault_id: u64,
    pub liquidated_debt: u64,
    pub collateral_received: u64,
    pub collateral_type: String,
    pub block_index: u64,
    pub fee: u64,
    pub collateral_price_e8s: u64,
}

/// Coarse classification of an `Event` for the explorer's type facet.
/// Each variant maps to one or more concrete `Event` cases via
/// `Event::type_filter()`. Adding a new `Event` variant requires extending
/// both the mapping there and (if needed) this enum.
#[derive(candid::CandidType, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventTypeFilter {
    OpenVault,
    CloseVault,
    AdjustVault,
    Borrow,
    Repay,
    Liquidation,
    PartialLiquidation,
    Redemption,
    ReserveRedemption,
    StabilityPoolDeposit,
    StabilityPoolWithdraw,
    AdminMint,
    AdminSweepToTreasury,
    Admin,
    PriceUpdate,
    AccrueInterest,
    /// Wave-8e LIQ-005: bad-debt accrued from an underwater liquidation.
    DeficitAccrued,
    /// Wave-8e LIQ-005: deficit repaid via fee revenue routing.
    DeficitRepaid,
    /// Wave-10 LIQ-008: an automatic mass-liquidation circuit-breaker trip.
    /// Distinct from the admin tunables (which collapse to `Admin`) so
    /// operators can audit every breaker firing in isolation.
    BreakerTripped,
    /// Wave-11 BOT-001: a `check_vaults` auto-cancel was skipped because the
    /// bot did not return the collateral within the 10-minute window. Distinct
    /// filter so operators can directly query "stuck claims awaiting
    /// reconciliation" without scanning the noisier admin bucket.
    BotClaimReconciliationNeeded,
}

/// Inclusive nanosecond timestamp window for the time facet.
#[derive(candid::CandidType, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EventTimeRange {
    pub start_ns: u64,
    pub end_ns: u64,
}

#[derive(candid::CandidType, Deserialize, Default, Clone, Debug)]
pub struct GetEventsArg {
    pub start: u64,
    pub length: u64,
    /// OR-combined within the vec; AND with other filters. Empty vec or null
    /// means "no filter on type" — preserves the legacy behavior of hiding
    /// `AccrueInterest` and `PriceUpdate`. When non-empty, only events whose
    /// `type_filter` matches one of the variants are returned (including
    /// `AccrueInterest`/`PriceUpdate` if explicitly requested).
    #[serde(default)]
    pub types: Option<Vec<EventTypeFilter>>,
    /// Match against the event's owner / caller / liquidator / target principal
    /// using `Event::involves_principal`.
    #[serde(default)]
    pub principal: Option<Principal>,
    /// Collateral token ledger principal. For vault-id events, resolved by
    /// looking up the vault's collateral type at open time.
    #[serde(default)]
    pub collateral_token: Option<Principal>,
    #[serde(default)]
    pub time_range: Option<EventTimeRange>,
    /// Minimum event size in icUSD e8s (= USD e8s). ICP/collateral amounts are
    /// converted at the current spot price. Events with no meaningful size
    /// pass through.
    #[serde(default)]
    pub min_size_e8s: Option<u64>,
    /// Narrow `EventTypeFilter::Admin` matches to these specific admin labels
    /// (variant names, e.g. `"SetBorrowingFee"`). No-op when `Admin` isn't in
    /// `types` or when the list is empty/null. Non-admin events are never
    /// affected by this field.
    #[serde(default)]
    pub admin_labels: Option<Vec<String>>,
}

#[derive(candid::CandidType, Clone)]
pub struct GetEventsFilteredResponse {
    pub total: u64,
    pub events: Vec<(u64, crate::event::Event)>,
}

/// Output cap on `get_vault_history` (DOS-001 legacy entry point) and
/// page-size cap on `get_vault_history_paged`. Bounds the per-call
/// reply size; for full historical access callers page via
/// `get_vault_history_paged`. Audit Wave 9a (DOS-001).
pub const MAX_VAULT_HISTORY: usize = 200;

/// Output cap on `get_events_by_principal` (DOS-003 legacy entry point):
/// the function returns the most recent matches in a bounded ring
/// buffer of this size. Audit Wave 9a (DOS-003).
pub const MAX_EVENTS_BY_PRINCIPAL_LEGACY: usize = 500;

/// Per-call scan-window cap on `get_events_by_principal_paged`. A
/// caller cannot scan more than this many event-log entries in a
/// single call — pages chain to cover larger ranges. Audit Wave 9a
/// (DOS-003).
pub const MAX_EVENTS_BY_PRINCIPAL_SCAN: u64 = 5_000;

/// Output cap on `get_events_by_principal_paged`: matches found in the
/// scan window beyond this count truncate (the caller resumes from
/// `scan_end`). Audit Wave 9a (DOS-003).
pub const MAX_EVENTS_BY_PRINCIPAL_OUTPUT: usize = 500;

/// Output cap on `get_all_vaults`, `get_vaults(None)`, and
/// `get_liquidatable_vaults` legacy entry points. Bounds the per-call
/// reply size; for full enumeration callers use the `*_page` paged
/// variants. Audit Wave 9a (DOS-004).
pub const MAX_VAULTS_LEGACY_PAGE: usize = 500;

/// Page-size cap on `get_vaults_page` and
/// `get_liquidatable_vaults_page`. Audit Wave 9a (DOS-004).
pub const MAX_VAULTS_PAGE_LIMIT: u64 = 500;

/// Wave-9b DOS-006: cache TTL for `get_protocol_status` aggregate
/// snapshot. Two consecutive query calls within this window serve the
/// same heavy fields (sum-over-vaults, weighted rate, per-collateral
/// totals) without re-aggregating. The 5-minute XRC tick refreshes
/// the cache as part of its existing vault walk; this 5-second TTL
/// covers cold start, post-upgrade, and the gap between ticks. Live
/// fields (mode, frozen, last_icp_rate, etc.) are NOT served from the
/// snapshot, see `main.rs::get_protocol_status` for the exact list.
pub const PROTOCOL_STATUS_SNAPSHOT_TTL_NANOS: u64 = 5_000_000_000;

/// Wave-9b DOS-007: cache TTL for `get_treasury_stats`. Same rationale
/// as `PROTOCOL_STATUS_SNAPSHOT_TTL_NANOS`. Heavy field cached:
/// `total_accrued_interest_system` (sum of `accrued_interest` across
/// every vault). All other fields in `TreasuryStats` are O(1) or
/// O(small) and read fresh on every call.
pub const TREASURY_STATS_SNAPSHOT_TTL_NANOS: u64 = 5_000_000_000;

/// Paginated response for `get_vault_history_paged`. `events` is the
/// page of matches in newest-first order within the requested window.
/// `total` is the total matched-event count for this vault so the
/// caller can render accurate page indicators. Audit Wave 9a (DOS-001).
#[derive(candid::CandidType, Clone)]
pub struct VaultHistoryPagedResponse {
    pub total: u64,
    pub events: Vec<(u64, crate::event::Event)>,
}

/// Paginated response for `get_events_by_principal_paged`. Cursor-based
/// pagination over the global event log: caller passes `scan_start` and
/// the response reports `scan_end` (resume offset for the next call)
/// plus an `exhausted` flag once the scan has reached `total_events`.
/// `events` are the matches found in the scanned window, in scan order.
/// Audit Wave 9a (DOS-003).
#[derive(candid::CandidType, Clone)]
pub struct EventsByPrincipalPagedResponse {
    pub events: Vec<(u64, crate::event::Event)>,
    pub scan_end: u64,
    pub exhausted: bool,
    pub total_events: u64,
}

/// Paginated response for `get_vaults_page` / `get_liquidatable_vaults_page`.
/// `vaults` is the page slice ordered by ascending `vault_id` starting at
/// `start_id`. `next_start_id` is `Some(id)` to continue paging, `None`
/// when the end of the map is reached. Audit Wave 9a (DOS-004).
#[derive(candid::CandidType, candid::Deserialize, Debug)]
pub struct VaultsPageResponse {
    pub vaults: Vec<crate::vault::CandidVault>,
    pub next_start_id: Option<u64>,
}

#[derive(CandidType, Deserialize, Debug)]
pub struct LiquidityStatus {
    pub liquidity_provided: u64,
    pub total_liquidity_provided: u64,
    pub liquidity_pool_share: f64,
    pub available_liquidity_reward: u64,
    pub total_available_returns: u64,
}

/// Read-only dump of all admin-settable protocol parameters in one call.
/// Returned by `get_protocol_config()` so operators can eyeball every threshold,
/// fee, ceiling, and collateral setting without multiple queries.
#[derive(CandidType, Deserialize, Debug)]
pub struct ProtocolConfig {
    // -- Protocol mode & safety --
    pub mode: Mode,
    pub frozen: bool,
    pub manual_mode_override: bool,

    // -- Global fees --
    pub borrowing_fee: f64,
    pub redemption_fee_floor: f64,
    pub redemption_fee_ceiling: f64,
    pub reserve_redemption_fee: f64,
    pub ckstable_repay_fee: f64,
    pub liquidation_bonus: f64,
    pub liquidation_protocol_share: f64,

    // -- RMR parameters --
    pub rmr_floor: f64,
    pub rmr_ceiling: f64,
    pub rmr_floor_cr: f64,
    pub rmr_ceiling_cr: f64,

    // -- Recovery mode --
    pub recovery_cr_multiplier: f64,
    pub recovery_mode_threshold: f64,
    pub max_partial_liquidation_ratio: f64,

    // -- Limits --
    pub min_icusd_amount: u64,
    pub global_icusd_mint_cap: u64,
    pub interest_flush_threshold_e8s: u64,

    // -- Interest split --
    pub interest_split: Vec<InterestSplitArg>,

    // -- Rate curves --
    pub global_rate_curve: Vec<(f64, f64)>,
    pub recovery_rate_curve: Vec<(String, f64)>,
    pub borrowing_fee_curve: Vec<(f64, f64)>,

    // -- Reserve redemptions --
    pub reserve_redemptions_enabled: bool,
    pub ckusdt_enabled: bool,
    pub ckusdc_enabled: bool,

    // -- Swap routing --
    /// Kill switch for ICPswap-backed swap routing. When false, frontend skips
    /// all ICPswap providers. Flipped via set_icpswap_routing_enabled.
    pub icpswap_routing_enabled: bool,

    // -- External principals --
    pub treasury_principal: Option<Principal>,
    pub stability_pool_canister: Option<Principal>,
    pub three_pool_canister: Option<Principal>,
    pub ckusdt_ledger_principal: Option<Principal>,
    pub ckusdc_ledger_principal: Option<Principal>,

    // -- Bot config --
    pub liquidation_bot_principal: Option<Principal>,
    pub bot_budget_total_e8s: u64,
    pub bot_budget_remaining_e8s: u64,
    pub bot_allowed_collateral_types: Vec<Principal>,

    // -- Per-collateral configs (all collateral types) --
    pub collateral_configs: Vec<(Principal, state::CollateralConfig)>,
}

/// Per-collateral aggregate totals — lightweight alternative to fetching all vaults.
#[derive(CandidType, Deserialize, Debug)]
pub struct CollateralTotals {
    pub collateral_type: Principal,
    pub symbol: String,
    pub decimals: u8,
    pub total_collateral: u64,      // Raw token units
    pub total_debt: u64,            // icUSD e8s
    pub vault_count: u64,
    pub price: f64,                 // Last USD price
}

/// Per-collateral data captured in each hourly protocol snapshot.
#[derive(CandidType, Deserialize, Serialize, Debug, Clone)]
pub struct CollateralSnapshot {
    pub collateral_type: Principal,
    pub total_collateral: u64,
    pub total_debt: u64,
    pub vault_count: u64,
    pub price: f64,
}

/// Hourly protocol snapshot for historical charts.
#[derive(CandidType, Deserialize, Serialize, Debug, Clone)]
pub struct ProtocolSnapshot {
    pub timestamp: u64,
    pub total_collateral_value_usd: u64,
    pub total_debt: u64,
    pub total_vault_count: u64,
    pub collateral_snapshots: Vec<CollateralSnapshot>,
}

#[derive(CandidType, Deserialize)]
pub struct GetSnapshotsArg {
    pub start: u64,
    pub length: u64,
}

/// Argument for adding a new collateral type via admin endpoint.
#[derive(CandidType, Clone, Debug, Deserialize)]
pub struct AddCollateralArg {
    /// ICRC-1 ledger canister ID for the new collateral token
    pub ledger_canister_id: Principal,
    /// How to fetch the USD price (e.g., XRC with specific asset pair)
    pub price_source: state::PriceSource,
    /// Below this ratio, the vault can be liquidated (e.g., 1.33)
    pub liquidation_ratio: f64,
    /// Below this ratio, recovery mode triggers (e.g., 1.5)
    pub borrow_threshold_ratio: f64,
    /// Bonus multiplier for liquidators (e.g., 1.15)
    pub liquidation_bonus: f64,
    /// One-time fee at borrow/mint time (e.g., 0.005)
    pub borrowing_fee: f64,
    /// Maximum total debt for this collateral (u64::MAX = no cap)
    pub debt_ceiling: u64,
    /// Minimum vault debt (dust threshold)
    pub min_vault_debt: u64,
    /// Ongoing annual interest rate (e.g., 0.02 = 2% APR)
    pub interest_rate_apr: f64,
    /// Minimum collateral deposit in native token units (e.g., 100_000 for 0.001 ICP)
    pub min_collateral_deposit: u64,
    /// Hex color for frontend display (e.g., "#F7931A")
    pub display_color: Option<String>,
    /// Minimum redemption fee (floor), e.g., 0.005 = 0.5%
    pub redemption_fee_floor: Option<f64>,
    /// Maximum redemption fee (ceiling), e.g., 0.05 = 5%
    pub redemption_fee_ceiling: Option<f64>,
    /// Redemption priority tier (1/2/3). Default: 1 if omitted.
    pub redemption_tier: Option<u8>,
}

#[derive(CandidType, Debug, Clone, Deserialize)]
pub enum ProtocolError {
    TransferFromError(TransferFromError, u64),
    TransferError(TransferError),
    TemporarilyUnavailable(String),
    AlreadyProcessing,
    AnonymousCallerNotAllowed,
    CallerNotOwner,
    AmountTooLow { minimum_amount: u64 },
    GenericError(String),
    /// Wave-8b LIQ-002: rejected because the requested vault is not within
    /// `liquidation_ordering_tolerance` of the lowest-CR vault. Liquidators
    /// must process the worst vault first so the protocol cannot be left
    /// with deeply-underwater bad debt while easy targets are picked off.
    /// The caller can either pick a worst-or-near-worst vault, or wait until
    /// admin widens the tolerance band.
    NotLowestCR,
}

impl From<GuardError> for ProtocolError {
    fn from(e: GuardError) -> Self {
        match e {
            GuardError::AlreadyProcessing => Self::AlreadyProcessing,
            GuardError::TooManyConcurrentRequests => {
                Self::TemporarilyUnavailable("too many concurrent requests".to_string())
            },
            GuardError::StaleOperation => {
                Self::TemporarilyUnavailable("previous operation is being cleaned up".to_string())
            }
        }
    }
}

/// Candid-compatible struct matching the stability pool's and bot's `LiquidatableVaultInfo`.
/// Defined inline to avoid a crate dependency between backend and pool/bot.
#[derive(CandidType, Clone, Debug, Deserialize)]
pub struct LiquidatableVaultInfo {
    pub vault_id: u64,
    pub collateral_type: Principal,
    pub debt_amount: u64,
    pub collateral_amount: u64,
    pub recommended_liquidation_amount: u64,
    pub collateral_price_e8s: u64,
}

pub async fn check_vaults() {
    // Auto-cancel bot claims that have been pending too long (10 minutes).
    // This prevents vaults from being permanently locked if the bot crashes.
    //
    // Wave-11 BOT-001: gate the auto-cancel on the protocol's collateral
    // balance having returned to (>=) `claim.collateral_amount - ledger_fee`.
    // Without this, a CLAIM → SWAP-ok → TRANSFER-fail → admin-AFK-10min
    // sequence would clear the claim while the bot still holds the
    // collateral, leaving the vault permanently underwater. Mirrors the
    // subaccount + fee derivation used by `bot_cancel_liquidation`. On a
    // shortfall we leave the claim in place and emit
    // `BotClaimReconciliationNeeded` so admin can reconcile manually.
    //
    // The guard re-emits the event on every tick the gate fires (no
    // per-claim "already emitted" flag, since a state-shape change is
    // out of scope for this wave). The explorer can group by `vault_id`
    // to dedupe; operator action is unchanged regardless of count.
    const BOT_CLAIM_TIMEOUT_NS: u64 = 600_000_000_000; // 10 minutes
    let now = ic_cdk::api::time();

    let expired_claims: Vec<(u64, crate::state::BotClaim)> = read_state(|s| {
        s.bot_claims.iter()
            .filter(|(_, claim)| now.saturating_sub(claim.claimed_at) >= BOT_CLAIM_TIMEOUT_NS)
            .map(|(vid, claim)| (*vid, claim.clone()))
            .collect()
    });

    let backend_id = ic_cdk::id();
    for (vault_id, claim) in &expired_claims {
        let required = read_state(|s| {
            let fee = s
                .get_collateral_config(&claim.collateral_type)
                .map(|c| c.ledger_fee)
                .unwrap_or(0);
            claim.collateral_amount.saturating_sub(fee)
        });

        let balance_result: Result<(candid::Nat,), _> = ic_cdk::call(
            claim.collateral_type,
            "icrc1_balance_of",
            (icrc_ledger_types::icrc1::account::Account {
                owner: backend_id,
                subaccount: None,
            },),
        )
        .await;

        let observed = match balance_result {
            Ok((bal,)) => bal.0.to_u64().unwrap_or(0),
            Err((code, msg)) => {
                log!(
                    INFO,
                    "[BOT-001] auto-cancel balance query failed for vault #{}: {:?} {}; deferring this tick",
                    vault_id,
                    code,
                    msg
                );
                continue;
            }
        };

        if observed < required {
            log!(
                INFO,
                "[BOT-001] auto-cancel skipped for vault #{}: balance {} < required {} (collateral_amount {})",
                vault_id,
                observed,
                required,
                claim.collateral_amount
            );
            mutate_state(|s| {
                // TOCTOU re-check: between collecting expired_claims and
                // awaiting the balance query, the bot may have called
                // `bot_cancel_liquidation` itself and cleared the claim.
                // Avoid emitting a misleading reconciliation event for a
                // vault that no longer needs reconciliation.
                if !s.bot_claims.contains_key(vault_id) {
                    return;
                }
                crate::event::record_bot_claim_reconciliation_needed(
                    s, *vault_id, observed, required,
                );
            });
            continue;
        }

        log!(
            INFO,
            "[check_vaults] Auto-cancelling stuck bot claim for vault #{} (claimed {}s ago, balance {} >= required {})",
            vault_id,
            (now - claim.claimed_at) / 1_000_000_000,
            observed,
            required
        );

        mutate_state(|s| {
            // TOCTOU re-check: skip the budget restore if the claim was
            // already cleared during the await window (e.g., bot raced us
            // by calling `bot_cancel_liquidation`). Without this guard the
            // budget would be double-credited.
            if !s.bot_claims.contains_key(vault_id) {
                return;
            }
            if let Some(vault) = s.vault_id_to_vaults.get_mut(vault_id) {
                vault.bot_processing = false;
            }
            s.bot_budget_remaining_e8s += claim.debt_amount;
            s.bot_claims.remove(vault_id);
        });
    }

    // Wave-10 LIQ-008: short-circuit the auto-publishing path when the
    // mass-liquidation circuit breaker is tripped. Bot-claim auto-cancel
    // above runs unconditionally because it is hygiene, not auto-publishing.
    // Manual liquidation endpoints (`liquidate_vault`, `liquidate_vault_partial`,
    // `liquidate_vault_partial_with_stable`, `partial_liquidate_vault`,
    // `liquidate_vault_debt_already_burned`) do not consult the breaker.
    if read_state(|s| s.liquidation_breaker_tripped) {
        log!(
            INFO,
            "[LIQ-008] check_vaults skipping notify (breaker tripped). Manual liquidation remains available."
        );
        return;
    }

    let dummy_rate = read_state(|s| {
        s.last_icp_rate.unwrap_or_else(|| {
            log!(INFO, "[check_vaults] No ICP rate available, using default rate");
            UsdIcp::from(dec!(1.0))
        })
    });

    // Only identify unhealthy vaults but don't liquidate them
    //
    // Wave-8b LIQ-002: walk `vault_cr_index` ascending so the bot / stability
    // pool receive worst-CR vaults first. The list of underwater vaults is
    // unchanged; the order is now sorted by CR ascending. This matches the
    // server-side band-gate behavior — the bot/pool see the same vault the
    // band gate would accept first.
    //
    // Wave-9c DOS-005: bound the walk to the at-risk band on most ticks.
    // `advance_check_vaults_tick` returns true on the Nth tick (default
    // every 12 = once per hour at the 5-min cadence), making that tick a
    // full sweep. This is the safety belt for cross-collateral CR-key
    // drift: a vault whose key is stale-above-threshold is missed by
    // band-only ticks but caught by the next full sweep. Tunable via
    // `set_check_vaults_alert_band_bps` and
    // `set_check_vaults_full_sweep_every_n_ticks`.
    let do_full_sweep = mutate_state(|s| s.advance_check_vaults_tick());
    let scan = read_state(|s| s.scan_unhealthy_vaults(dummy_rate, do_full_sweep));
    log!(
        INFO,
        "[check_vaults] {} tick: visited {} vault(s), threshold_key={}, found {} unhealthy",
        if scan.was_full_sweep { "full-sweep" } else { "band-only" },
        scan.vaults_visited,
        scan.threshold_key,
        scan.unhealthy_vaults.len(),
    );
    let unhealthy_vaults = scan.unhealthy_vaults;

    // Log unhealthy vaults but don't liquidate them
    if !unhealthy_vaults.is_empty() {
        log!(
            INFO,
            "[check_vaults] Found {} liquidatable vaults. Waiting for external liquidators.",
            unhealthy_vaults.len()
        );

        // Log detailed information about each unhealthy vault
        for vault in &unhealthy_vaults {
            let (ratio, min_ratio) = read_state(|s| {
                (
                    compute_collateral_ratio(vault, dummy_rate, s),
                    s.get_min_liquidation_ratio_for(&vault.collateral_type),
                )
            });
            log!(
                INFO,
                "[check_vaults] Liquidatable vault #{}: owner={}, borrowed={}, collateral={}, ratio={:.2}%, min_ratio={:.2}%",
                vault.vault_id,
                vault.owner,
                vault.borrowed_icusd_amount,
                vault.collateral_amount,
                ratio.to_f64() * 100.0,
                min_ratio.to_f64() * 100.0
            );
        }

        // Build enriched notification payload
        let vault_notifications: Vec<LiquidatableVaultInfo> = read_state(|s| {
            unhealthy_vaults.iter().map(|v| {
                let collateral_price_usd = s.get_collateral_price_decimal(&v.collateral_type)
                    .map(|p| UsdIcp::from(p))
                    .unwrap_or(UsdIcp::from(rust_decimal::Decimal::ZERO));
                let optimal_liq = s.compute_partial_liquidation_cap(v, collateral_price_usd);
                LiquidatableVaultInfo {
                    vault_id: v.vault_id,
                    collateral_type: v.collateral_type,
                    debt_amount: v.borrowed_icusd_amount.to_u64(),
                    collateral_amount: v.collateral_amount,
                    recommended_liquidation_amount: optimal_liq.to_u64(),
                    collateral_price_e8s: collateral_price_usd.to_e8s(),
                }
            }).collect()
        });

        // ── Priority-ordered liquidation cascade ──
        // 1. Bot gets first shot at vaults with bot-eligible collateral
        // 2. Stability pool handles: non-bot-eligible immediately + bot-eligible after timeout
        // 3. Manual liquidation is always available as last resort (via get_liquidatable_vaults)

        let now = ic_cdk::api::time();
        // Bot gets one check_vaults cycle (5 min = 300s) before fallback
        let bot_timeout_ns: u64 = 300_000_000_000;

        let (bot_allowed, bot_canister, pool_canister) = read_state(|s| {
            (
                s.bot_allowed_collateral_types.clone(),
                s.liquidation_bot_principal,
                s.stability_pool_canister,
            )
        });

        let mut for_bot: Vec<LiquidatableVaultInfo> = Vec::new();
        let mut for_pool: Vec<LiquidatableVaultInfo> = Vec::new();

        for vault_info in &vault_notifications {
            let bot_eligible = bot_canister.is_some()
                && bot_allowed.contains(&vault_info.collateral_type);

            let sp_already_tried = read_state(|s| s.sp_attempted_vaults.contains(&vault_info.vault_id));

            if sp_already_tried {
                // SP already had its shot → manual only, skip entirely
                continue;
            }

            if !bot_eligible {
                // Not bot-eligible → stability pool (one shot)
                for_pool.push(vault_info.clone());
            } else {
                // Bot-eligible: check if we already sent it and it timed out
                let pending_since = read_state(|s| {
                    s.bot_pending_vaults.get(&vault_info.vault_id).copied()
                });
                match pending_since {
                    None => {
                        // First time seeing this vault → send to bot
                        for_bot.push(vault_info.clone());
                    }
                    Some(ts) if now.saturating_sub(ts) >= bot_timeout_ns => {
                        // Bot had its chance and didn't liquidate → fallback to pool (one shot)
                        log!(
                            INFO,
                            "[check_vaults] Bot timeout for vault #{}, falling back to stability pool",
                            vault_info.vault_id
                        );
                        for_pool.push(vault_info.clone());
                    }
                    Some(_) => {
                        // Still within bot's window → re-send to bot
                        for_bot.push(vault_info.clone());
                    }
                }
            }
        }

        // Update tracking state
        let unhealthy_ids: std::collections::BTreeSet<u64> = vault_notifications
            .iter()
            .map(|v| v.vault_id)
            .collect();
        let bot_vault_ids: Vec<u64> = for_bot.iter().map(|v| v.vault_id).collect();
        let pool_vault_ids: Vec<u64> = for_pool.iter().map(|v| v.vault_id).collect();

        mutate_state(|s| {
            // Record newly-sent bot vaults
            for vid in &bot_vault_ids {
                s.bot_pending_vaults.entry(*vid).or_insert(now);
            }
            // Keep bot entries that are still unhealthy AND haven't timed out yet.
            s.bot_pending_vaults
                .retain(|vid, ts| unhealthy_ids.contains(vid) && now.saturating_sub(*ts) < bot_timeout_ns);

            // Mark vaults sent to SP — they only get one shot
            for vid in &pool_vault_ids {
                s.sp_attempted_vaults.insert(*vid);
            }
            // Clear SP tracking for vaults that are now healthy (owner repaid/added collateral)
            s.sp_attempted_vaults.retain(|vid| unhealthy_ids.contains(vid));
        });

        // Push to bot (fire-and-forget with error logging)
        if let Some(bot) = bot_canister {
            if !for_bot.is_empty() {
                let count = for_bot.len();
                ic_cdk::spawn(async move {
                    let result: Result<(), _> = ic_cdk::call(
                        bot,
                        "notify_liquidatable_vaults",
                        (for_bot,),
                    )
                    .await;
                    if let Err((code, msg)) = result {
                        log!(INFO, "[check_vaults] ERROR: bot notification failed: {:?} {}", code, msg);
                    }
                });
                log!(
                    INFO,
                    "[check_vaults] Sent {} bot-eligible vaults to bot {}",
                    count,
                    bot
                );
            }
        }

        // Push to stability pool (fire-and-forget with error logging)
        if let Some(pool) = pool_canister {
            if !for_pool.is_empty() {
                let count = for_pool.len();
                ic_cdk::spawn(async move {
                    let result: Result<(), _> = ic_cdk::call(
                        pool,
                        "notify_liquidatable_vaults",
                        (for_pool,),
                    )
                    .await;
                    if let Err((code, msg)) = result {
                        log!(INFO, "[check_vaults] ERROR: stability pool notification failed: {:?} {}", code, msg);
                    }
                });
                log!(
                    INFO,
                    "[check_vaults] Sent {} vaults to stability pool {} (non-bot-eligible or bot timeout)",
                    count,
                    pool
                );
            }
        }
    } else {
        log!(
            DEBUG,
            "[check_vaults] All vaults are healthy at the current ICP rate: {}",
            dummy_rate.to_f64()
        );
    }

    // No longer calling record_liquidate_vault to trigger automatic liquidations
}

/// Compute collateral ratio for a vault using per-collateral price and decimals.
/// Returns Ratio::ZERO when price or config is unavailable — callers must
/// independently check `last_price.is_some()` before performing operations.
pub fn compute_collateral_ratio(vault: &Vault, _rate: UsdIcp, state: &state::State) -> Ratio {
    if vault.borrowed_icusd_amount == 0 {
        return Ratio::from(Decimal::MAX);
    }
    let margin_value: ICUSD = if let Some(config) = state.get_collateral_config(&vault.collateral_type) {
        if let Some(price) = config.last_price {
            let price_dec = Decimal::from_f64(price).unwrap_or(Decimal::ZERO);
            numeric::collateral_usd_value(vault.collateral_amount, price_dec, config.decimals)
        } else {
            // No price available — return zero ratio (conservative / safe direction).
            // Operations must independently check last_price.is_some() and error out.
            return Ratio::from(Decimal::ZERO);
        }
    } else {
        // No config — return zero ratio. This vault's collateral type is unknown.
        return Ratio::from(Decimal::ZERO);
    };
    margin_value / vault.borrowed_icusd_amount
}

/// Drop a single pending-transfer entry from its owning map. Wave-4 ICC-005:
/// after LIQ-001's `(vault_id, owner)` re-keying, every pending entry has a
/// unique key, so abandon-paths are uniform across margin / excess / redemption.
/// This helper exists to make that uniformity legible (and to stop callers
/// from ever drifting back to `retain` over a vault_id range, which would
/// over-remove sibling liquidators' entries).
fn drop_pending<K: std::cmp::Ord>(
    map: &mut std::collections::BTreeMap<K, crate::state::PendingMarginTransfer>,
    key: &K,
) {
    map.remove(key);
}

pub(crate) async fn process_pending_transfer() {
    let _guard = match crate::guard::TimerLogicGuard::new() {
        Some(guard) => guard,
        None => {
            log!(INFO, "[process_pending_transfer] double entry.");
            return;
        }
    };

    // Process pending margin transfers
    //
    // Wave-3 + Wave-4 cleanup contract:
    //   * Success: `record_margin_transfer` writes a MarginTransfer event AND
    //     removes the entry by `(vault_id, owner)` (event.rs).
    //   * Skipped (margin <= fee): `drop_pending` drops the entry inline.
    //   * Abandon (>= MAX_PENDING_RETRIES): `drop_pending` drops the entry inline.
    //   * BadFee: refresh fee cache, do NOT drop. The next tick retries.
    // Excess and redemption loops follow the same contract; redemption uses
    // its own event recorder with the same removal semantics.
    let pending_transfers = read_state(|s| {
        // Log for visibility
        if !s.pending_margin_transfers.is_empty() {
            log!(INFO, "[process_pending_transfer] Found {} pending margin transfers",
                 s.pending_margin_transfers.len());
        }

        s.pending_margin_transfers
            .iter()
            .map(|(key, margin_transfer)| (*key, *margin_transfer))
            .collect::<Vec<((u64, candid::Principal), PendingMarginTransfer)>>()
    });
    for (key, transfer) in pending_transfers {
        let (vault_id, _key_owner) = key;
        // Look up per-collateral config for ledger and fee; fall back to global ICP defaults
        let (ledger, transfer_fee) = read_state(|s| {
            match s.get_collateral_config(&transfer.collateral_type) {
                Some(config) => (config.ledger_canister_id, ICP::from(config.ledger_fee)),
                None => (s.icp_ledger_principal, s.icp_ledger_fee),
            }
        });

        if transfer.margin <= transfer_fee {
            log!(INFO, "[transfering_margins] Skipping vault {} owner {} - margin {} <= fee {}, removing", vault_id, transfer.owner, transfer.margin, transfer_fee);
            mutate_state(|s| drop_pending(&mut s.pending_margin_transfers, &key));
            continue;
        }
        match crate::management::transfer_collateral_with_nonce(
            (transfer.margin - transfer_fee).to_u64(),
            transfer.owner,
            ledger,
            transfer.op_nonce,
        )
        .await
        {
            Ok(block_index) => {
                log!(
                    INFO,
                    "[transfering_margins] successfully transferred: {} to {} via ledger {}",
                    transfer.margin,
                    transfer.owner,
                    ledger
                );
                mutate_state(|s| crate::event::record_margin_transfer(s, vault_id, transfer.owner, block_index));
            }
            Err(error) => {
                // Improved error logging with more details
                log!(
                    INFO,
                    "[transfering_margins] failed to transfer margin: {}, to principal: {}, via ledger: {}, with error: {}",
                    transfer.margin,
                    transfer.owner,
                    ledger,
                    error
                );

                // If there was a transfer fee error, update the fee in collateral config
                if let TransferError::BadFee { expected_fee } = error {
                    log!(INFO, "[transfering_margins] Updating transfer fee to: {:?}", expected_fee);
                    mutate_state(|s| {
                        let expected_fee_u64: u64 = expected_fee
                            .0
                            .try_into()
                            .expect("failed to convert Nat to u64");
                        if let Some(config) = s.get_collateral_config_mut(&transfer.collateral_type) {
                            config.ledger_fee = expected_fee_u64;
                        }
                        // Also update global icp_ledger_fee if this is the ICP collateral
                        let icp_ct = s.icp_collateral_type();
                        let resolved_ct = if transfer.collateral_type == candid::Principal::anonymous() {
                            icp_ct
                        } else {
                            transfer.collateral_type
                        };
                        if resolved_ct == icp_ct {
                            s.icp_ledger_fee = ICP::from(expected_fee_u64);
                        }
                    });

                    // After updating the fee, we should retry this transfer next time
                } else {
                    // Increment retry count; abandon after MAX_PENDING_RETRIES
                    let retries = mutate_state(|s| {
                        if let Some(t) = s.pending_margin_transfers.get_mut(&key) {
                            t.retry_count = t.retry_count.saturating_add(1);
                            t.retry_count
                        } else {
                            0
                        }
                    });
                    if retries >= MAX_PENDING_RETRIES {
                        log!(INFO,
                            "[transfering_margins] CRITICAL: abandoning margin transfer for vault {} \
                             after {} retries. Owner: {}, amount: {}. Use recover_pending_transfer to retry manually.",
                            vault_id, retries, transfer.owner, transfer.margin
                        );
                        mutate_state(|s| drop_pending(&mut s.pending_margin_transfers, &key));
                    } else {
                        log!(INFO, "[transfering_margins] Will retry transfer for vault {} owner {} (attempt {}/{})",
                            vault_id, transfer.owner, retries, MAX_PENDING_RETRIES);
                    }
                }
            }
        }
    }

    // Process pending excess collateral transfers (from full liquidations)
    let pending_excess = read_state(|s| {
        s.pending_excess_transfers
            .iter()
            .map(|(key, transfer)| (*key, *transfer))
            .collect::<Vec<((u64, candid::Principal), PendingMarginTransfer)>>()
    });

    for (key, transfer) in pending_excess {
        let (vault_id, _key_owner) = key;
        let (ledger, transfer_fee) = read_state(|s| {
            match s.get_collateral_config(&transfer.collateral_type) {
                Some(config) => (config.ledger_canister_id, ICP::from(config.ledger_fee)),
                None => (s.icp_ledger_principal, s.icp_ledger_fee),
            }
        });

        if transfer.margin <= transfer_fee {
            log!(INFO, "[transfering_excess] Skipping vault {} owner {} - margin {} <= fee {}, removing", vault_id, transfer.owner, transfer.margin, transfer_fee);
            mutate_state(|s| drop_pending(&mut s.pending_excess_transfers, &key));
            continue;
        }
        match crate::management::transfer_collateral_with_nonce(
            (transfer.margin - transfer_fee).to_u64(),
            transfer.owner,
            ledger,
            transfer.op_nonce,
        )
        .await
        {
            Ok(_block_index) => {
                log!(
                    INFO,
                    "[transfering_excess] successfully transferred excess collateral: {} to {} via ledger {}",
                    transfer.margin,
                    transfer.owner,
                    ledger
                );
                mutate_state(|s| drop_pending(&mut s.pending_excess_transfers, &key));
            }
            Err(error) => {
                log!(
                    INFO,
                    "[transfering_excess] failed to transfer excess collateral: {}, to principal: {}, via ledger: {}, with error: {}",
                    transfer.margin,
                    transfer.owner,
                    ledger,
                    error
                );
                if let TransferError::BadFee { expected_fee } = error {
                    log!(INFO, "[transfering_excess] Updating transfer fee to: {:?}", expected_fee);
                    mutate_state(|s| {
                        let expected_fee_u64: u64 = expected_fee
                            .0
                            .try_into()
                            .expect("failed to convert Nat to u64");
                        if let Some(config) = s.get_collateral_config_mut(&transfer.collateral_type) {
                            config.ledger_fee = expected_fee_u64;
                        }
                        let icp_ct = s.icp_collateral_type();
                        let resolved_ct = if transfer.collateral_type == candid::Principal::anonymous() {
                            icp_ct
                        } else {
                            transfer.collateral_type
                        };
                        if resolved_ct == icp_ct {
                            s.icp_ledger_fee = ICP::from(expected_fee_u64);
                        }
                    });
                    // Don't increment retry counter on BadFee — refresh fee, retry next tick.
                } else {
                    let retries = mutate_state(|s| {
                        if let Some(t) = s.pending_excess_transfers.get_mut(&key) {
                            t.retry_count = t.retry_count.saturating_add(1);
                            t.retry_count
                        } else {
                            0
                        }
                    });
                    if retries >= MAX_PENDING_RETRIES {
                        log!(INFO,
                            "[transfering_excess] CRITICAL: abandoning excess transfer for vault {} \
                             after {} retries. Owner: {}, amount: {}. Use recover_pending_transfer to retry manually.",
                            vault_id, retries, transfer.owner, transfer.margin
                        );
                        mutate_state(|s| drop_pending(&mut s.pending_excess_transfers, &key));
                    }
                }
            }
        }
    }

    // Similar improved logic for redemption transfers
    let pending_redemptions = read_state(|s| {
        s.pending_redemption_transfer
            .iter()
            .map(|(icusd_block_index, margin_transfer)| (*icusd_block_index, *margin_transfer))
            .collect::<Vec<(u64, PendingMarginTransfer)>>()
    });

    for (icusd_block_index, pending_transfer) in pending_redemptions {
        let (ledger, transfer_fee) = read_state(|s| {
            match s.get_collateral_config(&pending_transfer.collateral_type) {
                Some(config) => (config.ledger_canister_id, ICP::from(config.ledger_fee)),
                None => (s.icp_ledger_principal, s.icp_ledger_fee),
            }
        });

        if pending_transfer.margin <= transfer_fee {
            log!(INFO, "[transfering_redemptions] Skipping redemption {} - margin {} <= fee {}, removing", icusd_block_index, pending_transfer.margin, transfer_fee);
            mutate_state(|s| drop_pending(&mut s.pending_redemption_transfer, &icusd_block_index));
            continue;
        }
        match crate::management::transfer_collateral_with_nonce(
            (pending_transfer.margin - transfer_fee).to_u64(),
            pending_transfer.owner,
            ledger,
            pending_transfer.op_nonce,
        )
        .await
        {
            Ok(block_index) => {
                log!(
                    INFO,
                    "[transfering_redemptions] successfully transferred: {} to {} via ledger {}",
                    pending_transfer.margin,
                    pending_transfer.owner,
                    ledger
                );
                mutate_state(|s| {
                    crate::event::record_redemption_transfered(s, icusd_block_index, block_index)
                });
            }
            Err(error) => {
                log!(
                    INFO,
                    "[transfering_redemptions] failed to transfer margin: {}, to principal: {}, via ledger: {}, with error: {}",
                    pending_transfer.margin,
                    pending_transfer.owner,
                    ledger,
                    error
                );
                if let TransferError::BadFee { expected_fee } = error {
                    log!(INFO, "[transfering_redemptions] Updating transfer fee to: {:?}", expected_fee);
                    mutate_state(|s| {
                        let expected_fee_u64: u64 = expected_fee
                            .0
                            .try_into()
                            .expect("failed to convert Nat to u64");
                        if let Some(config) = s.get_collateral_config_mut(&pending_transfer.collateral_type) {
                            config.ledger_fee = expected_fee_u64;
                        }
                        let icp_ct = s.icp_collateral_type();
                        let resolved_ct = if pending_transfer.collateral_type == candid::Principal::anonymous() {
                            icp_ct
                        } else {
                            pending_transfer.collateral_type
                        };
                        if resolved_ct == icp_ct {
                            s.icp_ledger_fee = ICP::from(expected_fee_u64);
                        }
                    });
                    // Don't increment retry counter on BadFee — refresh fee, retry next tick.
                } else {
                    let retries = mutate_state(|s| {
                        if let Some(t) = s.pending_redemption_transfer.get_mut(&icusd_block_index) {
                            t.retry_count = t.retry_count.saturating_add(1);
                            t.retry_count
                        } else {
                            0
                        }
                    });
                    if retries >= MAX_PENDING_RETRIES {
                        log!(INFO,
                            "[transfering_redemptions] CRITICAL: abandoning redemption transfer {} \
                             after {} retries. Owner: {}, amount: {}. Use recover_pending_transfer to retry manually.",
                            icusd_block_index, retries, pending_transfer.owner, pending_transfer.margin
                        );
                        mutate_state(|s| drop_pending(&mut s.pending_redemption_transfer, &icusd_block_index));
                    }
                }
            }
        }
    }

    // Wave-4 ICC-007: durable refund queue from `redeem_reserves` double-failures.
    // Each entry is keyed by the original burn block index (unique). We drive the
    // retry through `transfer_icusd_with_nonce` so the icUSD ledger deduplicates
    // if a previous attempt's reply was lost.
    let pending_refunds = read_state(|s| {
        s.pending_refunds
            .iter()
            .map(|(k, v)| (*k, *v))
            .collect::<Vec<(u64, crate::state::PendingRefund)>>()
    });

    for (icusd_block_index, refund) in pending_refunds {
        match crate::management::transfer_icusd_with_nonce(
            crate::numeric::ICUSD::new(refund.amount_e8s),
            refund.user,
            refund.op_nonce,
        ).await {
            Ok(block_index) => {
                log!(INFO,
                    "[refunding] icUSD refund settled for {} (burn block {}, refund block {}, amount {})",
                    refund.user, icusd_block_index, block_index, refund.amount_e8s
                );
                mutate_state(|s| { s.pending_refunds.remove(&icusd_block_index); });
            }
            Err(error) => {
                log!(INFO,
                    "[refunding] icUSD refund failed for {} (burn block {}): {}. Will retry.",
                    refund.user, icusd_block_index, error
                );
                if let TransferError::BadFee { expected_fee } = error {
                    // Refresh fee cache; do NOT increment retry count on BadFee.
                    if let Ok(expected_fee_u64) = expected_fee.0.clone().try_into() {
                        let icusd_ledger = read_state(|s| s.icusd_ledger_principal);
                        crate::management::set_cached_fee(icusd_ledger, expected_fee_u64);
                    }
                } else {
                    let retries = mutate_state(|s| {
                        if let Some(r) = s.pending_refunds.get_mut(&icusd_block_index) {
                            r.retry_count = r.retry_count.saturating_add(1);
                            r.retry_count
                        } else { 0 }
                    });
                    if retries >= MAX_PENDING_RETRIES {
                        log!(INFO,
                            "[refunding] CRITICAL: abandoning icUSD refund for {} (burn block {}) \
                             after {} retries. Amount: {}. Manual reconciliation required.",
                            refund.user, icusd_block_index, retries, refund.amount_e8s
                        );
                        mutate_state(|s| { s.pending_refunds.remove(&icusd_block_index); });
                    }
                }
            }
        }
    }

    // Schedule another run if needed, but with better timing
    if read_state(|s| {
        !s.pending_margin_transfers.is_empty()
            || !s.pending_excess_transfers.is_empty()
            || !s.pending_redemption_transfer.is_empty()
            || !s.pending_refunds.is_empty()
    }) {
        // Schedule another check in 5 seconds
        log!(INFO, "[process_pending_transfer] Scheduling another transfer attempt in 5 seconds");
        ic_cdk_timers::set_timer(std::time::Duration::from_secs(5), || {
            ic_cdk::spawn(crate::process_pending_transfer())
        });
    } else {
        log!(INFO, "[process_pending_transfer] No more pending transfers");
    }
}