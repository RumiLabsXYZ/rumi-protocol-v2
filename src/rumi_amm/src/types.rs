use candid::{CandidType, Principal};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ─── Pool Identifier ───

pub type PoolId = String; // e.g., "3USD_ICP"

// ─── Curve Type ───

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum CurveType {
    ConstantProduct,
    // Future: StableSwap { amp: u64 },
    // Future: Weighted { weight_a: u64, weight_b: u64 },
}

// ─── Pool ───

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct Pool {
    pub token_a: Principal,
    pub token_b: Principal,
    pub reserve_a: u128,
    pub reserve_b: u128,
    pub fee_bps: u16,
    pub protocol_fee_bps: u16,
    pub curve: CurveType,
    pub lp_shares: BTreeMap<Principal, u128>,
    pub total_lp_shares: u128,
    pub protocol_fees_a: u128,
    pub protocol_fees_b: u128,
    pub paused: bool,
    pub subaccount_a: [u8; 32],
    pub subaccount_b: [u8; 32],

    // ─── Reward distribution (added 2026-05-09) ───
    /// Per-LP reward bookkeeping (parallel to `lp_shares`).
    #[serde(default)]
    pub lp_rewards: BTreeMap<Principal, RewardState>,
    /// Accumulator: total rewards distributed per share, scaled by `REWARD_SCALE` (1e12).
    #[serde(default)]
    pub acc_reward_per_share: u128,
    /// Buffer for donations that arrive while `total_lp_shares == 0`.
    /// Drained on the next `add_liquidity` that produces positive shares.
    #[serde(default)]
    pub pending_no_lp: u128,
    /// Lifetime sum of rewards distributed to this pool. Analytics only.
    #[serde(default)]
    pub total_rewards_distributed: u128,
    /// Recently processed donation nonces (ring buffer, oldest first).
    /// Bounded by `MAX_PROCESSED_NONCES` to prevent unbounded growth.
    #[serde(default)]
    pub processed_donation_nonces: std::collections::VecDeque<u64>,
    /// Last verified on-chain icUSD balance held in the per-pool reward
    /// subaccount. Used by `notify_reward_received` to verify the
    /// donation amount actually arrived. After a successful notify, this
    /// is set to the live `icrc1_balance_of` reading (capturing any
    /// over-funding rather than just `+= amount`). After a successful
    /// claim, it is decremented by the amount transferred out.
    #[serde(default)]
    pub reward_balance_snapshot: u128,
}

// ─── Init Args ───

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct AmmInitArgs {
    pub admin: Principal,
}

// ─── Candid-facing types ───

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct PoolInfo {
    pub pool_id: PoolId,
    pub token_a: Principal,
    pub token_b: Principal,
    pub reserve_a: u128,
    pub reserve_b: u128,
    pub fee_bps: u16,
    pub protocol_fee_bps: u16,
    pub curve: CurveType,
    pub total_lp_shares: u128,
    pub paused: bool,
}

