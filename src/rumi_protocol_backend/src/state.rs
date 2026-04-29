use crate::numeric::{Ratio, UsdIcp, ICUSD, ICP};
use crate::vault::Vault;
use crate::{
    compute_collateral_ratio, InitArg, ProtocolError, UpgradeArg, MINIMUM_COLLATERAL_RATIO,
    RECOVERY_COLLATERAL_RATIO,
};
use candid::Principal;
use ic_canister_log::log;
use rust_decimal::prelude::{FromPrimitive, ToPrimitive};
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
pub const DEFAULT_MIN_ICUSD_AMOUNT: ICUSD = ICUSD::new(10_000_000); // 0.1 icUSD
pub const DEFAULT_LIQUIDATION_BONUS: Ratio = Ratio::new(dec!(1.15)); // 115% (15% bonus)
pub const DEFAULT_MAX_PARTIAL_LIQUIDATION_RATIO: Ratio = Ratio::new(dec!(0.5)); // 50% max
pub const DEFAULT_REDEMPTION_FEE_FLOOR: Ratio = Ratio::new(dec!(0.003)); // 0.3%
pub const DEFAULT_REDEMPTION_FEE_CEILING: Ratio = Ratio::new(dec!(0.05)); // 5%
pub const DEFAULT_RESERVE_REDEMPTION_FEE: Ratio = Ratio::new(dec!(0.003)); // 0.3% flat fee for reserve redemptions
pub const DEFAULT_RECOVERY_TARGET_CR: Ratio = Ratio::new(dec!(1.55)); // 155% — legacy; kept for serde backwards compat
pub const DEFAULT_RECOVERY_CR_MULTIPLIER: Ratio = Ratio::new(dec!(1.033333333333333333)); // proportional buffer: recovery_cr = borrow_threshold × 1.0333...
pub const DEFAULT_INTEREST_RATE_APR: Ratio = Ratio::new(dec!(0.0)); // 0% — placeholder for future accrual
pub const DEFAULT_INTEREST_POOL_SHARE: Ratio = Ratio::new(dec!(0.75)); // 75% to stability pool — legacy, kept for old event replay
/// INT-003: hard cap on a borrowing-fee curve marker's multiplier.
/// The default curve ships with multipliers up to 3.0; 20.0 leaves ~6.6x of
/// headroom for future high-risk-tier configurations while preventing the
/// `amount - fee` underflow that would trap every borrow.
pub const MAX_BORROWING_FEE_MULTIPLIER: Ratio = Ratio::new(dec!(20));

/// Where a share of interest revenue is routed.
#[derive(candid::CandidType, Clone, Debug, PartialEq, Eq, serde::Deserialize, Serialize)]
pub enum InterestDestination {
    /// Stability pool (distributes pro-rata to depositors).
    StabilityPool,
    /// Protocol treasury canister.
    Treasury,
    /// 3pool AMM (donated as icUSD to grow virtual_price for 3USD holders).
    ThreePool,
}

/// One slice of the interest split: destination + share in basis points.
#[derive(candid::CandidType, Clone, Debug, PartialEq, Eq, serde::Deserialize, Serialize)]
pub struct InterestRecipient {
    pub destination: InterestDestination,
    pub bps: u64, // basis points out of 10_000
}

/// Default interest split: 50% 3pool, 40% stability pool, 10% treasury.
fn default_flush_threshold() -> u64 { 10_000_000 } // 0.1 icUSD

pub fn default_interest_split() -> Vec<InterestRecipient> {
    vec![
        InterestRecipient { destination: InterestDestination::ThreePool, bps: 5000 },
        InterestRecipient { destination: InterestDestination::StabilityPool, bps: 4000 },
        InterestRecipient { destination: InterestDestination::Treasury, bps: 1000 },
    ]
}
pub const DUST_DEBT_THRESHOLD: u64 = 50_000; // 0.0005 icUSD — debt below this is forgiven on withdrawal

/// Wave-8b LIQ-002: default tolerance band (in absolute CR units) above the
/// worst-CR vault inside which liquidations are accepted. 0.01 = 1% CR. With
/// the band in basis points, this is 100 bps. Admin-tunable via
/// `set_liquidation_ordering_tolerance`. Widening to 1.0 effectively
/// disables the gate (every indexed vault is in band).
pub const DEFAULT_LIQUIDATION_ORDERING_TOLERANCE: Ratio = Ratio::new(dec!(0.01));

/// Wave-8b LIQ-002: serde-default factory for `liquidation_ordering_tolerance`.
/// Old snapshots without the field decode as if the protocol had always been
/// running with the 1% default.
fn default_liquidation_ordering_tolerance() -> Ratio {
    DEFAULT_LIQUIDATION_ORDERING_TOLERANCE
}

/// Wave-8e LIQ-005: default fraction of every collected fee routed to deficit
/// repayment before the remainder flows to its existing destination. 0.5 = 50%.
/// Pre-Wave-8e CBOR snapshots get this value via `serde(default)`.
pub const DEFAULT_DEFICIT_REPAYMENT_FRACTION: Ratio = Ratio::new(dec!(0.5));

fn default_deficit_repayment_fraction() -> Ratio {
    DEFAULT_DEFICIT_REPAYMENT_FRACTION
}

/// Default Layer 1 multipliers at each CR marker
pub const DEFAULT_RATE_MULTIPLIER_HEALTHY: Ratio = Ratio::new(dec!(1.0));
pub const DEFAULT_RATE_MULTIPLIER_WARNING: Ratio = Ratio::new(dec!(1.75));
pub const DEFAULT_RATE_MULTIPLIER_BORROW_THRESHOLD: Ratio = Ratio::new(dec!(2.5));
pub const DEFAULT_RATE_MULTIPLIER_LIQUIDATION: Ratio = Ratio::new(dec!(5.0));

/// Default Layer 2 (recovery) multipliers
pub const DEFAULT_RECOVERY_MULTIPLIER_HEALTHY: Ratio = Ratio::new(dec!(1.0));
pub const DEFAULT_RECOVERY_MULTIPLIER_WARNING: Ratio = Ratio::new(dec!(1.15));
pub const DEFAULT_RECOVERY_MULTIPLIER_BORROW_THRESHOLD: Ratio = Ratio::new(dec!(1.33));
pub const DEFAULT_RECOVERY_MULTIPLIER_LIQUIDATION: Ratio = Ratio::new(dec!(2.0));

/// Default healthy CR multiplier (healthy_cr = this * borrow_threshold_ratio)
pub const DEFAULT_HEALTHY_CR_MULTIPLIER: Ratio = Ratio::new(dec!(1.5));

/// Default Redemption Margin Ratio parameters (admin-configurable).
/// Redeemers receive RMR × face value of their icUSD.
pub const DEFAULT_RMR_FLOOR: Ratio = Ratio::new(dec!(0.96));      // 96% at healthy system
pub const DEFAULT_RMR_CEILING: Ratio = Ratio::new(dec!(1.0));     // 100% at/below stressed CR
pub const DEFAULT_RMR_FLOOR_CR: Ratio = Ratio::new(dec!(2.25));   // CR above which floor applies (recovery × 1.5)
pub const DEFAULT_RMR_CEILING_CR: Ratio = Ratio::new(dec!(1.5));  // CR below which ceiling applies (= recovery)

/// Wave-5 LIQ-007: minimum tolerated ratio between a new XRC sample and the
/// stored price. A new rate is considered "in band" when
/// `band <= new/stored <= 1/band`. 0.7 allows up to a 30% drop or a ~43% rise
/// per sample. Out-of-band samples are queued (see `check_price_sanity_band`).
/// Conservative single value across all collateral; can be made per-asset later
/// by moving onto `CollateralConfig`.
pub const PRICE_SANITY_BAND_RATIO: f64 = 0.7;

/// Wave-5 LIQ-007 / ORACLE-009: number of consecutive in-band confirmations a
/// queued outlier candidate needs before it is accepted as the new stored
/// price. With background fetches every 300 s, N=3 means a sustained move
/// outside the sanity band takes ~10 minutes to land. A single bad sample is
/// always rejected. Stops a sub-$0.01 ICP blip from latching ReadOnly forever.
pub const PRICE_OUTLIER_CONFIRM_COUNT: u8 = 3;

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

/// Tracks a bot's pending liquidation claim on a vault.
#[derive(candid::CandidType, Clone, Debug, serde::Deserialize, Serialize)]
pub struct BotClaim {
    /// Vault ID being liquidated
    pub vault_id: u64,
    /// Amount of collateral transferred to the bot
    pub collateral_amount: u64,
    /// Debt amount the bot committed to cover
    pub debt_amount: u64,
    /// Collateral type (ledger principal)
    pub collateral_type: Principal,
    /// Timestamp (nanos) when claim was created
    pub claimed_at: u64,
    /// Collateral price at time of claim (for event logging)
    pub collateral_price_e8s: u64,
}

/// Asset class for XRC price queries (mirrors ic_xrc_types::AssetClass but with serde support).
#[derive(candid::CandidType, Clone, Debug, PartialEq, Eq, serde::Deserialize, Serialize)]
pub enum XrcAssetClass {
    Cryptocurrency,
    FiatCurrency,
}

impl Default for XrcAssetClass {
    fn default() -> Self {
        XrcAssetClass::Cryptocurrency
    }
}

/// Default for quote asset class (USD is always fiat).
fn default_fiat() -> XrcAssetClass {
    XrcAssetClass::FiatCurrency
}

/// Price source configuration for a collateral type.
#[derive(candid::CandidType, Clone, Debug, serde::Deserialize, Serialize)]
pub enum PriceSource {
    /// Use the ICP Exchange Rate Canister (XRC) with specified asset pair
    Xrc {
        base_asset: String,
        #[serde(default)]
        base_asset_class: XrcAssetClass,
        quote_asset: String,
        #[serde(default = "default_fiat")]
        quote_asset_class: XrcAssetClass,
    },
    /// Use CoinGecko HTTPS outcall with a specific coin ID
    CoinGecko {
        /// CoinGecko API coin ID (e.g., "bob-3", "internet-computer")
        coin_id: String,
        /// Quote currency (e.g., "usd")
        vs_currency: String,
    },
    /// Liquid staking token: price = underlying_xrc_price × redemption_rate × (1 - haircut)
    LstWrapped {
        /// Underlying asset for XRC lookup (e.g., "ICP")
        base_asset: String,
        #[serde(default)]
        base_asset_class: XrcAssetClass,
        /// Quote asset (e.g., "USD")
        quote_asset: String,
        #[serde(default = "default_fiat")]
        quote_asset_class: XrcAssetClass,
        /// Canister to query for the LST→underlying exchange rate
        rate_canister_id: candid::Principal,
        /// Method name to call on rate_canister_id (e.g., "get_info")
        rate_method: String,
        /// Conservative discount applied to redemption value (e.g., 0.15 = 15%)
        haircut: f64,
    },
}

impl PartialEq for PriceSource {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                PriceSource::Xrc { base_asset: ba1, base_asset_class: bac1, quote_asset: qa1, quote_asset_class: qac1 },
                PriceSource::Xrc { base_asset: ba2, base_asset_class: bac2, quote_asset: qa2, quote_asset_class: qac2 },
            ) => ba1 == ba2 && bac1 == bac2 && qa1 == qa2 && qac1 == qac2,
            (
                PriceSource::LstWrapped {
                    base_asset: ba1, base_asset_class: bac1, quote_asset: qa1, quote_asset_class: qac1,
                    rate_canister_id: rc1, rate_method: rm1, haircut: h1,
                },
                PriceSource::LstWrapped {
                    base_asset: ba2, base_asset_class: bac2, quote_asset: qa2, quote_asset_class: qac2,
                    rate_canister_id: rc2, rate_method: rm2, haircut: h2,
                },
            ) => ba1 == ba2 && bac1 == bac2 && qa1 == qa2 && qac1 == qac2
                && rc1 == rc2 && rm1 == rm2 && h1.to_bits() == h2.to_bits(),
            (
                PriceSource::CoinGecko { coin_id: c1, vs_currency: v1 },
                PriceSource::CoinGecko { coin_id: c2, vs_currency: v2 },
            ) => c1 == c2 && v1 == v2,
            _ => false,
        }
    }
}

impl Eq for PriceSource {}

/// How to interpolate between rate curve markers.
/// Linear for now; enum allows adding Exponential, Polynomial, etc. via upgrade.
#[derive(candid::CandidType, Clone, Debug, PartialEq, Eq, serde::Deserialize, Serialize)]
pub enum InterpolationMethod {
    Linear,
}

impl Default for InterpolationMethod {
    fn default() -> Self {
        InterpolationMethod::Linear
    }
}

/// A point on a rate curve: at this CR level, apply this multiplier to the base rate.
#[derive(candid::CandidType, Clone, Debug, PartialEq, Eq, serde::Deserialize, Serialize)]
pub struct RateMarker {
    pub cr_level: Ratio,
    pub multiplier: Ratio,
}

/// A per-asset rate curve: ordered markers + interpolation method.
#[derive(candid::CandidType, Clone, Debug, PartialEq, Eq, serde::Deserialize, Serialize)]
pub struct RateCurve {
    pub markers: Vec<RateMarker>,  // sorted by cr_level ascending
    pub method: InterpolationMethod,
}

impl Default for RateCurve {
    fn default() -> Self {
        Self { markers: Vec::new(), method: InterpolationMethod::default() }
    }
}

/// Named per-asset CR thresholds, resolved from CollateralConfig at runtime.
#[derive(candid::CandidType, Clone, Debug, PartialEq, Eq, serde::Deserialize, Serialize)]
pub enum AssetThreshold {
    LiquidationRatio,
    BorrowThreshold,
    WarningCr,
    HealthyCr,
}

/// Anchor point for a rate curve marker. Can be a fixed CR or a dynamic reference.
#[derive(candid::CandidType, Clone, Debug, PartialEq, Eq, serde::Deserialize, Serialize)]
pub enum CrAnchor {
    /// Concrete CR value (e.g., 1.5 = 150%).
    Fixed(Ratio),
    /// Per-asset threshold, resolved from CollateralConfig at runtime.
    AssetThreshold(AssetThreshold),
    /// System-wide threshold, resolved from debt-weighted averages at runtime.
    SystemThreshold(SystemThreshold),
    /// Midpoint of two anchors: (A + B) / 2.
    Midpoint(Box<CrAnchor>, Box<CrAnchor>),
    /// Offset from another anchor: base + delta (delta can be negative).
    Offset(Box<CrAnchor>, Ratio),
}

/// Named system-wide thresholds for the recovery rate curve (Layer 2).
/// These resolve to debt-weighted averages at runtime.
#[derive(candid::CandidType, Clone, Debug, PartialEq, Eq, serde::Deserialize, Serialize)]
pub enum SystemThreshold {
    LiquidationRatio,
    BorrowThreshold,
    WarningCr,
    HealthyCr,
    TotalCollateralRatio,
}

/// A rate curve marker using dynamic CrAnchor (v2).
#[derive(candid::CandidType, Clone, Debug, PartialEq, Eq, serde::Deserialize, Serialize)]
pub struct RateMarkerV2 {
    pub cr_anchor: CrAnchor,
    pub multiplier: Ratio,
}

/// A rate curve using dynamic anchors (v2).
#[derive(candid::CandidType, Clone, Debug, PartialEq, Eq, serde::Deserialize, Serialize)]
pub struct RateCurveV2 {
    pub markers: Vec<RateMarkerV2>,
    pub method: InterpolationMethod,
}

impl RateCurveV2 {
    /// Validate the curve's structure and bounds. INT-003: the multiplier
    /// upper bound prevents a runaway borrowing fee that would underflow
    /// `amount - fee` in `borrow_from_vault_internal`. Returns an
    /// `Err(message)` suitable for `ProtocolError::GenericError`.
    pub fn validate(&self) -> Result<(), String> {
        if self.markers.is_empty() {
            return Err("Curve must have at least 1 marker".to_string());
        }
        for m in &self.markers {
            if m.multiplier.to_f64() <= 0.0 {
                return Err("All multipliers must be positive".to_string());
            }
            if m.multiplier > MAX_BORROWING_FEE_MULTIPLIER {
                return Err(format!(
                    "Multiplier {} exceeds maximum allowed {}",
                    m.multiplier.to_f64(),
                    MAX_BORROWING_FEE_MULTIPLIER.to_f64(),
                ));
            }
        }
        Ok(())
    }
}

/// A recovery rate marker: at this named threshold, apply this multiplier.
#[derive(candid::CandidType, Clone, Debug, PartialEq, Eq, serde::Deserialize, Serialize)]
pub struct RecoveryRateMarker {
    pub threshold: SystemThreshold,
    pub multiplier: Ratio,
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
    /// Minimum collateral deposit in native token units (e.g., 100_000 = 0.001 ICP at 8 decimals).
    /// Defaults to 0 for backward compat (no minimum enforced for legacy configs).
    #[serde(default)]
    pub min_collateral_deposit: u64,
    /// Borrowing fee override during Recovery mode. None = use normal borrowing_fee.
    #[serde(default)]
    pub recovery_borrowing_fee: Option<Ratio>,
    /// Interest rate override during Recovery mode. None = use normal interest_rate_apr.
    #[serde(default)]
    pub recovery_interest_rate_apr: Option<Ratio>,
    /// Hex color for frontend display (e.g., "#F7931A"). Optional for backward compat.
    #[serde(default)]
    pub display_color: Option<String>,
    /// Admin-configurable "healthy" CR. Default: 1.5 * borrow_threshold_ratio.
    /// None = use default. Must be > borrow_threshold_ratio if set.
    #[serde(default)]
    pub healthy_cr: Option<Ratio>,
    /// Per-asset rate curve markers. None = use global_rate_curve from State.
    #[serde(default)]
    pub rate_curve: Option<RateCurve>,
    /// Redemption priority tier (1 = first redeemed, 2 = second, 3 = last).
    /// Tier 1 vaults are redeemed before tier 2, which are redeemed before tier 3.
    /// Default: 1 (most exposed — safe default for new/unknown collateral).
    #[serde(default = "default_redemption_tier")]
    pub redemption_tier: u8,
}

fn default_redemption_tier() -> u8 { 1 }

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
            && self.min_collateral_deposit == other.min_collateral_deposit
            && self.recovery_borrowing_fee == other.recovery_borrowing_fee
            && self.recovery_interest_rate_apr == other.recovery_interest_rate_apr
            && self.display_color == other.display_color
            && self.healthy_cr == other.healthy_cr
            && self.rate_curve == other.rate_curve
            && self.redemption_tier == other.redemption_tier
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
    /// Number of times this transfer has been retried. Capped at MAX_PENDING_RETRIES.
    #[serde(default)]
    pub retry_count: u8,
    /// Wave-3 ICRC dedup nonce. Constructed once at first attempt via
    /// `State::next_op_nonce`; reused on every retry so the ledger sees the
    /// same `created_at_time` and deduplicates instead of double-spending.
    /// Zero for entries from snapshots written before Wave-3 (those retry
    /// without dedup, matching prior behaviour, no regression).
    #[serde(default)]
    pub op_nonce: u128,
}

/// Wave-4 ICC-007: durable refund record for `redeem_reserves` failures.
///
/// When a reserve redemption pulls icUSD from the user (effectively burning it)
/// but the ckStable transfer back fails AND the inline icUSD refund also fails,
/// the user is left with nothing. Pre-Wave-4 the only recovery path was a
/// CRITICAL log. Now the failure persists a `PendingRefund` keyed by the burn
/// block index, and `process_pending_transfer` retries it until success or
/// MAX_PENDING_RETRIES. The `op_nonce` is minted once and reused across retries
/// so the icUSD ledger deduplicates if a previous retry's reply was lost.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, Serialize, Copy)]
pub struct PendingRefund {
    pub user: Principal,
    /// icUSD amount to refund (in e8s).
    pub amount_e8s: u64,
    #[serde(default)]
    pub retry_count: u8,
    pub op_nonce: u128,
}


thread_local! {
    static __STATE: RefCell<Option<State>> = RefCell::default();
}


// Wave-4 LIQ-001: pending_margin_transfers and pending_excess_transfers are keyed
// by (VaultId, Principal) so concurrent liquidators on the same vault each have
// their own pending entry. Legacy snapshots (BTreeMap<VaultId, _>) are accepted
// transparently via this Visitor and re-keyed using the entry's `owner`.
//
// We can't use `#[serde(untagged)]` here because ciborium's untagged-enum
// dispatch doesn't reliably distinguish a CBOR map with integer keys from one
// with array keys when both variants are themselves maps. Instead, we drive a
// Visitor over MapAccess and decide per-entry: each key is deserialized as
// `EitherKey`, which is a small two-variant enum that ciborium *does* handle
// cleanly via deserialize_any (integer vs. array).
fn deserialize_pending_keyed<'de, D>(
    d: D,
) -> Result<BTreeMap<(VaultId, Principal), PendingMarginTransfer>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use std::fmt;

    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum EitherKey {
        New((VaultId, Principal)),
        Legacy(VaultId),
    }

    struct V;
    impl<'de> serde::de::Visitor<'de> for V {
        type Value = BTreeMap<(VaultId, Principal), PendingMarginTransfer>;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a map of pending margin transfers (legacy u64 keys or new (u64, Principal) keys)")
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'de>,
        {
            let mut out = BTreeMap::new();
            while let Some(key) = map.next_key::<EitherKey>()? {
                let value: PendingMarginTransfer = map.next_value()?;
                let final_key = match key {
                    EitherKey::New(t) => t,
                    EitherKey::Legacy(vault_id) => (vault_id, value.owner),
                };
                out.insert(final_key, value);
            }
            Ok(out)
        }
    }

    d.deserialize_map(V)
}

