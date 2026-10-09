use candid::{CandidType, Encode, Nat, Principal};
use ic_canister_log::log;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc2::approve::ApproveArgs;
use serde::{Deserialize, Serialize};

use crate::icpswap;
use crate::native_icp_blocks::{self, BlockSource};
#[cfg(test)]
use crate::native_icp_blocks::{
    Block as IcpCandidBlock, Operation as IcpCandidOperation, Timestamp as IcpTimestamp,
    Tokens as IcpTokens, Transaction as IcpCandidTransaction,
};
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedSwap {
    pub amount_in_e8s: u64,
    pub amount_out_minimum_e6: u64,
    pub zero_for_one: bool,
    pub input_fee_e8s: u64,
    pub output_fee_e6: u64,
}

/// Whether a swap failure proves the deposit call was never dispatched.
/// Everything reported by `depositFromAndSwap` itself is outcome-ambiguous.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SwapAttemptError {
    NoEffect(String),
    Ambiguous(String),
}

impl std::fmt::Display for SwapAttemptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoEffect(message) => write!(f, "{message}"),
            Self::Ambiguous(message) => write!(f, "swap outcome is ambiguous: {message}"),
        }
    }
}

fn classify_deposit_dispatch_error(error: icpswap::DepositAndSwapError) -> SwapAttemptError {
    SwapAttemptError::Ambiguous(error.to_string())
}

pub(crate) fn swap_error_allows_return(error: &SwapAttemptError) -> bool {
    matches!(error, SwapAttemptError::NoEffect(_))
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
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
) -> Result<SwapResult, SwapAttemptError> {
    let prepared = prepare_icp_for_ckusdc(config, icp_amount_e8s).await?;
    dispatch_prepared_swap(config, &prepared).await
}

