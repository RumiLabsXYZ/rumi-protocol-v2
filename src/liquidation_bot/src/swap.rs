use candid::{CandidType, Nat, Principal};
use ic_canister_log::log;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc2::approve::ApproveArgs;
use serde::{Deserialize, Serialize};

use crate::icpswap;
use crate::state::BotConfig;

/// Belt-and-suspenders default when `BotConfig.*_fee` is unset. The real fee
/// for ICP and ckUSDC is 10_000 in their respective base units (e8s for ICP
/// and e6 for ckUSDC, since ckUSDC has 6 decimals and a fee of $0.01).
/// Previously this default was 10 for ckUSDC, which is what blew up every
/// ICPSwap swap with "Wrong fee cache (expected: 10000, received: 10)".
const FALLBACK_LEDGER_FEE: u64 = 10_000;

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct SwapResult {
    pub ckusdc_received_e6: u64,
    pub effective_price_e8s: u64,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct TransferReceipt {
    pub block_index: u64,
    pub amount: u64,
    pub created_at_time: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransferAttemptError {
    /// Ledger returned an explicit ICRC error, so it performed no transfer.
    NoEffect(String),
    /// The ledger rejected this transfer's explicit fee. This attempt had no
    /// effect; a caller replaying an older possibly-submitted intent must still
    /// treat the overall outcome as ambiguous.
    BadFee { expected_fee: String },
    /// The inter-canister call did not return; the ledger may have committed.
    Ambiguous(String),
}

/// Fetch a token ledger's current ICRC-1 transfer fee.
pub async fn fetch_ledger_fee(ledger: Principal) -> Result<u64, String> {
    let result: Result<(Nat,), _> = ic_cdk::call(ledger, "icrc1_fee", ()).await;
    match result {
        Ok((n,)) => n
            .0
            .to_string()
            .parse::<u64>()
            .map_err(|_| format!("icrc1_fee returned non-u64 value from {}", ledger)),
        Err((code, msg)) => Err(format!("icrc1_fee call failed ({:?}): {}", code, msg)),
    }
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

/// Reject swaps when `icpswap_pool` is the anonymous-principal sentinel that
/// `BotConfig` falls back to after a legacy-state migration where the field
/// did not exist. Forces admin to call `set_config` with a real pool before
/// any liquidation routes through ICPSwap.
fn require_icpswap_pool_set(config: &BotConfig) -> Result<(), String> {
    if config.icpswap_pool == Principal::anonymous() {
        return Err(
            "icpswap_pool not configured (defaulted from legacy migration). \
             Admin must call set_config with a real ICPSwap pool principal."
                .to_string(),
        );
    }
    Ok(())
}

/// Quote how much ckUSDC we'd get for `icp_amount_e8s` ICP.
pub async fn quote_icp_for_ckusdc(config: &BotConfig, icp_amount_e8s: u64) -> Result<u64, String> {
    require_icpswap_pool_set(config)?;
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
    require_icpswap_pool_set(config)?;
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

    let icp_fee = config.icp_fee_e8s.unwrap_or(FALLBACK_LEDGER_FEE);
    let ckusdc_fee = config.ckusdc_fee_e6.unwrap_or(FALLBACK_LEDGER_FEE);

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

fn return_transfer_fee_arg(transfer_fee_e8s: Option<u64>) -> Option<Nat> {
    transfer_fee_e8s.map(Nat::from)
}

/// Transfer collateral (ICP) back to the backend canister.
pub async fn return_collateral_to_backend(
    config: &BotConfig,
    amount_e8s: u64,
    collateral_ledger: Principal,
    memo: Vec<u8>,
    send_amount: u64,
    fee_e8s: u64,
    transfer_fee_e8s: Option<u64>,
    created_at_time: u64,
) -> Result<TransferReceipt, TransferAttemptError> {
    if send_amount == 0
        || send_amount
            .checked_add(fee_e8s)
            .is_none_or(|debit| debit > amount_e8s)
        || transfer_fee_e8s.is_some_and(|wire_fee| wire_fee != fee_e8s)
    {
        return Err(TransferAttemptError::NoEffect("Collateral return amount does not fit the claim reservation".into()));
    }
    let transfer_args = icrc_ledger_types::icrc1::transfer::TransferArg {
        from_subaccount: None,
        to: Account {
            owner: config.backend_principal,
            subaccount: None,
        },
        amount: Nat::from(send_amount),
        fee: return_transfer_fee_arg(transfer_fee_e8s),
        memo: Some(icrc_ledger_types::icrc1::transfer::Memo(
            serde_bytes::ByteBuf::from(memo),
        )),
        created_at_time: Some(created_at_time),
    };

    let result: Result<
        (Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError>,),
        _,
    > = ic_cdk::call(collateral_ledger, "icrc1_transfer", (transfer_args,)).await;

    use icrc_ledger_types::icrc1::transfer::TransferError;
    match result {
        Ok((Ok(block),)) => Ok(TransferReceipt {
            block_index: block.0.to_string().parse().map_err(|_| TransferAttemptError::Ambiguous("return block index exceeds u64".into()))?,
            amount: send_amount,
            created_at_time,
        }),
        // Audit Wave-3: a Duplicate response means the previous attempt landed.
        Ok((Err(TransferError::Duplicate { duplicate_of }),)) => {
            log!(
                crate::INFO,
                "[return_collateral_to_backend] ledger reported Duplicate (block {}); treating as success",
                duplicate_of
            );
            Ok(TransferReceipt {
                block_index: duplicate_of.0.to_string().parse().map_err(|_| TransferAttemptError::Ambiguous("duplicate return block index exceeds u64".into()))?,
                amount: send_amount,
                created_at_time,
            })
        }
        Ok((Err(TransferError::BadFee { expected_fee }),)) => {
            Err(TransferAttemptError::BadFee {
                expected_fee: expected_fee.0.to_string(),
            })
        }
        Ok((Err(e),)) => Err(TransferAttemptError::NoEffect(format!("Transfer error: {:?}", e))),
        Err((code, msg)) => Err(TransferAttemptError::Ambiguous(format!("Transfer call failed: {:?} {}", code, msg))),
    }
}

/// Read the bot's own ckUSDC main-account balance from the ledger.
///
/// Used by `process_pending` to bracket the swap call and compute the
/// per-claim delta — the ckUSDC this specific liquidation actually deposited
/// in the bot wallet, regardless of any pre-existing balance left over from
/// prior runs or the swap router's claimed output.
pub async fn balance_of_self_ckusdc(config: &BotConfig) -> Result<u64, String> {
    let result: Result<(Nat,), _> = ic_cdk::call(
        config.ckusdc_ledger,
        "icrc1_balance_of",
        (Account {
            owner: ic_cdk::id(),
            subaccount: None,
        },),
    )
    .await;
    match result {
        Ok((n,)) => n
            .0
            .to_string()
            .parse::<u64>()
            .map_err(|_| format!("icrc1_balance_of returned non-u64 from {}", config.ckusdc_ledger)),
        Err((code, msg)) => Err(format!("icrc1_balance_of call failed ({:?}): {}", code, msg)),
    }
}

/// Transfer ckUSDC from bot to backend canister.
/// Returns the actual amount received by the backend (after fee subtraction).
pub async fn transfer_ckusdc_to_backend(
    config: &BotConfig,
    amount_e6: u64,
    memo: Vec<u8>,
    created_at_time: u64,
    fee_e6: u64,
) -> Result<TransferReceipt, TransferAttemptError> {
    if amount_e6 == 0 {
        return Err(TransferAttemptError::NoEffect(
            "ckUSDC amount too small to cover transfer fee".to_string(),
        ));
    }

    let transfer_args = icrc_ledger_types::icrc1::transfer::TransferArg {
        from_subaccount: None,
        to: Account {
            owner: config.backend_principal,
            subaccount: None,
        },
        amount: Nat::from(amount_e6),
        fee: Some(Nat::from(fee_e6)),
        memo: Some(icrc_ledger_types::icrc1::transfer::Memo(
            serde_bytes::ByteBuf::from(memo),
        )),
        created_at_time: Some(created_at_time),
    };

    let result: Result<
        (Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError>,),
        _,
    > = ic_cdk::call(config.ckusdc_ledger, "icrc1_transfer", (transfer_args,)).await;

    use icrc_ledger_types::icrc1::transfer::TransferError;
    match result {
        Ok((Ok(block),)) => Ok(TransferReceipt {
            block_index: block.0.to_string().parse().map_err(|_| TransferAttemptError::Ambiguous("ckUSDC block index exceeds u64".into()))?,
            amount: amount_e6,
            created_at_time,
        }),
        Ok((Err(TransferError::Duplicate { duplicate_of }),)) => {
            log!(
                crate::INFO,
                "[transfer_ckusdc_to_backend] ledger reported Duplicate (block {}); treating as success",
                duplicate_of
            );
            Ok(TransferReceipt {
                block_index: duplicate_of.0.to_string().parse().map_err(|_| TransferAttemptError::Ambiguous("duplicate ckUSDC block index exceeds u64".into()))?,
                amount: amount_e6,
                created_at_time,
            })
        }
        Ok((Err(e),)) => Err(TransferAttemptError::NoEffect(format!(
            "ckUSDC transfer error: {:?}", e
        ))),
        Err((code, msg)) => Err(TransferAttemptError::Ambiguous(format!(
            "ckUSDC transfer call failed: {:?} {}", code, msg
        ))),
    }
}

/// Transfer ICP to treasury (liquidation bonus).
pub async fn transfer_icp_to_treasury(
    config: &BotConfig,
    amount_e8s: u64,
) -> Result<(), String> {
    let fee = config.icp_fee_e8s.unwrap_or(FALLBACK_LEDGER_FEE);
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

    use icrc_ledger_types::icrc1::transfer::TransferError;
    match result {
        Ok((Ok(_),)) => {
            log!(crate::INFO, "Transferred {} e8s ICP to treasury", send_amount);
            Ok(())
        }
        Ok((Err(TransferError::Duplicate { duplicate_of }),)) => {
            log!(
                crate::INFO,
                "[transfer_icp_to_treasury] ledger reported Duplicate (block {}); treating as success",
                duplicate_of
            );
            Ok(())
        }
        Ok((Err(e),)) => Err(format!("ICP transfer to treasury failed: {:?}", e)),
        Err((code, msg)) => Err(format!("ICP transfer call failed: {:?} {}", code, msg)),
    }
}

#[cfg(test)]
mod return_transfer_tests {
    use super::*;

    #[test]
    fn return_transfer_fee_keeps_new_and_legacy_wire_identities_distinct() {
        assert_eq!(return_transfer_fee_arg(Some(10_000)), Some(Nat::from(10_000u64)));
        assert_eq!(return_transfer_fee_arg(None), None);
    }
}
