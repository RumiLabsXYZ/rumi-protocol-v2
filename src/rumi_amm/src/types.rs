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
