use candid::{CandidType, Principal};
use serde::{Serialize, Deserialize};

// ─── Constants ───

/// Number of coins in the pool.
pub const N_COINS: u64 = 3;

// ─── CoinIndex ───

/// Index of a coin within the 3pool.
#[derive(CandidType, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoinIndex {
    Coin0 = 0,
    Coin1 = 1,
    Coin2 = 2,
}

impl CoinIndex {
    pub fn as_usize(self) -> usize {
        self as usize
    }
}

impl TryFrom<u8> for CoinIndex {
    type Error = ThreePoolError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(CoinIndex::Coin0),
            1 => Ok(CoinIndex::Coin1),
            2 => Ok(CoinIndex::Coin2),
            _ => Err(ThreePoolError::InvalidCoinIndex),
        }
    }
}

// ─── Token & Pool Config ───

/// Configuration for a single token in the pool.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenConfig {
    /// Ledger canister principal for this token.
    pub ledger_id: Principal,
    /// Human-readable symbol (e.g. "icUSD", "ckUSDT", "ckUSDC").
    pub symbol: String,
    /// Native decimals of the token (e.g. 8 for icUSD, 6 for ckUSDT/ckUSDC).
    pub decimals: u8,
    /// Precision multiplier to normalize to the highest-decimal token.
    /// For a token with 6 decimals in a pool where max is 8: precision_mul = 100.
    pub precision_mul: u64,
}

/// Full pool configuration (includes ramping A parameter).
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct PoolConfig {
    /// Token configurations for each of the 3 coins.
    pub tokens: [TokenConfig; 3],
    /// Current amplification coefficient (or the initial value during ramping).
    pub initial_a: u64,
    /// Target amplification coefficient after ramp completes.
    pub future_a: u64,
    /// Timestamp (ns) when A ramping started.
    pub initial_a_time: u64,
    /// Timestamp (ns) when A ramping ends.
    pub future_a_time: u64,
    /// Swap fee in basis points (e.g. 4 = 0.04%).
    pub swap_fee_bps: u64,
    /// Admin fee in basis points, as a fraction of the swap fee (e.g. 5000 = 50% of swap fee).
    pub admin_fee_bps: u64,
    /// Admin principal who can adjust parameters.
    pub admin: Principal,
    /// Dynamic fee curve parameters. Optional for upgrade compatibility:
    /// pre-upgrade state won't have this field and will deserialize to None,
    /// which callers should treat as `FeeCurveParams::default()`.
    #[serde(default)]
    pub fee_curve: Option<FeeCurveParams>,
}

// ─── Init Args ───

/// Arguments passed to `init` when deploying the 3pool canister.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct ThreePoolInitArgs {
    /// Token configurations for each of the 3 coins.
    pub tokens: [TokenConfig; 3],
    /// Initial amplification coefficient.
    pub initial_a: u64,
    /// Swap fee in basis points (e.g. 4 = 0.04%).
    pub swap_fee_bps: u64,
    /// Admin fee in basis points (fraction of swap fee, e.g. 5000 = 50%).
    pub admin_fee_bps: u64,
    /// Admin principal.
    pub admin: Principal,
}

// ─── LP Balance ───

/// Represents an LP token balance.
#[derive(CandidType, Clone, Debug, Default, Serialize, Deserialize)]
pub struct LpBalance {
    pub amount: u128,
}

// ─── ICRC-2 Allowance ───

/// ICRC-2 allowance for LP token transfers.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct LpAllowance {
    pub amount: u128,
    pub expires_at: Option<u64>,
}

// ─── ICRC-3 Block Types ───

/// A single block in the ICRC-3 transaction log.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct Icrc3Block {
    /// Sequential block index (matches lp_tx_count).
    pub id: u64,
    /// Timestamp in nanoseconds since UNIX epoch.
    pub timestamp: u64,
    /// The transaction recorded in this block.
    pub tx: Icrc3Transaction,
}

