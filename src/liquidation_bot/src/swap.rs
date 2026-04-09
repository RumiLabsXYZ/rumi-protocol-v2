use candid::{Nat, Principal};
use ic_canister_log::log;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc2::approve::ApproveArgs;

use crate::icpswap;
use crate::state::BotConfig;

pub struct SwapResult {
    pub ckusdc_received_e6: u64,
    pub effective_price_e8s: u64,
}

/// One-time infinite ICRC-2 approve. Amount = u128::MAX, no expiry.
pub async fn approve_infinite(
    token_ledger: Principal,
    spender: Principal,
) -> Result<(), String> {
    let args = ApproveArgs {
        from_subaccount: None,
        spender: Account {
            owner: spender,
            subaccount: None,
        },
        amount: Nat::from(u128::MAX),
        expected_allowance: None,
        expires_at: None,
        fee: None,
        memo: None,
        created_at_time: None,
    };

    let result: Result<
        (Result<Nat, icrc_ledger_types::icrc2::approve::ApproveError>,),
        _,
    > = ic_cdk::call(token_ledger, "icrc2_approve", (args,)).await;

    match result {
        Ok((Ok(_),)) => Ok(()),
        Ok((Err(e),)) => Err(format!("Approve failed: {:?}", e)),
        Err((code, msg)) => Err(format!("Approve call failed: {:?} {}", code, msg)),
    }
}

/// Quote how much ckUSDC we'd get for `icp_amount_e8s` ICP.
pub async fn quote_icp_for_ckusdc(config: &BotConfig, icp_amount_e8s: u64) -> Result<u64, String> {
    let zero_for_one = config
        .icpswap_zero_for_one
        .ok_or("Pool ordering not configured. Call admin_resolve_pool_ordering first.")?;

    icpswap::quote(config.icpswap_pool, icp_amount_e8s, zero_for_one).await
}

/// Swap ICP for ckUSDC on ICPSwap.
/// Flow: get quote -> apply slippage -> depositFromAndSwap.
/// Requires infinite approve to already be in place.
pub async fn swap_icp_for_ckusdc(
    config: &BotConfig,
    icp_amount_e8s: u64,
) -> Result<SwapResult, String> {
    let zero_for_one = config
        .icpswap_zero_for_one
        .ok_or("Pool ordering not configured. Call admin_resolve_pool_ordering first.")?;

    let quoted_output =
        icpswap::quote(config.icpswap_pool, icp_amount_e8s, zero_for_one).await?;

    if quoted_output == 0 {
        return Err("Quote returned zero output".to_string());
    }

    let min_output = apply_slippage(quoted_output, config.max_slippage_bps);

    log!(
        crate::INFO,
        "ICPSwap quote: {} ICP e8s -> {} ckUSDC e6 (min: {})",
        icp_amount_e8s,
        quoted_output,
        min_output
    );

    let icp_fee = config.icp_fee_e8s.unwrap_or(10_000);
    let ckusdc_fee = config.ckusdc_fee_e6.unwrap_or(10);

    let received = icpswap::deposit_and_swap(
        config.icpswap_pool,
        icp_amount_e8s,
        min_output,
        zero_for_one,
        icp_fee,
        ckusdc_fee,
    )
    .await?;

    // Effective price in e8 format: (ckusdc_e6 / icp_e8s) * 1e8
    // = ckusdc_e6 * 1e2 * 1e8 / icp_e8s = ckusdc_e6 * 10_000_000_000 / icp_e8s
    let effective_price_e8s = if icp_amount_e8s > 0 {
        (received as u128 * 10_000_000_000 / icp_amount_e8s as u128) as u64
    } else {
        0
    };

    log!(
        crate::INFO,
        "ICPSwap swap complete: {} ckUSDC e6 received, effective price {} e8s",
        received,
        effective_price_e8s
    );

    Ok(SwapResult {
        ckusdc_received_e6: received,
        effective_price_e8s,
    })
}