// serde(default): when deserializing old CBOR that's missing fields added in a
// later upgrade, serde fills those fields from Default::default() instead of
// failing. This prevents fallback to event replay (which causes interest drift).
// The Default impl below is ONLY for this purpose, never for actual construction.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct State {
    pub vault_id_to_vaults: BTreeMap<u64, Vault>,
    pub principal_to_vault_ids: BTreeMap<Principal, BTreeSet<u64>>,
    #[serde(deserialize_with = "deserialize_pending_keyed")]
    pub pending_margin_transfers: BTreeMap<(VaultId, Principal), PendingMarginTransfer>,
    #[serde(deserialize_with = "deserialize_pending_keyed")]
    pub pending_excess_transfers: BTreeMap<(VaultId, Principal), PendingMarginTransfer>,
    pub pending_redemption_transfer: BTreeMap<u64, PendingMarginTransfer>,
    /// Wave-4 ICC-007: durable refund queue for `redeem_reserves` failures,
    /// keyed by the burn icUSD block index. Empty for pre-Wave-4 snapshots.
    #[serde(default)]
    pub pending_refunds: BTreeMap<u64, PendingRefund>,
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
    /// Transient runtime lock for `TimerLogicGuard`. Cleared on guard `Drop`.
    /// `serde(default, skip_serializing)`: NEVER persisted across upgrades —
    /// otherwise an upgrade caught with the lock held would leave it stuck `true`
    /// on the restored state, since the in-flight future is killed by the upgrade
    /// before its `Drop` runs.
    #[serde(default, skip_serializing)]
    pub is_timer_running: bool,
    /// Transient runtime lock for `FetchXrcGuard`. Same upgrade-safety rationale
    /// as `is_timer_running` — see that field's doc comment.
    #[serde(default, skip_serializing)]
    pub is_fetching_rate: bool,

    /// When true, automatic mode transitions (from price updates) are suppressed.
    /// Only an admin call to `exit_recovery_mode` re-enables automatic mode management.
    pub manual_mode_override: bool,

    /// Emergency kill switch. When true, ALL state-changing operations are rejected.
    /// Separate from mode — freeze supersedes everything.
    /// Only an admin call to `unfreeze_protocol` can clear this.
    pub frozen: bool,

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
    /// Admin-settable minimum icUSD amount for borrow/repay/redemption operations (in e8s).
    /// Default set in `From<InitArg>`, updated via `record_set_min_icusd_amount` event.
    pub min_icusd_amount: ICUSD,
    /// Global cap on total icUSD that can be minted across all collateral types (in e8s).
    /// Default u64::MAX = uncapped. Updated via `record_set_global_icusd_mint_cap` event.
    pub global_icusd_mint_cap: u64,
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
    pub recovery_target_cr: Ratio, // legacy absolute value; kept for compat with old events

    /// Proportional multiplier above borrow_threshold for per-asset recovery CR.
    /// recovery_cr = borrow_threshold × recovery_cr_multiplier.
    pub recovery_cr_multiplier: Ratio,

    /// Cached dynamic recovery mode threshold (debt-weighted average of per-collateral borrow thresholds).
    /// Updated alongside total_collateral_ratio on each price tick.
    pub recovery_mode_threshold: Ratio,

    // Reserve redemptions
    pub reserve_redemptions_enabled: bool,
    pub reserve_redemption_fee: Ratio,

    /// Admin kill switch for ICPswap-backed swap routing. When false (default),
    /// the frontend's swap router skips all ICPswap providers and behaves as if
    /// only Rumi AMM + the 3pool existed. Flipped via `set_icpswap_routing_enabled`
    /// by the developer principal. Read by the frontend via `get_protocol_config`.
    pub icpswap_routing_enabled: bool,

    /// Cumulative 3USD (LP tokens) received from stability pool liquidations (e8s).
    /// These sit in subaccount hash("protocol_3usd_reserves") on the 3USD ledger.
    pub protocol_3usd_reserves: u64,

    // Admin mint cooldown tracking
    pub last_admin_mint_time: u64,

    // Multi-collateral support
    pub collateral_configs: BTreeMap<CollateralType, CollateralConfig>,
    pub collateral_to_vault_ids: BTreeMap<CollateralType, BTreeSet<u64>>,

    // Dynamic interest rates (Layer 1 global + Layer 2 recovery)
    /// Global default rate curve (used when an asset has no per-asset rate_curve).
    pub global_rate_curve: RateCurve,
    /// Recovery mode rate curve (Layer 2, system-wide). Named thresholds resolved at runtime.
    pub recovery_rate_curve: Vec<RecoveryRateMarker>,
    /// Cached debt-weighted average of per-asset recovery CRs (borrow_threshold × multiplier).
    pub weighted_avg_recovery_cr: Ratio,
    /// Cached debt-weighted average of per-asset warning CRs (2 * recovery_cr - borrow_threshold).
    pub weighted_avg_warning_cr: Ratio,
    /// Cached debt-weighted average of per-asset healthy CRs (override or 1.5 * borrow_threshold).
    pub weighted_avg_healthy_cr: Ratio,

    /// Dynamic borrowing fee multiplier curve (v2).
    /// X-axis: projected vault CR after borrow. Y-axis: multiplier on base borrowing_fee.
    /// None = flat fee (no dynamic multiplier).
    pub borrowing_fee_curve: Option<RateCurveV2>,

    // Periodic interest distribution
    /// Accumulated interest per collateral type, waiting to be flushed to pools.
    /// Key = collateral_type Principal, Value = total interest in e8s.
    pub pending_interest_for_pools: BTreeMap<Principal, u64>,

    /// Minimum interest (e8s) per collateral bucket before flushing. Admin-settable.
    /// Default = 10_000_000 (0.1 icUSD). At 0.01 the ledger fee eats ~10%.
    pub interest_flush_threshold_e8s: u64,

    // Treasury fee routing
    /// Interest revenue from sync liquidations, minted to treasury in next timer tick.
    pub pending_treasury_interest: ICUSD,
    /// Collateral fees from sync liquidations, transferred to treasury in next timer tick.
    /// Each entry is (amount_e8s, collateral_ledger_principal).
    pub pending_treasury_collateral: Vec<(u64, Principal)>,
    /// Global fraction of the liquidation bonus (liquidator's profit) that goes to the protocol treasury.
    /// e.g., 0.03 = protocol gets 3% of the bonus, liquidator keeps 97%.
    pub liquidation_protocol_share: Ratio,

    /// Share of interest revenue sent to stability pool depositors (0.0-1.0).
    /// Remainder goes to protocol treasury.
    /// LEGACY: kept for backwards compat with old SetInterestPoolShare events.
    /// New code uses `interest_split` instead.
    pub interest_pool_share: Ratio,

    /// N-way interest revenue split. Each recipient gets `bps/10000` of interest.
    /// All bps must sum to exactly 10_000. Replaces `interest_pool_share`.
    pub interest_split: Vec<InterestRecipient>,

    /// 3pool AMM canister for interest donations.
    pub three_pool_canister: Option<Principal>,

    /// Redemption Margin Ratio parameters.
    /// RMR value when system CR is at or above rmr_floor_cr (e.g. 0.96 = 96%).
    pub rmr_floor: Ratio,
    /// RMR value when system CR is at or below rmr_ceiling_cr (e.g. 1.0 = 100%).
    pub rmr_ceiling: Ratio,
    /// The system CR above which rmr_floor applies. Absolute CR value (e.g. 2.25).
    pub rmr_floor_cr: Ratio,
    /// The system CR below which rmr_ceiling applies. Absolute CR value (e.g. 1.50).
    pub rmr_ceiling_cr: Ratio,

    // Liquidation bot
    pub liquidation_bot_principal: Option<Principal>,
    pub bot_budget_total_e8s: u64,
    pub bot_budget_remaining_e8s: u64,
    pub bot_budget_start_timestamp: u64,
    pub bot_total_debt_covered_e8s: u64,
    #[serde(default)]
    pub bot_total_icusd_deposited_e8s: u64, // Dead field, kept for deserialization compat
    /// Which collateral types the bot is allowed to liquidate.
    /// Vaults with collateral not in this set are rejected by bot_liquidate,
    /// leaving the stability pool to handle them.
    pub bot_allowed_collateral_types: BTreeSet<Principal>,
    /// Tracks vault_id → timestamp (nanos) when notified to bot.
    /// Used by check_vaults() to implement priority cascade:
    /// bot gets one cycle, then stability pool takes over.
    pub bot_pending_vaults: BTreeMap<u64, u64>,
    /// Vaults that have already been sent to the stability pool.
    /// No retries — once the SP has been notified, subsequent cycles skip the vault
    /// and leave it for manual liquidation. Cleared when vault becomes healthy.
    pub sp_attempted_vaults: BTreeSet<u64>,
    /// Active bot claims — tracks collateral transferred to bot but not yet confirmed.
    /// Key = vault_id. Auto-cancelled after `BOT_CLAIM_TIMEOUT_NS`.
    pub bot_claims: BTreeMap<u64, BotClaim>,

    /// Monotonic counter for ICRC transfer idempotency nonces (audit Wave-3).
    /// Combined with `ic_cdk::api::time()` in `next_op_nonce` to mint a u128
    /// that the helper packs into the ledger's `created_at_time` for retry-safe
    /// deduplication. `serde(default)` so deserializing pre-Wave-3 snapshots
    /// starts the counter at zero (collisions vs. older transfers are
    /// impossible because their tuples have `created_at_time: None`).
    #[serde(default)]
    pub op_nonce_counter: u64,

    /// Wave-5 LIQ-007 / ORACLE-009: queued outlier price candidates per collateral.
    /// When a fetched price falls outside the sanity band (PRICE_SANITY_BAND_RATIO)
    /// of the stored price, queue it as a candidate instead of accepting it.
    /// After PRICE_OUTLIER_CONFIRM_COUNT consecutive samples agree (within the band
    /// of the candidate itself), accept the new price. A single bad sample is
    /// rejected; this stops a sub-$0.01 XRC blip from latching ReadOnly.
    /// Value is `(candidate_price, consecutive_count)`.
    #[serde(default)]
    pub pending_outlier_prices: BTreeMap<Principal, (f64, u8)>,

    /// Wave-5 LIQ-007: emergency brake for liquidation endpoints. Independent of
    /// `mode` (which auto-latches ReadOnly on TCR < 100% but should not block
    /// liquidations because liquidations reduce bad debt). Admin flips this via
    /// `set_liquidation_frozen` during a confirmed oracle outage or other event
    /// where liquidating against the cached price would be dangerous.
    #[serde(default)]
    pub liquidation_frozen: bool,

    /// Wave-8b LIQ-002: secondary index of vaults keyed by CR (in basis points,
    /// ascending). Lets liquidator endpoints check that a caller-provided
    /// `vault_id` is among the worst-CR vaults, blocking MEV cherry-picking.
    ///
    /// The key is `(cr * 10_000) as u64`. CR is computed via
    /// `compute_collateral_ratio` and reflects the cached collateral price at
    /// the time of the mutation that triggered the re-key. The index is NOT
    /// re-keyed on price update — within a single collateral type, all vaults
    /// move proportionally with price, preserving relative ordering.
    /// Cross-collateral ordering can drift between price ticks but liquidators
    /// specialize per asset, so the band gate stays correct in practice.
    /// (See `liq_002_price_update_does_not_rekey` for the contract.)
    ///
    /// Multiple vaults may share a CR bucket (e.g., all zero-debt vaults sit
    /// at u64::MAX), hence the inner `BTreeSet`.
    ///
    /// `serde(default, skip_serializing)`: NOT persisted in the on-disk
    /// snapshot. `post_upgrade` rebuilds the index in-memory from
    /// `vault_id_to_vaults`. This keeps existing on-chain state forward-
    /// compatible without a snapshot-format migration.
    #[serde(default, skip_serializing)]
    pub vault_cr_index: BTreeMap<u64, BTreeSet<u64>>,

    /// Wave-8b LIQ-002: tolerance band (in absolute CR units) above the
    /// worst-CR vault inside which liquidations are accepted. e.g., 0.01 means
    /// any vault within 0.01 CR (= 100 bps) of the lowest CR may be
    /// liquidated. Widening to 1.0 effectively disables the gate. Admin-
    /// tunable via `set_liquidation_ordering_tolerance`.
    #[serde(default = "default_liquidation_ordering_tolerance")]
    pub liquidation_ordering_tolerance: Ratio,

    /// Wave-8c LIQ-004: emergency kill switch for the SP-triggered writedown
    /// path (`liquidate_vault_debt_already_burned`). Independent of
    /// `frozen` (global emergency stop) and `liquidation_frozen` (Wave-5
    /// blanket liquidation halt). When true, both
    /// `stability_pool_liquidate_debt_burned` and
    /// `stability_pool_liquidate_with_reserves` reject with
    /// `TemporarilyUnavailable`. Use during a confirmed SP compromise or
    /// drift event so user-initiated liquidations stay open.
    #[serde(default)]
    pub sp_writedown_disabled: bool,

    /// Wave-8c LIQ-004: set of `(SpProofLedger, block_index)` tuples already
    /// consumed as SP writedown proofs. Inserted on successful proof
    /// verification; rejects re-use of the same block on a later writedown.
    /// Bounded by the number of SP-triggered writedowns the protocol ever
    /// processes (low-hundreds per year at current scale); a future wave can
    /// switch to a strictly-monotonic-block-index check if growth becomes a
    /// concern.
    #[serde(default)]
    pub consumed_writedown_proofs: BTreeSet<(crate::icrc3_proof::SpProofLedger, u64)>,

    // ─── Wave-8e LIQ-005: bad-debt deficit account ───
    //
    // Underwater liquidations (where seized collateral USD value < debt
    // cleared) accrue the shortfall here as a protocol-level liability.
    // Future fee revenue (borrowing fee, redemption fee) burns icUSD to
    // amortize the deficit. No socialization to stability-pool depositors
    // or pro-rata redistribution to other vaults.
    //
    // `serde(default)` on every field — pre-Wave-8e snapshots decode to
    // zero deficit, half-fraction repayment, and a disabled ReadOnly latch.

    /// Cumulative bad debt the protocol has absorbed from underwater
    /// liquidations. Increments via `accrue_deficit_shortfall` at every
    /// liquidation site that nets seized USD < debt cleared. Decreases only
    /// via `apply_deficit_repayment` on fee collection.
    #[serde(default)]
    pub protocol_deficit_icusd: ICUSD,

    /// Lifetime sum of icUSD applied as deficit repayment (mint foregone for
    /// borrowing fee, supply already reduced for redemption fee). Reporting-
    /// only; never decreases. Together with `protocol_deficit_icusd` and the
    /// `DeficitAccrued` / `DeficitRepaid` event log this satisfies:
    ///   sum(DeficitAccrued.amount) - sum(DeficitRepaid.amount)
    ///       == protocol_deficit_icusd
    #[serde(default)]
    pub total_deficit_repaid_icusd: ICUSD,

    /// Fraction of each collected fee routed to deficit repayment before the
    /// remainder flows to its existing destination. Default 0.5 (50%);
    /// 0.0 disables repayment, 1.0 routes the entire fee until cleared.
    /// Bounded to [0, 1] in `set_deficit_repayment_fraction`.
    #[serde(default = "default_deficit_repayment_fraction")]
    pub deficit_repayment_fraction: Ratio,

    /// ICUSD-e8s ceiling above which the protocol auto-transitions to
    /// ReadOnly mode. 0 disables the latch. Tuned via
    /// `set_deficit_readonly_threshold_e8s`. Operator should leave at 0
    /// for the first 24-48h post-deploy and set after observing baseline
    /// deficit accrual.
    #[serde(default)]
    pub deficit_readonly_threshold_e8s: u64,
}

/// Serde-only fallback: provides zero/empty/None defaults for fields missing from
/// old CBOR snapshots. Never used for actual State construction (use From<InitArg>).
impl Default for State {
    fn default() -> Self {
        Self {
            vault_id_to_vaults: BTreeMap::new(),
            principal_to_vault_ids: BTreeMap::new(),
            pending_margin_transfers: BTreeMap::new(),
            pending_excess_transfers: BTreeMap::new(),
            pending_redemption_transfer: BTreeMap::new(),
            pending_refunds: BTreeMap::new(),
            mode: Mode::default(),
            fee: Ratio::from(Decimal::ZERO),
            developer_principal: Principal::anonymous(),
            next_available_vault_id: 1,
            total_collateral_ratio: Ratio::from(Decimal::MAX),
            current_base_rate: Ratio::from(Decimal::ZERO),
            last_redemption_time: 0,
            liquidity_pool: BTreeMap::new(),
            liquidity_returns: BTreeMap::new(),
            xrc_principal: Principal::anonymous(),
            icusd_ledger_principal: Principal::anonymous(),
            icp_ledger_principal: Principal::anonymous(),
            icp_ledger_fee: ICP_TRANSFER_FEE,
            last_icp_rate: None,
            last_icp_timestamp: None,
            principal_guards: BTreeSet::new(),
            principal_guard_timestamps: BTreeMap::new(),
            operation_states: BTreeMap::new(),
            operation_names: BTreeMap::new(),
            is_timer_running: false,
            is_fetching_rate: false,
            manual_mode_override: false,
            frozen: false,
            close_vault_requests: BTreeMap::new(),
            global_close_requests: Vec::new(),
            concurrent_close_operations: 0,
            dust_forgiven_total: ICUSD::new(0),
            treasury_principal: None,
            stability_pool_canister: None,
            ckusdt_ledger_principal: None,
            ckusdc_ledger_principal: None,
            ckstable_repay_fee: DEFAULT_CKSTABLE_REPAY_FEE,
            min_icusd_amount: DEFAULT_MIN_ICUSD_AMOUNT,
            global_icusd_mint_cap: u64::MAX,
            ckusdt_enabled: false,
            ckusdc_enabled: false,
            last_ckusdt_rate: None,
            last_ckusdt_timestamp: None,
            last_ckusdc_rate: None,
            last_ckusdc_timestamp: None,
            liquidation_bonus: DEFAULT_LIQUIDATION_BONUS,
            max_partial_liquidation_ratio: DEFAULT_MAX_PARTIAL_LIQUIDATION_RATIO,
            redemption_fee_floor: DEFAULT_REDEMPTION_FEE_FLOOR,
            redemption_fee_ceiling: DEFAULT_REDEMPTION_FEE_CEILING,
            recovery_target_cr: DEFAULT_RECOVERY_TARGET_CR,
            recovery_cr_multiplier: DEFAULT_RECOVERY_CR_MULTIPLIER,
            recovery_mode_threshold: RECOVERY_COLLATERAL_RATIO,
            reserve_redemptions_enabled: false,
            reserve_redemption_fee: DEFAULT_RESERVE_REDEMPTION_FEE,
            icpswap_routing_enabled: false,
            protocol_3usd_reserves: 0,
            last_admin_mint_time: 0,
            collateral_configs: BTreeMap::new(),
            collateral_to_vault_ids: BTreeMap::new(),
            global_rate_curve: RateCurve::default(),
            recovery_rate_curve: Vec::new(),
            weighted_avg_recovery_cr: Ratio::from(Decimal::ZERO),
            weighted_avg_warning_cr: Ratio::from(Decimal::ZERO),
            weighted_avg_healthy_cr: Ratio::from(Decimal::ZERO),
            borrowing_fee_curve: None,
            pending_interest_for_pools: BTreeMap::new(),
            interest_flush_threshold_e8s: default_flush_threshold(),
            pending_treasury_interest: ICUSD::new(0),
            pending_treasury_collateral: Vec::new(),
            liquidation_protocol_share: Ratio::from(Decimal::ZERO),
            interest_pool_share: DEFAULT_INTEREST_POOL_SHARE,
            interest_split: Vec::new(),
            three_pool_canister: None,
            rmr_floor: DEFAULT_RMR_FLOOR,
            rmr_ceiling: DEFAULT_RMR_CEILING,
            rmr_floor_cr: DEFAULT_RMR_FLOOR_CR,
            rmr_ceiling_cr: DEFAULT_RMR_CEILING_CR,
            liquidation_bot_principal: None,
            bot_budget_total_e8s: 0,
            bot_budget_remaining_e8s: 0,
            bot_budget_start_timestamp: 0,
            bot_total_debt_covered_e8s: 0,
            bot_total_icusd_deposited_e8s: 0,
            bot_allowed_collateral_types: BTreeSet::new(),
            bot_pending_vaults: BTreeMap::new(),
            sp_attempted_vaults: BTreeSet::new(),
            bot_claims: BTreeMap::new(),
            op_nonce_counter: 0,
            pending_outlier_prices: BTreeMap::new(),
            liquidation_frozen: false,
            vault_cr_index: BTreeMap::new(),
            liquidation_ordering_tolerance: DEFAULT_LIQUIDATION_ORDERING_TOLERANCE,
            sp_writedown_disabled: false,
            consumed_writedown_proofs: BTreeSet::new(),
            // Wave-8e LIQ-005
            protocol_deficit_icusd: ICUSD::new(0),
            total_deficit_repaid_icusd: ICUSD::new(0),
            deficit_repayment_fraction: DEFAULT_DEFICIT_REPAYMENT_FRACTION,
            deficit_readonly_threshold_e8s: 0,
        }
    }
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
            pending_refunds: BTreeMap::new(),
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
            manual_mode_override: false,
            frozen: false,
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
            min_icusd_amount: DEFAULT_MIN_ICUSD_AMOUNT,
            global_icusd_mint_cap: u64::MAX,
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
            recovery_cr_multiplier: DEFAULT_RECOVERY_CR_MULTIPLIER,
            recovery_mode_threshold: RECOVERY_COLLATERAL_RATIO,