/// A transaction recorded in the ICRC-3 block log.
///
/// Each role's subaccount is stored alongside the principal so the ICRC-3
/// block encoding can emit the full `[owner, subaccount]` Account shape
/// required by external verifiers (e.g. the protocol_backend's SP writedown
/// proof verification path). Subaccount fields are `Option<Vec<u8>>` and use
/// `#[serde(default)]` so blocks written before this change still decode
/// (their subaccount fields are `None`, and the encoder falls back to the
/// legacy `[owner]`-only encoding for those blocks — preserving the existing
/// ICRC-3 hash chain).
///
/// Note: the 3pool's per-balance bookkeeping is still keyed by `Principal`
/// only (subaccounts are accepted on the API surface but ignored for balance
/// lookups — see icrc_token.rs). This change only fixes the *block log* so
/// it correctly reflects the destination Account that ICRC-3 consumers need
/// to see.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub enum Icrc3Transaction {
    Mint {
        to: Principal,
        amount: u128,
        #[serde(default)]
        to_subaccount: Option<Vec<u8>>,
    },
    Burn {
        from: Principal,
        amount: u128,
        #[serde(default)]
        from_subaccount: Option<Vec<u8>>,
    },
    Transfer {
        from: Principal,
        to: Principal,
        amount: u128,
        spender: Option<Principal>,
        #[serde(default)]
        from_subaccount: Option<Vec<u8>>,
        #[serde(default)]
        to_subaccount: Option<Vec<u8>>,
        #[serde(default)]
        spender_subaccount: Option<Vec<u8>>,
    },
    Approve {
        from: Principal,
        spender: Principal,
        amount: u128,
        expires_at: Option<u64>,
        #[serde(default)]
        from_subaccount: Option<Vec<u8>>,
        #[serde(default)]
        spender_subaccount: Option<Vec<u8>>,
    },
}

// ─── Virtual Price Snapshots (for APY calculation) ───

/// A point-in-time snapshot of virtual_price, taken every 6 hours.
/// Used to compute APY from virtual_price growth over 24h/7d/30d windows.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct VirtualPriceSnapshot {
    /// Unix timestamp in seconds.
    pub timestamp_secs: u64,
    /// Virtual price scaled by 1e18 (D_18dec * 1e8 / supply_8dec).
    pub virtual_price: u128,
    /// Total LP supply at snapshot time.
    pub lp_total_supply: u128,
}

/// A recorded swap event for explorer/analytics (v1 schema, pre dynamic fees).
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct SwapEventV1 {
    /// Sequential event index.
    pub id: u64,
    /// Timestamp in nanoseconds since UNIX epoch.
    pub timestamp: u64,
    /// The principal who initiated the swap.
    pub caller: Principal,
    /// Index of the input token (0, 1, or 2).
    pub token_in: u8,
    /// Index of the output token (0, 1, or 2).
    pub token_out: u8,
    /// Amount of input token (native decimals).
    pub amount_in: u128,
    /// Amount of output token received (native decimals).
    pub amount_out: u128,
    /// Fee charged (in output token units, native decimals).
    pub fee: u128,
}

/// Back-compat alias so existing call sites continue to compile.
/// New code should use `SwapEventV2`.
pub type SwapEvent = SwapEventV1;

/// Swap event schema v2 with dynamic-fee metadata.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct SwapEventV2 {
    pub id: u64,
    pub timestamp: u64,
    pub caller: Principal,
    pub token_in: u8,
    pub token_out: u8,
    pub amount_in: u128,
    pub amount_out: u128,
    /// Fee charged (in output token units, native decimals).
    pub fee: u128,
    /// Actual fee rate charged in basis points.
    pub fee_bps: u16,
    /// Imbalance metric (1e9 fixed-point) before the swap.
    pub imbalance_before: u64,
    /// Imbalance metric (1e9 fixed-point) after the swap.
    pub imbalance_after: u64,
    /// True if the swap strictly reduced pool imbalance.
    pub is_rebalancing: bool,
    /// Pool balances (native decimals) after the swap.
    pub pool_balances_after: [u128; 3],
    /// Virtual price (scaled by 1e18) after the swap.
    pub virtual_price_after: u128,
    /// True if this entry was backfilled from a v1 event during migration.
    /// Migrated entries have sentinel values for the v2-only fields and are
    /// excluded from explorer aggregations to keep stats accurate.
    #[serde(default)]
    pub migrated: bool,
}