/// Query the pool and freeze every call argument without dispatching a swap.
/// Claim processing persists this tuple before calling `dispatch_prepared_swap`.
pub async fn prepare_icp_for_ckusdc(
    config: &BotConfig,
    icp_amount_e8s: u64,
) -> Result<PreparedSwap, SwapAttemptError> {
    require_icpswap_pool_set(config).map_err(SwapAttemptError::NoEffect)?;
    let zero_for_one = config
        .icpswap_zero_for_one
        .ok_or_else(|| {
            SwapAttemptError::NoEffect(
                "Pool ordering not configured. Call admin_resolve_pool_ordering first.".into(),
            )
        })?;

    let quoted_output = icpswap::quote(config.icpswap_pool, icp_amount_e8s, zero_for_one)
        .await
        .map_err(SwapAttemptError::NoEffect)?;

    if quoted_output == 0 {
        return Err(SwapAttemptError::NoEffect(
            "Quote returned zero output".to_string(),
        ));
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

    Ok(PreparedSwap {
        amount_in_e8s: icp_amount_e8s,
        amount_out_minimum_e6: min_output,
        zero_for_one,
        input_fee_e8s: icp_fee,
        output_fee_e6: ckusdc_fee,
    })
}

/// Dispatch one already prepared call. Any returned error is ambiguous because
/// the pool can complete ledger work before its reply reaches this canister.
pub async fn dispatch_prepared_swap(
    config: &BotConfig,
    prepared: &PreparedSwap,
) -> Result<SwapResult, SwapAttemptError> {
    let icp_amount_e8s = prepared.amount_in_e8s;

    let received = icpswap::deposit_and_swap(
        config.icpswap_pool,
        icp_amount_e8s,
        prepared.amount_out_minimum_e6,
        prepared.zero_for_one,
        prepared.input_fee_e8s,
        prepared.output_fee_e6,
    )
    .await
    .map_err(classify_deposit_dispatch_error)?;

    // Effective price in e8 format: (ckusdc_e6 / icp_e8s) * 1e8
    // = ckusdc_e6 * 1e2 * 1e8 / icp_e8s = ckusdc_e6 * 10_000_000_000 / icp_e8s
    let effective_price_e8s = if icp_amount_e8s > 0 {
        (received as u128 * 10_000_000_000 / icp_amount_e8s as u128) as u64
    } else {
        0
    };

    log!(
        crate::INFO,
        "ICPSwap call returned output claim {} ckUSDC e6, effective price {} e8s; ledger receipt is verified separately",
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
    collateral_ledger: Principal,
    memo: Vec<u8>,
    send_amount: u64,
    fee_e8s: u64,
    transfer_fee_e8s: Option<u64>,
    created_at_time: u64,
) -> Result<TransferReceipt, TransferAttemptError> {
    if send_amount == 0 || transfer_fee_e8s.is_some_and(|wire_fee| wire_fee != fee_e8s) {
        return Err(TransferAttemptError::NoEffect(
            "Collateral return tuple has an invalid amount or fee".into(),
        ));
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

/// Read the bot's main-account ICP balance to reserve claim collateral plus
/// the explicit return fee from its separate fee float.
pub async fn balance_of_self_icp(config: &BotConfig) -> Result<u64, String> {
    let result: Result<(Nat,), _> = ic_cdk::call(
        config.icp_ledger,
        "icrc1_balance_of",
        (Account {
            owner: ic_cdk::id(),
            subaccount: None,
        },),
    )
    .await;

    match result {
        Ok((balance,)) => balance.0.to_string().parse::<u64>().map_err(|_| {
            format!(
                "icrc1_balance_of returned non-u64 from {}",
                config.icp_ledger
            )
        }),
        Err((code, msg)) => Err(format!(
            "icrc1_balance_of call failed ({:?}): {}",
            code, msg
        )),
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

/// Prepare and persist this exact ICRC tuple before dispatch. `gross` includes
/// the fee; the recorded amount is what the treasury account receives.
pub fn prepare_icp_treasury_transfer(
    config: &BotConfig,
    record_id: u64,
    vault_id: u64,
    claim_generation: u64,
    gross_amount_e8s: u64,
    created_at_time: u64,
) -> Result<crate::state::BotTreasuryJournal, String> {
    let fee_e8s = config.icp_fee_e8s.unwrap_or(FALLBACK_LEDGER_FEE);
    let amount_e8s = gross_amount_e8s.checked_sub(fee_e8s).unwrap_or(0);
    let memo = icp_treasury_memo(record_id, claim_generation);
    Ok(crate::state::BotTreasuryJournal {
        record_id, vault_id, claim_generation,
        ledger_principal: config.icp_ledger,
        sender_principal: ic_cdk::id(),
        treasury_principal: config.treasury_principal,
        gross_amount_e8s, amount_e8s, fee_e8s, memo, created_at_time,
        status: if amount_e8s == 0 { crate::state::BotTreasuryStatus::NeedsPreparation } else { crate::state::BotTreasuryStatus::Prepared },
        receipt: None,
        paid_total_applied: false,
        record: crate::history::LiquidationRecordV1 {
            id: record_id, vault_id, timestamp: created_at_time,
            status: crate::history::LiquidationStatus::TransferFailed,
            collateral_claimed_e8s: 0, debt_to_cover_e8s: 0, icp_swapped_e8s: 0,
            ckusdc_received_e6: 0, ckusdc_transferred_e6: 0,
            icp_to_treasury_e8s: gross_amount_e8s,
            oracle_price_e8s: 0, effective_price_e8s: 0, slippage_bps: 0,
            error_message: None, confirm_retry_count: 0,
        },
    })
}

pub(crate) fn icp_treasury_transfer_amount(gross_e8s: u64, fee_e8s: u64) -> Result<u64, String> {
    gross_e8s.checked_sub(fee_e8s).filter(|amount| *amount > 0)
        .ok_or_else(|| format!("ICP treasury bonus {} e8s does not exceed fee {} e8s", gross_e8s, fee_e8s))
}

fn icp_treasury_memo(record_id: u64, claim_generation: u64) -> Vec<u8> {
    let mut memo = b"RUMI:TB1:".to_vec();
    memo.extend_from_slice(&record_id.to_be_bytes());
    memo.extend_from_slice(&claim_generation.to_be_bytes());
    memo
}

/// Replay only the journaled tuple. A success or Duplicate returns its block;
/// typed ledger errors prove this dispatch had no effect, rejects are ambiguous.
pub async fn transfer_icp_to_treasury(
    journal: &crate::state::BotTreasuryJournal,
) -> Result<TransferReceipt, TransferAttemptError> {
    if journal.sender_principal != ic_cdk::id()
        || journal.memo.len() > 32
        || journal.amount_e8s == 0
        || journal.amount_e8s.checked_add(journal.fee_e8s) != Some(journal.gross_amount_e8s)
    {
        return Err(TransferAttemptError::NoEffect("persisted ICP treasury tuple is invalid".into()));
    }
    let transfer_args = icrc_ledger_types::icrc1::transfer::TransferArg {
        from_subaccount: None,
        to: Account { owner: journal.treasury_principal, subaccount: None },
        amount: Nat::from(journal.amount_e8s),
        fee: Some(Nat::from(journal.fee_e8s)),
        memo: Some(icrc_ledger_types::icrc1::transfer::Memo(serde_bytes::ByteBuf::from(journal.memo.clone()))),
        created_at_time: Some(journal.created_at_time),
    };
    let result: Result<(Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError>,), _> =
        ic_cdk::call(journal.ledger_principal, "icrc1_transfer", (transfer_args,)).await;
    let block = match result {
        Err((code, msg)) => return Err(TransferAttemptError::Ambiguous(format!("ICP treasury call failed: {code:?} {msg}"))),
        Ok((Ok(block),)) => block,
        Ok((Err(icrc_ledger_types::icrc1::transfer::TransferError::Duplicate { duplicate_of }),)) => duplicate_of,
        Ok((Err(icrc_ledger_types::icrc1::transfer::TransferError::BadFee { expected_fee }),)) => {
            return Err(TransferAttemptError::BadFee { expected_fee: expected_fee.0.to_string() });
        }
        Ok((Err(error),)) => return Err(TransferAttemptError::NoEffect(format!("ICP treasury transfer rejected: {error:?}"))),
    };
    let block_index = block.0.to_string().parse::<u64>()
        .map_err(|_| TransferAttemptError::Ambiguous("ICP treasury block index exceeds u64".into()))?;
    Ok(TransferReceipt { block_index, amount: journal.amount_e8s, created_at_time: journal.created_at_time })
}

/// Verify the returned block against the persisted tuple using the native ICP
/// ledger's block query. Archive callbacks are trusted only as advertised by
/// the configured ledger, matching the backend native-ICP verifier; this is
/// ledger-response verification, not an independent ICRC-3 certificate proof.
pub async fn verify_icp_treasury_receipt(
    journal: &crate::state::BotTreasuryJournal,
    receipt: &TransferReceipt,
) -> Result<(), String> {
    if receipt.amount != journal.amount_e8s || receipt.created_at_time != journal.created_at_time {
        return Err("ICP treasury receipt metadata does not match its persisted intent".into());
    }
    let request = native_icp_blocks::GetBlocksArgs { start: receipt.block_index, length: 1 };
    let request_bytes = candid::Encode!(&request)
        .map_err(|error| format!("could not encode ICP query_blocks request: {error}"))?;
    let response_bytes = ic_cdk::api::call::call_raw(
        journal.ledger_principal,
        "query_blocks",
        request_bytes,
        0,
    ).await.map_err(|(code, msg)| format!("ICP query_blocks failed: {code:?} {msg}"))?;
    let response = native_icp_blocks::decode_query_blocks(&response_bytes)?;
    let block = match native_icp_blocks::select_block_source(response, receipt.block_index)? {
        BlockSource::Direct(block) => block,
        BlockSource::Archive { canister_id, method } => {
            let callback_args = candid::Encode!(&request)
                .map_err(|error| format!("could not encode ICP archive request: {error}"))?;
            let archive_bytes = ic_cdk::api::call::call_raw(
                canister_id,
                &method,
                callback_args,
                0,
            ).await.map_err(|(code, msg)| format!("ICP archive callback failed: {code:?} {msg}"))?;
            let mut blocks = native_icp_blocks::decode_archive_result(&archive_bytes)?
                .map_err(|error| format!("ICP archive rejected the requested block: {error:?}"))?
                .blocks;
            if blocks.len() != 1 {
                return Err("ICP archive callback did not return exactly one block for the exact request".into());
            }
            blocks.remove(0)
        }
    };
    if journal.sender_principal != ic_cdk::id() {
        return Err("persisted ICP treasury transfer belongs to a different sender canister".into());
    }
    native_icp_blocks::verify_treasury_transfer(
        &block,
        journal.sender_principal,
        journal.treasury_principal,
        journal.amount_e8s,
        journal.fee_e8s,
        &journal.memo,
        journal.created_at_time,
    )
}

#[cfg(test)]
mod return_transfer_tests {
    use super::*;

    #[test]
    fn icp_treasury_tuple_uses_claim_bound_dedup_identity_and_exact_fee() {
        assert_eq!(icp_treasury_transfer_amount(25_000, 10_000).unwrap(), 15_000);
        assert!(icp_treasury_transfer_amount(10_000, 10_000).is_err());
        assert!(icp_treasury_transfer_amount(9_999, 10_000).is_err());
        let memo = icp_treasury_memo(7, 42);
        assert_eq!(memo.len(), 25, "memo stays under ICP's 32-byte limit");
        assert_ne!(memo, icp_treasury_memo(7, 43));
    }

    #[test]
    fn native_icp_default_account_identifier_matches_canonical_vector() {
        let identifier = native_icp_blocks::default_account_identifier(Principal::from_slice(&[1]));
        assert_eq!(identifier, vec![
            0x5d, 0xc3, 0xba, 0x97, 0x57, 0xeb, 0xd7, 0xcc,
            0x99, 0x58, 0x2c, 0xd3, 0xe7, 0xe4, 0x8e, 0x37,
            0x46, 0x11, 0xc7, 0x22, 0x0c, 0xc4, 0x47, 0x60,
            0x1d, 0xdc, 0x74, 0x4e, 0xcd, 0xf8, 0x60, 0x90,
        ]);
    }

    #[test]
    fn native_icp_transfer_block_matches_exact_persisted_tuple() {
        let mut journal = crate::state::BotTreasuryJournal {
            record_id: 7, vault_id: 8, claim_generation: 12,
            ledger_principal: Principal::management_canister(),
            sender_principal: Principal::anonymous(), treasury_principal: Principal::anonymous(),
            gross_amount_e8s: 25_000, amount_e8s: 15_000, fee_e8s: 10_000,
            memo: Vec::new(), created_at_time: 100,
            status: crate::state::BotTreasuryStatus::ReceiptObserved,
            receipt: Some(TransferReceipt { block_index: 4, amount: 15_000, created_at_time: 100 }),
            paid_total_applied: false,
            record: crate::history::LiquidationRecordV1 {
                id: 7, vault_id: 8, timestamp: 90,
                status: crate::history::LiquidationStatus::TransferFailed,
                collateral_claimed_e8s: 30_000, debt_to_cover_e8s: 10, icp_swapped_e8s: 5_000,
                ckusdc_received_e6: 100, ckusdc_transferred_e6: 100, icp_to_treasury_e8s: 0,
                oracle_price_e8s: 1, effective_price_e8s: 1, slippage_bps: 0,
                error_message: None, confirm_retry_count: 1,
            },
        };
        journal.sender_principal = Principal::from_slice(&[1]);
        journal.treasury_principal = Principal::from_slice(&[2]);
        journal.memo = b"RUMI:TB1:test".to_vec();
        let block = IcpCandidBlock {
            transaction: IcpCandidTransaction {
                memo: 0,
                icrc1_memo: Some(journal.memo.clone()),
                operation: Some(IcpCandidOperation::Transfer {
                    from: native_icp_blocks::default_account_identifier(journal.sender_principal),
                    to: native_icp_blocks::default_account_identifier(journal.treasury_principal),
                    spender: None,
                    amount: IcpTokens { e8s: journal.amount_e8s },
                    fee: IcpTokens { e8s: journal.fee_e8s },
                }),
                created_at_time: IcpTimestamp { timestamp_nanos: journal.created_at_time },
            },
        };
        let verify = |candidate: &IcpCandidBlock| {
            native_icp_blocks::verify_treasury_transfer(
                candidate,
                journal.sender_principal,
                journal.treasury_principal,
                journal.amount_e8s,
                journal.fee_e8s,
                &journal.memo,
                journal.created_at_time,
            )
        };
        assert!(verify(&block).is_ok());
        let mut wrong_fee = block.clone();
        if let Some(IcpCandidOperation::Transfer { fee, .. }) = wrong_fee.transaction.operation.as_mut() {
            fee.e8s += 1;
        }
        assert!(verify(&wrong_fee).is_err());
        let mut wrong_memo = block;
        wrong_memo.transaction.icrc1_memo = Some(b"other transfer".to_vec());
        assert!(verify(&wrong_memo).is_err());
    }

    #[test]
    fn deposit_application_err_is_ambiguous_and_never_unwinds_claim() {
        let error = classify_deposit_dispatch_error(icpswap::DepositAndSwapError::Application(
            "InsufficientFunds".into(),
        ));
        assert!(matches!(error, SwapAttemptError::Ambiguous(_)));
        assert!(!swap_error_allows_return(&error));
    }

    #[test]
    fn commit_then_call_reject_is_ambiguous_and_never_unwinds_claim() {
        let error = classify_deposit_dispatch_error(icpswap::DepositAndSwapError::CallRejected(
            "SysTransient after callee execution".into(),
        ));
        assert!(matches!(error, SwapAttemptError::Ambiguous(_)));
        assert!(!swap_error_allows_return(&error));
    }

    #[test]
    fn pre_deposit_errors_are_definitive_and_may_use_gross_return_path() {
        let error = SwapAttemptError::NoEffect("quote returned zero output".into());
        assert!(swap_error_allows_return(&error));
        let error = SwapAttemptError::NoEffect("pool not configured".into());
        assert!(swap_error_allows_return(&error));
    }

    #[test]
    fn return_transfer_fee_keeps_new_and_legacy_wire_identities_distinct() {
        assert_eq!(
            return_transfer_fee_arg(Some(10_000)),
            Some(Nat::from(10_000u64))
        );
        assert_eq!(return_transfer_fee_arg(None), None);
    }
}