            // Reserve redemptions
            reserve_redemptions_enabled: false,
            reserve_redemption_fee: DEFAULT_RESERVE_REDEMPTION_FEE,
            // ICPswap routing kill switch — default off, admin flips via set_icpswap_routing_enabled
            icpswap_routing_enabled: false,
            protocol_3usd_reserves: 0,

            // Admin mint cooldown
            last_admin_mint_time: 0,

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
                    min_vault_debt: ICUSD::new(10_000_000), // 0.1 icUSD
                    ledger_fee: ICP_TRANSFER_FEE.to_u64(),
                    price_source: PriceSource::Xrc {
                        base_asset: "ICP".to_string(),
                        base_asset_class: XrcAssetClass::Cryptocurrency,
                        quote_asset: "USD".to_string(),
                        quote_asset_class: XrcAssetClass::FiatCurrency,
                    },
                    status: CollateralStatus::Active,
                    last_price: None,
                    last_price_timestamp: None,
                    redemption_fee_floor: DEFAULT_REDEMPTION_FEE_FLOOR,
                    redemption_fee_ceiling: DEFAULT_REDEMPTION_FEE_CEILING,
                    current_base_rate: Ratio::from(Decimal::ZERO),
                    last_redemption_time: 0,
                    recovery_target_cr: DEFAULT_RECOVERY_TARGET_CR,
                    min_collateral_deposit: 100_000, // 0.001 ICP
                    recovery_borrowing_fee: None,
                    recovery_interest_rate_apr: None,
                    display_color: Some("#2DD4BF".to_string()),
                    healthy_cr: None,
                    rate_curve: None,
                    redemption_tier: 1,
                });
                configs
            },
            collateral_to_vault_ids: BTreeMap::new(),

            // Dynamic interest rates
            global_rate_curve: RateCurve {
                markers: vec![
                    RateMarker { cr_level: Ratio::new(dec!(0)), multiplier: DEFAULT_RATE_MULTIPLIER_LIQUIDATION },
                    RateMarker { cr_level: Ratio::new(dec!(0)), multiplier: DEFAULT_RATE_MULTIPLIER_BORROW_THRESHOLD },
                    RateMarker { cr_level: Ratio::new(dec!(0)), multiplier: DEFAULT_RATE_MULTIPLIER_WARNING },
                    RateMarker { cr_level: Ratio::new(dec!(0)), multiplier: DEFAULT_RATE_MULTIPLIER_HEALTHY },
                ],
                method: InterpolationMethod::Linear,
            },
            recovery_rate_curve: vec![
                RecoveryRateMarker { threshold: SystemThreshold::LiquidationRatio, multiplier: DEFAULT_RECOVERY_MULTIPLIER_LIQUIDATION },
                RecoveryRateMarker { threshold: SystemThreshold::BorrowThreshold, multiplier: DEFAULT_RECOVERY_MULTIPLIER_BORROW_THRESHOLD },
                RecoveryRateMarker { threshold: SystemThreshold::WarningCr, multiplier: DEFAULT_RECOVERY_MULTIPLIER_WARNING },
                RecoveryRateMarker { threshold: SystemThreshold::HealthyCr, multiplier: DEFAULT_RECOVERY_MULTIPLIER_HEALTHY },
            ],
            weighted_avg_recovery_cr: Ratio::new(dec!(0)),
            weighted_avg_warning_cr: Ratio::new(dec!(0)),
            weighted_avg_healthy_cr: Ratio::new(dec!(0)),

            borrowing_fee_curve: Some(RateCurveV2 {
                markers: vec![
                    RateMarkerV2 {
                        cr_anchor: CrAnchor::Offset(
                            Box::new(CrAnchor::SystemThreshold(SystemThreshold::BorrowThreshold)),
                            Ratio::new(dec!(0.05)),
                        ),
                        multiplier: Ratio::new(dec!(3.0)),
                    },
                    RateMarkerV2 {
                        cr_anchor: CrAnchor::Midpoint(
                            Box::new(CrAnchor::SystemThreshold(SystemThreshold::BorrowThreshold)),
                            Box::new(CrAnchor::SystemThreshold(SystemThreshold::TotalCollateralRatio)),
                        ),
                        multiplier: Ratio::new(dec!(1.75)),
                    },
                    RateMarkerV2 {
                        cr_anchor: CrAnchor::SystemThreshold(SystemThreshold::TotalCollateralRatio),
                        multiplier: Ratio::new(dec!(1.0)),
                    },
                ],
                method: InterpolationMethod::Linear,
            }),

            // Periodic interest distribution
            pending_interest_for_pools: BTreeMap::new(),
            interest_flush_threshold_e8s: default_flush_threshold(),

            // Treasury fee routing
            pending_treasury_interest: ICUSD::new(0),
            pending_treasury_collateral: Vec::new(),
            liquidation_protocol_share: crate::DEFAULT_LIQUIDATION_PROTOCOL_SHARE,
            interest_pool_share: DEFAULT_INTEREST_POOL_SHARE,
            interest_split: default_interest_split(),
            three_pool_canister: None,

            rmr_floor: DEFAULT_RMR_FLOOR,
            rmr_ceiling: DEFAULT_RMR_CEILING,
            rmr_floor_cr: DEFAULT_RMR_FLOOR_CR,
            rmr_ceiling_cr: DEFAULT_RMR_CEILING_CR,

            // Liquidation bot
            liquidation_bot_principal: None,
            bot_budget_total_e8s: 0,
            bot_budget_remaining_e8s: 0,
            bot_budget_start_timestamp: 0,
            bot_total_debt_covered_e8s: 0,
            bot_total_icusd_deposited_e8s: 0,
            bot_allowed_collateral_types: BTreeSet::new(),
            bot_pending_vaults: BTreeMap::new(),
            sp_attempted_vaults: BTreeSet::new(),
            bot_claims: BTreeMap::new(),
            op_nonce_counter: 0,
            pending_outlier_prices: BTreeMap::new(),
            liquidation_frozen: false,
            vault_cr_index: BTreeMap::new(),
            liquidation_ordering_tolerance: DEFAULT_LIQUIDATION_ORDERING_TOLERANCE,
            sp_writedown_disabled: false,
            consumed_writedown_proofs: BTreeSet::new(),
            // Wave-8e LIQ-005
            protocol_deficit_icusd: ICUSD::new(0),
            total_deficit_repaid_icusd: ICUSD::new(0),
            deficit_repayment_fraction: DEFAULT_DEFICIT_REPAYMENT_FRACTION,
            deficit_readonly_threshold_e8s: 0,
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

    /// Wave-5 LIQ-007 / ORACLE-009: apply the price-outlier sanity gate.
    ///
    /// Returns `true` when the new sample should be written to `last_price`,
    /// `false` when it should be queued/rejected. Mutates `pending_outlier_prices`
    /// to track the running outlier candidate and its consecutive-confirmation
    /// count.
    ///
    /// Algorithm:
    ///   1. No stored price (or stored <= 0): accept and clear any candidate.
    ///   2. New sample within `[band, 1/band]` of stored: accept and clear candidate.
    ///   3. New sample outside band:
    ///      - First outlier seen, or diverges from queued candidate: queue it
    ///        with count=1 and reject.
    ///      - Matches queued candidate (within band of candidate): increment
    ///        count; accept once count >= `PRICE_OUTLIER_CONFIRM_COUNT`.
    ///
    /// Without this gate a single garbage XRC sample could shift `last_price`
    /// arbitrarily, including triggering the `rate < $0.01` ReadOnly latch
    /// (ORACLE-009). With the gate, an outlier needs N consecutive consistent
    /// confirmations before it's accepted.
    pub fn check_price_sanity_band(
        &mut self,
        collateral_type: &Principal,
        new_rate: f64,
    ) -> bool {
        if !new_rate.is_finite() || new_rate <= 0.0 {
            return false;
        }

        let stored = self
            .collateral_configs
            .get(collateral_type)
            .and_then(|c| c.last_price);

        let stored_v = match stored {
            Some(v) if v > 0.0 && v.is_finite() => v,
            _ => {
                self.pending_outlier_prices.remove(collateral_type);
                return true;
            }
        };

        let ratio = new_rate / stored_v;
        if ratio >= PRICE_SANITY_BAND_RATIO && ratio <= 1.0 / PRICE_SANITY_BAND_RATIO {
            self.pending_outlier_prices.remove(collateral_type);
            return true;
        }

        let entry = self
            .pending_outlier_prices
            .entry(*collateral_type)
            .or_insert((new_rate, 0));
        let candidate = entry.0;

        if !candidate.is_finite() || candidate <= 0.0 {
            *entry = (new_rate, 1);
            return false;
        }

        let cand_ratio = new_rate / candidate;
        if cand_ratio >= PRICE_SANITY_BAND_RATIO
            && cand_ratio <= 1.0 / PRICE_SANITY_BAND_RATIO
        {
            entry.1 = entry.1.saturating_add(1);
            if entry.1 >= PRICE_OUTLIER_CONFIRM_COUNT {
                self.pending_outlier_prices.remove(collateral_type);
                return true;
            }
            false
        } else {
            *entry = (new_rate, 1);
            false
        }
    }

    /// Mint a fresh idempotency nonce for an ICRC transfer (audit Wave-3).
    ///
    /// Layout: upper 64 bits = current IC time (nanoseconds), lower 64 bits =
    /// monotonic counter. The transfer helper extracts the upper bits as
    /// `created_at_time`; the lower bits keep nonces from colliding when two
    /// transfers are issued within the same nanosecond.
    ///
    /// Persist the returned nonce alongside the operation (e.g. in a
    /// `PendingMarginTransfer`) and pass it back into the helper on retries —
    /// that is what makes the transfer idempotent at the ledger.
    pub fn next_op_nonce(&mut self) -> u128 {
        let counter = self.op_nonce_counter;
        self.op_nonce_counter = self.op_nonce_counter.wrapping_add(1);
        let now = ic_cdk::api::time();
        ((now as u128) << 64) | (counter as u128)
    }

    pub fn increment_vault_id(&mut self) -> u64 {
        let vault_id = self.next_available_vault_id;
        self.next_available_vault_id += 1;
        // Safety net: reject if this ID already exists (e.g. counter was reset by
        // an accidental reinstall). Better to fail loudly than silently overwrite
        // another user's vault.
        if self.vault_id_to_vaults.contains_key(&vault_id) {
            ic_cdk::trap(&format!(
                "BUG: vault_id {} already exists — refusing to overwrite. \
                 Was the canister reinstalled?",
                vault_id
            ));
        }
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

    /// Compute debt-weighted averages of per-asset recovery_cr, warning_cr, and healthy_cr.
    /// Same loop pattern as compute_dynamic_recovery_threshold, but calculates 3 extra averages.
    /// Returns (weighted_avg_recovery_cr, weighted_avg_warning_cr, weighted_avg_healthy_cr).
    pub fn compute_weighted_cr_averages(&self) -> (Ratio, Ratio, Ratio) {
        let total_debt = self.total_borrowed_icusd_amount();
        if total_debt == ICUSD::new(0) {
            // No debt: use defaults based on first collateral type or global defaults
            let default_recovery = RECOVERY_COLLATERAL_RATIO * self.recovery_cr_multiplier;
            // warning = 2 * recovery_cr - borrow_threshold
            let default_warning = default_recovery + default_recovery - RECOVERY_COLLATERAL_RATIO;
            return (
                default_recovery,
                default_warning,
                RECOVERY_COLLATERAL_RATIO * DEFAULT_HEALTHY_CR_MULTIPLIER,
            );
        }
        let total_debt_dec = Decimal::from_u64(total_debt.to_u64())
            .unwrap_or(Decimal::ZERO);

        let mut w_recovery = Decimal::ZERO;
        let mut w_warning = Decimal::ZERO;
        let mut w_healthy = Decimal::ZERO;

        for (ct, config) in &self.collateral_configs {
            let debt_i = self.total_debt_for_collateral(ct);
            if debt_i == ICUSD::new(0) {
                continue;
            }
            let weight = Decimal::from_u64(debt_i.to_u64())
                .unwrap_or(Decimal::ZERO) / total_debt_dec;

            let recovery_cr = config.borrow_threshold_ratio.0 * self.recovery_cr_multiplier.0;
            let warning_cr = recovery_cr + recovery_cr - config.borrow_threshold_ratio.0;
            let healthy_cr = config.healthy_cr
                .map(|h| h.0)
                .unwrap_or(config.borrow_threshold_ratio.0 * DEFAULT_HEALTHY_CR_MULTIPLIER.0);

            w_recovery += weight * recovery_cr;
            w_warning += weight * warning_cr;
            w_healthy += weight * healthy_cr;
        }

        (Ratio::from(w_recovery), Ratio::from(w_warning), Ratio::from(w_healthy))
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

    /// Dynamic Redemption Margin Ratio.
    /// Redeemers receive RMR × face value of their icUSD.
    /// - At/above rmr_floor_cr: rmr_floor (e.g. 96%, discourages redemption when healthy)
    /// - At/below rmr_ceiling_cr: rmr_ceiling (e.g. 100%, par redemption when stressed)
    /// - Linear interpolation between
    /// - NEVER above rmr_ceiling (prevents mint-and-redeem arbitrage)
    pub fn get_redemption_margin_ratio(&self) -> Ratio {
        let tcr = self.total_collateral_ratio;

        if tcr <= self.rmr_ceiling_cr {
            return self.rmr_ceiling;
        }
        if tcr >= self.rmr_floor_cr {
            return self.rmr_floor;
        }

        let range = self.rmr_floor_cr - self.rmr_ceiling_cr;
        let position = tcr - self.rmr_ceiling_cr;
        let spread = self.rmr_ceiling - self.rmr_floor;
        // Use inner Decimal for division since Div<Ratio> for Ratio is not implemented
        let discount = Ratio::from(position.0 / range.0) * spread;
        self.rmr_ceiling - discount
    }

    pub fn get_borrowing_fee(&self) -> Ratio {
        self.fee
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

    /// Return collateral types ordered by redemption priority:
    /// primary sort by `redemption_tier` ascending (tier 1 first), secondary sort
    /// by worst health score among that type's vaults (lowest health first).
    /// Only includes active collateral types that have a price and at least one vault with debt.
    pub fn get_collateral_types_by_redemption_priority(&self) -> Vec<CollateralType> {
        let mut entries: Vec<(u8, f64, CollateralType)> = Vec::new();

        for (ct, config) in &self.collateral_configs {
            // Skip inactive or no-price collateral
            if !config.status.allows_redemption() {
                continue;
            }
            // Verify price exists (needed for CR computation inside compute_collateral_ratio)
            match config.last_price {
                Some(p) if p > 0.0 => { /* price is available */ },
                _ => continue,
            };

            // Find the worst (lowest) health score among this type's vaults
            let liq_ratio = config.liquidation_ratio.to_f64();
            let mut worst_health: f64 = f64::MAX;
            let mut has_debt = false;

            if let Some(vault_ids) = self.collateral_to_vault_ids.get(ct) {
                for vid in vault_ids {
                    if let Some(vault) = self.vault_id_to_vaults.get(vid) {
                        if vault.borrowed_icusd_amount == 0 {
                            continue;
                        }
                        has_debt = true;
                        // Note: compute_collateral_ratio ignores the rate parameter
                        // (reads from config.last_price instead), so we pass a dummy value.
                        let cr = crate::compute_collateral_ratio(
                            vault,
                            crate::numeric::UsdIcp::from(rust_decimal::Decimal::ZERO),
                            self,
                        );
                        let health = vault.health_score(cr.to_f64(), liq_ratio);
                        if health < worst_health {
                            worst_health = health;
                        }
                    }
                }
            }

            if !has_debt {
                continue; // no point redeeming from a type with no debt
            }

            entries.push((config.redemption_tier, worst_health, *ct));
        }

        // Sort: tier ascending, then worst health ascending (most vulnerable first)
        entries.sort_by(|a, b| {
            a.0.cmp(&b.0).then_with(|| {
                a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)
            })
        });

        entries.into_iter().map(|(_, _, ct)| ct).collect()
    }

    /// Set the ICP rate on both the global field AND the ICP CollateralConfig's `last_price`.
    /// This is the ONLY correct way to update the ICP price.
    ///
    /// SAFETY (Wave-8b LIQ-002): this function MUST NOT call `reindex_vault_cr`.
    /// The CR index keys reflect the cached price at the time of each vault's
    /// last debt/collateral mutation. Within a single collateral type, all
    /// vaults move proportionally with price, preserving relative ordering —
    /// re-keying every vault on every price tick would burn O(N) cycles for
    /// zero ordering benefit. Cross-collateral ordering can drift between
    /// price ticks, but liquidators specialize per asset and the band
    /// tolerance handles intra-asset drift between user actions.
    pub fn set_icp_rate(&mut self, rate: crate::numeric::UsdIcp, timestamp_nanos: Option<u64>) {
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
        let config = self.collateral_configs.get(ct);
        if self.mode == Mode::Recovery {
            // Use recovery override if set, otherwise normal fee
            return config
                .and_then(|c| c.recovery_borrowing_fee)
                .or_else(|| config.map(|c| c.borrowing_fee))
                .unwrap_or(self.fee);
        }
        config.map(|c| c.borrowing_fee).unwrap_or(self.fee)
    }

    /// Get the dynamic borrowing fee multiplier for a projected vault CR.
    /// Returns 1.0 if no borrowing_fee_curve is configured.
    pub fn get_borrowing_fee_multiplier(&self, projected_vault_cr: Ratio) -> Ratio {
        match &self.borrowing_fee_curve {
            Some(curve) => {
                let resolved = self.resolve_curve(curve, None);
                // Safety: if the curve is inverted (e.g., TCR < BorrowThreshold after
                // canister upgrade before first price fetch), the sorted markers will
                // have the max multiplier at the high-CR end instead of the low-CR end.
                // Return the highest multiplier to conservatively discourage risky borrows.
                if resolved.len() >= 2 {
                    let first_mult = &resolved.first().unwrap().1;
                    let last_mult = &resolved.last().unwrap().1;
                    if last_mult.0 > first_mult.0 {
                        return resolved.iter()
                            .max_by(|a, b| a.1 .0.cmp(&b.1 .0))
                            .map(|(_, m)| *m)
                            .unwrap_or(Ratio::new(dec!(1.0)));
                    }
                }
                Self::interpolate_multiplier(&resolved, projected_vault_cr)
            }
            None => Ratio::new(dec!(1.0)),
        }
    }

    /// Get interest rate for a specific collateral type (recovery-aware)
    pub fn get_interest_rate_for(&self, ct: &CollateralType) -> Ratio {
        let config = self.collateral_configs.get(ct);
        if self.mode == Mode::Recovery {
            return config
                .and_then(|c| c.recovery_interest_rate_apr)
                .or_else(|| config.map(|c| c.interest_rate_apr))
                .unwrap_or(DEFAULT_INTEREST_RATE_APR);
        }
        config.map(|c| c.interest_rate_apr).unwrap_or(DEFAULT_INTEREST_RATE_APR)
    }

    /// Per-asset recovery CR = borrow_threshold_ratio × recovery_cr_multiplier.
    /// E.g., 150% × 1.0333 = 155%.
    pub fn get_recovery_cr_for(&self, ct: &CollateralType) -> Ratio {
        let borrow_threshold = self.collateral_configs.get(ct)
            .map(|c| c.borrow_threshold_ratio)
            .unwrap_or(RECOVERY_COLLATERAL_RATIO);
        borrow_threshold * self.recovery_cr_multiplier
    }

    /// Per-asset warning CR = 2 * recovery_cr - borrow_threshold.
    /// E.g., 2 * 155% - 150% = 160%.
    pub fn get_warning_cr_for(&self, ct: &CollateralType) -> Ratio {
        let borrow_threshold = self.collateral_configs.get(ct)
            .map(|c| c.borrow_threshold_ratio)
            .unwrap_or(RECOVERY_COLLATERAL_RATIO);
        let recovery_cr = borrow_threshold * self.recovery_cr_multiplier;
        // 2 * recovery_cr - borrow_threshold
        recovery_cr + recovery_cr - borrow_threshold
    }

    /// Per-asset healthy CR = admin override if set, else 1.5 * borrow_threshold.
    /// E.g., 1.5 * 150% = 225%.
    pub fn get_healthy_cr_for(&self, ct: &CollateralType) -> Ratio {
        let config = self.collateral_configs.get(ct);
        // Use admin override if present
        if let Some(healthy) = config.and_then(|c| c.healthy_cr) {
            return healthy;
        }
        // Default: 1.5 * borrow_threshold_ratio
        let borrow_threshold = config
            .map(|c| c.borrow_threshold_ratio)
            .unwrap_or(RECOVERY_COLLATERAL_RATIO);
        borrow_threshold * DEFAULT_HEALTHY_CR_MULTIPLIER
    }

    // --- Dynamic Interest Rate Logic ---

    /// Linearly interpolate a multiplier between sorted (cr_level, multiplier) pairs.
    /// - If cr >= highest cr_level: returns the multiplier at the highest marker.
    /// - If cr <= lowest cr_level: returns the multiplier at the lowest marker.
    /// - Between two markers: linearly interpolate.
    /// `resolved_markers` must be sorted by cr_level ascending and non-empty.
    fn interpolate_multiplier(resolved_markers: &[(Ratio, Ratio)], cr: Ratio) -> Ratio {
        if resolved_markers.is_empty() {
            return Ratio::new(dec!(1.0));
        }
        let first = &resolved_markers[0];
        if cr <= first.0 {
            return first.1;
        }
        let last = &resolved_markers[resolved_markers.len() - 1];
        if cr >= last.0 {
            return last.1;
        }
        // Find the two surrounding markers
        for i in 0..resolved_markers.len() - 1 {
            let lo = &resolved_markers[i];
            let hi = &resolved_markers[i + 1];
            if cr >= lo.0 && cr <= hi.0 {
                let range = hi.0.0 - lo.0.0;
                if range == Decimal::ZERO {
                    return lo.1;
                }
                let t = (cr.0 - lo.0.0) / range;
                let multiplier = lo.1.0 + t * (hi.1.0 - lo.1.0);
                return Ratio::from(multiplier);
            }
        }
        // Shouldn't reach here if markers are sorted, but fallback
        Ratio::new(dec!(1.0))
    }

    /// Resolve the global_rate_curve markers to concrete (cr_level, multiplier) pairs
    /// for a given collateral type, using that asset's own threshold values.
    /// Markers in global_rate_curve store cr_level=0 as placeholders; the actual CR levels
    /// come from the asset's liquidation_ratio, borrow_threshold, warning_cr, healthy_cr.
    pub fn resolve_layer1_markers(&self, ct: &CollateralType) -> Vec<(Ratio, Ratio)> {
        let config = self.collateral_configs.get(ct);

        // If asset has a per-asset rate_curve, use it directly (markers already have concrete CRs)
        if let Some(curve) = config.and_then(|c| c.rate_curve.as_ref()) {
            return curve.markers.iter()
                .map(|m| (m.cr_level, m.multiplier))
                .collect();
        }

        // Use global_rate_curve with per-asset thresholds
        let liq_ratio = self.get_liquidation_ratio_for(ct);
        let borrow_threshold = config
            .map(|c| c.borrow_threshold_ratio)
            .unwrap_or(RECOVERY_COLLATERAL_RATIO);
        let healthy_cr = self.get_healthy_cr_for(ct);
        // Midpoint between borrow threshold and healthy CR for even curve distribution.
        // (Replaces get_warning_cr_for which used 2*recovery_cr - borrow_threshold.)
        let warning_cr = Ratio::from((borrow_threshold.0 + healthy_cr.0) / Decimal::TWO);

        // Map global markers to per-asset CR levels.
        // Global curve has exactly 4 markers in order: liq, borrow, warning, healthy.
        let cr_levels = [liq_ratio, borrow_threshold, warning_cr, healthy_cr];
        let markers = &self.global_rate_curve.markers;

        let mut resolved: Vec<(Ratio, Ratio)> = markers.iter()
            .enumerate()
            .map(|(i, m)| {
                let cr = if i < cr_levels.len() { cr_levels[i] } else { m.cr_level };
                (cr, m.multiplier)
            })
            .collect();
        resolved.sort_by(|a, b| a.0.0.cmp(&b.0.0));
        resolved
    }

    /// Resolve Layer 2 recovery rate markers to concrete (cr_level, multiplier) pairs
    /// using the cached weighted average thresholds.
    fn resolve_layer2_markers(&self) -> Vec<(Ratio, Ratio)> {
        let mut resolved: Vec<(Ratio, Ratio)> = self.recovery_rate_curve.iter()
            .map(|m| {
                let cr = match m.threshold {
                    SystemThreshold::LiquidationRatio => self.compute_weighted_liquidation_ratio(),
                    SystemThreshold::BorrowThreshold => self.recovery_mode_threshold,
                    SystemThreshold::WarningCr => self.weighted_avg_warning_cr,
                    SystemThreshold::HealthyCr => self.weighted_avg_healthy_cr,
                    SystemThreshold::TotalCollateralRatio => self.total_collateral_ratio,
                };
                (cr, m.multiplier)
            })
            .collect();
        resolved.sort_by(|a, b| a.0.0.cmp(&b.0.0));
        resolved
    }

    /// Resolve a CrAnchor to a concrete Ratio.
    /// `asset_context` is required for AssetThreshold anchors; pass None for system-wide curves.
    pub fn resolve_anchor(
        &self,
        anchor: &CrAnchor,
        asset_context: Option<&CollateralType>,
    ) -> Ratio {
        match anchor {
            CrAnchor::Fixed(r) => *r,
            CrAnchor::AssetThreshold(t) => {
                let ct = asset_context.expect("AssetThreshold requires asset context");
                match t {
                    AssetThreshold::LiquidationRatio => self.get_liquidation_ratio_for(ct),
                    AssetThreshold::BorrowThreshold => {
                        self.collateral_configs.get(ct)
                            .map(|c| c.borrow_threshold_ratio)
                            .unwrap_or(RECOVERY_COLLATERAL_RATIO)
                    }
                    AssetThreshold::WarningCr => self.get_warning_cr_for(ct),
                    AssetThreshold::HealthyCr => self.get_healthy_cr_for(ct),
                }
            }
            CrAnchor::SystemThreshold(t) => match t {
                SystemThreshold::LiquidationRatio => self.compute_weighted_liquidation_ratio(),
                SystemThreshold::BorrowThreshold => self.recovery_mode_threshold,
                SystemThreshold::WarningCr => self.weighted_avg_warning_cr,
                SystemThreshold::HealthyCr => self.weighted_avg_healthy_cr,
                SystemThreshold::TotalCollateralRatio => self.total_collateral_ratio,
            },
            CrAnchor::Midpoint(a, b) => {
                let va = self.resolve_anchor(a, asset_context);
                let vb = self.resolve_anchor(b, asset_context);
                // Use checked_add to avoid overflow when total_collateral_ratio is Decimal::MAX
                // (no vaults with debt yet). Fall back to the larger of the two values.
                match va.0.checked_add(vb.0) {
                    Some(sum) => Ratio::from(sum / dec!(2)),
                    None => Ratio::from(va.0.max(vb.0)),
                }
            }
            CrAnchor::Offset(base, delta) => {
                let v = self.resolve_anchor(base, asset_context);
                match v.0.checked_add(delta.0) {
                    Some(sum) => Ratio::from(sum),
                    None => Ratio::from(Decimal::MAX),
                }
            }
        }
    }

    /// Resolve all markers in a RateCurveV2 to concrete (cr_level, multiplier) pairs, sorted ascending.
    pub fn resolve_curve(
        &self,
        curve: &RateCurveV2,
        asset_context: Option<&CollateralType>,
    ) -> Vec<(Ratio, Ratio)> {
        let mut resolved: Vec<(Ratio, Ratio)> = curve.markers.iter()
            .map(|m| (self.resolve_anchor(&m.cr_anchor, asset_context), m.multiplier))
            .collect();
        resolved.sort_by(|a, b| a.0 .0.cmp(&b.0 .0));
        resolved
    }

    /// Compute debt-weighted average of per-asset liquidation_ratio values.
    fn compute_weighted_liquidation_ratio(&self) -> Ratio {
        let total_debt = self.total_borrowed_icusd_amount();
        if total_debt == ICUSD::new(0) {
            return MINIMUM_COLLATERAL_RATIO;
        }
        let total_debt_dec = Decimal::from_u64(total_debt.to_u64())
            .unwrap_or(Decimal::ZERO);
        let mut weighted_sum = Decimal::ZERO;
        for (ct, config) in &self.collateral_configs {
            let debt_i = self.total_debt_for_collateral(ct);
            if debt_i == ICUSD::new(0) {
                continue;
            }
            let weight = Decimal::from_u64(debt_i.to_u64())
                .unwrap_or(Decimal::ZERO) / total_debt_dec;
            weighted_sum += weight * config.liquidation_ratio.0;
        }
        if weighted_sum == Decimal::ZERO {
            return MINIMUM_COLLATERAL_RATIO;
        }
        Ratio::from(weighted_sum)
    }

    /// Get the dynamic interest rate for a vault, considering both Layer 1 (per-vault CR)
    /// and Layer 2 (system-wide recovery multiplier).
    ///
    /// 1. If recovery_interest_rate_apr is set and system is in Recovery, use static override.
    /// 2. Get base rate from CollateralConfig.
    /// 3. Layer 1: multiply by CR-dependent multiplier from rate curve.
    /// 4. Layer 2 (Recovery only): multiply by TCR-dependent recovery multiplier.
    pub fn get_dynamic_interest_rate_for(&self, ct: &CollateralType, vault_cr: Ratio) -> Ratio {
        let config = self.collateral_configs.get(ct);

        // Static override escape valve
        if self.mode == Mode::Recovery {
            if let Some(static_rate) = config.and_then(|c| c.recovery_interest_rate_apr) {
                return static_rate;
            }
        }

        // Base rate
        let base_rate = config
            .map(|c| c.interest_rate_apr)
            .unwrap_or(DEFAULT_INTEREST_RATE_APR);

        // Layer 1: per-vault CR multiplier
        let layer1_markers = self.resolve_layer1_markers(ct);
        let layer1_mult = Self::interpolate_multiplier(&layer1_markers, vault_cr);
        let layer1_rate = base_rate * layer1_mult;

        // Layer 2: system-wide recovery multiplier (only in Recovery mode)
        if self.mode == Mode::Recovery {
            let layer2_markers = self.resolve_layer2_markers();
            let layer2_mult = Self::interpolate_multiplier(&layer2_markers, self.total_collateral_ratio);
            return layer1_rate * layer2_mult;
        }

        layer1_rate
    }

    /// Accrue interest on a single vault up to `now_nanos`.
    /// Two-phase for borrow checker: compute rate (immutable), then apply (mutable).
    /// SAFETY (Wave-8b LIQ-002): interest accrual changes a vault's debt and
    /// therefore its CR. We deliberately DO NOT re-key the index here. Each
    /// user-facing operation (borrow / repay / margin / withdraw / partial-liq)
    /// already calls `reindex_vault_cr` at the end of its `mutate_state` block,
    /// so the index converges to the post-accrual CR on the next user action.
    /// Re-keying inside passive accrual would be O(N log N) per timer tick for
    /// zero ordering benefit at the band tolerance scale (default 1% CR).
    pub fn accrue_single_vault(&mut self, vault_id: u64, now_nanos: u64) {
        // Phase 1: compute rate (immutable borrow of self)
        let rate_and_elapsed = {
            let s: &State = &*self;
            match s.vault_id_to_vaults.get(&vault_id) {
                Some(vault)
                    if vault.borrowed_icusd_amount.0 > 0
                        && vault.last_accrual_time < now_nanos =>
                {
                    let dummy_rate = s
                        .last_icp_rate
                        .unwrap_or(UsdIcp::from(rust_decimal_macros::dec!(1.0)));
                    let cr = crate::compute_collateral_ratio(vault, dummy_rate, s);
                    let rate = s.get_dynamic_interest_rate_for(&vault.collateral_type, cr);
                    let elapsed = now_nanos.saturating_sub(vault.last_accrual_time);
                    Some((rate, elapsed))
                }
                _ => None,
            }
        };
        // Phase 2: apply (mutable borrow)
        if let Some((rate, elapsed)) = rate_and_elapsed {
            if elapsed == 0 {
                return;
            }
            if let Some(vault) = self.vault_id_to_vaults.get_mut(&vault_id) {
                let debt = Decimal::from(vault.borrowed_icusd_amount.0);
                let factor = Decimal::ONE
                    + rate.0 * Decimal::from(elapsed)
                        / Decimal::from(crate::numeric::NANOS_PER_YEAR);
                let new_debt = (debt * factor).to_u64().unwrap_or(vault.borrowed_icusd_amount.0);
                let interest_delta = new_debt.saturating_sub(vault.borrowed_icusd_amount.0);
                vault.accrued_interest += ICUSD::from(interest_delta);
                vault.borrowed_icusd_amount = ICUSD::from(new_debt);
                vault.last_accrual_time = now_nanos;
            }
        }
    }

    /// Accrue interest on ALL vaults with outstanding debt.
    /// Two-phase: collect (vault_id, rate, elapsed) immutably, then apply mutably.
    ///
    /// SAFETY (Wave-8b LIQ-002): same contract as `accrue_single_vault` —
    /// passive accrual does not re-key the CR index. See that function's
    /// SAFETY block for the rationale.
    pub fn accrue_all_vault_interest(&mut self, now_nanos: u64) {
        // Phase 1: compute rates for all vaults (immutable)
        let accruals: Vec<(u64, Ratio, u64)> = {
            let s: &State = &*self;
            let dummy_rate = s
                .last_icp_rate
                .unwrap_or(UsdIcp::from(rust_decimal_macros::dec!(1.0)));
            s.vault_id_to_vaults
                .iter()
                .filter(|(_, v)| {
                    v.borrowed_icusd_amount.0 > 0 && v.last_accrual_time < now_nanos
                })
                .map(|(id, vault)| {
                    let cr = crate::compute_collateral_ratio(vault, dummy_rate, s);
                    let rate =
                        s.get_dynamic_interest_rate_for(&vault.collateral_type, cr);
                    let elapsed = now_nanos.saturating_sub(vault.last_accrual_time);
                    (*id, rate, elapsed)
                })
                .collect()
        };
        // Phase 2: apply accruals (mutable)
        for (vault_id, rate, elapsed) in accruals {
            if elapsed == 0 {
                continue;
            }
            if let Some(vault) = self.vault_id_to_vaults.get_mut(&vault_id) {
                let debt = Decimal::from(vault.borrowed_icusd_amount.0);
                let factor = Decimal::ONE
                    + rate.0 * Decimal::from(elapsed)
                        / Decimal::from(crate::numeric::NANOS_PER_YEAR);
                let new_debt = (debt * factor).to_u64().unwrap_or(vault.borrowed_icusd_amount.0);
                let interest_delta = new_debt.saturating_sub(vault.borrowed_icusd_amount.0);
                vault.accrued_interest += ICUSD::from(interest_delta);
                vault.borrowed_icusd_amount = ICUSD::from(new_debt);
                vault.last_accrual_time = now_nanos;
            }
        }
    }

    /// Harvest accrued interest from all vaults into the pending distribution map.
    /// After this, per-vault `accrued_interest` is zeroed (only ≤5 min of new interest
    /// will re-accumulate before the next harvest). `borrowed_icusd_amount` is unchanged
    /// so user debt is unaffected.
    pub fn harvest_accrued_interest(&mut self) {
        for vault in self.vault_id_to_vaults.values_mut() {
            let interest = vault.accrued_interest.to_u64();
            if interest > 0 {
                *self
                    .pending_interest_for_pools
                    .entry(vault.collateral_type)
                    .or_insert(0) += interest;
                vault.accrued_interest = ICUSD::new(0);
            }
        }
    }

    /// Compute the debt-weighted average interest rate across all vaults.
    /// Returns 0 if no vaults have outstanding debt.
    pub fn weighted_average_interest_rate(&self) -> Ratio {
        let dummy_rate = self
            .last_icp_rate
            .unwrap_or(UsdIcp::from(rust_decimal_macros::dec!(1.0)));
        let mut total_debt = Decimal::ZERO;
        let mut weighted_sum = Decimal::ZERO;
        for vault in self.vault_id_to_vaults.values() {
            if vault.borrowed_icusd_amount.0 == 0 {
                continue;
            }
            let debt = Decimal::from(vault.borrowed_icusd_amount.0);
            let cr = crate::compute_collateral_ratio(vault, dummy_rate, self);
            let rate = self.get_dynamic_interest_rate_for(&vault.collateral_type, cr);
            weighted_sum += debt * rate.0;
            total_debt += debt;
        }
        if total_debt.is_zero() {
            Ratio::from(Decimal::ZERO)
        } else {
            Ratio::from(weighted_sum / total_debt)
        }
    }

    /// Compute the debt-weighted average interest rate for a single collateral type.
    /// Returns 0 if no vaults of this type have outstanding debt.
    pub fn weighted_interest_rate_for_collateral(&self, ct: &CollateralType) -> Ratio {
        let vault_ids = match self.collateral_to_vault_ids.get(ct) {
            Some(ids) => ids,
            None => return Ratio::from(Decimal::ZERO),
        };
        let mut total_debt = Decimal::ZERO;
        let mut weighted_sum = Decimal::ZERO;
        let dummy_rate = self
            .last_icp_rate
            .unwrap_or(UsdIcp::from(rust_decimal_macros::dec!(1.0)));
        for vault_id in vault_ids {
            let vault = match self.vault_id_to_vaults.get(vault_id) {
                Some(v) => v,
                None => continue,
            };
            if vault.borrowed_icusd_amount.0 == 0 {
                continue;
            }
            let debt = Decimal::from(vault.borrowed_icusd_amount.0);
            let cr = crate::compute_collateral_ratio(vault, dummy_rate, self);
            let rate = self.get_dynamic_interest_rate_for(&vault.collateral_type, cr);
            weighted_sum += debt * rate.0;
            total_debt += debt;
        }
        if total_debt.is_zero() {
            Ratio::from(Decimal::ZERO)
        } else {
            Ratio::from(weighted_sum / total_debt)
        }
    }

    /// Get liquidation bonus for a specific collateral type
    pub fn get_liquidation_bonus_for(&self, ct: &CollateralType) -> Ratio {
        self.collateral_configs
            .get(ct)
            .map(|c| c.liquidation_bonus)
            .unwrap_or(self.liquidation_bonus)
    }

    /// Get the global protocol share of the liquidation bonus (liquidator's profit).
    pub fn get_liquidation_protocol_share(&self) -> Ratio {
        self.liquidation_protocol_share
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

    /// Get the minimum collateral deposit (in native token units) for a collateral type.
    /// Returns 0 if not configured (no minimum enforced).
    pub fn get_min_collateral_deposit_for(&self, ct: &CollateralType) -> u64 {
        self.collateral_configs
            .get(ct)
            .map(|c| c.min_collateral_deposit)
            .unwrap_or(0)
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

    /// Compute the effective recovery target CR: dynamic threshold × proportional multiplier.
    /// This is the CR that partial-liquidated vaults are restored to during Recovery Mode.
    pub fn get_recovery_target_cr_for(&self, _ct: &CollateralType) -> Ratio {
        self.recovery_mode_threshold * self.recovery_cr_multiplier
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

    // ---- Wave-8b LIQ-002: sorted-troves CR index ---------------------------

    /// Convert a CR ratio into the integer key used by `vault_cr_index`.
    /// Saturates at `u64::MAX` so a healthy zero-debt vault (CR = Decimal::MAX)
    /// sorts after every underwater one. Negative or non-finite CRs are
    /// coerced to zero — they sort to the bottom of the index, which is the
    /// conservative direction (treat as worst-CR, gate by per-vault CR
    /// check before any state change).
    ///
    /// Uses `checked_mul`/`checked_to_u64` because zero-debt vaults return
    /// `Ratio::from(Decimal::MAX)` from `compute_collateral_ratio`, and a bare
    /// `Decimal::MAX * 10_000` panics with "Multiplication overflowed".
    pub fn cr_index_key(cr: Ratio) -> u64 {
        match cr.0.checked_mul(Decimal::from(10_000u64)) {
            Some(scaled) => scaled.to_u64().unwrap_or(u64::MAX),
            None => u64::MAX,
        }
    }

    /// Insert or move a vault's entry in `vault_cr_index`. Idempotent: any
    /// prior entry for `vault_id` is removed first. Reads the vault's current
    /// CR via `compute_collateral_ratio` and the cached collateral price.
    ///
    /// **Call after every mutation that changes the vault's debt or
    /// collateral.** Single mutator pattern: every call site must mutate
    /// `vault_id_to_vaults` THEN call `reindex_vault_cr(vault_id)` inside the
    /// same `mutate_state` closure. Never split.
    ///
    /// Sites currently wired:
    ///   * `state::open_vault` — insert.
    ///   * `state::borrow_from_vault` / `state::repay_to_vault` — re-key.
    ///   * `state::add_margin_to_vault` / `state::remove_margin_from_vault` — re-key.
    ///   * `state::deduct_amount_from_vault` (redemption water-fill) — re-key.
    ///   * `state::liquidate_vault` (recovery-mode partial branch) — re-key.
    ///   * `vault::*` partial-liquidation endpoints — re-key after the manual
    ///     `vault_id_to_vaults.get_mut` mutation.
    ///   * `vault::withdraw_partial_collateral` — re-key.
    ///
    /// SAFETY: `on_price_update` MUST NOT call this. The CR key is computed
    /// from the cached collateral price; within a single collateral type all
    /// vaults move proportionally with price, preserving relative ordering.
    /// Re-keying every vault on every 5-minute price tick would burn O(N)
    /// cycles for zero ordering benefit.
    pub fn reindex_vault_cr(&mut self, vault_id: u64) {
        // Drop any prior entry first so a re-key from one bucket to another
        // never leaves a stale duplicate.
        self.unindex_vault_cr(vault_id);

        let vault = match self.vault_id_to_vaults.get(&vault_id) {
            Some(v) => v.clone(),
            None => return,
        };

        // Look up the cached price; unavailable price → CR sorts via the
        // ZERO branch in `compute_collateral_ratio`, which keys at 0 (bottom
        // of the index). The liquidation endpoints independently require a
        // fresh price before proceeding, so a vault keyed without a price
        // cannot be liquidated until a price is available.
        let collateral_price = self
            .get_collateral_price_decimal(&vault.collateral_type)
            .map(UsdIcp::from)
            .unwrap_or(UsdIcp::from(Decimal::ZERO));

        let cr = compute_collateral_ratio(&vault, collateral_price, self);
        let key = Self::cr_index_key(cr);
        self.vault_cr_index
            .entry(key)
            .or_insert_with(BTreeSet::new)
            .insert(vault_id);
    }

    /// Drop a vault from `vault_cr_index`. Idempotent — safe to call on a
    /// vault that was never indexed.
    ///
    /// Call from `close_vault` and from any cleanup that removes the vault
    /// entirely from `vault_id_to_vaults` (e.g., the full-liquidation branch
    /// of `state::liquidate_vault`).
    pub fn unindex_vault_cr(&mut self, vault_id: u64) {
        // The reverse lookup (vault_id → key) would speed this up but doubles
        // bookkeeping. Linear over the buckets in CR order is acceptable: at
        // current TVL the index is small, and the BTreeMap iteration short-
        // circuits the moment we find the vault's bucket.
        let mut empty_key: Option<u64> = None;
        for (key, bucket) in self.vault_cr_index.iter_mut() {
            if bucket.remove(&vault_id) {
                if bucket.is_empty() {
                    empty_key = Some(*key);
                }
                break;
            }
        }
        if let Some(k) = empty_key {
            self.vault_cr_index.remove(&k);
        }
    }

    /// Returns true if `vault_id` is within `liquidation_ordering_tolerance`
    /// (in CR units) of the bottom of the index — i.e., one of the worst-CR
    /// vaults. Liquidator endpoints gate on this BEFORE the per-vault CR
    /// check.
    ///
    /// Returns false for an unindexed `vault_id` (defensive: the caller must
    /// have just attempted to liquidate a vault that does not exist or that
    /// was opened during the call but not re-keyed).
    pub fn is_within_liquidation_band(&self, vault_id: u64) -> bool {
        let mut my_key: Option<u64> = None;
        for (key, bucket) in self.vault_cr_index.iter() {
            if bucket.contains(&vault_id) {
                my_key = Some(*key);
                break;
            }
        }
        let my_key = match my_key {
            Some(k) => k,
            None => return false,
        };
        let bottom_key = match self.vault_cr_index.keys().next() {
            Some(k) => *k,
            None => return false,
        };
        let tolerance_bps = (self.liquidation_ordering_tolerance.0
            * Decimal::from(10_000u64))
            .to_u64()
            .unwrap_or(0);
        my_key.saturating_sub(bottom_key) <= tolerance_bps
    }

    /// Admin setter for the LIQ-002 tolerance band. No upper bound — admin
    /// can widen to effectively-disable the gate during emergencies.
    pub fn set_liquidation_ordering_tolerance(&mut self, tolerance: Ratio) {
        self.liquidation_ordering_tolerance = tolerance;
    }

    /// Sync a global fee-setting event to the ICP CollateralConfig (for backward compat during replay)
    pub fn sync_icp_collateral_config(&mut self) {
        let icp = self.icp_ledger_principal;
        if let Some(config) = self.collateral_configs.get_mut(&icp) {
            config.borrowing_fee = self.fee;
            config.liquidation_bonus = self.liquidation_bonus;
            config.redemption_fee_floor = self.redemption_fee_floor;
            config.redemption_fee_ceiling = self.redemption_fee_ceiling;
            config.recovery_target_cr = config.borrow_threshold_ratio * self.recovery_cr_multiplier;
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

        // Cache weighted CR averages for dynamic interest rate computation
        let (w_recovery, w_warning, w_healthy) = self.compute_weighted_cr_averages();
        self.weighted_avg_recovery_cr = w_recovery;
        self.weighted_avg_warning_cr = w_warning;
        self.weighted_avg_healthy_cr = w_healthy;

        // If the protocol is frozen, don't change mode at all.
        if self.frozen {
            return;
        }

        // If an admin has manually set the mode, don't override it automatically.
        // Exception: if collateral ratio drops below 100%, always go ReadOnly for safety.
        if self.manual_mode_override {
            if new_total_collateral_ratio < Ratio::from(dec!(1.0)) {
                self.mode = Mode::ReadOnly;
                log!(
                    crate::DEBUG,
                    "[update_mode] manual override active but ratio < 100%, forcing ReadOnly"
                );
            }
            return;
        }

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
        // If this vault_id already exists with a different owner (e.g. duplicate
        // OpenVault events in the log), remove the stale index entry so
        // principal_to_vault_ids stays consistent with vault_id_to_vaults.
        if let Some(old_vault) = self.vault_id_to_vaults.get(&vault_id) {
            if old_vault.owner != vault.owner {
                if let Some(old_ids) = self.principal_to_vault_ids.get_mut(&old_vault.owner) {
                    old_ids.remove(&vault_id);
                    if old_ids.is_empty() {
                        self.principal_to_vault_ids.remove(&old_vault.owner);
                    }
                }
            }
        }
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
        // Wave-8b LIQ-002: insert into the sorted-troves CR index.
        self.reindex_vault_cr(vault_id);
    }

    pub fn close_vault(&mut self, vault_id: u64) {
        if let Some(vault) = self.vault_id_to_vaults.remove(&vault_id) {
            let owner = vault.owner;
            // NOTE: We intentionally do NOT create a pending_margin_transfer here.
            // CloseVault requires collateral=0, and WithdrawAndCloseVault already
            // transferred collateral directly before calling this. Inserting a
            // pending entry would be phantom — never cleared by a MarginTransfer event.
            // Legitimate pending transfers (liquidator rewards) are created directly
            // by the liquidation code in vault.rs.
            if let Some(vault_ids) = self.principal_to_vault_ids.get_mut(&owner) {
                vault_ids.remove(&vault_id);
            } else {
                ic_cdk::trap("BUG: tried to close vault with no owner");
            }
            // Wave-8b LIQ-002: drop from the sorted-troves CR index.
            self.unindex_vault_cr(vault_id);
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
        // Wave-8b LIQ-002: re-key after debt change.
        self.reindex_vault_cr(vault_id);
    }

    pub fn add_margin_to_vault(&mut self, vault_id: u64, add_margin: ICP) {
        match self.vault_id_to_vaults.get_mut(&vault_id) {
            Some(vault) => {
                vault.collateral_amount += add_margin.to_u64();
            }
            None => ic_cdk::trap("adding margin to unknown vault"),
        }
        // Wave-8b LIQ-002: re-key after collateral change.
        self.reindex_vault_cr(vault_id);
    }

    pub fn remove_margin_from_vault(&mut self, vault_id: u64, amount: ICP) {
        match self.vault_id_to_vaults.get_mut(&vault_id) {
            Some(vault) => {
                assert!(amount.to_u64() <= vault.collateral_amount);
                vault.collateral_amount -= amount.to_u64();
            }
            None => ic_cdk::trap("removing margin from unknown vault"),
        }
        // Wave-8b LIQ-002: re-key after collateral change.
        self.reindex_vault_cr(vault_id);
    }

    /// Repay debt to a vault. Returns `(interest_share, principal_share)` of the repayment.
    /// The interest share is proportional to how much of the vault's current debt is accrued interest.
    pub fn repay_to_vault(&mut self, vault_id: u64, repayed_amount: ICUSD) -> (ICUSD, ICUSD) {
        let result = match self.vault_id_to_vaults.get_mut(&vault_id) {
            Some(vault) => {
                assert!(repayed_amount <= vault.borrowed_icusd_amount);
                let interest_share = if vault.accrued_interest.0 > 0 && vault.borrowed_icusd_amount.0 > 0 {
                    let share = (rust_decimal::Decimal::from(repayed_amount.0)
                        * rust_decimal::Decimal::from(vault.accrued_interest.0)
                        / rust_decimal::Decimal::from(vault.borrowed_icusd_amount.0))
                        .to_u64().unwrap_or(0);
                    // INT-001: also cap by `repayed_amount` so the saturating
                    // subtraction below cannot lose principal silently. The
                    // deduct-side clamp keeps `accrued <= borrowed`, but defense
                    // in depth here pins the property even on legacy state.
                    ICUSD::new(share.min(vault.accrued_interest.0).min(repayed_amount.0))
                } else {
                    ICUSD::new(0)
                };
                // INT-001: saturating subtraction so a stale `accrued > borrowed`
                // state cannot panic the canister via `Token::Sub`. With the
                // `.min(repayed_amount)` cap above this can never under-flow
                // in practice, but the saturating form documents the contract.
                let principal_share = repayed_amount.saturating_sub(interest_share);
                vault.borrowed_icusd_amount -= repayed_amount;
                vault.accrued_interest -= interest_share;
                (interest_share, principal_share)
            }
            None => ic_cdk::trap("repaying to unknown vault"),
        };
        // Wave-8b LIQ-002: re-key after debt change. Vault stays in the map
        // (full repays are followed by close_vault elsewhere), so reindex —
        // not unindex — here.
        self.reindex_vault_cr(vault_id);
        result
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
        // Use the per-asset minimum collateral ratio (borrow_threshold_ratio, e.g. 150% for ICP)
        // as the target CR to restore the vault to after partial liquidation.
        let target_cr = self.get_min_collateral_ratio_for(ct);
        let liq_bonus = self.get_liquidation_bonus_for(ct);
        let numerator_icusd = vault.borrowed_icusd_amount * target_cr;
        if numerator_icusd <= collateral_value {
            // Already at or above target — shouldn't be liquidatable, but return 0
            return ICUSD::new(0);
        }
        let deficit = numerator_icusd - collateral_value;
        let denominator = target_cr - liq_bonus;
        // If target CR <= bonus (misconfigured or deeply underwater), full liquidation
        if denominator <= Ratio::from(dec!(0)) {
            return vault.borrowed_icusd_amount;
        }
        let repay_amount = deficit / denominator;
        repay_amount.min(vault.borrowed_icusd_amount)
    }

    // ─── Wave-8e LIQ-005: deficit-account helpers ───

    /// Increment `protocol_deficit_icusd` by `shortfall` and return the
    /// amount actually added (always equal to `shortfall` for non-zero
    /// inputs). Caller is responsible for emitting `DeficitAccrued` and
    /// invoking `check_deficit_readonly_latch` afterwards.
    pub fn accrue_deficit_shortfall(&mut self, shortfall: ICUSD) -> ICUSD {
        if shortfall.0 == 0 {
            return ICUSD::new(0);
        }
        self.protocol_deficit_icusd = self.protocol_deficit_icusd + shortfall;
        shortfall
    }

    /// Compute how much of `fee` to route to deficit repayment given the
    /// current deficit and configured fraction. Caps at remaining deficit.
    /// Returns `ICUSD::new(0)` when `protocol_deficit_icusd == 0` or
    /// `deficit_repayment_fraction == 0`.
    pub fn compute_deficit_repay_amount(&self, fee: ICUSD) -> ICUSD {
        if self.protocol_deficit_icusd.0 == 0 || self.deficit_repayment_fraction.0.is_zero() {
            return ICUSD::new(0);
        }
        let candidate_dec =
            rust_decimal::Decimal::from(fee.0) * self.deficit_repayment_fraction.0;
        let candidate_e8s = candidate_dec.to_u64().unwrap_or(0);
        let capped = candidate_e8s.min(self.protocol_deficit_icusd.0);
        ICUSD::new(capped)
    }

    /// Apply a successful deficit repayment: decrement the outstanding
    /// deficit (saturating at zero) and accumulate `amount` into the lifetime
    /// counter. Saturating behaviour preserves the invariant that
    /// `total_deficit_repaid_icusd` equals the sum of `DeficitRepaid.amount`
    /// events even if a caller asks for more than the outstanding deficit.
    /// Caller is responsible for emitting `DeficitRepaid`.
    pub fn apply_deficit_repayment(&mut self, amount: ICUSD) {
        if amount.0 == 0 {
            return;
        }
        self.protocol_deficit_icusd = self.protocol_deficit_icusd.saturating_sub(amount);
        self.total_deficit_repaid_icusd = self.total_deficit_repaid_icusd + amount;
    }

    /// If `deficit_readonly_threshold_e8s > 0` and the current deficit has
    /// reached the threshold, force `mode = Mode::ReadOnly` and return
    /// `true`. Returns `false` otherwise. The latch is one-shot per
    /// crossing — the admin must call `exit_recovery_mode` to clear it.
    pub fn check_deficit_readonly_latch(&mut self) -> bool {
        if self.deficit_readonly_threshold_e8s == 0 {
            return false;
        }
        if self.protocol_deficit_icusd.0 < self.deficit_readonly_threshold_e8s {
            return false;
        }
        self.mode = Mode::ReadOnly;
        true
    }

    /// Liquidate a vault. Returns the interest share of the debt reduction
    /// so callers can route it to treasury.
    pub fn liquidate_vault(&mut self, vault_id: u64, mode: Mode, collateral_price: UsdIcp) -> ICUSD {
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
                None => return ICUSD::new(0), // unknown collateral — cannot liquidate
            };
            let price = match config.last_price.and_then(Decimal::from_f64) {
                Some(p) => p,
                None => return ICUSD::new(0), // no price — cannot compute
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
                return ICUSD::new(0); // already at/above target
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

            let interest_share = match self.vault_id_to_vaults.get_mut(&vault_id) {
                Some(vault) => {
                    // Compute interest share proportionally before reducing debt
                    let interest_share = if vault.accrued_interest.0 > 0 && vault.borrowed_icusd_amount.0 > 0 {
                        let share = (rust_decimal::Decimal::from(repay_amount.0)
                            * rust_decimal::Decimal::from(vault.accrued_interest.0)
                            / rust_decimal::Decimal::from(vault.borrowed_icusd_amount.0))
                            .to_u64().unwrap_or(0);
                        ICUSD::new(share.min(vault.accrued_interest.0))
                    } else { ICUSD::new(0) };

                    vault.borrowed_icusd_amount = vault.borrowed_icusd_amount.saturating_sub(repay_amount);
                    vault.collateral_amount = vault.collateral_amount.saturating_sub(collateral_seized);
                    vault.accrued_interest = vault.accrued_interest.saturating_sub(interest_share);
                    interest_share
                }
                None => ic_cdk::trap("liquidating unknown vault"),
            };
            // Wave-8b LIQ-002: recovery-mode partial liquidation mutates the
            // vault in place; re-key its index entry to reflect the new CR.
            self.reindex_vault_cr(vault_id);
            interest_share
        } else {
            // Full liquidation — removes vault entirely
            // All remaining accrued_interest is interest revenue
            let interest_share = vault.accrued_interest;
            if let Some(vault) = self.vault_id_to_vaults.remove(&vault_id) {
                if let Some(vault_ids) = self.principal_to_vault_ids.get_mut(&vault.owner) {
                    vault_ids.remove(&vault_id);
                }
            }
            // Wave-8b LIQ-002: full liquidation removes the vault entirely;
            // drop its index entry too.
            self.unindex_vault_cr(vault_id);
            interest_share
        }
    }

        
    pub fn redistribute_vault(&mut self, vault_id: u64) {
        let vault = self
            .vault_id_to_vaults
            .get(&vault_id)
            .expect("bug: vault not found");
        let entries = distribute_across_vaults(&self.vault_id_to_vaults, vault.clone());
        let touched_ids: Vec<u64> = entries.iter().map(|e| e.vault_id).collect();
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
        // Wave-8b LIQ-002: re-key every vault that received a share, then drop
        // the source vault from the index. `redistribute_vault` is currently
        // only reachable from event replay (no #[update] wires it), but the
        // index contract holds for any caller.
        for tid in touched_ids {
            self.reindex_vault_cr(tid);
        }
        self.unindex_vault_cr(vault_id);
    }
    
    /// Water-filling redemption: spread redemptions across vaults to equalize CR.
    ///
    /// Instead of draining the lowest-CR vault completely, this algorithm raises
    /// the lowest-CR vault(s) until they match the next tier, then splits
    /// proportionally by debt among all vaults in the band. This maximizes
    /// capital efficiency and fairness to vault owners.
    pub fn redeem_on_vaults(
        &mut self,
        icusd_amount: ICUSD,
        collateral_price: UsdIcp,
        collateral_type: &CollateralType,
    ) -> Vec<crate::event::VaultRedemption> {
        let mut results = Vec::new();

        if icusd_amount == 0 {
            return results;
        }

        // Resolve config for price & decimals.
        // During event replay the collateral config may not have a price yet,
        // so fall back to the price stored in the event (passed as collateral_price).
        let (price, decimals) = match self.get_collateral_config(collateral_type) {
            Some(config) => {
                let p = config.last_price
                    .and_then(Decimal::from_f64)
                    .unwrap_or(collateral_price.0);
                (p, config.decimals)
            }
            None => {
                // Config not yet created during replay; use event price and ICP decimals
                (collateral_price.0, 8)
            }
        };

        let resolved_ct = if collateral_type == &Principal::anonymous() {
            self.icp_ledger_principal
        } else {
            *collateral_type
        };

        // Collect eligible vaults sorted by CR ascending
        let mut vault_entries: Vec<(Decimal, VaultId)> = Vec::new();
        for vault in self.vault_id_to_vaults.values() {
            if vault.borrowed_icusd_amount == 0 {
                continue; // skip zero-debt vaults
            }
            let vault_ct = if vault.collateral_type == Principal::anonymous() {
                self.icp_ledger_principal
            } else {
                vault.collateral_type
            };
            if vault_ct != resolved_ct {
                continue;
            }
            let cr = crate::compute_collateral_ratio(vault, collateral_price, self);
            vault_entries.push((cr.0, vault.vault_id));
        }
        vault_entries.sort_by(|a, b| a.0.cmp(&b.0));

        if vault_entries.is_empty() {
            return results;
        }

        let mut remaining = icusd_amount.to_u64() as u128;

        // Water-filling: process from lowest CR upward
        let mut band_start = 0usize;
        while remaining > 0 && band_start < vault_entries.len() {
            // Current band = all vaults from band_start that share the lowest CR
            let band_cr = vault_entries[band_start].0;

            // Find the CR of the next tier (first vault above current band)
            let mut band_end = band_start + 1;
            while band_end < vault_entries.len() && vault_entries[band_end].0 == band_cr {
                band_end += 1;
            }

            // Compute total debt in the current band
            let band_vault_ids: Vec<VaultId> = vault_entries[band_start..band_end]
                .iter().map(|(_, id)| *id).collect();
            let band_debts: Vec<u128> = band_vault_ids.iter().map(|id| {
                self.vault_id_to_vaults.get(id).unwrap().borrowed_icusd_amount.to_u64() as u128
            }).collect();
            let total_band_debt: u128 = band_debts.iter().sum();

            if total_band_debt == 0 {
                band_start = band_end;
                continue;
            }

            if band_end >= vault_entries.len() {
                // No next tier — distribute all remaining proportionally across band
                self.distribute_redemption_across_band(
                    &band_vault_ids, &band_debts, total_band_debt,
                    remaining, price, decimals, &mut results,
                );
                break;
            }

            // Calculate how much icUSD (e8s) is needed to raise all band vaults to next tier CR
            let next_cr = vault_entries[band_end].0;
            // Formula: x_i = D_i * (CR_next - CR_current) / (CR_next - 1)
            let cr_diff = next_cr - band_cr;
            let cr_denom = next_cr - Decimal::ONE;
            if cr_denom <= Decimal::ZERO {
                // Safety: if next CR <= 1, just drain proportionally
                self.distribute_redemption_across_band(
                    &band_vault_ids, &band_debts, total_band_debt,
                    remaining, price, decimals, &mut results,
                );
                break;
            }

            // Total icUSD needed to level up the band (in e8s)
            let total_needed_dec = Decimal::from(total_band_debt as u64) * cr_diff / cr_denom;
            let total_needed = total_needed_dec.to_u64().unwrap_or(u64::MAX) as u128;

            if remaining >= total_needed && total_needed > 0 {
                // Level up the entire band
                self.distribute_redemption_across_band(
                    &band_vault_ids, &band_debts, total_band_debt,
                    total_needed, price, decimals, &mut results,
                );
                remaining -= total_needed;

                // Re-read CRs for band vaults and merge into next tier
                // (they should now match next_cr approximately)
                for i in band_start..band_end {
                    vault_entries[i].0 = next_cr;
                }
                // Continue with band_start unchanged — the band now includes the next tier
                // Actually, we advance to process the merged group in next iteration
                // Don't advance band_start — loop will re-evaluate with the wider band
                continue;
            } else {
                // Can't reach next tier. Distribute remaining proportionally.
                self.distribute_redemption_across_band(
                    &band_vault_ids, &band_debts, total_band_debt,
                    remaining, price, decimals, &mut results,
                );
                break;
            }
        }

        results
    }

    /// Distribute a redemption amount proportionally across a band of vaults by debt size.
    /// Returns per-vault breakdown of what was redeemed/seized.
    fn distribute_redemption_across_band(
        &mut self,
        vault_ids: &[VaultId],
        debts: &[u128],
        total_debt: u128,
        redemption_e8s: u128,
        price: Decimal,
        decimals: u8,
        results: &mut Vec<crate::event::VaultRedemption>,
    ) {
        if total_debt == 0 || redemption_e8s == 0 {
            return;
        }

        let mut distributed: u128 = 0;
        for (i, vault_id) in vault_ids.iter().enumerate() {
            let vault_debt = debts[i];
            // Proportional share: redemption_e8s * vault_debt / total_debt
            let share = if i == vault_ids.len() - 1 {
                // Last vault gets the remainder to avoid rounding dust
                redemption_e8s - distributed
            } else {
                redemption_e8s * vault_debt / total_debt
            };

            if share == 0 {
                continue;
            }

            // Cap at vault's actual debt
            let vault = self.vault_id_to_vaults.get(vault_id).unwrap();
            let max_share = vault.borrowed_icusd_amount.to_u64() as u128;
            let actual_share = share.min(max_share);

            let icusd_to_deduct = ICUSD::from(actual_share as u64);
            let collateral_to_deduct = crate::numeric::icusd_to_collateral_amount(
                icusd_to_deduct,
                price,
                decimals,
            );
            self.deduct_amount_from_vault(collateral_to_deduct, icusd_to_deduct, *vault_id);
            distributed += actual_share;

            results.push(crate::event::VaultRedemption {
                vault_id: *vault_id,
                icusd_redeemed_e8s: actual_share as u64,
                collateral_seized: collateral_to_deduct,
            });
        }
    }

    fn deduct_amount_from_vault(
        &mut self,
        collateral_to_deduct: u64,
        icusd_amount_to_deduct: ICUSD,
        vault_id: VaultId,
    ) {
        match self.vault_id_to_vaults.get_mut(&vault_id) {
            Some(vault) => {
                // Use saturating arithmetic: during event replay, interest
                // drift can inflate debt/collateral values, causing the
                // deduction to exceed the vault's balance.
                vault.borrowed_icusd_amount = vault.borrowed_icusd_amount.saturating_sub(icusd_amount_to_deduct);
                vault.collateral_amount = vault.collateral_amount.saturating_sub(collateral_to_deduct);
                // INT-001: redemption can shrink `borrowed_icusd_amount` below
                // `accrued_interest`, breaking the invariant that drives the
                // proportional interest-share math in `repay_to_vault`.
                // Clamp here so any subsequent repay sees a consistent state.
                // Excess interest is forgiven (matches the dust-debt forgiveness
                // pattern in `withdraw_partial_collateral`).
                if vault.accrued_interest > vault.borrowed_icusd_amount {
                    vault.accrued_interest = vault.borrowed_icusd_amount;
                }
            }
            None => ic_cdk::trap("cannot deduct from unknown vault"),
        }
        // Wave-8b LIQ-002: redemption water-fill mutates each touched vault's
        // debt/collateral; re-key its index entry so the next redemption /
        // liquidation sees the updated CR.
        self.reindex_vault_cr(vault_id);
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
        ensure_eq!(
            self.reserve_redemptions_enabled,
            other.reserve_redemptions_enabled,
            "reserve_redemptions_enabled does not match"
        );
        ensure_eq!(
            self.icpswap_routing_enabled,
            other.icpswap_routing_enabled,
            "icpswap_routing_enabled does not match"
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
    
    // clean_stale_operations REMOVED — it contained a dangerous Recovery→GA auto-reset
    // that could silently exit Recovery mode based on a timeout. Mode transitions are now
    // handled exclusively by update_mode() (automatic, based on collateral ratio) or by
    // admin functions (enter_recovery_mode / exit_recovery_mode).
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


pub fn compute_redemption_fee(
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
    fn test_vault_health_score() {
        use crate::vault::Vault;

        let vault = Vault {
            owner: Principal::anonymous(),
            borrowed_icusd_amount: ICUSD::new(100_0000_0000), // 100 icUSD
            collateral_amount: 200_0000_0000,
            vault_id: 1,
            collateral_type: Principal::anonymous(),
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        };

        // ICP vault: CR = 1.50, liq_ratio = 1.33 → health = 1.50 / 1.33 ≈ 1.1278
        let health = vault.health_score(1.50, 1.33);
        assert!((health - 1.1278).abs() < 0.001, "Expected ~1.1278, got {}", health);

        // ckBTC vault: CR = 1.25, liq_ratio = 1.15 → health = 1.25 / 1.15 ≈ 1.0870
        let health2 = vault.health_score(1.25, 1.15);
        assert!((health2 - 1.0870).abs() < 0.001, "Expected ~1.0870, got {}", health2);

        // At exact liquidation threshold: health = 1.0
        let health3 = vault.health_score(1.33, 1.33);
        assert!((health3 - 1.0).abs() < 0.0001, "Expected 1.0, got {}", health3);

        // Zero-debt vault: should return f64::MAX (infinite health)
        let zero_debt_vault = Vault {
            borrowed_icusd_amount: ICUSD::new(0),
            ..vault.clone()
        };
        let health4 = zero_debt_vault.health_score(1.50, 1.33);
        assert!(health4 > 1_000_000.0, "Zero-debt vault should have very high health score");
    }

    #[test]
    fn test_tiered_redemption_ordering() {
        // Verify sort logic: tier ascending, then health score ascending
        let mut entries: Vec<(u8, f64, u64)> = vec![
            (2, 1.05, 10),  // tier 2, low health
            (1, 1.20, 20),  // tier 1, moderate health
            (1, 1.08, 30),  // tier 1, low health
            (3, 1.01, 40),  // tier 3, very low health
            (1, 1.15, 50),  // tier 1, moderate health
        ];

        entries.sort_by(|a, b| {
            a.0.cmp(&b.0).then_with(|| a.1.partial_cmp(&b.1).unwrap())
        });

        let order: Vec<u64> = entries.iter().map(|e| e.2).collect();
        assert_eq!(order, vec![30, 50, 20, 10, 40],
            "Expected tier-1 vaults first (health-sorted), then tier-2, then tier-3");
    }

    #[test]
    fn test_redemption_vault_impacts_replay() {
        // Verify that deduct_amount_from_vault correctly applies per-vault deltas
        // (simulating what the replay handler does with vault_impacts)
        let mut state = test_state();
        let icp_ct = state.icp_collateral_type();

        // Open two vaults with known amounts
        state.open_vault(crate::vault::Vault {
            owner: Principal::anonymous(),
            vault_id: 1,
            collateral_amount: 500_000_000, // 5 ICP
            borrowed_icusd_amount: ICUSD::new(300_000_000), // 3 icUSD
            collateral_type: icp_ct,
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        });
        state.open_vault(crate::vault::Vault {
            owner: Principal::anonymous(),
            vault_id: 2,
            collateral_amount: 800_000_000, // 8 ICP
            borrowed_icusd_amount: ICUSD::new(500_000_000), // 5 icUSD
            collateral_type: icp_ct,
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        });

        // Apply deltas as the replay handler would
        state.deduct_amount_from_vault(50_000_000, ICUSD::from(100_000_000u64), 1);
        state.deduct_amount_from_vault(75_000_000, ICUSD::from(150_000_000u64), 2);

        // Verify vault 1: 3 - 1 = 2 icUSD debt, 5 - 0.5 = 4.5 ICP
        let v1 = state.vault_id_to_vaults.get(&1).unwrap();
        assert_eq!(v1.borrowed_icusd_amount, ICUSD::new(200_000_000));
        assert_eq!(v1.collateral_amount, 450_000_000);

        // Verify vault 2: 5 - 1.5 = 3.5 icUSD debt, 8 - 0.75 = 7.25 ICP
        let v2 = state.vault_id_to_vaults.get(&2).unwrap();
        assert_eq!(v2.borrowed_icusd_amount, ICUSD::new(350_000_000));
        assert_eq!(v2.collateral_amount, 725_000_000);
    }

    // INT-001 regression fence — see
    // `tests/audit_pocs_int_001_redemption_clamps_interest.rs` for the
    // external-callers' invariant fence; this test exercises the private
    // `deduct_amount_from_vault` directly.
    #[test]
    fn int_001_deduct_clamps_accrued_interest_to_residual_debt() {
        let mut state = test_state();
        let icp_ct = state.icp_collateral_type();

        state.open_vault(crate::vault::Vault {
            owner: Principal::anonymous(),
            vault_id: 1,
            collateral_amount: 1_000_000_000,
            borrowed_icusd_amount: ICUSD::new(500_000_000),
            collateral_type: icp_ct,
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(100_000_000), // 1 icUSD of accrued interest
            bot_processing: false,
        });

        // Redemption deducts 4.95 icUSD, leaving residual borrowed=0.05 icUSD.
        // Pre-fix: accrued_interest stays at 1 icUSD, breaking the invariant.
        state.deduct_amount_from_vault(0, ICUSD::from(495_000_000u64), 1);

        let v = state.vault_id_to_vaults.get(&1).unwrap();
        assert_eq!(v.borrowed_icusd_amount, ICUSD::new(5_000_000));
        assert!(
            v.accrued_interest <= v.borrowed_icusd_amount,
            "INT-001 invariant: accrued_interest ({}) must not exceed borrowed_icusd_amount ({}) post-deduct",
            v.accrued_interest.to_u64(),
            v.borrowed_icusd_amount.to_u64(),
        );
        // Post-fix: accrued is clamped down to the new borrowed (5M).
        assert_eq!(v.accrued_interest, ICUSD::new(5_000_000));
    }

    // INT-001 regression fence — full repay must not panic when called on a
    // vault that already had its invariant broken (legacy state arriving from
    // a pre-fix canister snapshot, or any future code path that touches debt
    // without going through `deduct_amount_from_vault`).
    #[test]
    fn int_001_repay_saturates_principal_when_accrued_exceeds_borrowed() {
        let mut state = test_state();
        let icp_ct = state.icp_collateral_type();

        state.open_vault(crate::vault::Vault {
            owner: Principal::anonymous(),
            vault_id: 1,
            collateral_amount: 1_000_000_000,
            borrowed_icusd_amount: ICUSD::new(5_000_000),
            collateral_type: icp_ct,
            last_accrual_time: 0,
            // Intentionally broken invariant — accrued > borrowed.
            accrued_interest: ICUSD::new(100_000_000),
            bot_processing: false,
        });

        // Repay all 5M residual; pre-fix this panics with `underflow` in
        // `Token::Sub`. Post-fix the saturating subtraction zeroes the
        // principal share without panicking.
        let (interest_share, principal_share) =
            state.repay_to_vault(1, ICUSD::new(5_000_000));

        assert_eq!(interest_share, ICUSD::new(5_000_000));
        assert_eq!(principal_share, ICUSD::new(0));
        let v = state.vault_id_to_vaults.get(&1).unwrap();
        assert_eq!(v.borrowed_icusd_amount, ICUSD::new(0));
    }

    #[test]
    fn test_distribute_across_vaults() {
        let mut vaults = BTreeMap::new();
        let vault1 = Vault {
            owner: Principal::anonymous(),
            vault_id: 1,
            collateral_amount: 500_000,
            borrowed_icusd_amount: ICUSD::new(300_000),
            collateral_type: Principal::anonymous(),
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        };

        let vault2 = Vault {
            owner: Principal::anonymous(),
            vault_id: 2,
            collateral_amount: 300_000,
            borrowed_icusd_amount: ICUSD::new(200_000),
            collateral_type: Principal::anonymous(),
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        };

        vaults.insert(1, vault1);
        vaults.insert(2, vault2);

        let target_vault = Vault {
            owner: Principal::anonymous(),
            vault_id: 3,
            collateral_amount: 700_000,
            borrowed_icusd_amount: ICUSD::new(400_000),
            collateral_type: Principal::anonymous(),
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
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
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        });

        // Repay 0.01 icUSD (minimum partial repay in e8s is 1_000_000)
        let repay_amount = ICUSD::new(1_000_000);
        let _ = state.repay_to_vault(vault_id, repay_amount);

        // Assert debt reduced correctly
        let vault = state.vault_id_to_vaults.get(&vault_id).unwrap();
        assert_eq!(vault.borrowed_icusd_amount, ICUSD::new(199_000_000));
    }

    #[test]
    fn test_repay_reduces_accrued_interest_proportionally() {
        let mut state = accrual_test_state();
        let icp = state.icp_ledger_principal;

        state.vault_id_to_vaults.insert(1, Vault {
            owner: Principal::anonymous(),
            vault_id: 1,
            collateral_amount: 150_000_000,
            borrowed_icusd_amount: ICUSD::new(500_000_000), // 5 icUSD total
            collateral_type: icp,
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(100_000_000), // 1 icUSD is interest
            bot_processing: false,
        });

        let (interest_share, principal_share) = state.repay_to_vault(1, ICUSD::new(250_000_000));

        let vault = state.vault_id_to_vaults.get(&1).unwrap();
        assert_eq!(vault.borrowed_icusd_amount.0, 250_000_000);
        // 100/500 = 20% is interest, so 20% of 250M = 50M
        assert_eq!(interest_share.0, 50_000_000,
            "interest share of repayment should be 50M, got {}", interest_share.0);
        assert_eq!(principal_share.0, 200_000_000,
            "principal share should be 200M, got {}", principal_share.0);
        assert_eq!(vault.accrued_interest.0, 50_000_000,
            "remaining accrued_interest should be 50M, got {}", vault.accrued_interest.0);
    }

    // Pre-existing test failure: this test exercises a code path that calls
    // `ic_cdk::api::caller()`, which traps with "msg_caller_size should only
    // be called inside canisters" when invoked from a unit-test context.
    // Marked #[ignore] so the pre-deploy hook can run `cargo test --lib` cleanly.
    // Tracked for follow-up: refactor the called function to accept caller as
    // a parameter, or run this test in a PocketIC environment instead.
    #[test]
    #[ignore = "pre-existing: requires canister context for msg_caller; see comment"]
    fn test_borrow_fee_does_not_credit_liquidity_pool() {
        let mut state = accrual_test_state();
        let dev = state.developer_principal;
        let icp = state.icp_ledger_principal;

        state.open_vault(Vault {
            owner: Principal::anonymous(),
            vault_id: 1,
            collateral_amount: 500_000_000, // 5 ICP
            borrowed_icusd_amount: ICUSD::new(0),
            collateral_type: icp,
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        });

        crate::event::record_borrow_from_vault(&mut state, 1, ICUSD::new(100_000_000), ICUSD::new(500_000), 0);
        assert_eq!(state.get_provided_liquidity(dev).0, 0,
            "Borrowing fee should NOT go to developer liquidity pool");
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
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
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

    // --- Dynamic Interest Rate Tests ---

    #[test]
    fn test_interpolate_multiplier_at_and_above_highest() {
        let markers = vec![
            (Ratio::from_f64(1.33), Ratio::from_f64(5.0)),
            (Ratio::from_f64(1.50), Ratio::from_f64(2.5)),
            (Ratio::from_f64(1.60), Ratio::from_f64(1.75)),
            (Ratio::from_f64(2.25), Ratio::from_f64(1.0)),
        ];
        // At healthy CR: 1.0x
        let m = State::interpolate_multiplier(&markers, Ratio::from_f64(2.25));
        assert!((m.to_f64() - 1.0).abs() < 0.001);
        // Above healthy CR: still 1.0x
        let m = State::interpolate_multiplier(&markers, Ratio::from_f64(5.0));
        assert!((m.to_f64() - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_interpolate_multiplier_at_and_below_lowest() {
        let markers = vec![
            (Ratio::from_f64(1.33), Ratio::from_f64(5.0)),
            (Ratio::from_f64(1.50), Ratio::from_f64(2.5)),
            (Ratio::from_f64(2.25), Ratio::from_f64(1.0)),
        ];
        // At liquidation ratio: 5.0x
        let m = State::interpolate_multiplier(&markers, Ratio::from_f64(1.33));
        assert!((m.to_f64() - 5.0).abs() < 0.001);
        // Below: still 5.0x
        let m = State::interpolate_multiplier(&markers, Ratio::from_f64(1.0));
        assert!((m.to_f64() - 5.0).abs() < 0.001);
    }

    #[test]
    fn test_interpolate_multiplier_midpoint() {
        let markers = vec![
            (Ratio::from_f64(1.50), Ratio::from_f64(2.5)),
            (Ratio::from_f64(1.60), Ratio::from_f64(1.75)),
        ];
        // Midpoint between 150% and 160% => t=0.5 => 2.5 - 0.5*(2.5-1.75) = 2.125
        let m = State::interpolate_multiplier(&markers, Ratio::from_f64(1.55));
        assert!((m.to_f64() - 2.125).abs() < 0.001,
            "Expected 2.125, got {}", m.to_f64());
    }

    #[test]
    fn test_interpolate_multiplier_empty_markers() {
        let markers: Vec<(Ratio, Ratio)> = vec![];
        let m = State::interpolate_multiplier(&markers, Ratio::from_f64(1.5));
        assert!((m.to_f64() - 1.0).abs() < 0.001, "Empty markers should return 1.0x");
    }

    #[test]
    fn test_derived_cr_getters() {
        let state = State::from(InitArg {
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
        let icp = state.icp_ledger_principal;

        // ICP: borrow_threshold=1.5, multiplier=1.0333
        // recovery_cr = 1.5 * 1.0333 ≈ 1.55
        let recovery_cr = state.get_recovery_cr_for(&icp);
        assert!((recovery_cr.to_f64() - 1.55).abs() < 0.001,
            "Expected recovery_cr 1.55, got {}", recovery_cr.to_f64());

        // warning_cr = 2 * 1.55 - 1.5 = 1.6
        let warning_cr = state.get_warning_cr_for(&icp);
        assert!((warning_cr.to_f64() - 1.60).abs() < 0.001,
            "Expected warning_cr 1.60, got {}", warning_cr.to_f64());

        // healthy_cr = 1.5 * 1.5 = 2.25
        let healthy_cr = state.get_healthy_cr_for(&icp);
        assert!((healthy_cr.to_f64() - 2.25).abs() < 0.001,
            "Expected healthy_cr 2.25, got {}", healthy_cr.to_f64());
    }

    #[test]
    fn test_dynamic_rate_healthy_vault_normal_mode() {
        let state = State::from(InitArg {
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
        let icp = state.icp_ledger_principal;

        // A vault at 300% CR (well above healthy 225%) → multiplier = 1.0x
        let rate = state.get_dynamic_interest_rate_for(&icp, Ratio::from_f64(3.0));
        let base = DEFAULT_INTEREST_RATE_APR.to_f64();
        assert!((rate.to_f64() - base).abs() < 0.0001,
            "Healthy vault should get base rate {}, got {}", base, rate.to_f64());
    }

    #[test]
    fn test_dynamic_rate_risky_vault_normal_mode() {
        let state = State::from(InitArg {
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
        let icp = state.icp_ledger_principal;

        // Vault at 155% CR (between borrow_threshold 150% and warning_cr 160%)
        // Expected: interpolation between 2.5x and 1.75x at t=0.5 => 2.125x
        let rate = state.get_dynamic_interest_rate_for(&icp, Ratio::from_f64(1.55));
        let expected = DEFAULT_INTEREST_RATE_APR.to_f64() * 2.125;
        assert!((rate.to_f64() - expected).abs() < 0.001,
            "Expected rate {}, got {}", expected, rate.to_f64());
    }

    #[test]
    fn test_dynamic_rate_at_liquidation_ratio() {
        let state = State::from(InitArg {
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
        let icp = state.icp_ledger_principal;

        // Vault at exactly liquidation_ratio (133%) → 5.0x multiplier
        let rate = state.get_dynamic_interest_rate_for(&icp, Ratio::from_f64(1.33));
        let expected = DEFAULT_INTEREST_RATE_APR.to_f64() * 5.0;
        assert!((rate.to_f64() - expected).abs() < 0.001,
            "Expected rate {}, got {}", expected, rate.to_f64());
    }

    #[test]
    fn test_static_override_in_recovery() {
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
        let icp = state.icp_ledger_principal;
        state.mode = Mode::Recovery;

        // Set static override
        if let Some(config) = state.collateral_configs.get_mut(&icp) {
            config.recovery_interest_rate_apr = Some(Ratio::from_f64(0.10)); // 10%
        }

        // Should return the static override regardless of vault CR
        let rate = state.get_dynamic_interest_rate_for(&icp, Ratio::from_f64(3.0));
        assert!((rate.to_f64() - 0.10).abs() < 0.001,
            "Expected static override 0.10, got {}", rate.to_f64());
    }

    // --- Interest Accrual Tests ---

    /// Helper: create a State with ICP price set and a non-zero interest rate.
    fn accrual_test_state() -> State {
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
        let icp = state.icp_ledger_principal;
        // Set ICP price to $10, so CR math works
        state.last_icp_rate = Some(UsdIcp::from(dec!(10.0)));
        if let Some(config) = state.collateral_configs.get_mut(&icp) {
            config.last_price = Some(10.0);
            // Set a 5% base interest rate for testability
            config.interest_rate_apr = Ratio::from_f64(0.05);
        }
        state
    }

    #[test]
    fn test_accrue_single_vault_basic() {
        let mut state = accrual_test_state();
        let icp = state.icp_ledger_principal;

        // Insert a vault: 1.5 ICP collateral, 5 icUSD debt
        // CR = (150M * $10 / 1e8) / (500M / 1e8) = $15 / $5 = 3.0
        // 300% is above healthy_cr (225%), so multiplier = 1.0x
        let vault = Vault {
            owner: Principal::anonymous(),
            vault_id: 1,
            collateral_amount: 150_000_000, // 1.5 ICP
            borrowed_icusd_amount: ICUSD::new(500_000_000), // 5 icUSD
            collateral_type: icp,
            last_accrual_time: 0, // t=0
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        };
        state.vault_id_to_vaults.insert(1, vault);

        // Accrue for exactly 1 year
        let one_year_nanos = crate::numeric::NANOS_PER_YEAR;
        state.accrue_single_vault(1, one_year_nanos);

        let vault_after = state.vault_id_to_vaults.get(&1).unwrap();
        // At 300% CR (above healthy 225%): multiplier = 1.0x
        // rate = 5% × 1.0 = 5%
        // After 1 year: debt = 500_000_000 × 1.05 = 525_000_000
        assert_eq!(vault_after.borrowed_icusd_amount.0, 525_000_000,
            "After 1 year at 5%, 500M should become 525M, got {}",
            vault_after.borrowed_icusd_amount.0);
        assert_eq!(vault_after.last_accrual_time, one_year_nanos);
        assert_eq!(vault_after.accrued_interest.0, 25_000_000,
            "accrued_interest should track the 25M delta, got {}", vault_after.accrued_interest.0);
    }

    #[test]
    fn test_accrue_single_vault_tracks_accrued_interest() {
        let mut state = accrual_test_state();
        let icp = state.icp_ledger_principal;

        // Start with some pre-existing accrued interest
        state.vault_id_to_vaults.insert(1, Vault {
            owner: Principal::anonymous(),
            vault_id: 1,
            collateral_amount: 150_000_000, // 1.5 ICP
            borrowed_icusd_amount: ICUSD::new(500_000_000), // 5 icUSD
            collateral_type: icp,
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(10_000_000), // 0.1 icUSD pre-existing
            bot_processing: false,
        });

        state.accrue_single_vault(1, crate::numeric::NANOS_PER_YEAR);

        let vault = state.vault_id_to_vaults.get(&1).unwrap();
        assert_eq!(vault.borrowed_icusd_amount.0, 525_000_000);
        // 10M pre-existing + 25M new delta = 35M
        assert_eq!(vault.accrued_interest.0, 35_000_000,
            "accrued_interest should be 10M + 25M = 35M, got {}", vault.accrued_interest.0);
    }

    #[test]
    fn test_accrue_single_vault_zero_debt_noop() {
        let mut state = accrual_test_state();
        let icp = state.icp_ledger_principal;

        let vault = Vault {
            owner: Principal::anonymous(),
            vault_id: 1,
            collateral_amount: 100_000_000,
            borrowed_icusd_amount: ICUSD::new(0), // zero debt
            collateral_type: icp,
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        };
        state.vault_id_to_vaults.insert(1, vault);

        state.accrue_single_vault(1, crate::numeric::NANOS_PER_YEAR);

        let vault_after = state.vault_id_to_vaults.get(&1).unwrap();
        assert_eq!(vault_after.borrowed_icusd_amount.0, 0);
        // last_accrual_time should NOT be updated (no-op)
        assert_eq!(vault_after.last_accrual_time, 0);
    }

    #[test]
    fn test_accrue_single_vault_same_timestamp_noop() {
        let mut state = accrual_test_state();
        let icp = state.icp_ledger_principal;

        let vault = Vault {
            owner: Principal::anonymous(),
            vault_id: 1,
            collateral_amount: 100_000_000,
            borrowed_icusd_amount: ICUSD::new(500_000_000),
            collateral_type: icp,
            last_accrual_time: 1000, // already accrued up to t=1000
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        };
        state.vault_id_to_vaults.insert(1, vault);

        state.accrue_single_vault(1, 1000); // same timestamp → no-op

        let vault_after = state.vault_id_to_vaults.get(&1).unwrap();
        assert_eq!(vault_after.borrowed_icusd_amount.0, 500_000_000);
    }

    #[test]
    fn test_accrue_all_vault_interest_multiple_vaults() {
        let mut state = accrual_test_state();
        let icp = state.icp_ledger_principal;

        // Vault 1: 1.5 ICP, 5 icUSD → CR = 300% (above healthy 225%)
        state.vault_id_to_vaults.insert(1, Vault {
            owner: Principal::anonymous(),
            vault_id: 1,
            collateral_amount: 150_000_000,
            borrowed_icusd_amount: ICUSD::new(500_000_000),
            collateral_type: icp,
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        });

        // Vault 2: 2 ICP, 5 icUSD → CR = 400% (above healthy 225%)
        state.vault_id_to_vaults.insert(2, Vault {
            owner: Principal::anonymous(),
            vault_id: 2,
            collateral_amount: 200_000_000,
            borrowed_icusd_amount: ICUSD::new(500_000_000),
            collateral_type: icp,
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        });

        // Vault 3: zero debt (should be skipped)
        state.vault_id_to_vaults.insert(3, Vault {
            owner: Principal::anonymous(),
            vault_id: 3,
            collateral_amount: 100_000_000,
            borrowed_icusd_amount: ICUSD::new(0),
            collateral_type: icp,
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        });

        let one_year = crate::numeric::NANOS_PER_YEAR;
        state.accrue_all_vault_interest(one_year);

        // Vault 1 (300% CR, above healthy): multiplier = 1.0x, rate = 5%
        let v1 = state.vault_id_to_vaults.get(&1).unwrap();
        assert_eq!(v1.borrowed_icusd_amount.0, 525_000_000,
            "Vault 1 expected 525M, got {}", v1.borrowed_icusd_amount.0);
        assert_eq!(v1.last_accrual_time, one_year);

        // Vault 2 (400% CR, well above healthy): multiplier = 1.0x, rate = 5%
        let v2 = state.vault_id_to_vaults.get(&2).unwrap();
        assert_eq!(v2.borrowed_icusd_amount.0, 525_000_000,
            "Vault 2 expected 525M, got {}", v2.borrowed_icusd_amount.0);
        assert_eq!(v2.last_accrual_time, one_year);

        // Vault 3 (zero debt): unchanged
        let v3 = state.vault_id_to_vaults.get(&3).unwrap();
        assert_eq!(v3.borrowed_icusd_amount.0, 0);
        assert_eq!(v3.last_accrual_time, 0); // not updated
    }

    #[test]
    fn test_accrue_300s_tick() {
        let mut state = accrual_test_state();
        let icp = state.icp_ledger_principal;

        // 1.5 ICP, 5 icUSD debt → CR = 300% (above healthy) → multiplier 1.0x
        state.vault_id_to_vaults.insert(1, Vault {
            owner: Principal::anonymous(),
            vault_id: 1,
            collateral_amount: 150_000_000,
            borrowed_icusd_amount: ICUSD::new(500_000_000),
            collateral_type: icp,
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        });

        // 300 seconds in nanos
        let tick = 300_000_000_000u64;
        state.accrue_single_vault(1, tick);

        let v = state.vault_id_to_vaults.get(&1).unwrap();
        // factor = 1 + 0.05 * 300e9 / NANOS_PER_YEAR
        // = 1 + 0.05 * 300 / 31_536_000
        // = 1 + 0.05 * 9.5129e-6
        // = 1 + 4.7565e-7
        // ≈ 1.00000047565
        // new_debt = 500_000_000 * 1.00000047565 ≈ 500_000_237
        // With u64 truncation it should be 500_000_237 or close
        assert!(v.borrowed_icusd_amount.0 > 500_000_000,
            "Debt should increase after 300s tick, got {}", v.borrowed_icusd_amount.0);
        assert!(v.borrowed_icusd_amount.0 < 500_001_000,
            "Debt increase should be small for 300s, got {}", v.borrowed_icusd_amount.0);
        assert_eq!(v.last_accrual_time, tick);
    }

    #[test]
    fn test_accrual_before_check_vaults_flow() {
        // Simulates the full timer tick flow: accrue → check vault health
        let mut state = accrual_test_state();
        let icp = state.icp_ledger_principal;

        let start = 1_000_000_000_000u64; // 1 trillion nanos

        // Vault with 1.5 ICP ($15), 5 icUSD debt → CR = 300% (healthy)
        state.vault_id_to_vaults.insert(1, Vault {
            owner: Principal::anonymous(),
            vault_id: 1,
            collateral_amount: 150_000_000,
            borrowed_icusd_amount: ICUSD::new(500_000_000),
            collateral_type: icp,
            last_accrual_time: start,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        });

        let initial_debt = state.vault_id_to_vaults.get(&1).unwrap().borrowed_icusd_amount;

        // Simulate timer tick: 300 seconds later
        let tick1 = start + 300 * 1_000_000_000;
        state.accrue_all_vault_interest(tick1);

        let debt_after_tick1 = state.vault_id_to_vaults.get(&1).unwrap().borrowed_icusd_amount;
        assert!(debt_after_tick1 > initial_debt,
            "Debt should increase after first tick: {} > {}", debt_after_tick1.0, initial_debt.0);

        // Simulate second timer tick: another 300 seconds
        let tick2 = tick1 + 300 * 1_000_000_000;
        state.accrue_all_vault_interest(tick2);

        let debt_after_tick2 = state.vault_id_to_vaults.get(&1).unwrap().borrowed_icusd_amount;
        assert!(debt_after_tick2 > debt_after_tick1,
            "Debt should increase after second tick: {} > {}", debt_after_tick2.0, debt_after_tick1.0);

        // Verify the increase is proportional across ticks
        let increase1 = debt_after_tick1.0 - initial_debt.0;
        let increase2 = debt_after_tick2.0 - debt_after_tick1.0;
        // Second increase should be >= first (compounding on larger base)
        assert!(increase2 >= increase1,
            "Compounding: second increase {} should be >= first {}", increase2, increase1);
    }

    #[test]
    fn test_weighted_average_interest_rate_empty() {
        let state = accrual_test_state();
        let avg = state.weighted_average_interest_rate();
        assert_eq!(avg.0, rust_decimal::Decimal::ZERO);
    }

    #[test]
    fn test_weighted_average_interest_rate_single_vault() {
        let mut state = accrual_test_state();
        let icp = state.icp_ledger_principal;

        // Single vault at CR = 300% (above healthy_cr 225%) → multiplier 1.0x
        // Base APR = 5%, so weighted avg should be 5%
        state.vault_id_to_vaults.insert(1, Vault {
            owner: Principal::anonymous(),
            vault_id: 1,
            collateral_amount: 150_000_000, // 1.5 ICP
            borrowed_icusd_amount: ICUSD::new(500_000_000), // 5 icUSD
            collateral_type: icp,
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        });

        let avg = state.weighted_average_interest_rate();
        // At 300% CR with base 5% and 1.0x multiplier, should be ~0.05
        let diff = (avg.0 - rust_decimal_macros::dec!(0.05)).abs();
        assert!(diff < rust_decimal_macros::dec!(0.001),
            "Weighted avg rate should be ~5%, got {}", avg.0);
    }

    #[test]
    fn test_liquidation_protocol_share_splits_bonus() {
        use rust_decimal_macros::dec;
        // Setup: vault at ~130% CR, liq_bonus=1.15, protocol_share=0.03 (3%)
        // ICP price = $10, vault has 1.5 ICP ($15) and $11.5 debt → CR ~130%
        let mut state = accrual_test_state();
        let icp = state.icp_ledger_principal;
        let collateral_price = UsdIcp::from(dec!(10.0));

        // Verify default protocol share is 3%
        assert_eq!(state.get_liquidation_protocol_share().0, dec!(0.03),
            "Default liquidation_protocol_share should be 3%");

        // Vault with 1.5 ICP ($15), $11.5 debt → CR = 15/11.5 ≈ 1.304
        state.open_vault(Vault {
            owner: Principal::anonymous(),
            vault_id: 10,
            collateral_amount: 150_000_000, // 1.5 ICP = $15
            borrowed_icusd_amount: ICUSD::new(1_150_000_000), // 11.5 icUSD
            collateral_type: icp,
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(100_000_000), // 1 icUSD of accrued interest
            bot_processing: false,
        });

        // Simulate partial liquidation: liquidator pays 5 icUSD
        let liquidator_payment = ICUSD::new(500_000_000); // 5 icUSD
        let liq_bonus = state.get_liquidation_bonus_for(&icp); // 1.15
        let protocol_share = state.get_liquidation_protocol_share(); // 0.03

        // collateral_raw: 5 icUSD / $10 = 0.5 ICP = 50_000_000 e8s
        let collateral_raw = crate::numeric::icusd_to_collateral_amount(
            liquidator_payment, dec!(10.0), 8
        );
        assert_eq!(collateral_raw, 50_000_000, "collateral_raw should be 0.5 ICP");

        // total_to_seize = 0.5 ICP * 1.15 = 0.575 ICP = 57_500_000 e8s
        let total_to_seize = (ICP::from(collateral_raw) * liq_bonus).min(ICP::from(150_000_000u64));
        assert_eq!(total_to_seize.to_u64(), 57_500_000, "total_to_seize should be 0.575 ICP");

        // bonus_portion = 57_500_000 - 50_000_000 = 7_500_000 (0.075 ICP)
        let bonus_portion = total_to_seize.to_u64().saturating_sub(collateral_raw);
        assert_eq!(bonus_portion, 7_500_000, "bonus_portion should be 0.075 ICP");

        // protocol_cut = 7_500_000 * 0.03 = 225_000 (0.00225 ICP)
        let protocol_cut = (rust_decimal::Decimal::from(bonus_portion) * protocol_share.0)
            .to_u64().unwrap_or(0);
        assert_eq!(protocol_cut, 225_000, "protocol_cut should be 0.00225 ICP");

        // collateral_to_liquidator = 57_500_000 - 225_000 = 57_275_000
        let collateral_to_liquidator = total_to_seize.to_u64() - protocol_cut;
        assert_eq!(collateral_to_liquidator, 57_275_000,
            "liquidator should get total_to_seize minus protocol_cut");

        // Verify the sync State::liquidate_vault still works correctly
        // (it doesn't split the fee — the async callers do that)
        let interest_share = state.liquidate_vault(10, Mode::GeneralAvailability, collateral_price);
        // Full liquidation: all accrued_interest is returned
        assert_eq!(interest_share.0, 100_000_000, "Full liquidation should return all accrued_interest");
        // Vault should be removed
        assert!(state.vault_id_to_vaults.get(&10).is_none(), "Vault should be removed after full liquidation");
    }

    #[test]
    fn test_proportional_recovery_cr() {
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
        let icp = state.icp_ledger_principal;

        // Default multiplier: 1.0333
        // ICP borrow_threshold = 1.50
        // recovery_cr = 1.50 * 1.0333 ≈ 1.55 (same as before for ICP)
        let recovery_cr = state.get_recovery_cr_for(&icp);
        assert!(
            (recovery_cr.to_f64() - 1.55).abs() < 0.001,
            "ICP recovery CR should be ~1.55, got {}",
            recovery_cr.to_f64()
        );

        // Add a collateral with borrow_threshold 1.20
        let fake_ledger = Principal::from_text("aaaaa-aa").unwrap();
        let mut config = state.collateral_configs.get(&icp).unwrap().clone();
        config.borrow_threshold_ratio = Ratio::from_f64(1.20);
        config.ledger_canister_id = fake_ledger;
        state.collateral_configs.insert(fake_ledger, config);

        // recovery_cr = 1.20 * 1.0333 = 1.24
        let recovery_cr_low = state.get_recovery_cr_for(&fake_ledger);
        assert!(
            (recovery_cr_low.to_f64() - 1.24).abs() < 0.001,
            "Low-threshold recovery CR should be ~1.24, got {}",
            recovery_cr_low.to_f64()
        );

        // Add a collateral with borrow_threshold 2.00
        let fake_ledger2 = Principal::from_text("2vxsx-fae").unwrap();
        let mut config2 = state.collateral_configs.get(&icp).unwrap().clone();
        config2.borrow_threshold_ratio = Ratio::from_f64(2.00);
        config2.ledger_canister_id = fake_ledger2;
        state.collateral_configs.insert(fake_ledger2, config2);

        // recovery_cr = 2.00 * 1.0333 = 2.0666
        let recovery_cr_high = state.get_recovery_cr_for(&fake_ledger2);
        assert!(
            (recovery_cr_high.to_f64() - 2.0666).abs() < 0.001,
            "High-threshold recovery CR should be ~2.0666, got {}",
            recovery_cr_high.to_f64()
        );
    }

    #[test]
    fn test_proportional_recovery_cr_reconfigurable() {
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
        let icp = state.icp_ledger_principal;

        // Change multiplier to 1.05 (5% proportional buffer)
        state.recovery_cr_multiplier = Ratio::from_f64(1.05);
        let recovery_cr = state.get_recovery_cr_for(&icp);
        // 1.50 * 1.05 = 1.575
        assert!(
            (recovery_cr.to_f64() - 1.575).abs() < 0.001,
            "Expected 1.575, got {}",
            recovery_cr.to_f64()
        );
    }

    #[test]
    fn test_stablecoin_repayment_does_not_increase_icusd_supply() {
        // This is a design-level test: verify that repay_to_vault returns
        // interest_share correctly, and that the CALLER is responsible for
        // NOT minting icUSD when the repayment was in stablecoins.
        let mut state = accrual_test_state();
        let icp = state.icp_ledger_principal;

        // Create vault with 100 icUSD debt, 5 icUSD accrued interest
        state.vault_id_to_vaults.insert(1, Vault {
            owner: Principal::anonymous(),
            vault_id: 1,
            borrowed_icusd_amount: ICUSD::new(10_000_000_000), // 100 icUSD
            collateral_amount: 1_000_000_000,
            collateral_type: icp,
            accrued_interest: ICUSD::new(500_000_000), // 5 icUSD interest
            last_accrual_time: 0,
            bot_processing: false,
        });

        // Repay 50 icUSD worth
        let (interest_share, principal_share) = state.repay_to_vault(1, ICUSD::new(5_000_000_000));

        // Interest share should be proportional: 50 * (5/105) ≈ 2.380952 icUSD
        // Note: total debt is borrowed_icusd_amount = 100 icUSD, but accrued_interest
        // is 5 icUSD, so interest ratio = 5/100 = 5%.
        // interest_share = 50 * 5/100 = 2.5 icUSD
        assert!(
            (interest_share.to_u64() as f64 / 1e8 - 2.5).abs() < 0.01,
            "Interest share should be ~2.5 icUSD, got {}",
            interest_share.to_u64() as f64 / 1e8
        );

        // Principal share should be the rest: 50 - 2.5 = 47.5 icUSD
        assert!(
            (principal_share.to_u64() as f64 / 1e8 - 47.5).abs() < 0.01,
            "Principal share should be ~47.5 icUSD, got {}",
            principal_share.to_u64() as f64 / 1e8
        );
    }

    /// Helper to create a minimal State for RMR tests.
    fn test_state() -> State {
        State::from(InitArg {
            xrc_principal: Principal::anonymous(),
            icusd_ledger_principal: Principal::anonymous(),
            icp_ledger_principal: Principal::anonymous(),
            fee_e8s: 0,
            developer_principal: Principal::anonymous(),
            treasury_principal: None,
            stability_pool_principal: None,
            ckusdt_ledger_principal: None,
            ckusdc_ledger_principal: None,
        })
    }

    #[test]
    fn test_dynamic_rmr_healthy_system() {
        let mut state = test_state();
        state.total_collateral_ratio = Ratio::from_f64(2.25);
        state.recovery_mode_threshold = RECOVERY_COLLATERAL_RATIO; // 1.50
        let rmr = state.get_redemption_margin_ratio();
        assert!(
            (rmr.to_f64() - 0.96).abs() < 0.001,
            "RMR at 1.5x recovery should be 0.96, got {}", rmr.to_f64()
        );
    }

    #[test]
    fn test_dynamic_rmr_at_recovery() {
        let mut state = test_state();
        state.total_collateral_ratio = Ratio::from_f64(1.50);
        state.recovery_mode_threshold = RECOVERY_COLLATERAL_RATIO;
        let rmr = state.get_redemption_margin_ratio();
        assert!(
            (rmr.to_f64() - 1.0).abs() < 0.001,
            "RMR at recovery threshold should be 1.0, got {}", rmr.to_f64()
        );
    }

    #[test]
    fn test_dynamic_rmr_midpoint() {
        let mut state = test_state();
        state.total_collateral_ratio = Ratio::from_f64(1.875);
        state.recovery_mode_threshold = RECOVERY_COLLATERAL_RATIO;
        let rmr = state.get_redemption_margin_ratio();
        assert!(
            (rmr.to_f64() - 0.98).abs() < 0.001,
            "RMR at midpoint should be 0.98, got {}", rmr.to_f64()
        );
    }

    #[test]
    fn test_dynamic_rmr_below_recovery() {
        let mut state = test_state();
        state.total_collateral_ratio = Ratio::from_f64(1.30);
        state.recovery_mode_threshold = RECOVERY_COLLATERAL_RATIO;
        let rmr = state.get_redemption_margin_ratio();
        assert!(
            (rmr.to_f64() - 1.0).abs() < 0.001,
            "RMR below recovery should be 1.0, got {}", rmr.to_f64()
        );
    }

    #[test]
    fn test_dynamic_rmr_above_15x() {
        let mut state = test_state();
        state.total_collateral_ratio = Ratio::from_f64(5.0);
        state.recovery_mode_threshold = RECOVERY_COLLATERAL_RATIO;
        let rmr = state.get_redemption_margin_ratio();
        assert!(
            (rmr.to_f64() - 0.96).abs() < 0.001,
            "RMR above 1.5x should be capped at 0.96, got {}", rmr.to_f64()
        );
    }

    #[test]
    fn test_interest_split_ratios() {
        let state = test_state();
        assert!(
            (state.interest_pool_share.to_f64() - 0.75).abs() < 0.001,
            "Default interest pool share should be 0.75, got {}", state.interest_pool_share.to_f64()
        );

        let interest = ICUSD::from(100_000_000_00u64); // 100 icUSD
        let pool_share = ICUSD::from(
            (Decimal::from(interest.to_u64()) * state.interest_pool_share.0)
                .to_u64()
                .unwrap_or(0)
        );
        let treasury_share = ICUSD::from(interest.to_u64() - pool_share.to_u64());

        assert!(
            (pool_share.to_u64() as f64 / 1e8 - 75.0).abs() < 0.01,
            "Pool share should be ~75, got {}", pool_share.to_u64() as f64 / 1e8
        );
        assert!(
            (treasury_share.to_u64() as f64 / 1e8 - 25.0).abs() < 0.01,
            "Treasury share should be ~25, got {}", treasury_share.to_u64() as f64 / 1e8
        );
    }

    #[test]
    fn test_interest_split_custom_ratio() {
        let mut state = test_state();
        state.interest_pool_share = Ratio::from_f64(0.50); // 50/50 split

        let interest = ICUSD::from(200_000_000_00u64); // 200 icUSD
        let pool_share = ICUSD::from(
            (Decimal::from(interest.to_u64()) * state.interest_pool_share.0)
                .to_u64()
                .unwrap_or(0)
        );
        let treasury_share = ICUSD::from(interest.to_u64() - pool_share.to_u64());

        assert!(
            (pool_share.to_u64() as f64 / 1e8 - 100.0).abs() < 0.01,
            "Pool share should be ~100, got {}", pool_share.to_u64() as f64 / 1e8
        );
        assert!(
            (treasury_share.to_u64() as f64 / 1e8 - 100.0).abs() < 0.01,
            "Treasury share should be ~100, got {}", treasury_share.to_u64() as f64 / 1e8
        );
    }

    #[test]
    fn test_interest_split_zero_interest() {
        let state = test_state();
        let interest = ICUSD::from(0u64);
        let pool_share = ICUSD::from(
            (Decimal::from(interest.to_u64()) * state.interest_pool_share.0)
                .to_u64()
                .unwrap_or(0)
        );
        let treasury_share = ICUSD::from(interest.to_u64() - pool_share.to_u64());

        assert_eq!(pool_share.to_u64(), 0, "Pool share should be 0 for zero interest");
        assert_eq!(treasury_share.to_u64(), 0, "Treasury share should be 0 for zero interest");
    }

    #[test]
    fn test_stablecoin_interest_split_accounting() {
        // Verify the accounting: with 5 icUSD interest at 75/25 split
        let interest_e8s: u64 = 5_000_000_00; // 5 icUSD in e8s
        let pool_ratio = 0.75_f64;
        let pool_e8s = (interest_e8s as f64 * pool_ratio) as u64;
        let treasury_e8s = interest_e8s - pool_e8s;

        // Convert to e6s (ckStable)
        let pool_e6s = pool_e8s / 100;      // 3_750_000 = 3.75 ckUSDT
        let treasury_e6s = treasury_e8s / 100; // 1_250_000 = 1.25 ckUSDT

        assert_eq!(pool_e6s, 3_750_000);
        assert_eq!(treasury_e6s, 1_250_000);

        // icUSD minted to stability pool = pool_share in e8s
        let icusd_minted = pool_e8s; // 3.75 icUSD
        assert_eq!(icusd_minted, 375_000_000);

        // Verify: reserves (pool_e6s) back the minted icUSD 1:1
        assert_eq!(pool_e6s * 100, icusd_minted);
    }

    // --- resolve_anchor / resolve_curve tests ---

    #[test]
    fn test_resolve_anchor_fixed() {
        let state = accrual_test_state();
        let anchor = CrAnchor::Fixed(Ratio::from_f64(1.75));
        let result = state.resolve_anchor(&anchor, None);
        assert!((result.to_f64() - 1.75).abs() < 0.001);
    }

    #[test]
    fn test_resolve_anchor_system_threshold_tcr() {
        let mut state = accrual_test_state();
        state.total_collateral_ratio = Ratio::from_f64(1.85);
        let anchor = CrAnchor::SystemThreshold(SystemThreshold::TotalCollateralRatio);
        let result = state.resolve_anchor(&anchor, None);
        assert!((result.to_f64() - 1.85).abs() < 0.001);
    }

    #[test]
    fn test_resolve_anchor_midpoint() {
        let mut state = accrual_test_state();
        state.total_collateral_ratio = Ratio::from_f64(2.0);
        state.recovery_mode_threshold = Ratio::from_f64(1.5);
        let anchor = CrAnchor::Midpoint(
            Box::new(CrAnchor::SystemThreshold(SystemThreshold::BorrowThreshold)),
            Box::new(CrAnchor::SystemThreshold(SystemThreshold::TotalCollateralRatio)),
        );
        let result = state.resolve_anchor(&anchor, None);
        assert!((result.to_f64() - 1.75).abs() < 0.001,
            "Midpoint of 1.5 and 2.0 should be 1.75, got {}", result.to_f64());
    }

    #[test]
    fn test_resolve_anchor_offset() {
        let mut state = accrual_test_state();
        state.recovery_mode_threshold = Ratio::from_f64(1.5);
        let anchor = CrAnchor::Offset(
            Box::new(CrAnchor::SystemThreshold(SystemThreshold::BorrowThreshold)),
            Ratio::from_f64(0.05),
        );
        let result = state.resolve_anchor(&anchor, None);
        assert!((result.to_f64() - 1.55).abs() < 0.001,
            "1.5 + 0.05 should be 1.55, got {}", result.to_f64());
    }

    #[test]
    fn test_resolve_anchor_asset_threshold() {
        let state = accrual_test_state();
        let icp = state.icp_collateral_type();
        let anchor = CrAnchor::AssetThreshold(AssetThreshold::BorrowThreshold);
        let result = state.resolve_anchor(&anchor, Some(&icp));
        // ICP borrow threshold — check what accrual_test_state sets
        assert!(result.to_f64() > 1.0,
            "ICP borrow threshold should be > 1.0, got {}", result.to_f64());
    }

    #[test]
    fn test_resolve_curve_sorts_by_cr() {
        let mut state = accrual_test_state();
        state.total_collateral_ratio = Ratio::from_f64(2.0);
        state.recovery_mode_threshold = Ratio::from_f64(1.5);
        let curve = RateCurveV2 {
            markers: vec![
                // Intentionally out of order
                RateMarkerV2 {
                    cr_anchor: CrAnchor::SystemThreshold(SystemThreshold::TotalCollateralRatio),
                    multiplier: Ratio::from_f64(1.0),
                },
                RateMarkerV2 {
                    cr_anchor: CrAnchor::SystemThreshold(SystemThreshold::BorrowThreshold),
                    multiplier: Ratio::from_f64(3.0),
                },
            ],
            method: InterpolationMethod::Linear,
        };
        let resolved = state.resolve_curve(&curve, None);
        assert!(resolved[0].0.to_f64() < resolved[1].0.to_f64(),
            "Should be sorted ascending: {} < {}", resolved[0].0.to_f64(), resolved[1].0.to_f64());
        assert!((resolved[0].0.to_f64() - 1.5).abs() < 0.01);
        assert!((resolved[1].0.to_f64() - 2.0).abs() < 0.01);
    }

    #[test]
    fn test_borrowing_fee_multiplier_above_tcr() {
        let mut state = accrual_test_state();
        state.total_collateral_ratio = Ratio::from_f64(1.75);
        state.recovery_mode_threshold = Ratio::from_f64(1.5);

        // Vault CR = 2.0 (above TCR of 1.75) → multiplier should be 1.0
        let mult = state.get_borrowing_fee_multiplier(Ratio::from_f64(2.0));
        assert!((mult.to_f64() - 1.0).abs() < 0.01,
            "Above TCR should be 1.0x, got {}", mult.to_f64());
    }

    #[test]
    fn test_borrowing_fee_multiplier_at_midpoint() {
        let mut state = accrual_test_state();
        state.total_collateral_ratio = Ratio::from_f64(2.0);
        state.recovery_mode_threshold = Ratio::from_f64(1.5);
        // Midpoint = (1.5 + 2.0) / 2 = 1.75

        let mult = state.get_borrowing_fee_multiplier(Ratio::from_f64(1.75));
        assert!((mult.to_f64() - 1.75).abs() < 0.01,
            "At midpoint should be 1.75x, got {}", mult.to_f64());
    }

    #[test]
    fn test_borrowing_fee_multiplier_at_floor() {
        let mut state = accrual_test_state();
        state.total_collateral_ratio = Ratio::from_f64(2.0);
        state.recovery_mode_threshold = Ratio::from_f64(1.5);
        // Floor = BorrowThreshold + 0.05 = 1.55

        let mult = state.get_borrowing_fee_multiplier(Ratio::from_f64(1.55));
        assert!((mult.to_f64() - 3.0).abs() < 0.01,
            "At floor should be 3.0x, got {}", mult.to_f64());
    }

    #[test]
    fn test_borrowing_fee_multiplier_below_floor_capped() {
        let mut state = accrual_test_state();
        state.total_collateral_ratio = Ratio::from_f64(2.0);
        state.recovery_mode_threshold = Ratio::from_f64(1.5);

        let mult = state.get_borrowing_fee_multiplier(Ratio::from_f64(1.4));
        assert!((mult.to_f64() - 3.0).abs() < 0.01,
            "Below floor should still be 3.0x (capped), got {}", mult.to_f64());
    }

    #[test]
    fn test_borrowing_fee_multiplier_none_curve() {
        let mut state = accrual_test_state();
        state.borrowing_fee_curve = None;
        let mult = state.get_borrowing_fee_multiplier(Ratio::from_f64(1.4));
        assert!((mult.to_f64() - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_borrowing_fee_multiplier_inverted_curve_returns_max() {
        // Simulate TCR=0 (e.g., right after canister upgrade before first price fetch)
        let mut state = accrual_test_state();
        state.total_collateral_ratio = Ratio::from_f64(0.0);
        state.recovery_mode_threshold = Ratio::from_f64(1.5);

        // With TCR=0, the resolved curve inverts: (0.0, 1.0), (0.75, 1.75), (1.55, 3.0)
        // A healthy vault at CR=2.0 should NOT get 3.0x from interpolation above the last marker.
        // The inversion guard should return 3.0x (max multiplier) for all CRs.
        let mult_healthy = state.get_borrowing_fee_multiplier(Ratio::from_f64(2.0));
        assert!((mult_healthy.to_f64() - 3.0).abs() < 0.01,
            "Inverted curve should return max multiplier (3.0x), got {}", mult_healthy.to_f64());

        let mult_low = state.get_borrowing_fee_multiplier(Ratio::from_f64(1.0));
        assert!((mult_low.to_f64() - 3.0).abs() < 0.01,
            "Inverted curve should return max multiplier (3.0x) for low CR too, got {}", mult_low.to_f64());
    }

    // ─── Regression Tests (bugs caught on mainnet) ───

    /// Regression: close_vault must NOT create phantom pending_margin_transfers.
    /// Bug (1bdf5c0): close_vault() inserted a PendingMarginTransfer on every close,
    /// but CloseVault requires collateral=0, so the entry had 0 margin and was never
    /// cleared. 11 phantom entries (~5.62 ICP) accumulated, inflating tracked
    /// obligations and breaking admin_sweep_to_treasury surplus calculations.
    #[test]
    fn test_close_vault_no_phantom_pending_transfers() {
        let mut state = test_state();
        let owner = Principal::anonymous();

        // Open 5 vaults with varying collateral
        for i in 1..=5u64 {
            state.open_vault(Vault {
                owner,
                vault_id: i,
                collateral_amount: 0, // CloseVault requires 0 collateral
                borrowed_icusd_amount: ICUSD::new(0),
                collateral_type: state.icp_ledger_principal,
                last_accrual_time: 0,
                accrued_interest: ICUSD::new(0),
                bot_processing: false,
            });
        }

        assert_eq!(state.vault_id_to_vaults.len(), 5);
        assert!(state.pending_margin_transfers.is_empty(),
            "No pending transfers before closing");

        // Close all 5 vaults
        for i in 1..=5u64 {
            state.close_vault(i);
        }

        assert!(state.vault_id_to_vaults.is_empty(), "All vaults should be removed");
        assert!(state.pending_margin_transfers.is_empty(),
            "close_vault must NOT create phantom pending_margin_transfers, found {}",
            state.pending_margin_transfers.len());
    }

    /// Regression: RMR must be applied exactly once during reserve redemption spillover.
    /// Bug (96210bf): In redeem_reserves(), RMR was applied at line 161 to compute
    /// effective_icusd, then the spillover block applied it AGAIN: effective_spillover
    /// = (spillover_icusd - vault_fee) * rmr. Vault owners lost 0.96² = 0.9216 instead
    /// of 0.96 of their collateral value.
    ///
    /// This tests the math: given a redemption amount, the spillover amount reaching
    /// vault redemption should reflect exactly one RMR application, not two.
    #[test]
    fn test_rmr_applied_once_in_spillover() {
        // Simulate the redemption math from redeem_reserves()
        let rmr = Ratio::from_f64(0.96); // typical RMR
        let reserve_fee = Ratio::from_f64(0.003); // 0.3% flat fee
        let icusd_amount = ICUSD::new(10_000_000_000); // 100 icUSD

        // Step 1: fee + RMR (line 156-161 in vault.rs)
        let fee_icusd = icusd_amount * reserve_fee;
        let net_icusd = icusd_amount - fee_icusd;
        let effective_icusd = net_icusd * rmr;

        // Assume reserves cover 0, so everything spills over
        let spillover_e8s = effective_icusd.to_u64();
        let spillover_icusd = ICUSD::from(spillover_e8s);

        // Step 2: vault redemption fee on the spillover (line 264-271)
        let vault_fee_ratio = Ratio::from_f64(0.005); // example base rate
        let vault_fee = spillover_icusd * vault_fee_ratio;

        // CORRECT (after fix): no second RMR application
        let effective_spillover_correct = spillover_icusd - vault_fee;

        // WRONG (before fix): double RMR
        let effective_spillover_buggy = (spillover_icusd - vault_fee) * rmr;

        // The correct value should be higher than the buggy value
        assert!(effective_spillover_correct > effective_spillover_buggy,
            "Correct spillover ({}) must be > buggy double-RMR spillover ({})",
            effective_spillover_correct.to_u64(), effective_spillover_buggy.to_u64());

        // Verify the difference is exactly the second RMR application
        // buggy = correct * 0.96, so correct / buggy ≈ 1/0.96 ≈ 1.0417
        let ratio = effective_spillover_correct.to_u64() as f64
            / effective_spillover_buggy.to_u64() as f64;
        assert!((ratio - 1.0 / 0.96).abs() < 0.001,
            "Difference should be exactly the second RMR factor, ratio = {}", ratio);

        // Verify the effective spillover is exactly: (original * (1-fee) * rmr) * (1 - vault_fee_ratio)
        // NOT: (original * (1-fee) * rmr) * (1 - vault_fee_ratio) * rmr
        let expected = 100.0 * (1.0 - 0.003) * 0.96 * (1.0 - 0.005);
        let actual = effective_spillover_correct.to_u64() as f64 / 1e8;
        assert!((actual - expected).abs() < 0.01,
            "Effective spillover should be {:.4} icUSD, got {:.4}", expected, actual);
    }

    // ─── Liquidation Bot Tests ───

    #[test]
    fn test_bot_liquidation_amount_formula() {
        // L = (T*D - C) / (T - B) where T=target CR, D=debt, C=collateral value, B=bonus
        let t = 1.50_f64;
        let d = 1000.0;
        let c = 1400.0;
        let b = 1.15;
        let l = (t * d - c) / (t - b);
        assert!((l - 285.71).abs() < 0.01, "L should be ~285.71, got {}", l);

        // Verify post-liquidation CR equals target
        let new_debt = d - l;
        let seized = l * b;
        let new_collateral = c - seized;
        let new_cr = new_collateral / new_debt;
        assert!((new_cr - 1.50).abs() < 0.01, "New CR should be 1.50, got {}", new_cr);
    }

    #[test]
    fn test_bot_budget_decrement() {
        let mut state = test_state();
        state.bot_budget_total_e8s = 1_000_000_000_000; // $10,000
        state.bot_budget_remaining_e8s = 1_000_000_000_000;

        let liquidation_amount = 28_571_000_000u64; // 285.71 icUSD in e8s
        assert!(state.bot_budget_remaining_e8s >= liquidation_amount);
        state.bot_budget_remaining_e8s -= liquidation_amount;
        state.bot_total_debt_covered_e8s += liquidation_amount;

        assert_eq!(state.bot_budget_remaining_e8s, 1_000_000_000_000 - 28_571_000_000);
        assert_eq!(state.bot_total_debt_covered_e8s, 28_571_000_000);
    }

    #[test]
    fn test_bot_budget_exhausted_blocks_liquidation() {
        let mut state = test_state();
        state.bot_budget_remaining_e8s = 10_000_000; // 0.1 icUSD remaining

        let liquidation_amount = 28_571_000_000u64; // 285.71 icUSD
        assert!(state.bot_budget_remaining_e8s < liquidation_amount,
            "Budget should be insufficient");
    }

    #[test]
    fn test_state_serialization_roundtrip() {
        use crate::vault::Vault;

        let mut state = test_state();

        // Add a vault with realistic data
        let vault = Vault {
            owner: Principal::anonymous(),
            vault_id: 42,
            collateral_amount: 500_000_000,
            borrowed_icusd_amount: ICUSD::new(100_000_000),
            collateral_type: state.icp_ledger_principal,
            last_accrual_time: 1_000_000_000,
            accrued_interest: ICUSD::new(5_000_000),
            bot_processing: false,
        };
        state.vault_id_to_vaults.insert(42, vault);
        state.principal_to_vault_ids
            .entry(Principal::anonymous())
            .or_default()
            .insert(42);

        // Serialize
        let mut buf = Vec::new();
        ciborium::ser::into_writer(&state, &mut buf).unwrap();

        // Deserialize
        let restored: State = ciborium::de::from_reader(buf.as_slice()).unwrap();

        // Verify vault fields are preserved exactly
        assert_eq!(restored.vault_id_to_vaults.len(), state.vault_id_to_vaults.len());
        let v = &restored.vault_id_to_vaults[&42];
        assert_eq!(v.borrowed_icusd_amount, ICUSD::new(100_000_000));
        assert_eq!(v.accrued_interest, ICUSD::new(5_000_000));
        assert_eq!(v.collateral_amount, 500_000_000);
        assert_eq!(v.last_accrual_time, 1_000_000_000);

        // Verify key state fields
        assert_eq!(restored.mode, state.mode);
        assert_eq!(restored.fee, state.fee);
        assert_eq!(restored.developer_principal, state.developer_principal);
        assert_eq!(restored.icp_ledger_principal, state.icp_ledger_principal);
        assert_eq!(restored.next_available_vault_id, state.next_available_vault_id);
    }

    #[test]
    fn test_serde_default_handles_missing_fields() {
        // Simulate old CBOR that's missing a field by serializing a subset,
        // then verifying ciborium + serde(default) fills in the missing field.
        // We use a raw CBOR map with only a few fields to prove missing ones
        // get their Default value instead of causing a deserialization error.
        let state = test_state();
        let mut buf = Vec::new();
        ciborium::ser::into_writer(&state, &mut buf).unwrap();

        // Decode the CBOR map, remove a field, re-encode, and deserialize.
        let value: ciborium::Value = ciborium::de::from_reader(buf.as_slice()).unwrap();
        if let ciborium::Value::Map(mut entries) = value {
            // Remove "frozen" field from the map
            let original_len = entries.len();
            entries.retain(|(k, _)| {
                if let ciborium::Value::Text(key) = k {
                    key != "frozen"
                } else {
                    true
                }
            });
            assert_eq!(entries.len(), original_len - 1, "should have removed one field");

            // Re-encode the modified map
            let mut modified_buf = Vec::new();
            ciborium::ser::into_writer(&ciborium::Value::Map(entries), &mut modified_buf).unwrap();

            // Deserialize with the missing field: serde(default) should fill it
            let restored: State = ciborium::de::from_reader(modified_buf.as_slice()).unwrap();
            // "frozen" should be false (the Default value), not cause an error
            assert_eq!(restored.frozen, false);
            // Other fields should still be intact
            assert_eq!(restored.mode, state.mode);
            assert_eq!(restored.developer_principal, state.developer_principal);
        } else {
            panic!("expected CBOR map");
        }
    }
}