// ─── Liquidity Events ───

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub enum LiquidityAction {
    AddLiquidity,
    RemoveLiquidity,
    RemoveOneCoin,
    Donate,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct LiquidityEventV1 {
    pub id: u64,
    pub timestamp: u64,
    pub caller: Principal,
    pub action: LiquidityAction,
    /// Per-token amounts (3 elements: icUSD, ckUSDT, ckUSDC)
    pub amounts: [u128; 3],
    /// LP tokens minted or burned
    pub lp_amount: u128,
    /// For RemoveOneCoin: which coin index was withdrawn
    pub coin_index: Option<u8>,
    /// Fee charged (for RemoveOneCoin)
    pub fee: Option<u128>,
}

/// Back-compat alias so existing call sites continue to compile.
/// New code should use `LiquidityEventV2`.
pub type LiquidityEvent = LiquidityEventV1;

/// Liquidity event schema v2 with dynamic-fee metadata.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct LiquidityEventV2 {
    pub id: u64,
    pub timestamp: u64,
    pub caller: Principal,
    pub action: LiquidityAction,
    pub amounts: [u128; 3],
    pub lp_amount: u128,
    pub coin_index: Option<u8>,
    pub fee: Option<u128>,
    /// Fee rate in basis points (None for ops that charge no fee, e.g. proportional remove).
    pub fee_bps: Option<u16>,
    /// Imbalance metric (1e9 fixed-point) before the operation.
    pub imbalance_before: u64,
    /// Imbalance metric (1e9 fixed-point) after the operation.
    pub imbalance_after: u64,
    /// True if the operation strictly reduced pool imbalance.
    pub is_rebalancing: bool,
    /// Pool balances (native decimals) after the operation.
    pub pool_balances_after: [u128; 3],
    /// Virtual price (scaled by 1e18) after the operation.
    pub virtual_price_after: u128,
    /// True if this entry was backfilled from a v1 event during migration.
    /// Migrated entries have sentinel values for the v2-only fields and are
    /// excluded from explorer aggregations to keep stats accurate.
    #[serde(default)]
    pub migrated: bool,
}

// ─── Admin Events ───

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub enum ThreePoolAdminAction {
    RampA { future_a: u64, future_a_time: u64 },
    StopRampA { frozen_a: u64 },
    WithdrawAdminFees { amounts: [u128; 3] },
    SetPaused { paused: bool },
    SetSwapFee { fee_bps: u64 },
    SetAdminFee { fee_bps: u64 },
    AddAuthorizedBurnCaller { canister: Principal },
    RemoveAuthorizedBurnCaller { canister: Principal },
    FeeCurveParamsUpdated { old: Option<FeeCurveParams>, new: FeeCurveParams },
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct ThreePoolAdminEvent {
    pub id: u64,
    pub timestamp: u64,
    pub caller: Principal,
    pub action: ThreePoolAdminAction,
}

// ─── Pool Status (query response) ───

/// Snapshot of pool state returned by queries.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct PoolStatus {
    /// Current balances of each coin in the pool (in native token units).
    pub balances: [u128; 3],
    /// Total LP tokens in circulation.
    pub lp_total_supply: u128,
    /// Current effective amplification coefficient.
    pub current_a: u64,
    /// Virtual price of LP token (scaled by 1e18; LP token has 8 decimals).
    pub virtual_price: u128,
    /// Swap fee in basis points.
    pub swap_fee_bps: u64,
    /// Admin fee in basis points.
    pub admin_fee_bps: u64,
    /// Token configurations.
    pub tokens: [TokenConfig; 3],
}

// ─── Errors ───

