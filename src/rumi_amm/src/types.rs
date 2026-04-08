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
}