impl Pool {
    pub fn to_info(&self, pool_id: &str) -> PoolInfo {
        PoolInfo {
            pool_id: pool_id.to_string(),
            token_a: self.token_a,
            token_b: self.token_b,
            reserve_a: self.reserve_a,
            reserve_b: self.reserve_b,
            fee_bps: self.fee_bps,
            protocol_fee_bps: self.protocol_fee_bps,
            curve: self.curve.clone(),
            total_lp_shares: self.total_lp_shares,
            paused: self.paused,
        }
    }
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct SwapResult {
    pub amount_out: u128,
    pub fee: u128,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct CreatePoolArgs {
    pub token_a: Principal,
    pub token_b: Principal,
    pub fee_bps: u16,
    pub curve: CurveType,
}

// ─── Pending Claims ───

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct PendingClaim {
    pub id: u64,
    pub pool_id: PoolId,
    pub claimant: Principal,
    pub token: Principal,
    pub subaccount: [u8; 32],
    pub amount: u128,
    pub reason: String,
    pub created_at: u64,
}

// ─── Swap Events ───

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct AmmSwapEvent {
    pub id: u64,
    pub caller: Principal,
    pub pool_id: PoolId,
    pub token_in: Principal,
    pub amount_in: u128,
    pub token_out: Principal,
    pub amount_out: u128,
    pub fee: u128,
    pub timestamp: u64,
}

// ─── Liquidity Events ───

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub enum AmmLiquidityAction {
    AddLiquidity,
    RemoveLiquidity,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct AmmLiquidityEvent {
    pub id: u64,
    pub caller: Principal,
    pub pool_id: PoolId,
    pub action: AmmLiquidityAction,
    pub token_a: Principal,
    pub amount_a: u128,
    pub token_b: Principal,
    pub amount_b: u128,
    pub lp_shares: u128,
    pub timestamp: u64,
}

// ─── Admin Events ───

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub enum AmmAdminAction {
    CreatePool { pool_id: PoolId, token_a: Principal, token_b: Principal, fee_bps: u16 },
    SetFee { pool_id: PoolId, fee_bps: u16 },
    SetProtocolFee { pool_id: PoolId, protocol_fee_bps: u16 },
    WithdrawProtocolFees { pool_id: PoolId, amount_a: u128, amount_b: u128 },
    PausePool { pool_id: PoolId },
    UnpausePool { pool_id: PoolId },
    SetPoolCreationOpen { open: bool },
    SetMaintenanceMode { enabled: bool },
    ClaimPending { claim_id: u64, claimant: Principal, amount: u128 },
    ResolvePendingClaim { claim_id: u64 },
    SetProtocolBackendPrincipal { backend: Principal },
    AdminBurnSubaccount {
        ledger: Principal,
        subaccount_hex: String,
        amount_burned: u128,
        block_index: u64,
    },
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct AmmAdminEvent {
    pub id: u64,
    pub caller: Principal,
    pub action: AmmAdminAction,
    pub timestamp: u64,
}

// ─── Holder Snapshots ───

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct HolderEntry {
    pub holder: Principal,
    pub balance: u128,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct HolderSnapshot {
    pub token: String,           // "icUSD" or "3USD"
    pub timestamp: u64,          // nanoseconds
    pub holder_count: u64,
    pub total_supply: u128,
    pub top_holders: Vec<HolderEntry>, // top 50
}

// ─── Analytics: windowed stats, time series, rankings ───

#[derive(CandidType, Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum AmmStatsWindow {
    Hour,
    Day,
    Week,
    Month,
    All,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct AmmSeriesQuery {
    pub pool: PoolId,
    pub window: AmmStatsWindow,
    pub points: u32,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct AmmStatsQuery {
    pub pool: PoolId,
    pub window: AmmStatsWindow,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct AmmTopSwappersQuery {
    pub pool: PoolId,
    pub window: AmmStatsWindow,
    pub limit: u32,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct AmmTopLpsQuery {
    pub pool: PoolId,
    pub limit: u32,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct AmmEventsByPrincipalQuery {
    pub pool: PoolId,
    pub who: Principal,
    pub start: u64,
    pub length: u64,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct AmmEventsByTimeRangeQuery {
    pub pool: PoolId,
    pub start_ns: u64,
    pub end_ns: u64,
    pub limit: u64,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct AmmVolumePoint {
    pub ts_ns: u64,
    pub volume_a_e8s: u128,
    pub volume_b_e8s: u128,
    pub swap_count: u32,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct AmmBalancePoint {
    pub ts_ns: u64,
    pub reserve_a_e8s: u128,
    pub reserve_b_e8s: u128,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct AmmFeePoint {
    pub ts_ns: u64,
    pub fees_a_e8s: u128,
    pub fees_b_e8s: u128,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct AmmPoolStats {
    pub pool: PoolId,
    pub window: AmmStatsWindow,
    pub volume_a_e8s: u128,
    pub volume_b_e8s: u128,
    pub fees_a_e8s: u128,
    pub fees_b_e8s: u128,
    pub swap_count: u32,
    pub unique_swappers: u32,
    pub unique_lps: u32,
    pub generated_at_ns: u64,
}

// ─── Errors ───

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub enum AmmError {
    PoolNotFound,
    PoolAlreadyExists,
    PoolPaused,
    ZeroAmount,
    InsufficientOutput { expected_min: u128, actual: u128 },
    InsufficientLiquidity,
    InsufficientLpShares { required: u128, available: u128 },
    InvalidToken,
    TransferFailed { token: String, reason: String },
    Unauthorized,
    MathOverflow,
    DisproportionateLiquidity,
    PoolCreationClosed,
    FeeBpsOutOfRange,
    MaintenanceMode,
    ClaimNotFound,
    PoolBusy,
    DuplicateNonce,
    NoLiquidity,
    BelowMinClaim { claimable: u128, min: u128 },
    RewardLedgerTransferFailed { reason: String },
    InsufficientOnChainBalance { expected: u128, actual: u128 },
    InvalidInput { reason: String },
}

// ─── Reward state (per LP) ───

/// Per-LP reward bookkeeping. Lives in a parallel map alongside `lp_shares`.
/// `reward_debt` is `shares * acc_reward_per_share / 1e12` at last settle.
/// `claimable` accumulates settled rewards that the LP has not yet claimed,
/// and persists across share changes (including full removal).
#[derive(CandidType, Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RewardState {
    pub reward_debt: u128,
    pub claimable: u128,
}

// ─── Reward events ───

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct AmmRewardEvent {
    pub id: u64,
    pub pool_id: PoolId,
    pub amount: u128,
    pub total_shares_at_time: u128,
    pub nonce: u64,
    pub timestamp: u64,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct AmmClaimEvent {
    pub id: u64,
    pub pool_id: PoolId,
    pub claimant: Principal,
    pub amount: u128,
    pub timestamp: u64,
}

// ─── TVL sampling (added 2026-05-09) ───

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct TvlSample {
    pub pool_id: PoolId,
    pub timestamp: u64,
    pub reserve_a: u128,
    pub reserve_b: u128,
    /// USD per token_a in e8s (3USD assumed at 1.00).
    pub price_a_e8s: u128,
    /// USD per token_b in e8s.
    pub price_b_e8s: u128,
    /// Computed: reserve_a * price_a_e8s / 1e8 + reserve_b * price_b_e8s / 1e8.
    pub tvl_usd_e8s: u128,
}

/// Lightweight shape for the cross-canister call to rumi_protocol_backend's
/// `get_icp_usd_price_e8s` query. Only the price field is needed.
#[derive(CandidType, Clone, Debug, Deserialize)]
pub struct ProtocolStatusLite {
    pub price_e8s: u128,
}