/// Errors that can occur during 3pool operations.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub enum ThreePoolError {
    /// Output amount is below the caller's minimum.
    InsufficientOutput { expected_min: u128, actual: u128 },
    /// Not enough liquidity in the pool for this operation.
    InsufficientLiquidity,
    /// Coin index is out of range (must be 0, 1, or 2).
    InvalidCoinIndex,
    /// Amount must be greater than zero.
    ZeroAmount,
    /// Pool has no liquidity — cannot swap or remove.
    PoolEmpty,
    /// Slippage tolerance exceeded.
    SlippageExceeded,
    /// ICRC-1 ledger transfer failed.
    TransferFailed { token: String, reason: String },
    /// Caller is not authorized for this operation.
    Unauthorized,
    /// Arithmetic overflow in u256 math.
    MathOverflow,
    /// Newton's method did not converge when computing the invariant.
    InvariantNotConverged,
    /// Pool is paused by admin.
    PoolPaused,
    /// Caller is not in the authorized burn callers set.
    NotAuthorizedBurnCaller,
    /// LP/token ratio exceeds max slippage tolerance.
    BurnSlippageExceeded { max_bps: u16, actual_bps: u16 },
    /// Insufficient pool balance of the target token.
    InsufficientPoolBalance { token: String, required: u128, available: u128 },
    /// Insufficient LP balance for the caller.
    InsufficientLpBalance { required: u128, available: u128 },
    /// The token burn on the ledger failed.
    BurnFailed { token: String, reason: String },
}

// ─── Authorized Redeem-and-Burn Types ───

/// Arguments for the authorized redeem-and-burn operation.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct AuthorizedRedeemAndBurnArgs {
    /// Which token to remove from the pool and burn (by ledger principal).
    pub token_ledger: Principal,
    /// Amount of the token to remove and burn (native decimals).
    pub token_amount: u128,
    /// Amount of LP tokens to burn in exchange.
    pub lp_amount: u128,
    /// Maximum acceptable slippage from virtual price (basis points).
    pub max_slippage_bps: u16,
}

/// Result of a successful redeem-and-burn operation.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct RedeemAndBurnResult {
    /// Actual token amount burned on the ledger.
    pub token_amount_burned: u128,
    /// LP tokens burned.
    pub lp_amount_burned: u128,
    /// Block index from the token ledger burn call.
    pub burn_block_index: u64,
}

// ─── Dynamic Fee Curve ───

/// Parameters for the directional dynamic fee curve.
///
/// `imb_saturation` is in 1e9 fixed-point (so 1.0 = 1_000_000_000).
/// Imbalance metric values are in the same fixed-point representation.
#[derive(CandidType, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeeCurveParams {
    /// Minimum fee in basis points (charged on rebalancing trades).
    pub min_fee_bps: u16,
    /// Maximum fee in basis points (saturation cap on imbalancing trades).
    pub max_fee_bps: u16,
    /// Imbalance level (1e9 fixed-point) at which the imbalancing fee saturates to max.
    pub imb_saturation: u64,
}

// ─── Bot Query Endpoint Types ───

/// Result of a non-mutating swap quote. Mirrors `SwapOutcome` plus the
/// virtual-price impact (1e18 fixed-point delta) of the simulated trade.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct QuoteSwapResult {
    pub token_in: u8,
    pub token_out: u8,
    pub amount_in: u128,
    pub amount_out: u128,
    pub fee_native: u128,
    pub fee_bps: u16,
    pub imbalance_before: u64,
    pub imbalance_after: u64,
    pub is_rebalancing: bool,
    /// Virtual price (1e18 fp) before the simulated trade.
    pub virtual_price_before: u128,
    /// Virtual price (1e18 fp) after the simulated trade.
    pub virtual_price_after: u128,
}

/// Snapshot of the live pool state suitable for the rebalancing bot's
/// per-loop polling. All values are reads, no projections.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct PoolStateView {
    /// Native-decimal balances for each of the 3 coins.
    pub balances: [u128; 3],
    /// 18-decimal-normalized balances for each of the 3 coins.
    pub normalized_balances: [u128; 3],
    /// Current imbalance (1e9 fp).
    pub imbalance: u64,
    /// Current virtual price (1e18 fp). Zero if pool empty.
    pub virtual_price: u128,
    /// LP token total supply (8-decimal).
    pub lp_total_supply: u128,
    /// Live fee curve parameters.
    pub fee_curve: FeeCurveParams,
    /// Effective amplification coefficient at query time.
    pub amp: u64,
}

