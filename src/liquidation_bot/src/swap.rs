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

// swap_amounts returns: variant { Ok : SwapAmountsReply; Err : text }
#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum KongSwapAmountsResult {
    Ok(KongSwapAmountsReply),
    Err(String),
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct KongSwapAmountsReply {
    pub pay_chain: String,
    pub pay_symbol: String,
    pub pay_address: String,
    pub pay_amount: Nat,
    pub receive_chain: String,
    pub receive_symbol: String,
    pub receive_address: String,
    pub receive_amount: Nat,
    pub price: f64,
    pub mid_price: f64,
    pub slippage: f64,
}

// SwapArgs record for swap()
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct KongSwapArgs {
    pub pay_token: String,
    pub pay_amount: Nat,
    pub pay_tx_id: Option<KongTxId>,
    pub receive_token: String,
    pub receive_amount: Option<Nat>,
    pub receive_address: Option<String>,
    pub max_slippage: Option<f64>,
    pub referred_by: Option<String>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum KongTxId {
    BlockIndex(Nat),
    TransactionId(String),
}

// swap returns: variant { Ok : SwapReply; Err : text }
#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum KongSwapResult {
    Ok(KongSwapReply),
    Err(String),
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct KongSwapReply {
    pub tx_id: u64,
    pub request_id: u64,
    pub status: String,
    pub pay_chain: String,
    pub pay_address: String,
    pub pay_symbol: String,
    pub pay_amount: Nat,
    pub receive_chain: String,
    pub receive_address: String,
    pub receive_symbol: String,
    pub receive_amount: Nat,
    pub mid_price: f64,
    pub price: f64,
    pub slippage: f64,
}

/// Format a canister principal as KongSwap token identifier: "IC.<principal>"
fn kong_token_id(principal: Principal) -> String {
    format!("IC.{}", principal.to_text())
}

/// Query KongSwap for a quote: ICP → target stablecoin.
/// swap_amounts takes (text, nat, text) — three separate args.
async fn kong_get_quote(
    kong_principal: Principal,
    icp_ledger: Principal,
    target_ledger: Principal,
    amount_e8s: u64,
) -> Result<(u64, f64), SwapError> {
    let pay_token = kong_token_id(icp_ledger);
    let pay_amount = Nat::from(amount_e8s);
    let receive_token = kong_token_id(target_ledger);

    let result: Result<(KongSwapAmountsResult,), _> =
        ic_cdk::call(kong_principal, "swap_amounts", (&pay_token, &pay_amount, &receive_token)).await;

    match result {
        Ok((KongSwapAmountsResult::Ok(reply),)) => {
            let amount = nat_to_u64(&reply.receive_amount);
            Ok((amount, reply.price))
        }
        Ok((KongSwapAmountsResult::Err(e),)) => {
            Err(SwapError::DexCallFailed(e))
        }
        Err((code, msg)) => Err(SwapError::DexCallFailed(format!(
            "KongSwap call failed ({:?}): {}",
            code, msg
        ))),
    }
}

/// Execute a swap on KongSwap: ICP → target stablecoin.
/// Uses icrc2_approve + swap (KongSwap does icrc2_transfer_from).
async fn kong_execute_swap(
    kong_principal: Principal,
    icp_ledger: Principal,
    target_ledger: Principal,
    amount_e8s: u64,
    max_slippage_bps: u16,
) -> Result<(u64, f64), SwapError> {
    // Subtract ICP ledger fee for approve + transfer_from overhead
    let amount_e8s = amount_e8s.saturating_sub(20_000); // 0.0002 ICP buffer

    // First, approve KongSwap to spend our ICP
    let approve_args = ApproveArgs {
        from_subaccount: None,
        spender: Account {
            owner: kong_principal,
            subaccount: None,
        },
        amount: Nat::from(amount_e8s * 2), // 2x buffer for fees
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

    // KongSwap expects slippage as a percentage (e.g., 5.0 for 5%), not a fraction
    let slippage_pct = max_slippage_bps as f64 / 100.0;
    let args = KongSwapArgs {
        pay_token: kong_token_id(icp_ledger),
        pay_amount: Nat::from(amount_e8s),
        pay_tx_id: None,
        receive_token: kong_token_id(target_ledger),
        receive_amount: None,
        max_slippage: Some(slippage_pct),
        receive_address: None,
        referred_by: None,
    };

    let result: Result<(KongSwapResult,), _> =
        ic_cdk::call(kong_principal, "swap", (args,)).await;

    match result {
        Ok((KongSwapResult::Ok(reply),)) => {
            let amount = nat_to_u64(&reply.receive_amount);
            Ok((amount, reply.price))
        }
        Ok((KongSwapResult::Err(e),)) => {
            Err(SwapError::DexCallFailed(e))
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
            // Both are 6 decimals — compare directly
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

    // Subtract fee buffer: approve costs a fee, and transfer_from charges fee on top of amount
    // ckUSDC/ckUSDT fees are ~10 native units, use 1000 as safe buffer (~$0.001)
    let amount_native = amount_native.saturating_sub(1000);

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
    // ckUSDC/ckUSDT (6 decimals) → icUSD (8 decimals): multiply by 100
    let expected_e8s = amount_native as u128 * 100;
    let slippage = expected_e8s * config.max_slippage_bps as u128 / 10_000;
    let min_output_e8s = (expected_e8s - slippage) as u64;

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
    n.0.to_string().parse::<u64>().unwrap_or(0)
}
