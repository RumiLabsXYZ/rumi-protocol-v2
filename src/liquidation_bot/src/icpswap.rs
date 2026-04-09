use candid::{CandidType, Deserialize, Nat, Principal};

// -- ICPSwap Types --

/// Args for `depositFromAndSwap` (5 fields).
#[derive(CandidType, Clone, Debug, serde::Serialize)]
pub struct DepositAndSwapArgs {
    #[serde(rename = "amountIn")]
    pub amount_in: String,
    #[serde(rename = "amountOutMinimum")]
    pub amount_out_minimum: String,
    #[serde(rename = "zeroForOne")]
    pub zero_for_one: bool,
    #[serde(rename = "tokenInFee")]
    pub token_in_fee: Nat,
    #[serde(rename = "tokenOutFee")]
    pub token_out_fee: Nat,
}

/// Args for `quote` (3 fields, different from DepositAndSwapArgs).
#[derive(CandidType, Clone, Debug, serde::Serialize)]
pub struct SwapArgs {
    #[serde(rename = "amountIn")]
    pub amount_in: String,
    #[serde(rename = "amountOutMinimum")]
    pub amount_out_minimum: String,
    #[serde(rename = "zeroForOne")]
    pub zero_for_one: bool,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum IcpSwapError {
    CommonError,
    InsufficientFunds,
    InternalError(String),
    UnsupportedToken(String),
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum IcpSwapResult {
    #[serde(rename = "ok")]
    Ok(Nat),
    #[serde(rename = "err")]
    Err(IcpSwapError),
}

impl std::fmt::Display for IcpSwapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IcpSwapError::CommonError => write!(f, "CommonError"),
            IcpSwapError::InsufficientFunds => write!(f, "InsufficientFunds"),
            IcpSwapError::InternalError(s) => write!(f, "InternalError: {}", s),
            IcpSwapError::UnsupportedToken(s) => write!(f, "UnsupportedToken: {}", s),
        }
    }
}

// -- Pool metadata --

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct Token {
    pub address: String,
    pub standard: String,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct PoolMetadata {
    pub fee: Nat,
    pub key: String,
    pub liquidity: Nat,
    #[serde(rename = "maxLiquidityPerTick")]
    pub max_liquidity_per_tick: Nat,
    #[serde(rename = "nextPositionId")]
    pub next_position_id: Nat,
    #[serde(rename = "sqrtPriceX96")]
    pub sqrt_price_x96: Nat,
    pub tick: candid::Int,
    pub token0: Token,
    pub token1: Token,
}

/// metadata() returns variant { ok: PoolMetadata; err: Error }
#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum MetadataResult {
    #[serde(rename = "ok")]
    Ok(PoolMetadata),
    #[serde(rename = "err")]
    Err(IcpSwapError),
}

// -- Call wrappers --

/// Query a swap quote. Uses 3-field SwapArgs.
pub async fn quote(
    pool: Principal,
    amount_in: u64,
    zero_for_one: bool,
) -> Result<u64, String> {
    let args = SwapArgs {
        amount_in: amount_in.to_string(),
        amount_out_minimum: "0".to_string(),
        zero_for_one,
    };

    let result: Result<(IcpSwapResult,), _> = ic_cdk::call(pool, "quote", (args,)).await;

    match result {
        Ok((IcpSwapResult::Ok(n),)) => Ok(nat_to_u64(&n)),
        Ok((IcpSwapResult::Err(e),)) => Err(format!("ICPSwap quote error: {}", e)),
        Err((code, msg)) => Err(format!("ICPSwap quote call failed ({:?}): {}", code, msg)),
    }
}

/// Execute a swap via depositFromAndSwap. Uses 5-field DepositAndSwapArgs.
pub async fn deposit_and_swap(
    pool: Principal,
    amount_in: u64,
    min_amount_out: u64,
    zero_for_one: bool,
    token_in_fee: u64,
    token_out_fee: u64,
) -> Result<u64, String> {
    let args = DepositAndSwapArgs {
        amount_in: amount_in.to_string(),
        amount_out_minimum: min_amount_out.to_string(),
        zero_for_one,
        token_in_fee: Nat::from(token_in_fee),
        token_out_fee: Nat::from(token_out_fee),
    };

    let result: Result<(IcpSwapResult,), _> =
        ic_cdk::call(pool, "depositFromAndSwap", (args,)).await;

    match result {
        Ok((IcpSwapResult::Ok(n),)) => Ok(nat_to_u64(&n)),
        Ok((IcpSwapResult::Err(e),)) => Err(format!("ICPSwap swap error: {}", e)),
        Err((code, msg)) => Err(format!("ICPSwap swap call failed ({:?}): {}", code, msg)),
    }
}

/// Fetch pool metadata to determine token ordering.
pub async fn fetch_metadata(pool: Principal) -> Result<PoolMetadata, String> {
    let result: Result<(MetadataResult,), _> = ic_cdk::call(pool, "metadata", ()).await;

    match result {
        Ok((MetadataResult::Ok(m),)) => Ok(m),
        Ok((MetadataResult::Err(e),)) => Err(format!("ICPSwap metadata error: {}", e)),
        Err((code, msg)) => Err(format!("ICPSwap metadata call failed ({:?}): {}", code, msg)),
    }
}

fn nat_to_u64(n: &Nat) -> u64 {
    n.0.to_string().parse::<u64>().unwrap_or(0)
}