/// A bot-facing answer to "what is the most rebalancing dx I can push from
/// token i to token j right now?"
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct OptimalRebalanceQuote {
    pub token_in: u8,
    pub token_out: u8,
    /// Recommended dx (native units of token_in). Zero means no rebalancing
    /// trade exists in this direction.
    pub dx: u128,
    /// Expected output (native units of token_out) for that dx.
    pub amount_out: u128,
    /// Fee bps that would be charged.
    pub fee_bps: u16,
    /// Imbalance before (1e9 fp).
    pub imbalance_before: u64,
    /// Imbalance after the recommended trade (1e9 fp).
    pub imbalance_after: u64,
    /// Drop in imbalance (imb_before - imb_after), in 1e9 fp. The bot uses this
    /// as a profitability proxy.
    pub profit_bps_estimate: u64,
}

/// Imbalance snapshot derived from a recorded swap or liquidity event.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct ImbalanceSnapshot {
    pub timestamp: u64,
    pub imbalance_after: u64,
    pub virtual_price_after: u128,
    pub event_kind: ImbalanceEventKind,
}

#[derive(CandidType, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImbalanceEventKind {
    Swap,
    Liquidity,
}

// ─── Explorer Types ───

/// Time window selector used by aggregated/series explorer queries.
#[derive(CandidType, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StatsWindow {
    Last24h,
    Last7d,
    Last30d,
    AllTime,
}

/// Aggregated swap and liquidity stats over a window.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct PoolStats {
    pub swap_count: u64,
    pub swap_volume_per_token: [u128; 3],
    pub total_fees_collected: [u128; 3],
    pub unique_swappers: u64,
    pub liquidity_added_count: u64,
    pub liquidity_removed_count: u64,
    pub avg_fee_bps: u32,
    pub arb_swap_count: u64,
    pub arb_volume_per_token: [u128; 3],
}

/// Imbalance stats over a window with per-event samples.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct ImbalanceStats {
    pub current: u64,
    pub min: u64,
    pub max: u64,
    pub avg: u64,
    /// (timestamp_ns, imbalance_after_1e9fp) per swap in the window.
    pub samples: Vec<(u64, u64)>,
}

/// One bucket in the fee distribution.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct FeeBucket {
    pub min_bps: u16,
    pub max_bps: u16,
    pub swap_count: u64,
    pub volume_per_token: [u128; 3],
}

/// Fee bucket distribution and rebalancing share over a window.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct FeeStats {
    pub buckets: Vec<FeeBucket>,
    pub rebalancing_swap_count: u64,
    /// Rebalancing share in basis points (0..10000).
    pub rebalancing_swap_pct: u32,
}

/// Bucketed swap volume time-series point.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct VolumePoint {
    pub timestamp: u64,
    pub volume_per_token: [u128; 3],
}

/// Bucketed pool-balance time-series point (last balances seen in the bucket).
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct BalancePoint {
    pub timestamp: u64,
    pub balances: [u128; 3],
}

/// Bucketed virtual price time-series point.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct VirtualPricePoint {
    pub timestamp: u64,
    pub virtual_price: u128,
}

/// Bucketed average fee bps time-series point (volume-weighted).
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct FeePoint {
    pub timestamp: u64,
    pub avg_fee_bps: u32,
}

/// Snapshot of pool health for at-a-glance dashboards.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct PoolHealth {
    pub current_imbalance: u64,
    /// Signed delta vs the imbalance recorded ~1h ago (negative = improving).
    pub imbalance_trend_1h: i32,
    pub last_swap_age_seconds: u64,
    pub fee_at_min: u16,
    /// Fee bps that would be charged on a hypothetical worst-case imbalancing trade.
    pub fee_at_max_imbalance_swap: u16,
    /// 0..100 linear in current imbalance up to saturation.
    pub arb_opportunity_score: u8,
}

impl Default for FeeCurveParams {
    fn default() -> Self {
        Self {
            min_fee_bps: 1,
            max_fee_bps: 99,
            imb_saturation: 250_000_000, // 0.25 in 1e9 fixed-point
        }
    }
}