fn apply_slippage(amount: u64, max_slippage_bps: u16) -> u64 {
    let reduction = amount as u128 * max_slippage_bps as u128 / 10_000;
    (amount as u128 - reduction) as u64
}

/// Transfer collateral (ICP) back to the backend canister.
pub async fn return_collateral_to_backend(
    config: &BotConfig,
    amount_e8s: u64,
    collateral_ledger: Principal,
) -> Result<(), String> {
    let fee = config.icp_fee_e8s.unwrap_or(10_000);
    let send_amount = amount_e8s.saturating_sub(fee);
    if send_amount == 0 {
        return Err("Collateral amount too small to cover transfer fee".to_string());
    }

    let transfer_args = icrc_ledger_types::icrc1::transfer::TransferArg {
        from_subaccount: None,
        to: Account {
            owner: config.backend_principal,
            subaccount: None,
        },
        amount: Nat::from(send_amount),
        fee: None,
        memo: None,
        created_at_time: Some(ic_cdk::api::time()),
    };

    let result: Result<
        (Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError>,),
        _,
    > = ic_cdk::call(collateral_ledger, "icrc1_transfer", (transfer_args,)).await;

    match result {
        Ok((Ok(_),)) => Ok(()),
        Ok((Err(e),)) => Err(format!("Transfer error: {:?}", e)),
        Err((code, msg)) => Err(format!("Transfer call failed: {:?} {}", code, msg)),
    }
}

/// Transfer ckUSDC from bot to backend canister.
/// Returns the actual amount received by the backend (after fee subtraction).
pub async fn transfer_ckusdc_to_backend(
    config: &BotConfig,
    amount_e6: u64,
) -> Result<u64, String> {
    let fee = config.ckusdc_fee_e6.unwrap_or(10);
    let send_amount = amount_e6.saturating_sub(fee);
    if send_amount == 0 {
        return Err("ckUSDC amount too small to cover transfer fee".to_string());
    }

    let transfer_args = icrc_ledger_types::icrc1::transfer::TransferArg {
        from_subaccount: None,
        to: Account {
            owner: config.backend_principal,
            subaccount: None,
        },
        amount: Nat::from(send_amount),
        fee: None,
        memo: None,
        created_at_time: Some(ic_cdk::api::time()),
    };

    let result: Result<
        (Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError>,),
        _,
    > = ic_cdk::call(config.ckusdc_ledger, "icrc1_transfer", (transfer_args,)).await;

    match result {
        Ok((Ok(_),)) => Ok(send_amount),
        Ok((Err(e),)) => Err(format!("ckUSDC transfer error: {:?}", e)),
        Err((code, msg)) => Err(format!("ckUSDC transfer call failed: {:?} {}", code, msg)),
    }
}

/// Transfer ICP to treasury (liquidation bonus).
pub async fn transfer_icp_to_treasury(
    config: &BotConfig,
    amount_e8s: u64,
) -> Result<(), String> {
    let fee = config.icp_fee_e8s.unwrap_or(10_000);
    let send_amount = amount_e8s.saturating_sub(fee);
    if send_amount == 0 {
        return Ok(());
    }

    let transfer_args = icrc_ledger_types::icrc1::transfer::TransferArg {
        from_subaccount: None,
        to: Account {
            owner: config.treasury_principal,
            subaccount: None,
        },
        amount: Nat::from(send_amount),
        fee: None,
        memo: None,
        created_at_time: Some(ic_cdk::api::time()),
    };

    let result: Result<
        (Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError>,),
        _,
    > = ic_cdk::call(config.icp_ledger, "icrc1_transfer", (transfer_args,)).await;

    match result {
        Ok((Ok(_),)) => {
            log!(crate::INFO, "Transferred {} e8s ICP to treasury", send_amount);
            Ok(())
        }
        Ok((Err(e),)) => Err(format!("ICP transfer to treasury failed: {:?}", e)),
        Err((code, msg)) => Err(format!("ICP transfer call failed: {:?} {}", code, msg)),
    }
}
