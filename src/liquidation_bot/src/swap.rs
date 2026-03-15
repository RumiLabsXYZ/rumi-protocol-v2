use candid::{CandidType, Deserialize, Nat, Principal};
use ic_canister_log::log;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc2::approve::{ApproveArgs, ApproveError};

use crate::state::BotConfig;

#[derive(Debug)]
pub struct SwapResult {
    pub output_amount: u64,
    pub route: String,
    pub target_token: Principal,
}

#[derive(Debug)]
pub enum SwapError {
    SlippageExceeded { expected: u64, actual: u64 },
    DexCallFailed(String),
    InsufficientLiquidity,
    ApproveFailed(String),
}

impl std::fmt::Display for SwapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SwapError::SlippageExceeded { expected, actual } => {
                write!(f, "Slippage exceeded: expected {} got {}", expected, actual)
            }
            SwapError::DexCallFailed(msg) => write!(f, "DEX call failed: {}", msg),
            SwapError::InsufficientLiquidity => write!(f, "Insufficient liquidity"),
            SwapError::ApproveFailed(msg) => write!(f, "Approve failed: {}", msg),
        }
    }
}

// ─── KongSwap Types ───

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct KongSwapArgs {
    pub pay_token: String,
    pub pay_amount: Nat,
    pub receive_token: String,
    pub receive_amount: Option<Nat>,
    pub max_slippage: Option<f64>,
    pub receive_address: Option<String>,
    pub referred_by: Option<String>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct KongSwapAmountsResult {
    #[serde(rename = "Ok")]
    pub ok: Option<KongSwapAmountsReply>,
    #[serde(rename = "Err")]
    pub err: Option<String>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct KongSwapAmountsReply {
    pub pay_amount: Nat,
    pub receive_amount: Nat,
    pub price: f64,
    pub mid_price: f64,
    pub slippage: f64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct KongSwapResult {
    #[serde(rename = "Ok")]
    pub ok: Option<KongSwapReply>,
    #[serde(rename = "Err")]
    pub err: Option<String>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct KongSwapReply {
    pub request_id: u64,
    pub status: String,
    pub pay_amount: Nat,
    pub receive_amount: Nat,
    pub price: f64,
    pub mid_price: f64,
    pub slippage: f64,
}

/// Query KongSwap for a quote: ICP → target stablecoin.
async fn kong_get_quote(
    kong_principal: Principal,
    icp_ledger: Principal,
    target_ledger: Principal,
    amount_e8s: u64,
) -> Result<(u64, f64), SwapError> {
    let args = KongSwapArgs {
        pay_token: icp_ledger.to_text(),
        pay_amount: Nat::from(amount_e8s),
        receive_token: target_ledger.to_text(),
        receive_amount: None,
        max_slippage: None,
        receive_address: None,
        referred_by: None,
    };

    let result: Result<(KongSwapAmountsResult,), _> =
        ic_cdk::call(kong_principal, "swap_amounts", (args,)).await;

    match result {
        Ok((r,)) => {
            if let Some(reply) = r.ok {
                let amount = nat_to_u64(&reply.receive_amount);
                Ok((amount, reply.price))
            } else {
                Err(SwapError::DexCallFailed(
                    r.err.unwrap_or_else(|| "Unknown KongSwap error".to_string()),
                ))
            }
        }
        Err((code, msg)) => Err(SwapError::DexCallFailed(format!(
            "KongSwap call failed ({:?}): {}",
            code, msg
        ))),
    }
}

/// Execute a swap on KongSwap: ICP → target stablecoin.
async fn kong_execute_swap(
    kong_principal: Principal,
    icp_ledger: Principal,
    target_ledger: Principal,
    amount_e8s: u64,
    max_slippage_bps: u16,
) -> Result<(u64, f64), SwapError> {
    // First, approve KongSwap to spend our ICP
    let approve_args = ApproveArgs {
        from_subaccount: None,
        spender: Account {
            owner: kong_principal,
            subaccount: None,
        },
        amount: Nat::from(amount_e8s * 2), // 2x buffer
        expected_allowance: None,
        expires_at: Some(ic_cdk::api::time() + 300_000_000_000), // 5 min
        fee: None,
        memo: None,
        created_at_time: None,
    };

    let approve_result: Result<(Result<Nat, ApproveError>,), _> =
        ic_cdk::call(icp_ledger, "icrc2_approve", (approve_args,)).await;

    match approve_result {
        Ok((Ok(_),)) => {}
        Ok((Err(e),)) => {
            return Err(SwapError::ApproveFailed(format!("{:?}", e)));
        }
        Err((code, msg)) => {
            return Err(SwapError::ApproveFailed(format!("{:?}: {}", code, msg)));
        }
    }

    let slippage_pct = max_slippage_bps as f64 / 10_000.0;
    let args = KongSwapArgs {
        pay_token: icp_ledger.to_text(),
        pay_amount: Nat::from(amount_e8s),
        receive_token: target_ledger.to_text(),
        receive_amount: None,
        max_slippage: Some(slippage_pct),
        receive_address: None,
        referred_by: None,
    };

    let result: Result<(KongSwapResult,), _> =
        ic_cdk::call(kong_principal, "swap", (args,)).await;

    match result {
        Ok((r,)) => {
            if let Some(reply) = r.ok {
                let amount = nat_to_u64(&reply.receive_amount);
                Ok((amount, reply.price))
            } else {
                Err(SwapError::DexCallFailed(
                    r.err.unwrap_or_else(|| "Unknown KongSwap error".to_string()),
                ))
            }
        }
        Err((code, msg)) => Err(SwapError::DexCallFailed(format!(
            "KongSwap swap call failed ({:?}): {}",
            code, msg
        ))),
    }
}

/// Swap ICP for the best-rate stablecoin (ckUSDC or ckUSDT) on KongSwap.
pub async fn swap_icp_for_stable(
    config: &BotConfig,
    amount_e8s: u64,
) -> Result<SwapResult, SwapError> {
    // Query both ckUSDC and ckUSDT quotes in parallel
    let usdc_quote = kong_get_quote(
        config.kong_swap_principal,
        config.icp_ledger,
        config.ckusdc_ledger,
        amount_e8s,
    );
    let usdt_quote = kong_get_quote(
        config.kong_swap_principal,
        config.icp_ledger,
        config.ckusdt_ledger,
        amount_e8s,
    );

    let (usdc_result, usdt_result) = futures::future::join(usdc_quote, usdt_quote).await;

    // Pick the better rate (higher output amount)
    let (target_ledger, target_name, _quote_amount) = match (usdc_result, usdt_result) {
        (Ok((usdc_amt, _)), Ok((usdt_amt, _))) => {
            // Normalize: ckUSDC is 6 decimals, ckUSDT is 6 decimals — compare directly
            if usdc_amt >= usdt_amt {
                log!(crate::INFO, "Best rate: ckUSDC ({} vs {} ckUSDT)", usdc_amt, usdt_amt);
                (config.ckusdc_ledger, "ckUSDC", usdc_amt)
            } else {
                log!(crate::INFO, "Best rate: ckUSDT ({} vs {} ckUSDC)", usdt_amt, usdc_amt);
                (config.ckusdt_ledger, "ckUSDT", usdt_amt)
            }
        }
        (Ok((usdc_amt, _)), Err(e)) => {
            log!(crate::INFO, "ckUSDT quote failed ({}), using ckUSDC", e);
            (config.ckusdc_ledger, "ckUSDC", usdc_amt)
        }
        (Err(e), Ok((usdt_amt, _))) => {
            log!(crate::INFO, "ckUSDC quote failed ({}), using ckUSDT", e);
            (config.ckusdt_ledger, "ckUSDT", usdt_amt)
        }
        (Err(e1), Err(e2)) => {
            return Err(SwapError::DexCallFailed(format!(
                "Both quotes failed: ckUSDC={}, ckUSDT={}",
                e1, e2
            )));
        }
    };

    // Execute the swap with the better rate
    let (output_amount, _price) = kong_execute_swap(
        config.kong_swap_principal,
        config.icp_ledger,
        target_ledger,
        amount_e8s,
        config.max_slippage_bps,
    )
    .await?;

    Ok(SwapResult {
        output_amount,
        route: format!("ICP→{}→icUSD", target_name),
        target_token: target_ledger,
    })
}

/// Swap a stablecoin (ckUSDC or ckUSDT) for icUSD via our 3pool.
/// 3pool token indices: 0=icUSD, 1=ckUSDT, 2=ckUSDC
pub async fn swap_stable_for_icusd(
    config: &BotConfig,
    amount_native: u64,
    stable_token: Principal,
) -> Result<u64, SwapError> {
    let token_index: u8 = if stable_token == config.ckusdc_ledger {
        2
    } else if stable_token == config.ckusdt_ledger {
        1
    } else {
        return Err(SwapError::DexCallFailed(
            "Unknown stablecoin for 3pool swap".to_string(),
        ));
    };

    // Approve 3pool to spend our stablecoin
    let approve_args = ApproveArgs {
        from_subaccount: None,
        spender: Account {
            owner: config.three_pool_principal,
            subaccount: None,
        },
        amount: Nat::from(amount_native * 2),
        expected_allowance: None,
        expires_at: Some(ic_cdk::api::time() + 300_000_000_000),
        fee: None,
        memo: None,
        created_at_time: None,
    };

    let approve_result: Result<(Result<Nat, ApproveError>,), _> =
        ic_cdk::call(stable_token, "icrc2_approve", (approve_args,)).await;

    match approve_result {
        Ok((Ok(_),)) => {}
        Ok((Err(e),)) => {
            return Err(SwapError::ApproveFailed(format!("3pool approve: {:?}", e)));
        }
        Err((code, msg)) => {
            return Err(SwapError::ApproveFailed(format!(
                "3pool approve call: {:?} {}",
                code, msg
            )));
        }
    }

    // Calculate minimum output with slippage tolerance
    // For stablecoins, 1:1 minus slippage is a reasonable min
    let min_output_e8s = if token_index == 2 {
        // ckUSDC (6 decimals) → icUSD (8 decimals): multiply by 100
        let expected_e8s = amount_native as u128 * 100;
        let slippage = expected_e8s * config.max_slippage_bps as u128 / 10_000;
        (expected_e8s - slippage) as u64
    } else {
        // ckUSDT (6 decimals) → icUSD (8 decimals): multiply by 100
        let expected_e8s = amount_native as u128 * 100;
        let slippage = expected_e8s * config.max_slippage_bps as u128 / 10_000;
        (expected_e8s - slippage) as u64
    };

    // 3pool swap: swap(from_index, to_index, dx, min_dy)
    #[derive(CandidType, Deserialize, Debug)]
    enum ThreePoolResult {
        Ok(Nat),
        Err(ThreePoolError),
    }

    #[derive(CandidType, Deserialize, Debug)]
    enum ThreePoolError {
        InsufficientOutput {
            expected_min: Nat,
            actual: Nat,
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
    }

    let result: Result<(ThreePoolResult,), _> = ic_cdk::call(
        config.three_pool_principal,
        "swap",
        (
            token_index,
            0u8, // to icUSD (index 0)
            Nat::from(amount_native),
            Nat::from(min_output_e8s),
        ),
    )
    .await;

    match result {
        Ok((ThreePoolResult::Ok(amount),)) => Ok(nat_to_u64(&amount)),
        Ok((ThreePoolResult::Err(e),)) => Err(SwapError::DexCallFailed(format!(
            "3pool swap error: {:?}",
            e
        ))),
        Err((code, msg)) => Err(SwapError::DexCallFailed(format!(
            "3pool call failed ({:?}): {}",
            code, msg
        ))),
    }
}

fn nat_to_u64(n: &Nat) -> u64 {
    use candid::utils::encode_one;
    // Nat → u64 conversion
    n.0.to_string().parse::<u64>().unwrap_or(0)
}
