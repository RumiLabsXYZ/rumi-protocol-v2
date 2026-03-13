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
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub enum Icrc3Transaction {
    Mint { to: Principal, amount: u128 },
    Burn { from: Principal, amount: u128 },
    Transfer { from: Principal, to: Principal, amount: u128, spender: Option<Principal> },
    Approve { from: Principal, spender: Principal, amount: u128, expires_at: Option<u64> },
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
}
