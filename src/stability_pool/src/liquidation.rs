use candid::{Nat, Principal};
use ic_canister_log::log;
use ic_cdk::call;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::{Memo, TransferArg, TransferError};
use icrc_ledger_types::icrc2::approve::{ApproveArgs, ApproveError};
use icrc_ledger_types::icrc3::blocks::{GetBlocksRequest, GetBlocksResult};
use num_traits::ToPrimitive;
use rumi_protocol_backend::chains::config::ChainId;
use std::collections::BTreeMap;

use crate::logs::INFO;
use crate::state::{mutate_state, read_state, StabilityPoolState};
use crate::types::*;

/// Conservative fallback for a collateral ledger's transfer fee, used only when
/// the live `icrc1_fee` query fails (SP-104). Set to the common ICRC fee
/// (10_000 e8s, as on ICP/ckBTC-class ledgers). Over-estimating the fee
/// under-credits depositors slightly (solvency-safe) rather than over-crediting
/// them as a fee=0 fallback would. The next successful liquidation reconciles.
/// Shared with `claim_collateral`'s fee lookup (ICRC-004 / SP-203).
pub(crate) const FALLBACK_COLLATERAL_FEE_E8S: u64 = 10_000;

pub(crate) const CHAIN_WRITEDOWN_MEMO_PREFIX: &[u8] = b"RUMI-LIQ-004:";
const AMBIGUOUS_BURN_RECOVERY_SCAN_BLOCKS: u64 = 256;

#[derive(Clone, Debug)]
pub(crate) struct IcusdBurnAttemptError {
    error: StabilityPoolError,
    definitive_no_effect: bool,
}

impl IcusdBurnAttemptError {
    fn definite(error: StabilityPoolError) -> Self {
        Self {
            error,
            definitive_no_effect: true,
        }
    }

    fn ambiguous(error: StabilityPoolError) -> Self {
        Self {
            error,
            definitive_no_effect: false,
        }
    }
}

fn ledger_transfer_error_proves_no_effect(error: &TransferError) -> bool {
    matches!(
        error,
        TransferError::BadFee { .. }
            | TransferError::BadBurn { .. }
            | TransferError::InsufficientFunds { .. }
            | TransferError::CreatedInFuture { .. }
            | TransferError::TooOld
    )
}

async fn burn_native_xrp_icusd_with_account(
    icusd_ledger: Principal,
    minting_account: Account,
    amount_e8s: u64,
    vault_id: u64,
    created_at_time: u64,
) -> Result<rumi_protocol_backend::icrc3_proof::SpWritedownProof, IcusdBurnAttemptError> {
    let transfer_arg =
        build_icusd_burn_transfer_arg(minting_account, amount_e8s, vault_id, created_at_time);
    let result: Result<(Result<Nat, TransferError>,), _> =
        call(icusd_ledger, "icrc1_transfer", (transfer_arg,)).await;
    let block_index = match result {
        Ok((Ok(block),))
        | Ok((Err(TransferError::Duplicate {
            duplicate_of: block,
        }),)) => nat_block_index_to_u64(block).map_err(IcusdBurnAttemptError::ambiguous)?,
        Ok((Err(error),)) => {
            let failure = StabilityPoolError::LedgerTransferFailed {
                reason: format!("{error:?}"),
            };
            return Err(if ledger_transfer_error_proves_no_effect(&error) {
                IcusdBurnAttemptError::definite(failure)
            } else {
                IcusdBurnAttemptError::ambiguous(failure)
            });
        }
        Err(_) => {
            return Err(IcusdBurnAttemptError::ambiguous(
                StabilityPoolError::InterCanisterCallFailed {
                    target: icusd_ledger.to_string(),
                    method: "icrc1_transfer".into(),
                },
            ))
        }
    };
    Ok(build_icusd_burn_proof(block_index, vault_id))
}

pub fn encode_chain_writedown_memo(vault_id: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(CHAIN_WRITEDOWN_MEMO_PREFIX.len() + 8);
    out.extend_from_slice(CHAIN_WRITEDOWN_MEMO_PREFIX);
    out.extend_from_slice(&vault_id.to_be_bytes());
    out
}

pub fn build_icusd_burn_transfer_arg(
    minting_account: Account,
    amount_e8s: u64,
    vault_id: u64,
    created_at_time: u64,
) -> TransferArg {
    TransferArg {
        from_subaccount: None,
        to: minting_account,
        fee: None,
        created_at_time: Some(created_at_time),
        memo: Some(Memo::from(encode_chain_writedown_memo(vault_id))),
        amount: Nat::from(amount_e8s),
    }
}

pub fn build_icusd_burn_proof(
    block_index: u64,
    vault_id: u64,
) -> rumi_protocol_backend::icrc3_proof::SpWritedownProof {
    rumi_protocol_backend::icrc3_proof::SpWritedownProof {
        block_index,
        ledger_kind: rumi_protocol_backend::icrc3_proof::SpProofLedger::IcusdBurn,
        vault_id_memo: vault_id,
    }
}

/// Validate the positive evidence needed to recover an ambiguous native-XRP
/// burn. A standard ICRC-1 burn records the source account and burn tuple;
/// the minting account is established separately from the pinned ledger.
/// Missing fields are never treated as a substitute for these checks.
fn validate_ambiguous_native_xrp_burn_block(
    block: &rumi_protocol_backend::icrc3_proof::DecodedBlock,
    intent: &NativeXrpAbsorbIntent,
    minting_account: &Account,
    stability_pool: Principal,
) -> Result<(), String> {
    if block.btype.as_deref().is_some_and(|kind| kind != "1burn") || block.op != "burn" {
        return Err("candidate is not a standard ICRC-1 burn block".into());
    }
    if minting_account != &intent.icusd_minting_account {
        return Err("pinned ledger minting account differs from the persisted burn intent".into());
    }
    let expected_from = Account {
        owner: stability_pool,
        subaccount: None,
    };
    if block.from.as_ref() != Some(&expected_from) {
        return Err("burn source account differs from the Stability Pool account".into());
    }
    if block.spender.is_some() {
        return Err("ICRC-1 burn unexpectedly records a spender".into());
    }
    if block.to.as_ref().is_some_and(|to| to != minting_account) {
        return Err("burn destination differs from the pinned minting account".into());
    }
    if block.amount != u128::from(intent.icusd_to_burn_e8s)
        || block.memo.as_deref() != Some(encode_chain_writedown_memo(intent.vault_id).as_slice())
        || block.created_at_time != Some(intent.burn_created_at_time_ns)
    {
        return Err("burn block does not match the persisted amount, memo, or timestamp".into());
    }
    // ICRC-1 burns have a zero fee. Some ledger block encodings omit fee for
    // burn operations; when it is present, it must agree with that standard.
    if block.fee.is_some_and(|fee| fee != 0) {
        return Err("burn block records a nonzero fee".into());
    }
    Ok(())
}

async fn fetch_direct_icusd_burn_blocks(
    ledger: Principal,
    start: u64,
    length: u64,
) -> Result<
    Vec<(
        u64,
        Option<rumi_protocol_backend::icrc3_proof::DecodedBlock>,
    )>,
    StabilityPoolError,
> {
    if length == 0 || length > AMBIGUOUS_BURN_RECOVERY_SCAN_BLOCKS {
        return Err(StabilityPoolError::LedgerTransferFailed {
            reason: "invalid direct ledger recovery page length".into(),
        });
    }
    let request = vec![GetBlocksRequest {
        start: Nat::from(start),
        length: Nat::from(length),
    }];
    let result: Result<(GetBlocksResult,), _> = call(ledger, "icrc3_get_blocks", (request,)).await;
    let (response,) = result.map_err(|_| StabilityPoolError::InterCanisterCallFailed {
        target: ledger.to_string(),
        method: "icrc3_get_blocks".into(),
    })?;
    let log_length =
        response
            .log_length
            .0
            .to_u64()
            .ok_or_else(|| StabilityPoolError::LedgerTransferFailed {
                reason: "ledger log length does not fit in u64".into(),
            })?;
    if start
        .checked_add(length)
        .map_or(true, |end| end > log_length)
        || !response.archived_blocks.is_empty()
        || response.blocks.len() != length as usize
    {
        return Err(StabilityPoolError::LedgerTransferFailed {
            reason: "ledger did not return the complete direct block range".into(),
        });
    }
    response
        .blocks
        .iter()
        .enumerate()
        .map(|(offset, block)| {
            let expected_index = start + offset as u64;
            if block.id.0.to_u64() != Some(expected_index) {
                return Err(StabilityPoolError::LedgerTransferFailed {
                    reason: "ledger direct block range contains a gap or wrong index".into(),
                });
            }
            // This page can contain unrelated ICRC operations (including
            // ledger-specific blocks the shared decoder does not understand).
            // Their index still contributes to page completeness, but they
            // cannot constitute positive burn evidence.
            Ok((
                expected_index,
                rumi_protocol_backend::icrc3_proof::decode_block(&block.block).ok(),
            ))
        })
        .collect()
}

async fn direct_icusd_burn_recovery_candidate(
    intent: &NativeXrpAbsorbIntent,
    candidate_index: Option<u64>,
    minting_account: &Account,
) -> Result<u64, StabilityPoolError> {
    let (start, length) = if let Some(index) = candidate_index {
        (index, 1)
    } else {
        // A zero-length direct request obtains the current log length without
        // asking an archive callback to supply any transaction evidence.
        let tip_request = vec![GetBlocksRequest {
            start: Nat::from(0u64),
            length: Nat::from(0u64),
        }];
        let tip_call: Result<(GetBlocksResult,), _> =
            call(intent.icusd_ledger, "icrc3_get_blocks", (tip_request,)).await;
        let (tip_response,) =
            tip_call.map_err(|_| StabilityPoolError::InterCanisterCallFailed {
                target: intent.icusd_ledger.to_string(),
                method: "icrc3_get_blocks".into(),
            })?;
        if !tip_response.blocks.is_empty() || !tip_response.archived_blocks.is_empty() {
            return Err(StabilityPoolError::LedgerTransferFailed {
                reason: "ledger tip response unexpectedly included blocks or archives".into(),
            });
        }
        let log_length = tip_response.log_length.0.to_u64().ok_or_else(|| {
            StabilityPoolError::LedgerTransferFailed {
                reason: "ledger log length does not fit in u64".into(),
            }
        })?;
        let start = log_length.saturating_sub(AMBIGUOUS_BURN_RECOVERY_SCAN_BLOCKS);
        (start, log_length.saturating_sub(start))
    };
    if length == 0 {
        return Err(StabilityPoolError::LedgerTransferFailed {
            reason: "icUSD ledger contains no candidate blocks to scan".into(),
        });
    }
    let blocks = fetch_direct_icusd_burn_blocks(intent.icusd_ledger, start, length).await?;

    let mut matches = Vec::new();
    for (index, block) in blocks {
        if let Some(block) = block {
            if validate_ambiguous_native_xrp_burn_block(
                &block,
                intent,
                minting_account,
                ic_cdk::api::id(),
            )
            .is_ok()
            {
                matches.push(index);
            }
        }
    }
    if matches.len() != 1 {
        return Err(StabilityPoolError::LedgerTransferFailed {
            reason: if matches.is_empty() {
                "no exact direct ledger burn block matched the persisted intent".into()
            } else {
                "multiple direct ledger burn blocks matched the persisted intent".into()
            },
        });
    }
    Ok(matches[0])
}

/// Attach a proof only after a direct ledger block has matched the persisted
/// ambiguous burn tuple. The caller compares the complete intent after awaits
/// to prevent applying evidence to changed state.
pub(crate) async fn recover_ambiguous_native_xrp_burn_proof(
    vault_id: u64,
    candidate_index: Option<u64>,
) -> Result<(), StabilityPoolError> {
    let intent =
        read_state(|state| state.get_pending_native_xrp_absorb(vault_id)).ok_or_else(|| {
            StabilityPoolError::LiquidationFailed {
                vault_id,
                reason: "missing pending native XRP absorb intent".into(),
            }
        })?;
    if intent.burn_proof.is_some()
        || !matches!(intent.burn_attempted, Some(true) | None)
        || intent.status != NativeXrpAbsorbIntentStatus::Prepared
    {
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: "intent is not an unresolved ambiguous native XRP burn".into(),
        });
    }
    let minting_account = fetch_icusd_minting_account(intent.icusd_ledger).await?;
    let block_index =
        direct_icusd_burn_recovery_candidate(&intent, candidate_index, &minting_account).await?;
    let proof = build_icusd_burn_proof(block_index, vault_id);
    mutate_state(|state| {
        mark_native_xrp_absorb_recovered_burn_proof_in_state(
            state,
            &intent,
            proof,
            ic_cdk::api::time(),
        )
    })
}

fn mark_native_xrp_absorb_recovered_burn_proof_in_state(
    state: &mut StabilityPoolState,
    expected: &NativeXrpAbsorbIntent,
    proof: rumi_protocol_backend::icrc3_proof::SpWritedownProof,
    now_ns: u64,
) -> Result<(), StabilityPoolError> {
    let vault_id = expected.vault_id;
    let mut current = state
        .get_pending_native_xrp_absorb(vault_id)
        .ok_or_else(|| StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: "pending native XRP intent disappeared during ledger verification".into(),
        })?;
    if current != *expected
        || current.burn_proof.is_some()
        || !matches!(current.burn_attempted, Some(true) | None)
        || current.status != NativeXrpAbsorbIntentStatus::Prepared
        || proof.ledger_kind != rumi_protocol_backend::icrc3_proof::SpProofLedger::IcusdBurn
        || proof.vault_id_memo != vault_id
    {
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: "pending native XRP intent changed or proof is not applicable".into(),
        });
    }
    current.burn_proof = Some(proof);
    current.burn_attempted = Some(true);
    current.status = NativeXrpAbsorbIntentStatus::Burned;
    current.last_error = None;
    current.updated_at_ns = now_ns;
    state.put_pending_native_xrp_absorb(current)
}

pub async fn fetch_icusd_minting_account(
    icusd_ledger: Principal,
) -> Result<Account, StabilityPoolError> {
    match call::<(), (Option<Account>,)>(icusd_ledger, "icrc1_minting_account", ()).await {
        Ok((Some(account),)) => Ok(account),
        Ok((None,)) => Err(StabilityPoolError::LedgerTransferFailed {
            reason: "icUSD ledger has no minting account; cannot burn".to_string(),
        }),
        Err(_) => Err(StabilityPoolError::InterCanisterCallFailed {
            target: format!("{}", icusd_ledger),
            method: "icrc1_minting_account".to_string(),
        }),
    }
}

pub async fn burn_icusd_for_chain_writedown_with_account(
    icusd_ledger: Principal,
    minting_account: Account,
    amount_e8s: u64,
    vault_id: u64,
    created_at_time: u64,
) -> Result<rumi_protocol_backend::icrc3_proof::SpWritedownProof, StabilityPoolError> {
    if amount_e8s == 0 {
        return Err(StabilityPoolError::AmountTooLow { minimum_e8s: 1 });
    }
    let transfer_arg =
        build_icusd_burn_transfer_arg(minting_account, amount_e8s, vault_id, created_at_time);

    let result: Result<(Result<Nat, TransferError>,), _> =
        call(icusd_ledger, "icrc1_transfer", (transfer_arg,)).await;

    let block_index = match result {
        Ok((Ok(block_index),)) => nat_block_index_to_u64(block_index)?,
        Ok((Err(TransferError::Duplicate { duplicate_of }),)) => {
            nat_block_index_to_u64(duplicate_of)?
        }
        Ok((Err(error),)) => {
            return Err(StabilityPoolError::LedgerTransferFailed {
                reason: format!("{:?}", error),
            });
        }
        Err(_) => {
            return Err(StabilityPoolError::InterCanisterCallFailed {
                target: format!("{}", icusd_ledger),
                method: "icrc1_transfer".to_string(),
            });
        }
    };

    Ok(build_icusd_burn_proof(block_index, vault_id))
}

pub async fn burn_icusd_for_chain_writedown(
    icusd_ledger: Principal,
    amount_e8s: u64,
    vault_id: u64,
) -> Result<rumi_protocol_backend::icrc3_proof::SpWritedownProof, StabilityPoolError> {
    let minting_account = fetch_icusd_minting_account(icusd_ledger).await?;
    burn_icusd_for_chain_writedown_with_account(
        icusd_ledger,
        minting_account,
        amount_e8s,
        vault_id,
        ic_cdk::api::time(),
    )
    .await
}

fn nat_block_index_to_u64(block_index: Nat) -> Result<u64, StabilityPoolError> {
    block_index
        .0
        .to_u64()
        .ok_or_else(|| StabilityPoolError::LedgerTransferFailed {
            reason: format!("ledger block index {} does not fit in u64", block_index),
        })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ChainAbsorbPlan {
    pub vault_id: u64,
    pub chain_id: ChainId,
    pub chain_sentinel: Principal,
    pub icusd_ledger: Principal,
    pub icusd_to_burn_e8s: u64,
    pub stables_consumed: BTreeMap<Principal, u64>,
}

fn chain_absorb_result_from_backend(
    plan: &ChainAbsorbPlan,
    result: ChainStabilityPoolLiquidationResult,
) -> Result<ChainSpAbsorbResult, StabilityPoolError> {
    if !result.success {
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id: plan.vault_id,
            reason: "backend reported unsuccessful chain absorb".to_string(),
        });
    }
    if result.vault_id != plan.vault_id || result.chain_id != plan.chain_id {
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id: plan.vault_id,
            reason: "backend chain absorb result does not match requested vault".to_string(),
        });
    }
    if result.liquidated_debt_e8s != plan.icusd_to_burn_e8s as u128 {
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id: plan.vault_id,
            reason: "backend liquidated debt does not match SP burn".to_string(),
        });
    }
    if result.collateral_received_native == 0 {
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id: plan.vault_id,
            reason: "backend returned zero chain collateral".to_string(),
        });
    }

    Ok(ChainSpAbsorbResult {
        success: true,
        vault_id: result.vault_id,
        chain_id: result.chain_id,
        icusd_burned_e8s: plan.icusd_to_burn_e8s,
        liquidated_debt_e8s: result.liquidated_debt_e8s,
        collateral_received_native: result.collateral_received_native,
        claim_id: result.claim_id,
        custody_address: result.custody_address,
        block_index: result.block_index,
        collateral_price_e8s: result.collateral_price_e8s,
    })
}

fn intent_matches_plan(
    intent: &ChainSpAbsorbIntent,
    plan: &ChainAbsorbPlan,
    minting_account: Account,
) -> bool {
    intent.vault_id == plan.vault_id
        && intent.chain_id == plan.chain_id
        && intent.chain_sentinel == plan.chain_sentinel
        && intent.icusd_ledger == plan.icusd_ledger
        && intent.icusd_minting_account == minting_account
        && intent.icusd_to_burn_e8s == plan.icusd_to_burn_e8s
        && intent.stables_consumed == plan.stables_consumed
}

fn chain_absorb_plan_from_intent(intent: &ChainSpAbsorbIntent) -> ChainAbsorbPlan {
    ChainAbsorbPlan {
        vault_id: intent.vault_id,
        chain_id: intent.chain_id,
        chain_sentinel: intent.chain_sentinel,
        icusd_ledger: intent.icusd_ledger,
        icusd_to_burn_e8s: intent.icusd_to_burn_e8s,
        stables_consumed: intent.stables_consumed.clone(),
    }
}

fn burned_chain_absorb_replay_plan(
    intent: &ChainSpAbsorbIntent,
) -> Option<(
    ChainAbsorbPlan,
    rumi_protocol_backend::icrc3_proof::SpWritedownProof,
)> {
    intent
        .burn_proof
        .clone()
        .map(|proof| (chain_absorb_plan_from_intent(intent), proof))
}

fn ensure_no_other_pending_chain_absorb(
    state: &StabilityPoolState,
    vault_id: u64,
) -> Result<(), StabilityPoolError> {
    if state.has_pending_chain_absorbs() && state.get_pending_chain_absorb(vault_id).is_none() {
        return Err(StabilityPoolError::SystemBusy);
    }
    Ok(())
}

fn expected_sp_burn_refund_memo(burn_block_index: u64, vault_id: u64) -> Vec<u8> {
    let mut memo = b"RSPRFND:".to_vec();
    memo.extend_from_slice(&burn_block_index.to_be_bytes());
    memo.extend_from_slice(&vault_id.to_be_bytes());
    memo
}

fn sp_burn_refund_memo_matches(memo: &[u8], burn_block_index: u64, vault_id: u64) -> bool {
    let original = expected_sp_burn_refund_memo(burn_block_index, vault_id);
    if memo == original {
        return true;
    }

    const ROTATED_MEMO_LEN: usize = 32;
    if memo.len() != ROTATED_MEMO_LEN || !memo.starts_with(&original) {
        return false;
    }
    let Ok(attempt) = <[u8; 8]>::try_from(&memo[original.len()..]) else {
        return false;
    };
    let attempt = u64::from_be_bytes(attempt);
    attempt > 0
        && attempt <= rumi_protocol_backend::state::MAX_SP_BURN_REFUND_ATTEMPTS as u64
}

pub(crate) async fn refund_and_verify_sp_burn(
    protocol_id: Principal,
    vault_id: u64,
    amount_e8s: u64,
    ledger: Principal,
    proof: rumi_protocol_backend::icrc3_proof::SpWritedownProof,
) -> Result<rumi_protocol_backend::sp_burn_refund::SpBurnRefundReceipt, StabilityPoolError> {
    let (result,): (
        Result<
            rumi_protocol_backend::sp_burn_refund::SpBurnRefundReceipt,
            rumi_protocol_backend::ProtocolError,
        >,
    ) = call(
        protocol_id,
        "refund_stability_pool_burn",
        (vault_id, amount_e8s, proof.clone()),
    )
    .await
    .map_err(|_| StabilityPoolError::InterCanisterCallFailed {
        target: protocol_id.to_string(),
        method: "refund_stability_pool_burn".into(),
    })?;
    let receipt = result.map_err(|error| StabilityPoolError::LiquidationFailed {
        vault_id,
        reason: format!("backend rejected exact icUSD burn refund: {error:?}"),
    })?;
    if receipt.vault_id != vault_id
        || receipt.amount_e8s != amount_e8s
        || receipt.ledger != ledger
        || receipt.recipient != ic_cdk::id()
        || receipt.burn_block_index != proof.block_index
        || !sp_burn_refund_memo_matches(&receipt.refund_memo, proof.block_index, vault_id)
    {
        return Err(StabilityPoolError::LedgerTransferFailed {
            reason: "backend refund receipt does not match the exact burned principal".into(),
        });
    }
    rumi_protocol_backend::icrc3_proof::verify_icrc3_transfer_block(
        ledger,
        receipt.refund_block_index,
        None,
        Account {
            owner: ic_cdk::id(),
            subaccount: None,
        },
        amount_e8s,
        Some(&receipt.refund_memo),
        Some(receipt.refund_created_at_time),
    )
    .await
    .map_err(|error| StabilityPoolError::LedgerTransferFailed {
        reason: format!("exact refund mint block verification failed: {error}"),
    })?;
    Ok(receipt)
}

fn ensure_no_other_pending_pool_absorb_for_chain(
    state: &StabilityPoolState,
    vault_id: u64,
) -> Result<(), StabilityPoolError> {
    ensure_no_other_pending_chain_absorb(state, vault_id)?;
    if state.has_pending_native_xrp_absorbs() {
        return Err(StabilityPoolError::SystemBusy);
    }
    Ok(())
}

fn ensure_no_other_pending_pool_absorb_for_native_xrp(
    state: &StabilityPoolState,
    vault_id: u64,
) -> Result<(), StabilityPoolError> {
    if state.has_pending_chain_absorbs() {
        return Err(StabilityPoolError::SystemBusy);
    }
    if state.has_pending_native_xrp_absorbs()
        && state.get_pending_native_xrp_absorb(vault_id).is_none()
    {
        return Err(StabilityPoolError::SystemBusy);
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NativeXrpAbsorbPlan {
    pub vault_id: u64,
    pub collateral_type: Principal,
    pub icusd_ledger: Principal,
    pub icusd_minting_account: Account,
    pub icusd_to_burn_e8s: u64,
    pub stables_consumed: BTreeMap<Principal, u64>,
    pub collateral_received_drops: u64,
    pub collateral_price_e8s: u64,
    pub allocations: Vec<XrpSpPayoutAllocation>,
}

fn native_xrp_intent_matches_plan(
    intent: &NativeXrpAbsorbIntent,
    plan: &NativeXrpAbsorbPlan,
) -> bool {
    intent.vault_id == plan.vault_id
        && intent.collateral_type == plan.collateral_type
        && intent.icusd_ledger == plan.icusd_ledger
        && intent.icusd_minting_account == plan.icusd_minting_account
        && intent.icusd_to_burn_e8s == plan.icusd_to_burn_e8s
        && intent.stables_consumed == plan.stables_consumed
        && intent.collateral_received_drops == plan.collateral_received_drops
        && intent.collateral_price_e8s == plan.collateral_price_e8s
        && intent.allocations == plan.allocations
}

pub(crate) fn native_xrp_request_from_intent(
    intent: &NativeXrpAbsorbIntent,
    proof: rumi_protocol_backend::icrc3_proof::SpWritedownProof,
) -> XrpSpAbsorbRequest {
    XrpSpAbsorbRequest {
        vault_id: intent.vault_id,
        icusd_burned_e8s: intent.icusd_to_burn_e8s,
        proof,
        allocations: intent.allocations.clone(),
    }
}

pub(crate) fn prepare_or_reuse_native_xrp_absorb_intent_in_state(
    state: &mut StabilityPoolState,
    plan: &NativeXrpAbsorbPlan,
    now_ns: u64,
) -> Result<NativeXrpAbsorbIntent, StabilityPoolError> {
    if let Some(existing) = state.get_pending_native_xrp_absorb(plan.vault_id) {
        if native_xrp_intent_matches_plan(&existing, plan) {
            return Ok(existing);
        }
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id: plan.vault_id,
            reason: "pending native XRP absorb intent conflicts with current preflight".to_string(),
        });
    }

    let intent = NativeXrpAbsorbIntent {
        vault_id: plan.vault_id,
        collateral_type: plan.collateral_type,
        icusd_ledger: plan.icusd_ledger,
        icusd_minting_account: plan.icusd_minting_account,
        icusd_to_burn_e8s: plan.icusd_to_burn_e8s,
        stables_consumed: plan.stables_consumed.clone(),
        collateral_received_drops: plan.collateral_received_drops,
        collateral_price_e8s: plan.collateral_price_e8s,
        allocations: plan.allocations.clone(),
        burn_created_at_time_ns: now_ns,
        burn_attempted: Some(false),
        status: NativeXrpAbsorbIntentStatus::Prepared,
        burn_proof: None,
        backend_result: None,
        last_error: None,
        created_at_ns: now_ns,
        updated_at_ns: now_ns,
    };
    state.put_pending_native_xrp_absorb(intent.clone())?;
    Ok(intent)
}

pub(crate) fn mark_native_xrp_absorb_burned_in_state(
    state: &mut StabilityPoolState,
    vault_id: u64,
    proof: rumi_protocol_backend::icrc3_proof::SpWritedownProof,
    now_ns: u64,
) -> Result<NativeXrpAbsorbIntent, StabilityPoolError> {
    let mut intent = state
        .get_pending_native_xrp_absorb(vault_id)
        .ok_or_else(|| StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: "missing pending native XRP absorb intent".to_string(),
        })?;
    if let Some(existing) = &intent.burn_proof {
        if existing != &proof {
            return Err(StabilityPoolError::LiquidationFailed {
                vault_id,
                reason: "pending native XRP absorb burn proof conflicts with retry proof"
                    .to_string(),
            });
        }
    }
    intent.burn_proof = Some(proof);
    intent.burn_attempted = Some(true);
    intent.status = NativeXrpAbsorbIntentStatus::Burned;
    intent.last_error = None;
    intent.updated_at_ns = now_ns;
    state.put_pending_native_xrp_absorb(intent.clone())?;
    Ok(intent)
}

pub(crate) fn mark_native_xrp_absorb_burn_attempted_in_state(
    state: &mut StabilityPoolState,
    vault_id: u64,
    now_ns: u64,
) -> Result<NativeXrpAbsorbIntent, StabilityPoolError> {
    let mut intent = state
        .get_pending_native_xrp_absorb(vault_id)
        .ok_or_else(|| StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: "missing pending native XRP absorb intent".into(),
        })?;
    match intent.burn_attempted {
        Some(false) => intent.burn_attempted = Some(true),
        Some(true) => {
            return Err(StabilityPoolError::LiquidationFailed {
                vault_id,
                reason: "native XRP burn was already dispatched; exact ledger evidence is required"
                    .into(),
            });
        }
        None => {
            return Err(StabilityPoolError::LiquidationFailed {
                vault_id,
                reason: "legacy native XRP burn has unknown dispatch history; held fail-closed"
                    .into(),
            });
        }
    }
    intent.updated_at_ns = now_ns;
    state.put_pending_native_xrp_absorb(intent.clone())?;
    Ok(intent)
}

fn cancel_first_definitive_native_burn_failure_in_state(
    state: &mut StabilityPoolState,
    expected: &NativeXrpAbsorbIntent,
) -> bool {
    let Some(mut current) = state.get_pending_native_xrp_absorb(expected.vault_id) else {
        return false;
    };
    let exact_first_failure = expected.burn_attempted == Some(false)
        && current.burn_attempted == Some(true)
        && current.burn_proof.is_none()
        && current.backend_result.is_none()
        && current.icusd_ledger == expected.icusd_ledger
        && current.icusd_to_burn_e8s == expected.icusd_to_burn_e8s
        && current.burn_created_at_time_ns == expected.burn_created_at_time_ns;
    if !exact_first_failure {
        return false;
    }
    current.burn_attempted = Some(false);
    if state.put_pending_native_xrp_absorb(current).is_err() {
        return false;
    }
    clear_unburned_native_xrp_absorb_intent_in_state(state, expected.vault_id)
}

pub(crate) fn clear_refunded_native_xrp_absorb_in_state(
    state: &mut StabilityPoolState,
    expected: &NativeXrpAbsorbIntent,
) -> bool {
    let Some(current) = state.get_pending_native_xrp_absorb(expected.vault_id) else {
        return false;
    };
    if current.vault_id == expected.vault_id
        && current.icusd_ledger == expected.icusd_ledger
        && current.icusd_to_burn_e8s == expected.icusd_to_burn_e8s
        && current.burn_created_at_time_ns == expected.burn_created_at_time_ns
        && current.burn_proof == expected.burn_proof
        && current.backend_result.is_none()
        && current.burn_proof.is_some()
    {
        state.take_pending_native_xrp_absorb(expected.vault_id);
        true
    } else {
        false
    }
}

pub(crate) fn mark_native_xrp_absorb_backend_result_in_state(
    state: &mut StabilityPoolState,
    vault_id: u64,
    result: XrpSpAbsorbResult,
    now_ns: u64,
) -> Result<NativeXrpAbsorbIntent, StabilityPoolError> {
    let mut intent = state
        .get_pending_native_xrp_absorb(vault_id)
        .ok_or_else(|| StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: "missing pending native XRP absorb intent".to_string(),
        })?;
    if let Some(existing) = &intent.backend_result {
        if existing != &result {
            return Err(StabilityPoolError::LiquidationFailed {
                vault_id,
                reason: "pending native XRP absorb backend result conflicts with retry result"
                    .to_string(),
            });
        }
    }
    intent.backend_result = Some(result);
    intent.status = NativeXrpAbsorbIntentStatus::BackendAccepted;
    intent.last_error = None;
    intent.updated_at_ns = now_ns;
    state.put_pending_native_xrp_absorb(intent.clone())?;
    Ok(intent)
}

pub(crate) fn mark_native_xrp_absorb_error_in_state(
    state: &mut StabilityPoolState,
    vault_id: u64,
    status: NativeXrpAbsorbIntentStatus,
    reason: String,
    now_ns: u64,
) {
    if let Some(mut intent) = state.get_pending_native_xrp_absorb(vault_id) {
        intent.status = status;
        intent.last_error = Some(reason);
        intent.updated_at_ns = now_ns;
        let _ = state.put_pending_native_xrp_absorb(intent);
    }
}

/// Abandon a native-XRP absorb attempt that reserved a backend preflight but
/// has NOT burned any icUSD: drop the local intent and hand the reservation
/// back. Every call site is positioned strictly before the burn (or on a burn
/// that returned an error, which the local clear already treats as unburned),
/// so releasing can never strand an in-flight burn.
async fn abandon_unburned_native_xrp_absorb(
    io: &mut dyn NativeXrpAbsorbIo,
    protocol_id: Principal,
    vault_id: u64,
    icusd_burn_e8s: u64,
) {
    mutate_state(|s| {
        clear_unburned_native_xrp_absorb_intent_in_state(s, vault_id);
    });
    io.release_xrp_absorb_preflight(protocol_id, vault_id, icusd_burn_e8s)
        .await;
}

pub(crate) fn clear_unburned_native_xrp_absorb_intent_in_state(
    state: &mut StabilityPoolState,
    vault_id: u64,
) -> bool {
    let Some(intent) = state.get_pending_native_xrp_absorb(vault_id) else {
        return false;
    };
    if intent.status == NativeXrpAbsorbIntentStatus::Prepared
        && intent.burn_attempted == Some(false)
        && intent.burn_proof.is_none()
        && intent.backend_result.is_none()
    {
        state.take_pending_native_xrp_absorb(vault_id);
        return true;
    }
    false
}

pub(crate) fn apply_native_xrp_absorb_success_in_state_at(
    state: &mut StabilityPoolState,
    intent: &NativeXrpAbsorbIntent,
    now_ns: u64,
) -> Result<LiquidationResult, StabilityPoolError> {
    let backend_result =
        intent
            .backend_result
            .clone()
            .ok_or_else(|| StabilityPoolError::LiquidationFailed {
                vault_id: intent.vault_id,
                reason: "missing accepted native XRP backend result".to_string(),
            })?;
    validate_xrp_absorb_backend_result(
        intent.vault_id,
        intent.icusd_to_burn_e8s,
        intent.collateral_received_drops,
        &intent.allocations,
        &backend_result,
    )?;
    state.process_native_xrp_absorb_success_at(
        intent.vault_id,
        intent.collateral_type,
        &intent.stables_consumed,
        intent.collateral_received_drops,
        &backend_result.payout_claims,
        now_ns,
    )?;
    state.take_pending_native_xrp_absorb(intent.vault_id);

    // Emit the same audit event the generic ICRC path emits. Without it the
    // only Explorer trace of an absorb is `LiquidationNotification`, which
    // carries a bare vault count and cannot distinguish a completed absorb
    // from one that failed. `collateral_gained` is in drops (6-decimal), so
    // consumers must format it with the collateral's own decimals.
    let stables_consumed_e8s: u64 = intent.stables_consumed.values().sum();
    state.push_event_at(
        state.protocol_canister_id,
        PoolEventType::LiquidationExecuted {
            vault_id: intent.vault_id,
            stables_consumed_e8s,
            collateral_gained: intent.collateral_received_drops,
            collateral_type: intent.collateral_type,
            success: true,
        },
        now_ns,
    );

    Ok(LiquidationResult {
        vault_id: intent.vault_id,
        stables_consumed: intent.stables_consumed.clone(),
        collateral_gained: intent.collateral_received_drops,
        collateral_type: intent.collateral_type,
        success: true,
        error_message: None,
    })
}

#[async_trait::async_trait(?Send)]
pub(crate) trait NativeXrpAbsorbIo {
    fn now_ns(&self) -> u64;

    async fn fetch_icusd_minting_account(
        &mut self,
        icusd_ledger: Principal,
    ) -> Result<Account, StabilityPoolError>;

    async fn preflight_xrp_absorb(
        &mut self,
        protocol_id: Principal,
        vault_id: u64,
        expected_icusd_burn_e8s: u64,
    ) -> Result<XrpSpAbsorbPreflight, StabilityPoolError>;

    async fn burn_icusd(
        &mut self,
        icusd_ledger: Principal,
        minting_account: Account,
        amount_e8s: u64,
        vault_id: u64,
        created_at_time: u64,
    ) -> Result<rumi_protocol_backend::icrc3_proof::SpWritedownProof, IcusdBurnAttemptError>;

    async fn submit_xrp_absorb(
        &mut self,
        protocol_id: Principal,
        request: XrpSpAbsorbRequest,
    ) -> Result<XrpSpAbsorbResult, StabilityPoolError>;

    /// Hand an unburned reservation back to the backend. Best-effort: the
    /// caller is already on a failure path, and the reservation expires on its
    /// own, so a failed release is logged rather than propagated.
    async fn release_xrp_absorb_preflight(
        &mut self,
        protocol_id: Principal,
        vault_id: u64,
        icusd_burn_e8s: u64,
    );
}

/// IO seam for the native-XRP auto-settlement sweep, so the tick logic is
/// unit-testable off-canister (mirrors `NativeXrpAbsorbIo`).
#[async_trait::async_trait(?Send)]
pub(crate) trait NativeXrpSettleSweepIo {
    /// Backend `stability_pool_xrp_claim_outstanding`: does the claim still
    /// exist for this claimant? `false` means settled+validated (or resolved
    /// by an admin), so the SP-side reminder can be dropped.
    async fn claim_outstanding(
        &mut self,
        protocol: Principal,
        claim_id: u64,
        claimant: Principal,
    ) -> Result<bool, StabilityPoolError>;

    /// Backend `stability_pool_settle_xrp_claim`: sign + submit the XRPL
    /// Payment for the claim to the depositor's registered address. Repeat
    /// calls are safe: the backend confirms a previously-submitted Payment
    /// before ever signing a new one.
    async fn settle_on_behalf(
        &mut self,
        protocol: Principal,
        claim_id: u64,
        claimant: Principal,
        destination: String,
        destination_tag: Option<u32>,
    ) -> Result<String, StabilityPoolError>;
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct NativeXrpSettleSweepSummary {
    pub examined: usize,
    pub acked: usize,
    pub submitted: usize,
    pub failed: usize,
    /// Submits that XRPL rejected with `tefPAST_SEQ`. The Payment is almost
    /// certainly ON-LEDGER: IC https-outcalls fan out to every replica, all
    /// POST the same signed blob, and the nodes that lose the race report
    /// "sequence already used" against our own applied transaction. The claim
    /// keeps its recorded settlement, so the next tick confirms it.
    pub pending_confirmation: usize,
    /// Cursor for the next tick: the last claim id this tick examined.
    pub last_claim_id: Option<u64>,
}

/// True when a settle error is XRPL's `tefPAST_SEQ`, which for this rail means
/// "already applied" rather than "did not send" (see
/// `NativeXrpSettleSweepSummary::pending_confirmation`).
fn is_past_sequence_submit(error: &StabilityPoolError) -> bool {
    matches!(
        error,
        StabilityPoolError::LiquidationFailed { reason, .. } if reason.contains("tefPAST_SEQ")
    )
}

/// One bounded tick of the native-XRP payout settlement sweep.
///
/// Depositors opted into XRP absorption by registering an XRPL address; the
/// product promise is that liquidation proceeds REACH that address, not that a
/// claim waits for a manual click. This walks pending payouts in claim-id
/// order, starting after `start_after_claim_id` and wrapping, so one
/// perpetually-failing claim (bad address, quarantined backend claim) cannot
/// starve the rest.
///
/// Per payout: if the backend no longer knows the claim it was settled and
/// validated, so the local reminder is dropped; otherwise settlement is
/// (re)submitted with the stored address. Records are never removed on the
/// submit path — a later tick observes the validated settlement and acks.
pub(crate) async fn run_native_xrp_settle_sweep_with_io(
    io: &mut dyn NativeXrpSettleSweepIo,
    start_after_claim_id: Option<u64>,
    max_per_tick: usize,
) -> NativeXrpSettleSweepSummary {
    let mut summary = NativeXrpSettleSweepSummary::default();
    if read_state(|s| s.configuration.emergency_pause) {
        return summary;
    }
    let (protocol, all) =
        read_state(|s| (s.protocol_canister_id, s.all_native_xrp_pending_payouts()));
    if all.is_empty() {
        return summary;
    }

    // Rotate: entries strictly after the cursor first, then wrap.
    let split = match start_after_claim_id {
        Some(cursor) => all.partition_point(|(_, p)| p.claim_id <= cursor),
        None => 0,
    };
    let ordered = all[split..].iter().chain(all[..split].iter());

    for (user, payout) in ordered.take(max_per_tick) {
        summary.examined += 1;
        summary.last_claim_id = Some(payout.claim_id);

        let outstanding = match io.claim_outstanding(protocol, payout.claim_id, *user).await {
            Ok(v) => v,
            Err(error) => {
                log!(
                    INFO,
                    "[xrp-settle-sweep] outstanding check failed for claim {}: {:?}",
                    payout.claim_id,
                    error
                );
                summary.failed += 1;
                continue;
            }
        };

        if !outstanding {
            // Settled and validated (by a prior sweep tick or a manual click).
            let _ = mutate_state(|s| s.ack_native_xrp_payout_settled(user, payout.claim_id));
            summary.acked += 1;
            continue;
        }

        match io
            .settle_on_behalf(
                protocol,
                payout.claim_id,
                *user,
                payout.payout_address.clone(),
                payout.destination_tag,
            )
            .await
        {
            Ok(tx_hash) => {
                log!(
                    INFO,
                    "[xrp-settle-sweep] submitted settlement for claim {} ({} drops) tx {}",
                    payout.claim_id,
                    payout.drops,
                    tx_hash
                );
                summary.submitted += 1;
            }
            Err(error) if is_past_sequence_submit(&error) => {
                log!(
                    INFO,
                    "[xrp-settle-sweep] claim {} already submitted (tefPAST_SEQ); \
                     awaiting confirmation on a later tick",
                    payout.claim_id
                );
                summary.pending_confirmation += 1;
            }
            Err(error) => {
                log!(
                    INFO,
                    "[xrp-settle-sweep] settlement failed for claim {}: {:?}",
                    payout.claim_id,
                    error
                );
                summary.failed += 1;
            }
        }
    }
    summary
}

pub(crate) struct CdkNativeXrpSettleSweepIo;

#[async_trait::async_trait(?Send)]
impl NativeXrpSettleSweepIo for CdkNativeXrpSettleSweepIo {
    async fn claim_outstanding(
        &mut self,
        protocol: Principal,
        claim_id: u64,
        claimant: Principal,
    ) -> Result<bool, StabilityPoolError> {
        let result: Result<(Result<bool, rumi_protocol_backend::ProtocolError>,), _> = call(
            protocol,
            "stability_pool_xrp_claim_outstanding",
            (claim_id, claimant),
        )
        .await;
        match result {
            Ok((Ok(outstanding),)) => Ok(outstanding),
            Ok((Err(error),)) => Err(StabilityPoolError::LiquidationFailed {
                vault_id: claim_id,
                reason: format!("backend rejected claim-outstanding check: {:?}", error),
            }),
            Err(_) => Err(StabilityPoolError::InterCanisterCallFailed {
                target: format!("{}", protocol),
                method: "stability_pool_xrp_claim_outstanding".to_string(),
            }),
        }
    }

    async fn settle_on_behalf(
        &mut self,
        protocol: Principal,
        claim_id: u64,
        claimant: Principal,
        destination: String,
        destination_tag: Option<u32>,
    ) -> Result<String, StabilityPoolError> {
        let result: Result<(Result<String, rumi_protocol_backend::ProtocolError>,), _> = call(
            protocol,
            "stability_pool_settle_xrp_claim",
            (claim_id, claimant, destination, destination_tag),
        )
        .await;
        match result {
            Ok((Ok(tx_hash),)) => Ok(tx_hash),
            Ok((Err(error),)) => Err(StabilityPoolError::LiquidationFailed {
                vault_id: claim_id,
                reason: format!("backend rejected settle-on-behalf: {:?}", error),
            }),
            Err(_) => Err(StabilityPoolError::InterCanisterCallFailed {
                target: format!("{}", protocol),
                method: "stability_pool_settle_xrp_claim".to_string(),
            }),
        }
    }
}

struct CdkNativeXrpAbsorbIo;

#[async_trait::async_trait(?Send)]
impl NativeXrpAbsorbIo for CdkNativeXrpAbsorbIo {
    fn now_ns(&self) -> u64 {
        ic_cdk::api::time()
    }

    async fn fetch_icusd_minting_account(
        &mut self,
        icusd_ledger: Principal,
    ) -> Result<Account, StabilityPoolError> {
        fetch_icusd_minting_account(icusd_ledger).await
    }

    async fn preflight_xrp_absorb(
        &mut self,
        protocol_id: Principal,
        vault_id: u64,
        expected_icusd_burn_e8s: u64,
    ) -> Result<XrpSpAbsorbPreflight, StabilityPoolError> {
        let preflight_result: Result<
            (Result<XrpSpAbsorbPreflight, rumi_protocol_backend::ProtocolError>,),
            _,
        > = call(
            protocol_id,
            "stability_pool_preflight_xrp_absorb",
            (vault_id, expected_icusd_burn_e8s),
        )
        .await;

        match preflight_result {
            Ok((Ok(preflight),)) => Ok(preflight),
            Ok((Err(error),)) => Err(StabilityPoolError::LiquidationFailed {
                vault_id,
                reason: format!("backend rejected native XRP preflight: {:?}", error),
            }),
            Err(_) => Err(StabilityPoolError::InterCanisterCallFailed {
                target: format!("{}", protocol_id),
                method: "stability_pool_preflight_xrp_absorb".to_string(),
            }),
        }
    }

    async fn burn_icusd(
        &mut self,
        icusd_ledger: Principal,
        minting_account: Account,
        amount_e8s: u64,
        vault_id: u64,
        created_at_time: u64,
    ) -> Result<rumi_protocol_backend::icrc3_proof::SpWritedownProof, IcusdBurnAttemptError> {
        burn_native_xrp_icusd_with_account(
            icusd_ledger,
            minting_account,
            amount_e8s,
            vault_id,
            created_at_time,
        )
        .await
    }

    async fn submit_xrp_absorb(
        &mut self,
        protocol_id: Principal,
        request: XrpSpAbsorbRequest,
    ) -> Result<XrpSpAbsorbResult, StabilityPoolError> {
        let vault_id = request.vault_id;
        let backend_result: Result<
            (Result<XrpSpAbsorbResult, rumi_protocol_backend::ProtocolError>,),
            _,
        > = call(
            protocol_id,
            "stability_pool_liquidate_xrp_vault",
            (request,),
        )
        .await;

        match backend_result {
            Ok((Ok(result),)) => Ok(result),
            Ok((Err(error),)) => Err(StabilityPoolError::LiquidationFailed {
                vault_id,
                reason: format!("backend rejected native XRP absorb after burn: {:?}", error),
            }),
            Err(_) => Err(StabilityPoolError::InterCanisterCallFailed {
                target: format!("{}", protocol_id),
                method: "stability_pool_liquidate_xrp_vault".to_string(),
            }),
        }
    }

    async fn release_xrp_absorb_preflight(
        &mut self,
        protocol_id: Principal,
        vault_id: u64,
        icusd_burn_e8s: u64,
    ) {
        let released: Result<(Result<bool, rumi_protocol_backend::ProtocolError>,), _> = call(
            protocol_id,
            "stability_pool_release_xrp_absorb_preflight",
            (vault_id, icusd_burn_e8s),
        )
        .await;
        match released {
            Ok((Ok(_),)) => {}
            Ok((Err(error),)) => log!(
                INFO,
                "native XRP preflight release rejected for vault {}: {:?}; reservation will expire on its own",
                vault_id,
                error
            ),
            Err((code, msg)) => log!(
                INFO,
                "native XRP preflight release call failed for vault {}: {:?} {}; reservation will expire on its own",
                vault_id,
                code,
                msg
            ),
        }
    }
}

fn xrp_claims_match_allocations(
    allocations: &[XrpSpPayoutAllocation],
    claims: &[XrpSpPayoutClaim],
) -> bool {
    allocations.len() == claims.len()
        && allocations
            .iter()
            .zip(claims.iter())
            .all(|(allocation, claim)| {
                allocation.claimant == claim.claimant
                    && allocation.payout_address == claim.payout_address
                    && allocation.destination_tag == claim.destination_tag
                    && allocation.drops == claim.drops
            })
}

pub(crate) fn validate_xrp_absorb_backend_result(
    vault_id: u64,
    icusd_burned_e8s: u64,
    collateral_received_drops: u64,
    allocations: &[XrpSpPayoutAllocation],
    result: &XrpSpAbsorbResult,
) -> Result<(), StabilityPoolError> {
    if !result.success {
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: "backend reported unsuccessful native XRP absorb".to_string(),
        });
    }
    if result.vault_id != vault_id {
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: "backend native XRP result does not match requested vault".to_string(),
        });
    }
    if result.liquidated_debt_e8s != icusd_burned_e8s {
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: "backend native XRP liquidated debt does not match SP burn".to_string(),
        });
    }
    if result.collateral_received_drops != collateral_received_drops {
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: "backend native XRP collateral does not match preflight".to_string(),
        });
    }
    if !xrp_claims_match_allocations(allocations, &result.payout_claims) {
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: "backend native XRP payout claims do not match requested allocations"
                .to_string(),
        });
    }
    Ok(())
}

fn liquidation_failure(
    vault_info: &LiquidatableVaultInfo,
    error: StabilityPoolError,
) -> LiquidationResult {
    LiquidationResult {
        vault_id: vault_info.vault_id,
        stables_consumed: BTreeMap::new(),
        collateral_gained: 0,
        collateral_type: vault_info.collateral_type,
        success: false,
        error_message: Some(format!("{:?}", error)),
    }
}

async fn submit_native_xrp_absorb_to_backend(
    protocol_id: Principal,
    intent: &NativeXrpAbsorbIntent,
    io: &mut dyn NativeXrpAbsorbIo,
) -> Result<NativeXrpAbsorbIntent, StabilityPoolError> {
    let proof = intent
        .burn_proof
        .clone()
        .ok_or_else(|| StabilityPoolError::LiquidationFailed {
            vault_id: intent.vault_id,
            reason: "missing native XRP burn proof for backend submit".to_string(),
        })?;
    let request = native_xrp_request_from_intent(intent, proof);
    match io.submit_xrp_absorb(protocol_id, request).await {
        Ok(result) => {
            if let Err(error) = validate_xrp_absorb_backend_result(
                intent.vault_id,
                intent.icusd_to_burn_e8s,
                intent.collateral_received_drops,
                &intent.allocations,
                &result,
            ) {
                let reason = format!("{:?}", error);
                mutate_state(|s| {
                    mark_native_xrp_absorb_error_in_state(
                        s,
                        intent.vault_id,
                        NativeXrpAbsorbIntentStatus::BackendRejected,
                        reason,
                        io.now_ns(),
                    );
                });
                return Err(error);
            }
            mutate_state(|s| {
                mark_native_xrp_absorb_backend_result_in_state(
                    s,
                    intent.vault_id,
                    result,
                    io.now_ns(),
                )
            })
        }
        Err(error) => {
            let status = match &error {
                StabilityPoolError::LiquidationFailed { .. } => {
                    NativeXrpAbsorbIntentStatus::BackendRejected
                }
                _ => NativeXrpAbsorbIntentStatus::Burned,
            };
            let reason = format!("{:?}", error);
            mutate_state(|s| {
                mark_native_xrp_absorb_error_in_state(
                    s,
                    intent.vault_id,
                    status,
                    reason,
                    io.now_ns(),
                );
            });
            Err(error)
        }
    }
}

pub(crate) async fn reconcile_pending_native_xrp_absorb_from_status(
    vault_id: u64,
) -> Result<(), StabilityPoolError> {
    let _liquidation_guard = crate::pool_guard::SpLiquidationGuard::new()?;
    if read_state(|state| !state.in_flight_liquidations.is_empty()) {
        return Err(StabilityPoolError::SystemBusy);
    }
    let intent =
        read_state(|state| state.get_pending_native_xrp_absorb(vault_id)).ok_or_else(|| {
            StabilityPoolError::LiquidationFailed {
                vault_id,
                reason: "missing pending native XRP absorb intent".into(),
            }
        })?;
    let proof = intent
        .burn_proof
        .clone()
        .ok_or_else(|| StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: "pending native XRP absorb has no exact burn proof".into(),
        })?;
    if intent.backend_result.is_none()
        && !matches!(
            intent.status,
            NativeXrpAbsorbIntentStatus::Burned | NativeXrpAbsorbIntentStatus::BackendRejected
        )
    {
        return Err(StabilityPoolError::SystemBusy);
    }
    mutate_state(|state| {
        state.in_flight_liquidations.insert(vault_id);
    });

    let recovery = async {
        let protocol_id = read_state(|state| state.protocol_canister_id);
        let status = if let Some(result) = intent.backend_result.clone() {
            rumi_protocol_backend::XrpSpAbsorbStatus::Accepted(result)
        } else {
            let request = native_xrp_request_from_intent(&intent, proof.clone());
            let status_call: Result<
                (
                    Result<
                        rumi_protocol_backend::XrpSpAbsorbStatus,
                        rumi_protocol_backend::ProtocolError,
                    >,
                ),
                _,
            > = call(
                protocol_id,
                "stability_pool_xrp_absorb_status",
                (request,),
            )
            .await;
            match status_call {
                Ok((Ok(status),)) => status,
                Ok((Err(error),)) => {
                    return Err(StabilityPoolError::LiquidationFailed {
                        vault_id,
                        reason: format!("backend could not resolve native XRP absorb: {error:?}"),
                    });
                }
                Err(_) => {
                    return Err(StabilityPoolError::InterCanisterCallFailed {
                        target: protocol_id.to_string(),
                        method: "stability_pool_xrp_absorb_status".into(),
                    });
                }
            }
        };
        if read_state(|state| state.get_pending_native_xrp_absorb(vault_id))
            != Some(intent.clone())
        {
            return Err(StabilityPoolError::LiquidationFailed {
                vault_id,
                reason: "pending native XRP intent changed during backend lookup".into(),
            });
        }
        match status {
            rumi_protocol_backend::XrpSpAbsorbStatus::Accepted(result) => {
                validate_xrp_absorb_backend_result(
                    vault_id,
                    intent.icusd_to_burn_e8s,
                    intent.collateral_received_drops,
                    &intent.allocations,
                    &result,
                )?;
                let accepted = mutate_state(|state| {
                    mark_native_xrp_absorb_backend_result_in_state(
                        state,
                        vault_id,
                        result,
                        ic_cdk::api::time(),
                    )
                })?;
                mutate_state(|state| {
                    apply_native_xrp_absorb_success_in_state_at(
                        state,
                        &accepted,
                        ic_cdk::api::time(),
                    )
                })?;
                Ok(())
            }
            rumi_protocol_backend::XrpSpAbsorbStatus::Unseen
            | rumi_protocol_backend::XrpSpAbsorbStatus::RefundJournaled => {
                let receipt = refund_and_verify_sp_burn(
                    protocol_id,
                    vault_id,
                    intent.icusd_to_burn_e8s,
                    intent.icusd_ledger,
                    proof.clone(),
                )
                .await?;
                if read_state(|state| state.get_pending_native_xrp_absorb(vault_id))
                    != Some(intent.clone())
                {
                    return Err(StabilityPoolError::LiquidationFailed {
                        vault_id,
                        reason: "pending native XRP intent changed while verifying its refund".into(),
                    });
                }
                if !mutate_state(|state| clear_refunded_native_xrp_absorb_in_state(state, &intent)) {
                    return Err(StabilityPoolError::LiquidationFailed {
                        vault_id,
                        reason: format!(
                            "refund block {} verified but exact native XRP intent could not be cleared",
                            receipt.refund_block_index
                        ),
                    });
                }
                let release: Result<(Result<bool, rumi_protocol_backend::ProtocolError>,), _> =
                    call(
                        protocol_id,
                        "stability_pool_release_xrp_absorb_preflight",
                        (vault_id, intent.icusd_to_burn_e8s),
                    )
                    .await;
                if let Err(error) = release {
                    ic_cdk::println!(
                        "native XRP preflight release after verified refund could not be confirmed: {:?}",
                        error
                    );
                }
                Ok(())
            }
            rumi_protocol_backend::XrpSpAbsorbStatus::ConsumedWithoutResult => {
                mutate_state(|state| {
                    mark_native_xrp_absorb_error_in_state(
                        state,
                        vault_id,
                        NativeXrpAbsorbIntentStatus::Burned,
                        "backend reports proof consumed without exact absorb or refund result"
                            .into(),
                        ic_cdk::api::time(),
                    );
                });
                Err(StabilityPoolError::LiquidationFailed {
                    vault_id,
                    reason: "backend consumed this burn without a recoverable terminal result; intent remains held".into(),
                })
            }
        }
    }
    .await;

    mutate_state(|state| {
        state.in_flight_liquidations.remove(&vault_id);
    });
    recovery
}

/// Select proof-bearing recovery intents with a rotating, bounded cursor.
/// Intentionally does not consult liquidatable-vault discovery.
pub(crate) fn pending_native_xrp_recovery_vault_ids(
    start_after_vault_id: Option<u64>,
    max_per_tick: usize,
) -> Vec<u64> {
    if max_per_tick == 0 {
        return Vec::new();
    }
    let mut ids = read_state(|state| {
        state
            .pending_native_xrp_absorbs()
            .into_iter()
            .filter(|intent| {
                intent.burn_proof.is_some()
                    && (intent.backend_result.is_some()
                        || matches!(
                            intent.status,
                            NativeXrpAbsorbIntentStatus::Burned
                                | NativeXrpAbsorbIntentStatus::BackendRejected
                        ))
            })
            .map(|intent| intent.vault_id)
            .collect::<Vec<_>>()
    });
    let split = start_after_vault_id
        .map(|cursor| ids.partition_point(|vault_id| *vault_id <= cursor))
        .unwrap_or(0);
    ids.rotate_left(split);
    ids.truncate(max_per_tick);
    ids
}

pub(crate) async fn execute_native_xrp_absorb_with_io(
    vault_info: &LiquidatableVaultInfo,
    io: &mut dyn NativeXrpAbsorbIo,
) -> LiquidationResult {
    if let Err(error) = crate::ensure_no_pool_balance_async_in_flight() {
        return liquidation_failure(vault_info, error);
    }
    if let Err(error) =
        read_state(|s| ensure_no_other_pending_pool_absorb_for_native_xrp(s, vault_info.vault_id))
    {
        return liquidation_failure(vault_info, error);
    }

    if let Some(intent) = read_state(|s| s.get_pending_native_xrp_absorb(vault_info.vault_id)) {
        if intent.backend_result.is_some() {
            return match mutate_state(|s| {
                apply_native_xrp_absorb_success_in_state_at(s, &intent, io.now_ns())
            }) {
                Ok(result) => result,
                Err(error) => liquidation_failure(vault_info, error),
            };
        }
        if intent.burn_proof.is_some() {
            let protocol_id = read_state(|s| s.protocol_canister_id);
            let accepted = match submit_native_xrp_absorb_to_backend(protocol_id, &intent, io).await
            {
                Ok(intent) => intent,
                Err(error) => return liquidation_failure(vault_info, error),
            };
            return match mutate_state(|s| {
                apply_native_xrp_absorb_success_in_state_at(s, &accepted, io.now_ns())
            }) {
                Ok(result) => result,
                Err(error) => liquidation_failure(vault_info, error),
            };
        }
    }

    let current = match read_state(|s| {
        if !s.collateral_requires_payout_address(&vault_info.collateral_type) {
            return Err(StabilityPoolError::PayoutAddressRequired {
                collateral: vault_info.collateral_type,
            });
        }
        if let Some(intent) = s.get_pending_native_xrp_absorb(vault_info.vault_id) {
            return Ok((
                s.protocol_canister_id,
                intent.icusd_ledger,
                Some(intent.icusd_minting_account),
                intent.icusd_to_burn_e8s,
                intent.stables_consumed,
            ));
        }
        let draw_amount = if vault_info.recommended_liquidation_amount > 0 {
            vault_info.recommended_liquidation_amount
        } else {
            vault_info.debt_amount
        };
        let icusd_ledger = s
            .icusd_ledger()
            .ok_or(StabilityPoolError::TokenNotAccepted {
                ledger: Principal::anonymous(),
            })?;
        let icusd_to_burn_e8s =
            draw_amount.min(s.effective_icusd_pool_for_collateral(&vault_info.collateral_type));
        if icusd_to_burn_e8s == 0 {
            return Err(StabilityPoolError::InsufficientPoolBalance);
        }
        let mut stables_consumed = BTreeMap::new();
        stables_consumed.insert(icusd_ledger, icusd_to_burn_e8s);
        Ok((
            s.protocol_canister_id,
            icusd_ledger,
            None,
            icusd_to_burn_e8s,
            stables_consumed,
        ))
    }) {
        Ok(current) => current,
        Err(error) => return liquidation_failure(vault_info, error),
    };
    let (protocol_id, icusd_ledger, existing_minting_account, icusd_to_burn_e8s, stables_consumed) =
        current;

    let preflight = match io
        .preflight_xrp_absorb(protocol_id, vault_info.vault_id, icusd_to_burn_e8s)
        .await
    {
        Ok(preflight) => preflight,
        Err(error) => {
            mutate_state(|s| {
                clear_unburned_native_xrp_absorb_intent_in_state(s, vault_info.vault_id);
            });
            return liquidation_failure(vault_info, error);
        }
    };
    if preflight.vault_id != vault_info.vault_id || preflight.icusd_burn_e8s != icusd_to_burn_e8s {
        abandon_unburned_native_xrp_absorb(
            io,
            protocol_id,
            vault_info.vault_id,
            icusd_to_burn_e8s,
        )
        .await;
        return liquidation_failure(
            vault_info,
            StabilityPoolError::LiquidationFailed {
                vault_id: vault_info.vault_id,
                reason: "native XRP preflight does not match requested burn".to_string(),
            },
        );
    }

    let allocations = match read_state(|s| {
        s.build_native_xrp_payout_allocations(
            vault_info.collateral_type,
            &stables_consumed,
            preflight.collateral_received_drops,
        )
    }) {
        Ok(allocations) if !allocations.is_empty() => allocations
            .into_iter()
            .map(XrpSpPayoutAllocation::from)
            .collect::<Vec<_>>(),
        Ok(_) => {
            abandon_unburned_native_xrp_absorb(
                io,
                protocol_id,
                vault_info.vault_id,
                icusd_to_burn_e8s,
            )
            .await;
            return liquidation_failure(
                vault_info,
                StabilityPoolError::LiquidationFailed {
                    vault_id: vault_info.vault_id,
                    reason: "native XRP absorb produced no payout allocations".to_string(),
                },
            );
        }
        Err(error) => {
            abandon_unburned_native_xrp_absorb(
                io,
                protocol_id,
                vault_info.vault_id,
                icusd_to_burn_e8s,
            )
            .await;
            return liquidation_failure(vault_info, error);
        }
    };

    let minting_account = if let Some(account) = existing_minting_account {
        account
    } else {
        match io.fetch_icusd_minting_account(icusd_ledger).await {
            Ok(account) => account,
            Err(error) => {
                abandon_unburned_native_xrp_absorb(
                    io,
                    protocol_id,
                    vault_info.vault_id,
                    icusd_to_burn_e8s,
                )
                .await;
                return liquidation_failure(vault_info, error);
            }
        }
    };

    let plan = NativeXrpAbsorbPlan {
        vault_id: vault_info.vault_id,
        collateral_type: vault_info.collateral_type,
        icusd_ledger,
        icusd_minting_account: minting_account,
        icusd_to_burn_e8s,
        stables_consumed,
        collateral_received_drops: preflight.collateral_received_drops,
        collateral_price_e8s: preflight.collateral_price_e8s,
        allocations,
    };
    let now = io.now_ns();
    let mut intent =
        match mutate_state(|s| prepare_or_reuse_native_xrp_absorb_intent_in_state(s, &plan, now)) {
            Ok(intent) => intent,
            Err(error) => return liquidation_failure(vault_info, error),
        };

    let proof = if let Some(proof) = intent.burn_proof.clone() {
        proof
    } else {
        let first_attempt = intent.burn_attempted == Some(false);
        let attempt_identity = intent.clone();
        intent = match mutate_state(|s| {
            mark_native_xrp_absorb_burn_attempted_in_state(s, vault_info.vault_id, io.now_ns())
        }) {
            Ok(intent) => intent,
            Err(error) => return liquidation_failure(vault_info, error),
        };
        match io
            .burn_icusd(
                intent.icusd_ledger,
                intent.icusd_minting_account,
                intent.icusd_to_burn_e8s,
                intent.vault_id,
                intent.burn_created_at_time_ns,
            )
            .await
        {
            Ok(proof) => {
                intent = match mutate_state(|s| {
                    mark_native_xrp_absorb_burned_in_state(
                        s,
                        vault_info.vault_id,
                        proof.clone(),
                        io.now_ns(),
                    )
                }) {
                    Ok(intent) => intent,
                    Err(error) => return liquidation_failure(vault_info, error),
                };
                proof
            }
            Err(failure) => {
                let cleared = if first_attempt && failure.definitive_no_effect {
                    mutate_state(|s| {
                        cancel_first_definitive_native_burn_failure_in_state(s, &attempt_identity)
                    })
                } else {
                    mutate_state(|s| {
                        mark_native_xrp_absorb_error_in_state(
                            s,
                            vault_info.vault_id,
                            NativeXrpAbsorbIntentStatus::Prepared,
                            format!("ambiguous icUSD burn result: {:?}", failure.error),
                            io.now_ns(),
                        );
                        false
                    })
                };
                if cleared {
                    io.release_xrp_absorb_preflight(
                        protocol_id,
                        vault_info.vault_id,
                        icusd_to_burn_e8s,
                    )
                    .await;
                }
                return liquidation_failure(vault_info, failure.error);
            }
        }
    };
    intent.burn_proof = Some(proof);

    let accepted = match submit_native_xrp_absorb_to_backend(protocol_id, &intent, io).await {
        Ok(intent) => intent,
        Err(error) => return liquidation_failure(vault_info, error),
    };
    match mutate_state(|s| apply_native_xrp_absorb_success_in_state_at(s, &accepted, io.now_ns())) {
        Ok(result) => result,
        Err(error) => liquidation_failure(vault_info, error),
    }
}

pub(crate) fn prepare_or_reuse_chain_absorb_intent_in_state(
    state: &mut StabilityPoolState,
    plan: &ChainAbsorbPlan,
    minting_account: Account,
    now_ns: u64,
) -> Result<ChainSpAbsorbIntent, StabilityPoolError> {
    if let Some(completion) = state.completed_chain_absorb(plan.vault_id) {
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id: plan.vault_id,
            reason: format!(
                "chain absorb already completed at block {}",
                completion.result.block_index
            ),
        });
    }

    if let Some(existing) = state.get_pending_chain_absorb(plan.vault_id) {
        if intent_matches_plan(&existing, plan, minting_account) {
            return Ok(existing);
        }
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id: plan.vault_id,
            reason: "pending chain absorb intent conflicts with current preflight".to_string(),
        });
    }

    let intent = ChainSpAbsorbIntent {
        vault_id: plan.vault_id,
        chain_id: plan.chain_id,
        chain_sentinel: plan.chain_sentinel,
        icusd_ledger: plan.icusd_ledger,
        icusd_minting_account: minting_account,
        icusd_to_burn_e8s: plan.icusd_to_burn_e8s,
        stables_consumed: plan.stables_consumed.clone(),
        burn_created_at_time_ns: now_ns,
        status: ChainSpAbsorbIntentStatus::Prepared,
        burn_proof: None,
        backend_result: None,
        last_error: None,
        created_at_ns: now_ns,
        updated_at_ns: now_ns,
    };
    state.put_pending_chain_absorb(intent.clone())?;
    Ok(intent)
}

pub(crate) fn mark_chain_absorb_burned_in_state(
    state: &mut StabilityPoolState,
    vault_id: u64,
    proof: rumi_protocol_backend::icrc3_proof::SpWritedownProof,
    now_ns: u64,
) -> Result<ChainSpAbsorbIntent, StabilityPoolError> {
    let mut intent = state.get_pending_chain_absorb(vault_id).ok_or_else(|| {
        StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: "missing pending chain absorb intent".to_string(),
        }
    })?;
    if let Some(existing) = &intent.burn_proof {
        if existing != &proof {
            return Err(StabilityPoolError::LiquidationFailed {
                vault_id,
                reason: "pending chain absorb burn proof conflicts with retry proof".to_string(),
            });
        }
    }
    intent.burn_proof = Some(proof);
    intent.status = ChainSpAbsorbIntentStatus::Burned;
    intent.last_error = None;
    intent.updated_at_ns = now_ns;
    state.put_pending_chain_absorb(intent.clone())?;
    Ok(intent)
}

pub(crate) fn mark_chain_absorb_backend_result_in_state(
    state: &mut StabilityPoolState,
    vault_id: u64,
    result: ChainStabilityPoolLiquidationResult,
    now_ns: u64,
) -> Result<ChainSpAbsorbIntent, StabilityPoolError> {
    let mut intent = state.get_pending_chain_absorb(vault_id).ok_or_else(|| {
        StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: "missing pending chain absorb intent".to_string(),
        }
    })?;
    if let Some(existing) = &intent.backend_result {
        if existing != &result {
            return Err(StabilityPoolError::LiquidationFailed {
                vault_id,
                reason: "pending chain absorb backend result conflicts with retry result"
                    .to_string(),
            });
        }
    }
    intent.backend_result = Some(result);
    intent.status = ChainSpAbsorbIntentStatus::BackendAccepted;
    intent.last_error = None;
    intent.updated_at_ns = now_ns;
    state.put_pending_chain_absorb(intent.clone())?;
    Ok(intent)
}

pub(crate) fn mark_chain_absorb_error_in_state(
    state: &mut StabilityPoolState,
    vault_id: u64,
    status: ChainSpAbsorbIntentStatus,
    reason: String,
    now_ns: u64,
) {
    if let Some(mut intent) = state.get_pending_chain_absorb(vault_id) {
        intent.status = status;
        intent.last_error = Some(reason);
        intent.updated_at_ns = now_ns;
        let _ = state.put_pending_chain_absorb(intent);
    }
}

pub(crate) fn clear_unburned_chain_absorb_intent_in_state(
    state: &mut StabilityPoolState,
    vault_id: u64,
) -> bool {
    let Some(intent) = state.get_pending_chain_absorb(vault_id) else {
        return false;
    };
    if intent.status == ChainSpAbsorbIntentStatus::Prepared
        && intent.burn_proof.is_none()
        && intent.backend_result.is_none()
    {
        state.take_pending_chain_absorb(vault_id);
        return true;
    }
    false
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CfxClaimPayoutPlan {
    pub claimant: Principal,
    pub chain_sentinel: Principal,
    pub claim_id: u64,
    pub amount_wei: u128,
    pub dest_evm: String,
}

fn chain_id_from_sentinel(sentinel: &Principal) -> Option<ChainId> {
    let bytes = sentinel.as_slice();
    let prefix = b"rumi-chain-collateral";
    if bytes.len() != 29 || !bytes.starts_with(prefix) || bytes[28] != 0x7f {
        return None;
    }
    if bytes[prefix.len()..24].iter().any(|b| *b != 0) {
        return None;
    }
    let mut chain_bytes = [0u8; 4];
    chain_bytes.copy_from_slice(&bytes[24..28]);
    Some(ChainId(u32::from_le_bytes(chain_bytes)))
}

pub(crate) fn registered_chain_ids_from_sentinels(state: &StabilityPoolState) -> Vec<ChainId> {
    let mut chains: Vec<ChainId> = state
        .chain_collateral_sentinels
        .as_ref()
        .into_iter()
        .flat_map(|sentinels| sentinels.iter())
        .filter_map(chain_id_from_sentinel)
        .collect();
    chains.sort();
    chains.dedup();
    chains
}

pub(crate) fn prepare_chain_absorb_plan_in_state(
    state: &StabilityPoolState,
    vault: &ChainLiquidatableVaultInfo,
) -> Result<ChainAbsorbPlan, StabilityPoolError> {
    if !vault.sp_attempted {
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id: vault.vault_id,
            reason: "chain vault has not been escalated to the stability pool".to_string(),
        });
    }
    if !state.is_chain_collateral_sentinel(&vault.chain_collateral_sentinel) {
        return Err(StabilityPoolError::CollateralNotFound {
            ledger: vault.chain_collateral_sentinel,
        });
    }
    if chain_id_from_sentinel(&vault.chain_collateral_sentinel) != Some(vault.chain_id) {
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id: vault.vault_id,
            reason: "chain collateral sentinel does not match chain id".to_string(),
        });
    }
    let debt_e8s =
        u64::try_from(vault.debt_e8s).map_err(|_| StabilityPoolError::LiquidationFailed {
            vault_id: vault.vault_id,
            reason: format!(
                "chain vault debt {} exceeds SP u64 burn amount",
                vault.debt_e8s
            ),
        })?;
    if debt_e8s == 0 {
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id: vault.vault_id,
            reason: "chain vault has no debt to absorb".to_string(),
        });
    }

    let available_icusd =
        state.effective_icusd_pool_for_collateral(&vault.chain_collateral_sentinel);
    if available_icusd < debt_e8s {
        return Err(StabilityPoolError::InsufficientPoolBalance);
    }
    let stables_consumed =
        state.compute_icusd_chain_draw(debt_e8s, &vault.chain_collateral_sentinel);
    let icusd_ledger = state
        .icusd_ledger()
        .ok_or(StabilityPoolError::TokenNotAccepted {
            ledger: Principal::anonymous(),
        })?;
    if stables_consumed.get(&icusd_ledger).copied().unwrap_or(0) != debt_e8s {
        return Err(StabilityPoolError::InsufficientPoolBalance);
    }

    Ok(ChainAbsorbPlan {
        vault_id: vault.vault_id,
        chain_id: vault.chain_id,
        chain_sentinel: vault.chain_collateral_sentinel,
        icusd_ledger,
        icusd_to_burn_e8s: debt_e8s,
        stables_consumed,
    })
}

pub(crate) fn apply_chain_absorb_success_in_state(
    state: &mut StabilityPoolState,
    plan: &ChainAbsorbPlan,
    result: ChainStabilityPoolLiquidationResult,
) -> Result<ChainSpAbsorbResult, StabilityPoolError> {
    apply_chain_absorb_success_in_state_at(state, plan, result, ic_cdk::api::time())
}

pub(crate) fn apply_chain_absorb_success_in_state_at(
    state: &mut StabilityPoolState,
    plan: &ChainAbsorbPlan,
    result: ChainStabilityPoolLiquidationResult,
    timestamp: u64,
) -> Result<ChainSpAbsorbResult, StabilityPoolError> {
    let absorbed = chain_absorb_result_from_backend(plan, result)?;
    if let Some(completion) = state.completed_chain_absorb(plan.vault_id) {
        if completion.result == absorbed {
            return Ok(completion.result);
        }
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id: plan.vault_id,
            reason: "completed chain absorb conflicts with backend result".to_string(),
        });
    }

    state.record_chain_claim_source(
        plan.chain_sentinel,
        absorbed.claim_id,
        absorbed.collateral_received_native,
    );
    state.process_chain_liquidation_gains_at(
        plan.vault_id,
        plan.chain_sentinel,
        &plan.stables_consumed,
        absorbed.collateral_received_native,
        absorbed.collateral_price_e8s,
        timestamp,
    );
    state.take_pending_chain_absorb(plan.vault_id);
    state.record_completed_chain_absorb(ChainSpAbsorbCompletion {
        vault_id: plan.vault_id,
        result: absorbed.clone(),
        completed_at_ns: timestamp,
    });

    Ok(absorbed)
}

pub(crate) fn prepare_cfx_claim_payout_in_state(
    state: &mut StabilityPoolState,
    claimant: Principal,
    chain_sentinel: Principal,
    dest_evm: String,
    address_validator: impl Fn(&str) -> bool,
) -> Result<Option<CfxClaimPayoutPlan>, StabilityPoolError> {
    if !address_validator(&dest_evm) {
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id: 0,
            reason: "invalid EVM address".to_string(),
        });
    }
    if !state.is_chain_collateral_sentinel(&chain_sentinel) {
        return Err(StabilityPoolError::CollateralNotFound {
            ledger: chain_sentinel,
        });
    }
    let owed = state
        .deposits
        .get(&claimant)
        .and_then(|pos| pos.cfx_claims.as_ref())
        .and_then(|claims| claims.get(&chain_sentinel).copied())
        .unwrap_or(0);
    if owed == 0 {
        return Ok(None);
    }

    let (claim_id, amount_wei) = {
        let sources = state
            .chain_claim_sources
            .as_mut()
            .and_then(|m| m.get_mut(&chain_sentinel))
            .ok_or_else(|| StabilityPoolError::LiquidationFailed {
                vault_id: 0,
                reason: "no backend chain claim source available".to_string(),
            })?;
        let source_index = sources
            .iter()
            .position(|source| source.remaining_native > 0)
            .ok_or_else(|| StabilityPoolError::LiquidationFailed {
                vault_id: 0,
                reason: "no funded backend chain claim source available".to_string(),
            })?;
        let source = &mut sources[source_index];
        let amount_wei = owed.min(source.remaining_native);
        let claim_id = source.claim_id;
        source.remaining_native = source.remaining_native.saturating_sub(amount_wei);
        if source.remaining_native == 0 {
            sources.remove(source_index);
        }
        (claim_id, amount_wei)
    };

    if state
        .chain_claim_sources
        .as_ref()
        .and_then(|m| m.get(&chain_sentinel))
        .map(|sources| sources.is_empty())
        .unwrap_or(false)
    {
        if let Some(sources) = state.chain_claim_sources.as_mut() {
            sources.remove(&chain_sentinel);
        }
    }

    state.mark_cfx_claimed(&claimant, &chain_sentinel, amount_wei);

    Ok(Some(CfxClaimPayoutPlan {
        claimant,
        chain_sentinel,
        claim_id,
        amount_wei,
        dest_evm,
    }))
}

pub(crate) fn rollback_cfx_claim_payout_in_state(
    state: &mut StabilityPoolState,
    plan: &CfxClaimPayoutPlan,
) {
    state.record_chain_claim_source(plan.chain_sentinel, plan.claim_id, plan.amount_wei);
    let position = state
        .deposits
        .entry(plan.claimant)
        .or_insert_with(|| DepositPosition::new(0));
    let claims = position.cfx_claims.get_or_insert_with(BTreeMap::new);
    let entry = claims.entry(plan.chain_sentinel).or_insert(0);
    *entry = entry.saturating_add(plan.amount_wei);
}

pub(crate) fn recredit_failed_cfx_claim_payout_in_state(
    state: &mut StabilityPoolState,
    recovery: CfxClaimPayoutRecovery,
) -> Result<bool, StabilityPoolError> {
    recredit_failed_cfx_claim_payout_in_state_at(state, recovery.clone(), recovery.failed_at_ns)
}

pub(crate) fn recredit_failed_cfx_claim_payout_in_state_at(
    state: &mut StabilityPoolState,
    recovery: CfxClaimPayoutRecovery,
    recovered_at_ns: u64,
) -> Result<bool, StabilityPoolError> {
    if recovery.amount_wei == 0 {
        return Err(StabilityPoolError::AmountTooLow { minimum_e8s: 1 });
    }
    if !state.is_chain_collateral_sentinel(&recovery.chain_sentinel) {
        return Err(StabilityPoolError::CollateralNotFound {
            ledger: recovery.chain_sentinel,
        });
    }

    let key = recovery.key();
    if let Some(existing) = state.completed_cfx_claim_payout_recovery(&key) {
        if existing.claim_id == recovery.claim_id
            && existing.claimant == recovery.claimant
            && existing.amount_wei == recovery.amount_wei
        {
            return Ok(false);
        }
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id: recovery.claim_id,
            reason: format!(
                "completed CFX claim payout recovery conflicts for op {}",
                recovery.op_id
            ),
        });
    }
    if state.completed_cfx_claim_payout_recovery_was_evicted(&key) {
        return Ok(false);
    }

    state.record_chain_claim_source(
        recovery.chain_sentinel,
        recovery.claim_id,
        recovery.amount_wei,
    );
    let position = state
        .deposits
        .entry(recovery.claimant)
        .or_insert_with(|| DepositPosition::new(0));
    let claims = position.cfx_claims.get_or_insert_with(BTreeMap::new);
    let entry = claims.entry(recovery.chain_sentinel).or_insert(0);
    *entry = entry.saturating_add(recovery.amount_wei);

    state.record_completed_cfx_claim_payout_recovery(CfxClaimPayoutRecoveryRecord {
        key,
        claim_id: recovery.claim_id,
        claimant: recovery.claimant,
        amount_wei: recovery.amount_wei,
        reason: recovery.reason,
        failed_at_ns: recovery.failed_at_ns,
        recovered_at_ns,
    });

    Ok(true)
}

pub(crate) fn is_duplicate_chain_claim_error(error: &rumi_protocol_backend::ProtocolError) -> bool {
    match error {
        rumi_protocol_backend::ProtocolError::ChainAdmin(msg)
        | rumi_protocol_backend::ProtocolError::GenericError(msg) => {
            msg.contains("Duplicate chain collateral claim payout idempotency key")
        }
        _ => false,
    }
}

/// Called by the backend when it detects liquidatable vaults (push model).
/// Processes each vault sequentially, consuming stablecoins and distributing collateral.
pub async fn notify_liquidatable_vaults(
    vaults: Vec<LiquidatableVaultInfo>,
) -> Vec<LiquidationResult> {
    if read_state(|s| s.configuration.emergency_pause) {
        log!(
            INFO,
            "Pool is paused — ignoring {} liquidatable vaults",
            vaults.len()
        );
        return vec![];
    }

    if crate::ensure_no_pool_balance_async_in_flight().is_err() {
        log!(INFO, "notify_liquidatable_vaults: a stablecoin balance operation is still in flight; skipping this batch");
        return vec![];
    }

    // SP-102: hold the per-pool liquidation lock across the whole batch so
    // deposit/withdraw/claim cannot land between a vault's snapshot and its
    // burn apportionment (which would let a withdrawer escape their share).
    // If another liquidation is already running, skip this batch (no retry —
    // the backend re-notifies on its next tick).
    let _liq_guard = match crate::pool_guard::SpLiquidationGuard::new() {
        Ok(g) => g,
        Err(_) => {
            log!(INFO, "notify_liquidatable_vaults: a liquidation is already in flight; skipping this batch");
            return vec![];
        }
    };

    log!(
        INFO,
        "Received push notification: {} liquidatable vaults",
        vaults.len()
    );

    let max_batch = read_state(|s| s.configuration.max_liquidations_per_batch) as usize;

    let mut results = Vec::new();
    for vault_info in vaults.into_iter().take(max_batch) {
        // Skip if already in-flight
        if read_state(|s| s.in_flight_liquidations.contains(&vault_info.vault_id)) {
            log!(
                INFO,
                "Vault {} already in-flight, skipping",
                vault_info.vault_id
            );
            continue;
        }

        // Check effective pool coverage for this collateral type
        let effective_pool =
            read_state(|s| s.effective_pool_for_collateral(&vault_info.collateral_type));
        if effective_pool < vault_info.debt_amount {
            log!(
                INFO,
                "Insufficient pool coverage for vault {}: need {} e8s, have {} e8s",
                vault_info.vault_id,
                vault_info.debt_amount,
                effective_pool
            );
            continue;
        }

        // Mark as in-flight
        mutate_state(|s| {
            s.in_flight_liquidations.insert(vault_info.vault_id);
        });

        let result = execute_single_liquidation(&vault_info).await;

        // Clear in-flight
        mutate_state(|s| {
            s.in_flight_liquidations.remove(&vault_info.vault_id);
        });

        if result.success {
            log!(
                INFO,
                "Liquidated vault {}: gained {} collateral",
                vault_info.vault_id,
                result.collateral_gained
            );
        } else {
            log!(
                INFO,
                "Liquidation failed for vault {}: {}",
                vault_info.vault_id,
                result.error_message.as_deref().unwrap_or("unknown")
            );
        }

        results.push(result);
    }

    results
}

/// Public fallback: anyone (except the anonymous principal) can call this to
/// trigger a liquidation for a specific vault.
///
/// SP-111 (audit 2026-06-05): the previous comment claimed a per-caller guard
/// was enforced at the lib.rs level — there was none. Concurrency is now
/// serialized by the per-pool `SpLiquidationGuard` acquired below (SP-102), and
/// the anonymous principal is rejected here to keep the permissionless trigger
/// from being driven by unauthenticated cycle-griefing callers.
pub async fn execute_liquidation(vault_id: u64) -> Result<LiquidationResult, StabilityPoolError> {
    if ic_cdk::api::caller() == Principal::anonymous() {
        return Err(StabilityPoolError::Unauthorized);
    }

    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }

    crate::ensure_no_pool_balance_async_in_flight()?;

    if read_state(|s| s.in_flight_liquidations.contains(&vault_id)) {
        return Err(StabilityPoolError::SystemBusy);
    }

    // SP-102: hold the per-pool liquidation lock across snapshot -> await ->
    // apportion so deposit/withdraw/claim cannot race the apportionment.
    let _liq_guard = crate::pool_guard::SpLiquidationGuard::new()?;

    // Fetch vault info from backend
    let protocol_id = read_state(|s| s.protocol_canister_id);

    let (vaults,): (Vec<rumi_protocol_backend::vault::CandidVault>,) =
        call(protocol_id, "get_liquidatable_vaults", ())
            .await
            .map_err(|_e| StabilityPoolError::InterCanisterCallFailed {
                target: "Protocol".to_string(),
                method: "get_liquidatable_vaults".to_string(),
            })?;
    let target_vault = vaults.into_iter().find(|v| v.vault_id == vault_id);

    let vault = match target_vault {
        Some(v) => v,
        None => {
            return Err(StabilityPoolError::LiquidationFailed {
                vault_id,
                reason: "Vault not found in liquidatable list".to_string(),
            })
        }
    };

    let vault_info = LiquidatableVaultInfo {
        vault_id: vault.vault_id,
        collateral_type: vault.collateral_type,
        debt_amount: vault.borrowed_icusd_amount,
        collateral_amount: vault.icp_margin_amount,
        recommended_liquidation_amount: 0,
        collateral_price_e8s: 0,
    };

    // Check pool coverage
    let effective_pool =
        read_state(|s| s.effective_pool_for_collateral(&vault_info.collateral_type));
    if effective_pool < vault_info.debt_amount {
        return Err(StabilityPoolError::InsufficientPoolBalance);
    }

    mutate_state(|s| {
        s.in_flight_liquidations.insert(vault_id);
    });
    let result = execute_single_liquidation(&vault_info).await;
    mutate_state(|s| {
        s.in_flight_liquidations.remove(&vault_id);
    });

    Ok(result)
}

pub async fn scan_chain_absorb_candidates(
    max_per_chain: Option<u64>,
) -> Result<Vec<ChainSpAbsorbCandidate>, StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    if caller == Principal::anonymous() {
        return Err(StabilityPoolError::Unauthorized);
    }
    if !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }

    discover_chain_absorb_candidates(max_per_chain).await
}

async fn discover_chain_absorb_candidates(
    max_per_chain: Option<u64>,
) -> Result<Vec<ChainSpAbsorbCandidate>, StabilityPoolError> {
    let (protocol_id, chains) = read_state(|s| {
        (
            s.protocol_canister_id,
            registered_chain_ids_from_sentinels(s),
        )
    });
    let per_chain = max_per_chain.unwrap_or(100).min(500) as usize;
    let mut candidates = Vec::new();
    for chain in chains {
        let call_result: Result<(Vec<ChainLiquidatableVaultInfo>,), _> =
            call(protocol_id, "get_chain_liquidatable_vaults", (chain,)).await;
        let (vaults,) = call_result.map_err(|_| StabilityPoolError::InterCanisterCallFailed {
            target: format!("{}", protocol_id),
            method: "get_chain_liquidatable_vaults".to_string(),
        })?;

        let mut eligible_for_chain = 0usize;
        for vault in vaults.into_iter().filter(|v| v.sp_attempted) {
            if let Ok(plan) = read_state(|s| prepare_chain_absorb_plan_in_state(s, &vault)) {
                let pending_status = read_state(|s| s.pending_chain_absorb_status(vault.vault_id));
                candidates.push(ChainSpAbsorbCandidate {
                    vault,
                    icusd_to_burn_e8s: plan.icusd_to_burn_e8s,
                    pending_status,
                });
                eligible_for_chain += 1;
                if eligible_for_chain >= per_chain {
                    break;
                }
            }
        }
    }

    Ok(candidates)
}

pub(crate) fn select_chain_absorb_auto_vault(
    state: &StabilityPoolState,
    candidates: &[ChainSpAbsorbCandidate],
) -> Option<u64> {
    state
        .pending_chain_absorbs()
        .into_iter()
        .map(|intent| intent.vault_id)
        .next()
        .or_else(|| candidates.first().map(|candidate| candidate.vault.vault_id))
}

async fn submit_chain_absorb_to_backend(
    protocol_id: Principal,
    vault_id: u64,
    plan: &ChainAbsorbPlan,
    proof: rumi_protocol_backend::icrc3_proof::SpWritedownProof,
) -> Result<ChainStabilityPoolLiquidationResult, StabilityPoolError> {
    let backend_result: Result<
        (Result<ChainStabilityPoolLiquidationResult, rumi_protocol_backend::ProtocolError>,),
        _,
    > = call(
        protocol_id,
        "stability_pool_liquidate_chain_vault",
        (vault_id, plan.icusd_to_burn_e8s, proof),
    )
    .await;

    match backend_result {
        Ok((Ok(result),)) => {
            mutate_state(|s| {
                mark_chain_absorb_backend_result_in_state(
                    s,
                    vault_id,
                    result.clone(),
                    ic_cdk::api::time(),
                )
            })?;
            Ok(result)
        }
        Ok((Err(error),)) => {
            let reason = format!("backend rejected chain absorb after burn: {:?}", error);
            mutate_state(|s| {
                mark_chain_absorb_error_in_state(
                    s,
                    vault_id,
                    ChainSpAbsorbIntentStatus::BackendRejected,
                    reason.clone(),
                    ic_cdk::api::time(),
                );
            });
            Err(StabilityPoolError::LiquidationFailed { vault_id, reason })
        }
        Err(_) => {
            mutate_state(|s| {
                mark_chain_absorb_error_in_state(
                    s,
                    vault_id,
                    ChainSpAbsorbIntentStatus::Burned,
                    "backend call failed after icUSD burn".to_string(),
                    ic_cdk::api::time(),
                );
            });
            Err(StabilityPoolError::InterCanisterCallFailed {
                target: format!("{}", protocol_id),
                method: "stability_pool_liquidate_chain_vault".to_string(),
            })
        }
    }
}

async fn preflight_chain_absorb_with_backend(
    protocol_id: Principal,
    vault_id: u64,
    icusd_to_burn_e8s: u64,
) -> Result<(), StabilityPoolError> {
    let preflight_result: Result<(Result<(), rumi_protocol_backend::ProtocolError>,), _> = call(
        protocol_id,
        "stability_pool_preflight_chain_absorb",
        (vault_id, icusd_to_burn_e8s),
    )
    .await;

    match preflight_result {
        Ok((Ok(()),)) => Ok(()),
        Ok((Err(error),)) => Err(StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: format!("backend rejected chain absorb preflight: {:?}", error),
        }),
        Err(_) => Err(StabilityPoolError::InterCanisterCallFailed {
            target: format!("{}", protocol_id),
            method: "stability_pool_preflight_chain_absorb".to_string(),
        }),
    }
}

pub async fn sp_absorb_chain_vault(
    vault_id: u64,
) -> Result<ChainSpAbsorbResult, StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    if caller == Principal::anonymous() {
        return Err(StabilityPoolError::Unauthorized);
    }
    if !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    sp_absorb_chain_vault_core(vault_id).await
}

async fn sp_absorb_chain_vault_core(
    vault_id: u64,
) -> Result<ChainSpAbsorbResult, StabilityPoolError> {
    if let Some(completion) = read_state(|s| s.completed_chain_absorb(vault_id)) {
        return Ok(completion.result);
    }

    crate::ensure_no_pool_balance_async_in_flight()?;
    let _liq_guard = crate::pool_guard::SpLiquidationGuard::new()?;
    read_state(|s| ensure_no_other_pending_pool_absorb_for_chain(s, vault_id))?;
    if let Some(intent) = read_state(|s| s.get_pending_chain_absorb(vault_id)) {
        let plan = chain_absorb_plan_from_intent(&intent);
        if let Some(result) = intent.backend_result.clone() {
            return mutate_state(|s| apply_chain_absorb_success_in_state(s, &plan, result));
        }
        if let Some((plan, proof)) = burned_chain_absorb_replay_plan(&intent) {
            let protocol_id = read_state(|s| s.protocol_canister_id);
            let result =
                submit_chain_absorb_to_backend(protocol_id, vault_id, &plan, proof).await?;
            return mutate_state(|s| apply_chain_absorb_success_in_state(s, &plan, result));
        }

        if read_state(|s| s.configuration.emergency_pause) {
            return Err(StabilityPoolError::EmergencyPaused);
        }
        let protocol_id = read_state(|s| s.protocol_canister_id);
        if let Err(error) =
            preflight_chain_absorb_with_backend(protocol_id, vault_id, plan.icusd_to_burn_e8s).await
        {
            mutate_state(|s| {
                clear_unburned_chain_absorb_intent_in_state(s, vault_id);
            });
            return Err(error);
        }
        if read_state(|s| s.configuration.emergency_pause) {
            return Err(StabilityPoolError::EmergencyPaused);
        }

        let proof = match burn_icusd_for_chain_writedown_with_account(
            intent.icusd_ledger,
            intent.icusd_minting_account,
            intent.icusd_to_burn_e8s,
            intent.vault_id,
            intent.burn_created_at_time_ns,
        )
        .await
        {
            Ok(proof) => {
                mutate_state(|s| {
                    mark_chain_absorb_burned_in_state(
                        s,
                        vault_id,
                        proof.clone(),
                        ic_cdk::api::time(),
                    )
                })?;
                proof
            }
            Err(error) => {
                mutate_state(|s| {
                    clear_unburned_chain_absorb_intent_in_state(s, vault_id);
                });
                return Err(error);
            }
        };

        let result = submit_chain_absorb_to_backend(protocol_id, vault_id, &plan, proof).await?;
        return mutate_state(|s| apply_chain_absorb_success_in_state(s, &plan, result));
    }

    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }

    let (protocol_id, chains) = read_state(|s| {
        (
            s.protocol_canister_id,
            registered_chain_ids_from_sentinels(s),
        )
    });
    if chains.is_empty() {
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: "no registered chain collateral sentinels".to_string(),
        });
    }

    let mut candidate: Option<ChainLiquidatableVaultInfo> = None;
    for chain in chains {
        let call_result: Result<(Vec<ChainLiquidatableVaultInfo>,), _> =
            call(protocol_id, "get_chain_liquidatable_vaults", (chain,)).await;
        let (vaults,) = call_result.map_err(|_| StabilityPoolError::InterCanisterCallFailed {
            target: format!("{}", protocol_id),
            method: "get_chain_liquidatable_vaults".to_string(),
        })?;
        if let Some(vault) = vaults.into_iter().find(|v| v.vault_id == vault_id) {
            candidate = Some(vault);
            break;
        }
    }

    let candidate = candidate.ok_or_else(|| StabilityPoolError::LiquidationFailed {
        vault_id,
        reason: "chain vault not found in liquidatable discovery".to_string(),
    })?;
    let plan = read_state(|s| prepare_chain_absorb_plan_in_state(s, &candidate))?;

    let minting_account = match read_state(|s| s.get_pending_chain_absorb(vault_id)) {
        Some(intent) => intent.icusd_minting_account,
        None => fetch_icusd_minting_account(plan.icusd_ledger).await?,
    };
    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }
    preflight_chain_absorb_with_backend(protocol_id, vault_id, plan.icusd_to_burn_e8s).await?;
    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }
    let now = ic_cdk::api::time();
    let mut intent = mutate_state(|s| {
        prepare_or_reuse_chain_absorb_intent_in_state(s, &plan, minting_account, now)
    })?;
    if read_state(|s| s.configuration.emergency_pause) {
        mutate_state(|s| {
            clear_unburned_chain_absorb_intent_in_state(s, vault_id);
        });
        return Err(StabilityPoolError::EmergencyPaused);
    }

    let proof = if let Some(proof) = intent.burn_proof.clone() {
        proof
    } else {
        match burn_icusd_for_chain_writedown_with_account(
            intent.icusd_ledger,
            intent.icusd_minting_account,
            intent.icusd_to_burn_e8s,
            intent.vault_id,
            intent.burn_created_at_time_ns,
        )
        .await
        {
            Ok(proof) => {
                intent = mutate_state(|s| {
                    mark_chain_absorb_burned_in_state(
                        s,
                        vault_id,
                        proof.clone(),
                        ic_cdk::api::time(),
                    )
                })?;
                proof
            }
            Err(error) => {
                mutate_state(|s| {
                    clear_unburned_chain_absorb_intent_in_state(s, vault_id);
                });
                return Err(error);
            }
        }
    };

    let result = if let Some(result) = intent.backend_result.clone() {
        result
    } else {
        submit_chain_absorb_to_backend(protocol_id, vault_id, &plan, proof).await?
    };

    mutate_state(|s| apply_chain_absorb_success_in_state(s, &plan, result))
}

pub async fn run_chain_absorb_auto_tick(
) -> Result<Option<ChainAbsorbAutoTickRecord>, StabilityPoolError> {
    let started_at_ns = ic_cdk::api::time();
    if !read_state(|s| s.chain_absorb_auto_due(started_at_ns)) {
        return Ok(None);
    }

    let _tick_guard = crate::pool_guard::ChainAbsorbAutoTickGuard::new()?;
    let config = read_state(|s| s.chain_absorb_auto_config());
    if !config.enabled || !read_state(|s| s.chain_absorb_auto_due(started_at_ns)) {
        return Ok(None);
    }

    if read_state(|s| s.configuration.emergency_pause) {
        let tick = ChainAbsorbAutoTickRecord {
            started_at_ns,
            completed_at_ns: ic_cdk::api::time(),
            attempted_vault_id: None,
            candidates_scanned: 0,
            absorbed: None,
            error: None,
            skipped_reason: Some("emergency pause".to_string()),
        };
        mutate_state(|s| s.record_chain_absorb_auto_tick(tick.clone()));
        return Ok(Some(tick));
    }

    let pending_vault = read_state(|s| {
        s.pending_chain_absorbs()
            .into_iter()
            .map(|intent| intent.vault_id)
            .next()
    });
    let (vault_id, candidates_scanned) = if let Some(vault_id) = pending_vault {
        (vault_id, 0)
    } else {
        let candidates =
            match discover_chain_absorb_candidates(Some(config.max_scan_per_chain)).await {
                Ok(candidates) => candidates,
                Err(error) => {
                    let tick = ChainAbsorbAutoTickRecord {
                        started_at_ns,
                        completed_at_ns: ic_cdk::api::time(),
                        attempted_vault_id: None,
                        candidates_scanned: 0,
                        absorbed: None,
                        error: Some(format!("{error:?}")),
                        skipped_reason: None,
                    };
                    mutate_state(|s| s.record_chain_absorb_auto_tick(tick.clone()));
                    return Ok(Some(tick));
                }
            };
        let candidates_scanned = candidates.len() as u64;
        match read_state(|s| select_chain_absorb_auto_vault(s, &candidates)) {
            Some(vault_id) => (vault_id, candidates_scanned),
            None => {
                let tick = ChainAbsorbAutoTickRecord {
                    started_at_ns,
                    completed_at_ns: ic_cdk::api::time(),
                    attempted_vault_id: None,
                    candidates_scanned,
                    absorbed: None,
                    error: None,
                    skipped_reason: Some("no eligible candidates".to_string()),
                };
                mutate_state(|s| s.record_chain_absorb_auto_tick(tick.clone()));
                return Ok(Some(tick));
            }
        }
    };

    let result = sp_absorb_chain_vault_core(vault_id).await;
    let tick = match result {
        Ok(absorbed) => ChainAbsorbAutoTickRecord {
            started_at_ns,
            completed_at_ns: ic_cdk::api::time(),
            attempted_vault_id: Some(vault_id),
            candidates_scanned,
            absorbed: Some(absorbed),
            error: None,
            skipped_reason: None,
        },
        Err(error) => ChainAbsorbAutoTickRecord {
            started_at_ns,
            completed_at_ns: ic_cdk::api::time(),
            attempted_vault_id: Some(vault_id),
            candidates_scanned,
            absorbed: None,
            error: Some(format!("{error:?}")),
            skipped_reason: None,
        },
    };
    mutate_state(|s| s.record_chain_absorb_auto_tick(tick.clone()));
    Ok(Some(tick))
}

pub async fn claim_cfx(
    chain_sentinel: Principal,
    dest_evm: String,
) -> Result<u128, StabilityPoolError> {
    if crate::pool_balance_mutation_blocked() {
        return Err(StabilityPoolError::SystemBusy);
    }
    let caller = ic_cdk::api::caller();
    if caller == Principal::anonymous() {
        return Err(StabilityPoolError::Unauthorized);
    }
    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }

    let plan = mutate_state(|s| {
        prepare_cfx_claim_payout_in_state(
            s,
            caller,
            chain_sentinel,
            dest_evm,
            rumi_protocol_backend::chains::evm::tecdsa::is_valid_evm_address,
        )
    })?;
    let Some(plan) = plan else {
        return Ok(0);
    };

    let protocol_id = read_state(|s| s.protocol_canister_id);
    let backend_result: Result<(Result<u64, rumi_protocol_backend::ProtocolError>,), _> = call(
        protocol_id,
        "claim_chain_collateral",
        (
            plan.claim_id,
            plan.claimant,
            plan.amount_wei,
            plan.dest_evm.clone(),
        ),
    )
    .await;

    match backend_result {
        Ok((Ok(_op_id),)) => Ok(plan.amount_wei),
        Ok((Err(error),)) if is_duplicate_chain_claim_error(&error) => Ok(plan.amount_wei),
        Ok((Err(error),)) => {
            mutate_state(|s| rollback_cfx_claim_payout_in_state(s, &plan));
            Err(StabilityPoolError::LiquidationFailed {
                vault_id: plan.claim_id,
                reason: format!("backend rejected CFX claim: {:?}", error),
            })
        }
        Err(_) => {
            mutate_state(|s| rollback_cfx_claim_payout_in_state(s, &plan));
            Err(StabilityPoolError::InterCanisterCallFailed {
                target: format!("{}", protocol_id),
                method: "claim_chain_collateral".to_string(),
            })
        }
    }
}

fn lp_liquidation_equivalent_e8s(
    state: &crate::state::StabilityPoolState,
    token_ledger: Principal,
    amount: u64,
) -> Option<u64> {
    let virtual_price = state
        .virtual_prices()
        .get(&token_ledger)
        .copied()
        .filter(|virtual_price| *virtual_price > 0)?;
    Some(lp_to_usd_e8s(amount, virtual_price))
}

/// Core liquidation logic for a single vault.
///
/// Strategy:
/// 1. Non-LP stablecoins (icUSD, ckUSDC, ckUSDT): approve backend → call liquidate_vault_partial
/// 2. LP tokens (3USD): burn on 3pool via authorized_redeem_and_burn → call backend
///    stability_pool_liquidate_debt_burned to write down debt and release collateral
///
/// No circuit breaker / suspension mechanism — if a token fails, we skip it and try the
/// next one. If they all fail, the liquidation simply doesn't happen this round.
enum ThreeUsdReserveAbsorbStatusResolution {
    Unseen,
    TransferSubmittedOrUnknown,
    TransferConfirmed,
    Absorbed {
        verified_payout: VerifiedThreeUsdCollateralPayout,
        ingress_fee_e8s: u64,
        refund_fee_e8s: u64,
    },
    PreTransferRejected,
    FullyRefunded {
        ingress_fee_e8s: u64,
        refund_fee_e8s: u64,
    },
    Pending(String),
}

/// Opaque evidence that the backend payout for one immutable 3USD request was
/// read back from the configured collateral ledger at its exact candidate
/// block. Only the direct-ledger verifier below constructs this value in
/// production; settlement cannot be authorized by a backend amount alone.
#[derive(Clone, Debug, PartialEq, Eq)]
struct VerifiedThreeUsdCollateralPayout {
    intent: ThreeUsdReserveAbsorbIntent,
    backend_result: StabilityPoolLiquidationResult,
    backend: Principal,
    pool: Principal,
    payout: rumi_protocol_backend::state::ThreeUsdReserveCollateralPayout,
    block_index: u64,
}

enum ThreeUsdSettlementEvidence<'a> {
    TerminalRefund,
    CollateralPayout(&'a VerifiedThreeUsdCollateralPayout),
}

impl VerifiedThreeUsdCollateralPayout {
    fn is_for(
        &self,
        intent: &ThreeUsdReserveAbsorbIntent,
        current_backend: Principal,
    ) -> bool {
        let Some(collateral_type) = intent.collateral_type else {
            return false;
        };
        let Some(collateral_ledger) = intent.collateral_ledger else {
            return false;
        };
        self.intent == *intent
            && self.backend == current_backend
            && self.backend_result.success
            && self.backend_result.vault_id == intent.vault_id
            && self.backend_result.liquidated_debt <= intent.debt_e8s
            && self.backend_result.collateral_type == collateral_type.to_text()
            && self.backend_result.collateral_received == self.payout.gross_e8s
            && three_usd_payout_identity_matches(
                &self.payout,
                intent,
                &self.backend_result,
                self.backend,
                self.pool,
            )
            && self.payout.collateral_type == collateral_type
            && self.payout.ledger == collateral_ledger
            && self.payout.candidate_block_index == Some(self.block_index)
            && self.payout.source == (Account { owner: self.backend, subaccount: None })
            && self.payout.destination == (Account { owner: self.pool, subaccount: None })
            && self.payout.net_e8s.checked_add(self.payout.expected_fee_e8s)
                == Some(self.payout.gross_e8s)
            && self.payout.observed_fee_e8s
                .is_none_or(|fee| fee == self.payout.expected_fee_e8s)
    }

    fn from_direct_icrc3_block(
        intent: &ThreeUsdReserveAbsorbIntent,
        backend_result: &StabilityPoolLiquidationResult,
        backend: Principal,
        pool: Principal,
        payout: rumi_protocol_backend::state::ThreeUsdReserveCollateralPayout,
        block_index: u64,
        block: &rumi_protocol_backend::icrc3_proof::DecodedBlock,
    ) -> Result<Self, String> {
        if !three_usd_payout_identity_matches(
            &payout,
            intent,
            backend_result,
            backend,
            pool,
        ) || payout.candidate_block_index != Some(block_index)
            || !three_usd_payout_block_matches(&payout, block)
        {
            return Err("direct ledger block does not match the exact immutable 3USD payout".into());
        }
        Ok(Self {
            intent: intent.clone(),
            backend_result: backend_result.clone(),
            backend,
            pool,
            payout,
            block_index,
        })
    }
}

/// Independently authenticate the backend's unique payout candidate against
/// the collateral ledger's directly served ICRC-3 block. Archive callbacks are
/// deliberately unsupported: they do not establish membership in the ledger's
/// current block log.
async fn verify_three_usd_collateral_payout(
    protocol_id: Principal,
    intent: &ThreeUsdReserveAbsorbIntent,
    result: &StabilityPoolLiquidationResult,
) -> Result<VerifiedThreeUsdCollateralPayout, String> {
    use rumi_protocol_backend::state::ThreeUsdReserveCollateralPayout;

    let (candidate,): (Option<ThreeUsdReserveCollateralPayout>,) = call(
        protocol_id,
        "get_stability_pool_liquidate_with_reserves_v2_payout_candidate",
        (intent.vault_id, intent.absorb_id),
    )
    .await
    .map_err(|error| format!("backend payout candidate unavailable: {error:?}"))?;
    let payout = candidate.ok_or_else(|| "backend has no unique payout candidate".to_string())?;
    let block_index = payout
        .candidate_block_index
        .ok_or_else(|| "payout candidate has no block index".to_string())?;
    let pool = ic_cdk::api::id();
    if !three_usd_payout_identity_matches(&payout, intent, result, protocol_id, pool) {
        return Err("payout tuple conflicts with the immutable absorb or backend result".into());
    }
    if rumi_protocol_backend::native_icp_proof::is_native_icp_ledger(payout.ledger) {
        let args = rumi_protocol_backend::native_icp_proof::GetBlocksArgs {
            start: block_index,
            length: 1,
        };
        let (response,): (rumi_protocol_backend::native_icp_proof::QueryBlocksResponse,) =
            call(payout.ledger, "query_blocks", (args,))
                .await
                .map_err(|error| format!("native ICP query_blocks failed: {error:?}"))?;
        let block = native_icp_direct_candidate_block(response, block_index)?;
        let fee = rumi_protocol_backend::native_icp_proof::verify_transfer_block(
            &block,
            protocol_id,
            pool,
            payout.net_e8s,
            &payout.memo,
            payout.created_at_time_ns,
        )?;
        if fee != payout.expected_fee_e8s {
            return Err(
                "native ICP candidate fee differs from the exact backend payout fee".into(),
            );
        }
        return Ok(VerifiedThreeUsdCollateralPayout {
            intent: intent.clone(),
            backend_result: result.clone(),
            backend: protocol_id,
            pool,
            payout,
            block_index,
        });
    }
    let (response,): (GetBlocksResult,) = call(
        payout.ledger,
        "icrc3_get_blocks",
        (vec![GetBlocksRequest {
            start: Nat::from(block_index),
            length: Nat::from(1u64),
        }],),
    )
    .await
    .map_err(|error| format!("direct collateral ledger block query failed: {error:?}"))?;
    let log_length = response
        .log_length
        .0
        .to_u64()
        .ok_or("collateral ledger log length exceeds u64")?;
    if log_length <= block_index
        || !response.archived_blocks.is_empty()
        || response.blocks.len() != 1
        || response.blocks[0].id.0.to_u64() != Some(block_index)
    {
        return Err("collateral ledger did not directly serve the exact candidate block".into());
    }
    let block = rumi_protocol_backend::icrc3_proof::decode_block(&response.blocks[0].block)
        .map_err(|error| format!("malformed collateral payout block: {error}"))?;
    VerifiedThreeUsdCollateralPayout::from_direct_icrc3_block(
        intent,
        result,
        protocol_id,
        pool,
        payout,
        block_index,
        &block,
    )
}

/// Native ICP archive callbacks can serve authentic ledger blocks, but that
/// response does not prove the candidate's current ledger-global membership.
/// For this terminal settlement require the canonical ledger's direct response.
fn native_icp_direct_candidate_block(
    response: rumi_protocol_backend::native_icp_proof::QueryBlocksResponse,
    block_index: u64,
) -> Result<rumi_protocol_backend::native_icp_proof::Block, String> {
    if response.first_block_index != block_index
        || response.blocks.len() != 1
        || !response.archived_blocks.is_empty()
    {
        return Err("native ICP candidate block was not directly served at the exact index".into());
    }
    response
        .blocks
        .into_iter()
        .next()
        .ok_or_else(|| "native ICP query_blocks returned no candidate block".into())
}

fn three_usd_payout_identity_matches(
    payout: &rumi_protocol_backend::state::ThreeUsdReserveCollateralPayout,
    intent: &ThreeUsdReserveAbsorbIntent,
    result: &StabilityPoolLiquidationResult,
    protocol_id: Principal,
    pool: Principal,
) -> bool {
    let Some(collateral_type) = intent.collateral_type else { return false };
    let Some(collateral_ledger) = intent.collateral_ledger else { return false };
    let Some(net) = payout.gross_e8s.checked_sub(payout.expected_fee_e8s) else { return false };
    result.success
        && result.vault_id == intent.vault_id
        && result.liquidated_debt <= intent.debt_e8s
        && result.collateral_type == collateral_type.to_text()
        && result.collateral_received == payout.gross_e8s
        && payout.operation_id != 0
        && payout.op_nonce == payout.operation_id
        && payout.collateral_type == collateral_type
        && payout.ledger == collateral_ledger
        && payout.source == (Account { owner: protocol_id, subaccount: None })
        && payout.destination == (Account { owner: pool, subaccount: None })
        && payout.gross_e8s == result.collateral_received
        && payout.net_e8s == net
        && payout.fee_arg_e8s == Some(payout.expected_fee_e8s)
        && payout.observed_fee_e8s.is_none_or(|fee| fee == payout.expected_fee_e8s)
        && payout.memo.as_slice()
            == rumi_protocol_backend::management::nonce_to_memo(payout.op_nonce).0.as_slice()
        && payout.created_at_time_ns
            == rumi_protocol_backend::management::nonce_to_created_at_time(payout.op_nonce)
}

fn three_usd_payout_block_matches(
    payout: &rumi_protocol_backend::state::ThreeUsdReserveCollateralPayout,
    block: &rumi_protocol_backend::icrc3_proof::DecodedBlock,
) -> bool {
    block.op == "xfer"
        && block.btype.as_deref().is_none_or(|kind| kind == "1xfer")
        && block.from == Some(payout.source)
        && block.to == Some(payout.destination)
        && block.spender.is_none()
        && block.amount == u128::from(payout.net_e8s)
        && block.fee == Some(payout.expected_fee_e8s)
        && block.memo.as_deref() == Some(payout.memo.as_slice())
        && block.created_at_time == Some(payout.created_at_time_ns)
}

fn exact_three_usd_refund_receipt_matches(
    protocol_id: Principal,
    stability_pool: Principal,
    required_net_credit_e8s: u64,
    transfer_block_index: u64,
    receipt: &rumi_protocol_backend::state::ThreeUsdReserveRefundReceipt,
) -> bool {
    let tuple = &receipt.tuple;
    receipt.block_index > transfer_block_index
        && tuple.source_owner == protocol_id
        && tuple.source_subaccount.is_none()
        && tuple.destination.owner == stability_pool
        && tuple.destination.subaccount.is_none()
        && tuple.amount_e8s == required_net_credit_e8s
        && tuple.charged_fee_e8s == 0
        && tuple.fee_e8s == Some(0)
}

fn three_usd_realized_amount(intent: &ThreeUsdReserveAbsorbIntent, liquidated_debt: u64) -> u64 {
    if intent.debt_e8s > 0 && liquidated_debt < intent.debt_e8s {
        ((intent.amount as u128).saturating_mul(liquidated_debt as u128)
            / intent.debt_e8s as u128) as u64
    } else {
        intent.amount
    }
}

fn three_usd_backend_ready_for_new_absorb(readiness: Option<bool>) -> bool {
    readiness == Some(true)
}

async fn resolve_three_usd_absorb_status(
    protocol_id: Principal,
    intent: &ThreeUsdReserveAbsorbIntent,
) -> ThreeUsdReserveAbsorbStatusResolution {
    use rumi_protocol_backend::ThreeUsdReserveIngressV2Status as Status;

    let view: Result<(rumi_protocol_backend::ThreeUsdReserveIngressV2StatusView,), _> = call(
        protocol_id,
        "get_stability_pool_liquidate_with_reserves_v2_status",
        (intent.vault_id, intent.absorb_id),
    )
    .await;
    let view = match view {
        Ok((view,)) => view,
        Err(error) => {
            return ThreeUsdReserveAbsorbStatusResolution::Pending(format!(
                "could not read backend status: {error:?}"
            ))
        }
    };
    let stability_pool = ic_cdk::api::id();
    if view.stability_pool != stability_pool
        || view.vault_id != intent.vault_id
        || view.absorb_id != intent.absorb_id
    {
        return ThreeUsdReserveAbsorbStatusResolution::Pending(
            "backend status identity did not match the persisted request".into(),
        );
    }

    match view.status {
        Status::Unseen => ThreeUsdReserveAbsorbStatusResolution::Unseen,
        Status::TransferSubmittedOrUnknown => ThreeUsdReserveAbsorbStatusResolution::TransferSubmittedOrUnknown,
        Status::TransferConfirmed { .. } => ThreeUsdReserveAbsorbStatusResolution::TransferConfirmed,
        Status::PreTransferRejected { .. } => {
            ThreeUsdReserveAbsorbStatusResolution::PreTransferRejected
        }
        Status::Absorbed {
            result,
            transfer_block_index,
            ingress_fee_e8s,
            proportional_refund,
        } => {
            let realized = three_usd_realized_amount(intent, result.liquidated_debt);
            let expected_refund = intent.amount.saturating_sub(realized);
            let pool_fee_e8s = match (expected_refund, proportional_refund.as_ref()) {
                (0, None) => Some(ingress_fee_e8s),
                (principal, Some(receipt)) => principal.checked_add(ingress_fee_e8s).and_then(|required_net_credit| exact_three_usd_refund_receipt_matches(
                    protocol_id,
                    stability_pool,
                    required_net_credit,
                    transfer_block_index,
                    receipt,
                )
                .then_some(0)),
                _ => None,
            };
            if result.success
                && result.vault_id == intent.vault_id
                && result.block_index == transfer_block_index
                && result.liquidated_debt <= intent.debt_e8s
                && pool_fee_e8s.is_some()
            {
                let verified_payout = match verify_three_usd_collateral_payout(protocol_id, intent, &result).await {
                    Ok(receipt) => receipt,
                    Err(reason) => return ThreeUsdReserveAbsorbStatusResolution::Pending(
                        format!("collateral payout remains held: {reason}"),
                    ),
                };
                ThreeUsdReserveAbsorbStatusResolution::Absorbed {
                    verified_payout,
                    ingress_fee_e8s: pool_fee_e8s.unwrap_or_default(),
                    refund_fee_e8s: 0,
                }
            } else {
                ThreeUsdReserveAbsorbStatusResolution::Pending(
                    "absorbed status lacks a matching result or terminal refund receipt".into(),
                )
            }
        }
        Status::AbsorbedRefundPending { .. } => ThreeUsdReserveAbsorbStatusResolution::Pending(
            "backend absorb committed while its proportional refund is pending".into(),
        ),
        Status::FailedAfterTransferRefunded {
            transfer_block_index,
            ingress_fee_e8s,
            refund_fee_e8s,
            refund_receipt,
            ..
        } if intent.amount.checked_add(ingress_fee_e8s).is_some_and(|required_net_credit| exact_three_usd_refund_receipt_matches(
            protocol_id,
            stability_pool,
            required_net_credit,
            transfer_block_index,
            &refund_receipt,
        )) => {
            ThreeUsdReserveAbsorbStatusResolution::FullyRefunded {
                ingress_fee_e8s: 0,
                refund_fee_e8s: 0,
            }
        }
        status => ThreeUsdReserveAbsorbStatusResolution::Pending(format!(
            "backend reserve absorb remains unresolved: {status:?}"
        )),
    }
}

/// Apply verified terminal refund accounting, or receipt-backed gain accounting,
/// and clear the durable identity in the same stable-state mutation.
fn apply_three_usd_absorb_settlement(
    state: &mut StabilityPoolState,
    intent: &ThreeUsdReserveAbsorbIntent,
    fee_total_e8s: u64,
    evidence: ThreeUsdSettlementEvidence<'_>,
) -> Result<(), StabilityPoolError> {
    apply_three_usd_absorb_settlement_at(
        state,
        intent,
        fee_total_e8s,
        evidence,
        ic_cdk::api::time(),
    )
}

fn apply_three_usd_absorb_settlement_at(
    state: &mut StabilityPoolState,
    intent: &ThreeUsdReserveAbsorbIntent,
    fee_total_e8s: u64,
    evidence: ThreeUsdSettlementEvidence<'_>,
    timestamp: u64,
) -> Result<(), StabilityPoolError> {
    let verified_payout = match evidence {
        ThreeUsdSettlementEvidence::TerminalRefund => None,
        ThreeUsdSettlementEvidence::CollateralPayout(receipt) => {
            if !receipt.is_for(intent, state.protocol_canister_id) {
                return Err(StabilityPoolError::LiquidationFailed {
                    vault_id: intent.vault_id,
                    reason: "verified collateral payout is bound to a different request or backend".into(),
                });
            }
            Some(receipt)
        }
    };
    if state.get_pending_three_usd_absorb(intent.vault_id).as_ref() != Some(intent) {
        return Err(StabilityPoolError::SystemBusy);
    }
    let stable_before = state
        .total_stablecoin_balances
        .get(&intent.ledger)
        .copied()
        .unwrap_or(0);
    let user_stable_before = state
        .deposits
        .values()
        .try_fold(0u64, |sum, position| {
            sum.checked_add(
                position
                    .stablecoin_balances
                    .get(&intent.ledger)
                    .copied()
                    .unwrap_or(0),
            )
        })
        .ok_or(StabilityPoolError::SystemBusy)?;
    state.deduct_exact_fee_from_pool(intent.ledger, fee_total_e8s)?;
    let mut realized_3usd_e8s = 0;
    if let Some(receipt) = verified_payout {
        let realized_3usd = three_usd_realized_amount(intent, receipt.backend_result.liquidated_debt);
        let collateral_net_e8s = receipt.payout.net_e8s;
        let collateral_price_e8s = intent.collateral_price_e8s.unwrap_or_default();
        if realized_3usd == 0 || collateral_net_e8s == 0 || collateral_price_e8s == 0 {
            return Err(StabilityPoolError::LiquidationFailed {
                vault_id: intent.vault_id,
                reason: "3USD absorb payout or exact realized amount is zero".into(),
            });
        }
        realized_3usd_e8s = realized_3usd;
        let collateral_type = intent
            .collateral_type
            .ok_or(StabilityPoolError::SystemBusy)?;
        let gains_before = state
            .deposits
            .values()
            .try_fold(0u64, |sum, position| {
                sum.checked_add(
                    position
                        .collateral_gains
                        .get(&collateral_type)
                        .copied()
                        .unwrap_or(0),
                )
            })
            .ok_or(StabilityPoolError::SystemBusy)?;
        state.process_three_usd_reserve_gains_exact_at(
            intent.vault_id,
            intent.ledger,
            collateral_type,
            realized_3usd,
            collateral_net_e8s,
            collateral_price_e8s,
            timestamp,
        )?;
        let gains_after = state
            .deposits
            .values()
            .try_fold(0u64, |sum, position| {
                sum.checked_add(
                    position
                        .collateral_gains
                        .get(&collateral_type)
                        .copied()
                        .unwrap_or(0),
                )
            })
            .ok_or(StabilityPoolError::SystemBusy)?;
        if gains_after.checked_sub(gains_before) != Some(collateral_net_e8s) {
            return Err(StabilityPoolError::LiquidationFailed {
                vault_id: intent.vault_id,
                reason:
                    "exact 3USD debit or collateral gain conservation failed; absorb remains held"
                        .into(),
            });
        }
    }
    let stable_after = state
        .total_stablecoin_balances
        .get(&intent.ledger)
        .copied()
        .unwrap_or(0);
    let user_stable_after = state
        .deposits
        .values()
        .try_fold(0u64, |sum, position| {
            sum.checked_add(
                position
                    .stablecoin_balances
                    .get(&intent.ledger)
                    .copied()
                    .unwrap_or(0),
            )
        })
        .ok_or(StabilityPoolError::SystemBusy)?;
    let expected_debit = fee_total_e8s
        .checked_add(realized_3usd_e8s)
        .ok_or(StabilityPoolError::SystemBusy)?;
    if stable_before.checked_sub(stable_after) != Some(expected_debit)
        || user_stable_before.checked_sub(user_stable_after) != Some(expected_debit)
    {
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id: intent.vault_id,
            reason: "3USD aggregate and depositor balances did not fall by the exact realized amount plus fees".into(),
        });
    }
    state.complete_three_usd_absorb(intent.vault_id, intent.absorb_id);
    Ok(())
}

/// Independently reconcile a bounded prefix of durable 3USD intents. This
/// does not require the vault to remain liquidatable: the collateral type and
/// price are the immutable pre-dispatch snapshot stored in the intent.
pub async fn recover_pending_three_usd_absorbs() {
    const MAX_PER_TICK: usize = 8;
    let Ok(_pool_guard) = crate::pool_guard::SpLiquidationGuard::new() else {
        return;
    };
    if crate::ensure_no_pool_balance_async_in_flight().is_err() {
        return;
    }
    let protocol_id = read_state(|s| s.protocol_canister_id);
    let pending: Vec<ThreeUsdReserveAbsorbIntent> =
        mutate_state(|state| state.take_pending_three_usd_absorb_page(MAX_PER_TICK));
    for intent in pending {
        let mut resolution = resolve_three_usd_absorb_status(protocol_id, &intent).await;
        if matches!(resolution, ThreeUsdReserveAbsorbStatusResolution::TransferSubmittedOrUnknown) {
            // Resume the same durable identity directly. This call never
            // performs another approval; the backend replays the exact tuple
            // or advances its durable TooOld full-prefix scan.
            let _resume: Result<(
                Result<StabilityPoolLiquidationResult, rumi_protocol_backend::ProtocolError>,
            ), _> = call(
                protocol_id,
                "stability_pool_liquidate_with_reserves_v2",
                (intent.vault_id, intent.absorb_id, intent.debt_e8s, intent.amount, intent.ledger),
            ).await;
            resolution = resolve_three_usd_absorb_status(protocol_id, &intent).await;
        }
        if matches!(resolution, ThreeUsdReserveAbsorbStatusResolution::TransferConfirmed) {
            if intent.collateral_type.is_none() || intent.collateral_ledger.is_none() || intent.collateral_price_e8s.is_none() {
                log!(INFO, "Holding legacy 3USD absorb {}: cannot resume a confirmed pull without immutable collateral metadata", intent.absorb_id);
                continue;
            }
            let _resume: Result<(
                Result<StabilityPoolLiquidationResult, rumi_protocol_backend::ProtocolError>,
            ), _> = call(
                protocol_id,
                "stability_pool_liquidate_with_reserves_v2",
                (intent.vault_id, intent.absorb_id, intent.debt_e8s, intent.amount, intent.ledger),
            ).await;
            resolution = resolve_three_usd_absorb_status(protocol_id, &intent).await;
        }
        match resolution {
            ThreeUsdReserveAbsorbStatusResolution::PreTransferRejected => {
                mutate_state(|s| s.clear_pending_three_usd_absorb(intent.vault_id));
            }
            ThreeUsdReserveAbsorbStatusResolution::FullyRefunded { ingress_fee_e8s, refund_fee_e8s } => {
                let Some(fees) = ingress_fee_e8s.checked_add(refund_fee_e8s) else { continue };
                let settled = mutate_state(|state| {
                    let mut next = state.clone();
                    apply_three_usd_absorb_settlement(
                        &mut next,
                        &intent,
                        fees,
                        ThreeUsdSettlementEvidence::TerminalRefund,
                    )?;
                    *state = next;
                    Ok::<(), StabilityPoolError>(())
                });
                if let Err(error) = settled {
                    log!(INFO, "3USD absorb {} terminal refund accounting remains held: {:?}", intent.absorb_id, error);
                }
            }
            ThreeUsdReserveAbsorbStatusResolution::Absorbed { verified_payout, ingress_fee_e8s, refund_fee_e8s } => {
                let Some(fees) = ingress_fee_e8s.checked_add(refund_fee_e8s) else { continue };
                let settled = mutate_state(|state| {
                    let mut next = state.clone();
                    apply_three_usd_absorb_settlement(
                        &mut next,
                        &intent,
                        fees,
                        ThreeUsdSettlementEvidence::CollateralPayout(&verified_payout),
                    )?;
                    *state = next;
                    Ok::<(), StabilityPoolError>(())
                });
                match settled {
                    Ok(()) => log!(INFO, "3USD absorb {} promoted exact collateral receipt {} at payout block {}; debited {} 3USD", intent.absorb_id, verified_payout.payout.net_e8s, verified_payout.block_index, three_usd_realized_amount(&intent, verified_payout.backend_result.liquidated_debt)),
                    Err(error) => log!(INFO, "3USD absorb {} receipt verified but exact depositor conservation failed; preserving intent: {:?}", intent.absorb_id, error),
                }
            }
            ThreeUsdReserveAbsorbStatusResolution::Pending(reason) => {
                log!(INFO, "3USD absorb {} remains held for reconciliation: {}", intent.absorb_id, reason);
            }
            ThreeUsdReserveAbsorbStatusResolution::Unseen
            | ThreeUsdReserveAbsorbStatusResolution::TransferConfirmed
            | ThreeUsdReserveAbsorbStatusResolution::TransferSubmittedOrUnknown => {}
        }
    }
}

async fn execute_single_liquidation(vault_info: &LiquidatableVaultInfo) -> LiquidationResult {
    if read_state(|s| s.collateral_requires_payout_address(&vault_info.collateral_type)) {
        return execute_native_xrp_absorb_with_io(vault_info, &mut CdkNativeXrpAbsorbIo).await;
    }

    let pending_intent = read_state(|s| s.get_pending_three_usd_absorb(vault_info.vault_id));
    if pending_intent.is_none() && read_state(|s| s.pending_three_usd_absorb_count() > 0) {
        return liquidation_failure(vault_info, StabilityPoolError::SystemBusy);
    }
    let protocol_id = read_state(|s| s.protocol_canister_id);
    let stablecoin_configs: BTreeMap<Principal, StablecoinConfig> =
        read_state(|s| s.stablecoin_registry.clone());
    let icusd_ledger = stablecoin_configs
        .iter()
        .find(|(_, c)| c.symbol == "icUSD")
        .map(|(id, _)| *id);

    // New work is limited to the one configured 3USD LP ledger. Ordinary
    // icUSD/ck-stable routes remain held until their own receipt rails exist.
    // Existing intents always take precedence and retain their saved tuple.
    let token_draw = if let Some(intent) = &pending_intent {
        BTreeMap::from([(intent.ledger, intent.amount)])
    } else {
        let fresh_draw = read_state(|state| {
            let target_debt = if vault_info.recommended_liquidation_amount > 0 {
                vault_info.recommended_liquidation_amount.min(vault_info.debt_amount)
            } else {
                vault_info.debt_amount
            };
            state.compute_three_usd_token_draw(target_debt, &vault_info.collateral_type)
        });
        let Some((ledger, amount, _)) = fresh_draw else {
            return LiquidationResult {
                vault_id: vault_info.vault_id,
                stables_consumed: BTreeMap::new(),
                collateral_gained: 0,
                collateral_type: vault_info.collateral_type,
                success: false,
                error_message: Some(
                    "No eligible configured 3USD balance; ordinary stablecoin liquidation remains held".into(),
                ),
            };
        };
        if vault_info.collateral_price_e8s == 0
            || read_state(|state| {
                state
                    .collateral_registry
                    .get(&vault_info.collateral_type)
                    .is_none()
            })
        {
            return LiquidationResult {
                vault_id: vault_info.vault_id,
                stables_consumed: BTreeMap::new(),
                collateral_gained: 0,
                collateral_type: vault_info.collateral_type,
                success: false,
                error_message: Some(
                    "Fresh 3USD liquidation requires a pinned collateral price and ledger"
                        .into(),
                ),
            };
        }
        if read_state(|state| state.configuration.emergency_pause) {
            return LiquidationResult {
                vault_id: vault_info.vault_id,
                stables_consumed: BTreeMap::new(),
                collateral_gained: 0,
                collateral_type: vault_info.collateral_type,
                success: false,
                error_message: Some("Pool paused before new 3USD absorb admission".into()),
            };
        }
        BTreeMap::from([(ledger, amount)])
    };
    // Recovery may only use the pre-existing exact identity. Never compute a
    // fresh draw for this vault, even if reconciliation clears the intent while
    // this call is in progress.
    log!(
        INFO,
        "Token draw for vault {}: {:?}",
        vault_info.vault_id,
        token_draw
    );

    // Step 2: Process each token in the draw
    let mut total_collateral_gained: u64 = 0;
    let mut actual_consumed: BTreeMap<Principal, u64> = BTreeMap::new();
    let mut non_lp_outcome_unknown = false;
    let mut held_absorb = false;

    // --- Non-LP tokens: approve + liquidate_vault_partial ---
    for (token_ledger, amount) in &token_draw {
        // A reserve request with an unknown backend outcome owns this vault's
        // next action. Do not issue a different-token liquidation against a
        // debt state that the pending request may already have changed.
        if read_state(|s| s.get_pending_three_usd_absorb(vault_info.vault_id).is_some()) {
            continue;
        }
        // Skip LP tokens — handled separately below
        if stablecoin_configs
            .get(token_ledger)
            .map(|c| c.is_lp_token.unwrap_or(false))
            .unwrap_or(false)
        {
            continue;
        }

        let is_icusd = icusd_ledger.map(|id| id == *token_ledger).unwrap_or(false);
        let token_decimals = stablecoin_configs
            .get(token_ledger)
            .map(|c| c.decimals)
            .unwrap_or(8);

        // Pre-check: backend minimum is 10_000_000 e8s (0.1 icUSD)
        let amount_e8s_check = if is_icusd {
            *amount
        } else {
            crate::types::normalize_to_e8s(*amount, token_decimals)
        };
        if amount_e8s_check < 10_000_000 {
            log!(
                INFO,
                "Skipping token {}: amount {} e8s below backend minimum (0.1)",
                token_ledger,
                amount_e8s_check
            );
            continue;
        }

        // The pool pays the LIVE ledger fee twice per liquidated token: once on
        // the `icrc2_approve` below, and again when the backend's
        // `icrc2_transfer_from` pulls the tokens (both are charged to the pool as
        // the `from`/approver account). Use the live `icrc1_fee` rather than the
        // (possibly stale) registry `transfer_fee` so the book decrement matches
        // exactly what the ledger charges.
        let ledger_fee = crate::deposits::ledger_transfer_fee(*token_ledger).await;

        // Approve backend to spend this token
        let approve_args = ApproveArgs {
            from_subaccount: None,
            spender: Account {
                owner: protocol_id,
                subaccount: None,
            },
            amount: candid::Nat::from(*amount as u128 * 2), // 2x buffer for fees
            expected_allowance: None,
            expires_at: Some(ic_cdk::api::time() + 300_000_000_000), // 5 min
            fee: None,
            memo: None,
            created_at_time: Some(ic_cdk::api::time()),
        };

        let approve_result: Result<(Result<candid::Nat, ApproveError>,), _> =
            call(*token_ledger, "icrc2_approve", (approve_args,)).await;

        match approve_result {
            Ok((Ok(_),)) => {
                // Deduct the approve fee from tracked balances. The matching
                // transfer_from fee is deducted only on a successful pull below
                // (a failed backend call charges no transfer_from fee).
                if ledger_fee > 0 {
                    mutate_state(|s| s.deduct_fee_from_pool(*token_ledger, ledger_fee));
                }
            }
            Ok((Err(e),)) => {
                log!(INFO, "Approve failed for {}: {:?}", token_ledger, e);
                continue;
            }
            Err(e) => {
                log!(INFO, "Approve call failed for {}: {:?}", token_ledger, e);
                continue;
            }
        }

        // No pre-deduct of depositor balances: `process_liquidation_gains` is the
        // single point of truth for stablecoin bookkeeping on a successful
        // liquidation (SP-001 regression fix, audit 2026-04-22-28e9896). Calling
        // `deduct_burned_lp_from_balances` here previously caused depositor balances
        // and the aggregate total to be decremented twice per liquidation — once
        // pre-call, once inside `process_liquidation_gains_at` — leaving phantom
        // tokens in the pool account per liquidation.

        // Call the appropriate backend endpoint
        let liq_result = if is_icusd {
            let call_result: Result<
                (
                    Result<
                        rumi_protocol_backend::SuccessWithFee,
                        rumi_protocol_backend::ProtocolError,
                    >,
                ),
                _,
            > = call(
                protocol_id,
                "liquidate_vault_partial",
                (rumi_protocol_backend::vault::VaultArg {
                    vault_id: vault_info.vault_id,
                    amount: *amount,
                },),
            )
            .await;
            call_result.map(|(r,)| r)
        } else {
            let token_type = determine_stable_token_type(*token_ledger, &stablecoin_configs);
            match token_type {
                Some(tt) => {
                    let amount_e8s = crate::types::normalize_to_e8s(*amount, token_decimals);
                    let call_result: Result<
                        (
                            Result<
                                rumi_protocol_backend::SuccessWithFee,
                                rumi_protocol_backend::ProtocolError,
                            >,
                        ),
                        _,
                    > = call(
                        protocol_id,
                        "liquidate_vault_partial_with_stable",
                        (rumi_protocol_backend::VaultArgWithToken {
                            vault_id: vault_info.vault_id,
                            amount: amount_e8s,
                            token_type: tt,
                        },),
                    )
                    .await;
                    call_result.map(|(r,)| r)
                }
                None => {
                    // Backend was never called; no bookkeeping to roll back.
                    log!(
                        INFO,
                        "Unknown stable token type for {}, skipping",
                        token_ledger
                    );
                    continue;
                }
            }
        };

        match liq_result {
            Ok(Ok(success)) => {
                let collateral = success
                    .collateral_amount_received
                    .unwrap_or(success.fee_amount_paid);
                log!(
                    INFO,
                    "Liquidation succeeded for vault {} with token {}: collateral={}, fee={}",
                    vault_info.vault_id,
                    token_ledger,
                    collateral,
                    success.fee_amount_paid
                );
                // SP-101 / SP-110: debit by what the backend ACTUALLY pulled from
                // the pool, not the amount we requested, so the tracked aggregate
                // never drifts from the real ledger balance. `process_liquidation_gains`
                // debits depositor balances exactly once, after this loop.
                //   - icUSD path: the backend pulled exactly the realized debt
                //     (`debt_liquidated_e8s`), no surcharge.
                //   - ckStable path: the backend pulled `base + repay-fee surcharge`
                //     (`stable_pulled_e6s`). Using only the base-debt conversion
                //     left the surcharge un-debited and the aggregate above the
                //     ledger (SP-110). Prefer the exact `stable_pulled_e6s`; fall
                //     back to the base conversion for an older backend wasm.
                let realized_consumed = match (success.debt_liquidated_e8s, is_icusd) {
                    (Some(debt_e8), true) => debt_e8,
                    (Some(debt_e8), false) => success.stable_pulled_e6s.unwrap_or_else(|| {
                        crate::types::denormalize_from_e8s(debt_e8, token_decimals)
                    }),
                    (None, _) => *amount,
                };
                actual_consumed.insert(*token_ledger, realized_consumed);
                // The backend's `icrc2_transfer_from` pull charged the pool a
                // second ledger fee (the approve fee was already deducted above).
                // Debit it too, otherwise the tracked aggregate stays one fee
                // above the live ledger balance per liquidation and eventually
                // trips the withdraw guard for non-sole holders (SP live-vs-ledger
                // drift, 2026-07-16).
                if ledger_fee > 0 {
                    mutate_state(|s| s.deduct_fee_from_pool(*token_ledger, ledger_fee));
                }
                total_collateral_gained += collateral;
                // Bug 7: one token per vault per round — vault state changed, remaining draws are stale
                break;
            }
            Ok(Err(protocol_error)) => {
                // Backend explicitly rejected; nothing was pre-deducted, so no rollback needed.
                log!(
                    INFO,
                    "Protocol rejected liquidation for vault {} with token {}: {:?}",
                    vault_info.vault_id,
                    token_ledger,
                    protocol_error
                );
            }
            Err(call_error) => {
                // Inter-canister call failed; outcome is unknown. Do not issue a
                // second liquidation call in this round: the backend may already
                // have changed vault state even though no reply arrived. We still
                // leave depositor bookkeeping untouched pending reconciliation.
                log!(
                    INFO,
                    "Liquidation call failed for vault {} with token {}: {:?}. \
                      Outcome unknown; holding remaining draws for a later round and \
                      reconciling pool balances if tokens moved silently.",
                    vault_info.vault_id,
                    token_ledger,
                    call_error
                );
                non_lp_outcome_unknown = true;
                break;
            }
        }
    }

    // --- LP tokens (3USD): approve + backend pull (atomic) ---
    let lp_draws: Vec<(&Principal, &u64)> = if pending_intent.is_some() {
        // Recovery follows the immutable journal even if registry metadata
        // changed after the operation was first admitted.
        token_draw.iter().collect()
    } else {
        lp_token_draws_after_non_lp_attempts(
            &token_draw,
            &stablecoin_configs,
            &actual_consumed,
            non_lp_outcome_unknown,
        )
    };
    for (token_ledger, amount) in lp_draws {
        if pending_intent.is_none() && !stablecoin_configs.get(token_ledger).is_some_and(|config| {
            config.ledger_id == *token_ledger
                && config.symbol == "3USD"
                && config.is_lp_token == Some(true)
                && config.decimals == 8
                && config.is_active
        }) {
            log!(INFO, "Skipping LP token {}: it is not the configured 3USD ledger", token_ledger);
            continue;
        }
        // A pending request pins the exact tuple across an ambiguous backend
        // reply. Resume only from its recorded ledger and amount; a fresh
        // liquidation scan may have calculated a different draw.
        let pending_intent = read_state(|s| s.get_pending_three_usd_absorb(vault_info.vault_id));
        if pending_intent
            .as_ref()
            .is_some_and(|intent| intent.ledger != *token_ledger)
        {
            continue;
        }
        let (requested_amount, icusd_equiv_e8s) = if let Some(intent) = &pending_intent {
            (intent.amount, intent.debt_e8s)
        } else {
            let equivalent = read_state(|s| {
                lp_liquidation_equivalent_e8s(s, *token_ledger, *amount)
            });
            let Some(equivalent) = equivalent else {
                log!(
                    INFO,
                    "Skipping LP token {} for vault {}: virtual price is unavailable",
                    token_ledger,
                    vault_info.vault_id
                );
                continue;
            };
            (*amount, equivalent)
        };
        let had_existing_intent = pending_intent.is_some();

        if icusd_equiv_e8s < 10_000_000 {
            log!(
                INFO,
                "Skipping LP token {}: icUSD equivalent {} e8s below backend minimum",
                token_ledger,
                icusd_equiv_e8s
            );
            continue;
        }

        // Persist the immutable request before any inter-canister await. Pause
        // blocks new admissions; it does not block recovery of an existing ID.
        let intent = match pending_intent {
            Some(intent) => intent,
            None => {
                if read_state(|s| s.configuration.emergency_pause) {
                    log!(INFO, "Holding new 3USD absorb for vault {} because the pool is paused", vault_info.vault_id);
                    continue;
                }
                match mutate_state(|s| {
                    if s.configuration.emergency_pause {
                        return Err(StabilityPoolError::EmergencyPaused);
                    }
                    let config_matches = s.stablecoin_registry.get(token_ledger).is_some_and(|config| {
                        config.ledger_id == *token_ledger
                            && config.symbol == "3USD"
                            && config.is_lp_token == Some(true)
                            && config.decimals == 8
                            && config.is_active
                    });
                    if !config_matches {
                        return Err(StabilityPoolError::LiquidationFailed {
                            vault_id: vault_info.vault_id,
                            reason: "3USD ledger is no longer the configured LP ledger".into(),
                        });
                    }
                    s.prepare_three_usd_absorb(
                        vault_info.vault_id,
                        icusd_equiv_e8s,
                        requested_amount,
                        *token_ledger,
                        vault_info.collateral_type,
                        vault_info.collateral_price_e8s,
                    )
                }) {
                    Ok(intent) => intent,
                    Err(error) => {
                        log!(INFO, "Could not persist 3USD reserve absorb for vault {}: {:?}", vault_info.vault_id, error);
                        continue;
                    }
                }
            }
        };

        if intent.collateral_ledger.is_none() {
            log!(INFO, "Holding 3USD absorb {}: collateral ledger identity was not pinned before backend dispatch", intent.absorb_id);
            continue;
        }

        // Resolve an existing durable identity before considering an approval.
        // Ambiguous and refund-pending backend states must never trigger a fresh
        // allowance transaction on every timer tick. Terminal states are replayed
        // through the idempotent V2 endpoint without another approval.
        let should_approve = if had_existing_intent {
            let existing_status = resolve_three_usd_absorb_status(protocol_id, &intent).await;
            match existing_status {
                ThreeUsdReserveAbsorbStatusResolution::Unseen => true,
                ThreeUsdReserveAbsorbStatusResolution::TransferSubmittedOrUnknown => {
                    log!(INFO, "Holding ambiguous 3USD absorb {} without re-approval; recovery timer will resume the exact tuple", intent.absorb_id);
                    continue;
                }
                ThreeUsdReserveAbsorbStatusResolution::TransferConfirmed => false,
                ThreeUsdReserveAbsorbStatusResolution::PreTransferRejected => {
                    mutate_state(|s| s.clear_pending_three_usd_absorb(intent.vault_id));
                    continue;
                }
                ThreeUsdReserveAbsorbStatusResolution::Absorbed { .. }
                | ThreeUsdReserveAbsorbStatusResolution::FullyRefunded { .. } => false,
                ThreeUsdReserveAbsorbStatusResolution::Pending(reason) => {
                    log!(INFO, "Holding 3USD absorb {} without re-approval: {}", intent.absorb_id, reason);
                    continue;
                }
            }
        } else {
            true
        };

        if should_approve && !read_state(|s| s.is_unique_pending_three_usd_absorb(&intent)) {
            log!(INFO, "Holding pre-dispatch 3USD absorb {}: another saved 3USD absorb still depends on the same depositor books", intent.absorb_id);
            continue;
        }

        // Step A: Approve backend to pull 3USD only when backend status proves
        // there is no durable transfer outcome. Existing ambiguous identities
        // are held above, so retries cannot repeatedly charge approval fees.
        if should_approve {
            // An Unseen saved intent is still pre-dispatch, so it must pass the
            // readiness handshake on every retry. Dispatched requests bypass this
            // branch and remain recoverable even if admission is later disabled.
            let ack: Result<(Result<(), rumi_protocol_backend::ProtocolError>,), _> =
                call(protocol_id, "acknowledge_three_usd_reserve_v2_client", ()).await;
            if !matches!(ack, Ok((Ok(()),))) {
                log!(INFO, "Holding 3USD absorb {}: V2 client handshake failed", intent.absorb_id);
                continue;
            }
            let ready: Result<(bool,), _> =
                call(protocol_id, "get_three_usd_reserve_ingress_enabled", ()).await;
            let readiness = match ready {
                Ok((enabled,)) => Some(enabled),
                Err(error) => {
                    log!(INFO, "Holding 3USD absorb {}: backend V2 readiness is unavailable: {:?}", intent.absorb_id, error);
                    None
                }
            };
            if !three_usd_backend_ready_for_new_absorb(readiness) {
                log!(INFO, "Holding 3USD absorb {}: backend V2 ingress is disabled or unavailable", intent.absorb_id);
                continue;
            }
            if read_state(|s| s.configuration.emergency_pause) {
                log!(INFO, "Holding 3USD absorb {} before approval because the pool is paused", intent.absorb_id);
                continue;
            }
            let live_approval_fee = crate::deposits::ledger_transfer_fee(*token_ledger).await;
            if live_approval_fee != 0 {
                log!(INFO, "Holding 3USD absorb {} because nonzero approval fees are unsupported", intent.absorb_id);
                continue;
            }
            if read_state(|s| s.configuration.emergency_pause) {
                log!(INFO, "Holding 3USD absorb {} after fee query because the pool is paused", intent.absorb_id);
                continue;
            }
            let approve_args = ApproveArgs {
                from_subaccount: None,
                spender: Account {
                    owner: protocol_id,
                    subaccount: None,
                },
                // Backend fee is pinned to zero on this 3pool route; authorize
                // exactly this absorb amount and leave no excess allowance behind.
                amount: candid::Nat::from(intent.amount as u128),
                expected_allowance: None,
                expires_at: Some(ic_cdk::api::time() + 300_000_000_000), // 5 min
                fee: Some(candid::Nat::from(0u64)),
                memo: None,
                created_at_time: Some(ic_cdk::api::time()),
            };

            let approve_result: Result<(Result<candid::Nat, ApproveError>,), _> =
                call(*token_ledger, "icrc2_approve", (approve_args,)).await;

            match approve_result {
                Ok((Ok(_),)) => {
                    // This path is admitted only for the current zero-fee 3pool
                    // ledger. Explicit fee=0 prevents fee drift from silently
                    // charging depositor assets; a changed fee returns BadFee.
                }
                Ok((Err(e),)) => {
                    // Preserve the request identity. The backend was not called,
                    // and a later retry can safely resolve Unseen before approval.
                    log!(
                        INFO,
                        "3USD approve failed for vault {}: {:?}",
                        vault_info.vault_id,
                        e
                    );
                    continue;
                }
                Err(e) => {
                    log!(
                        INFO,
                        "3USD approve call failed for vault {}: {:?}",
                        vault_info.vault_id,
                        e
                    );
                    continue;
                }
            }
        }

        // Approval can complete while an administrator pauses the pool. Do not
        // cross the backend pull boundary for a fresh/unseen request afterward.
        if should_approve && read_state(|s| s.configuration.emergency_pause) {
            log!(INFO, "Holding 3USD absorb {} after approval because the pool was paused before backend dispatch", intent.absorb_id);
            continue;
        }
        if should_approve && !read_state(|s| s.is_unique_pending_three_usd_absorb(&intent)) {
            log!(INFO, "Holding 3USD absorb {} after approval because the saved request is no longer the sole pending 3USD absorb", intent.absorb_id);
            continue;
        }

        // Step B: Ask backend to pull 3USD + write down debt atomically.
        // `process_liquidation_gains` runs once after this loop and is the single
        // point of truth for bookkeeping — no pre-deduct (SP-001 regression fix,
        // audit 2026-04-22-28e9896).

        let liq_result: Result<
            (Result<StabilityPoolLiquidationResult, rumi_protocol_backend::ProtocolError>,),
            _,
        > = call(
            protocol_id,
            "stability_pool_liquidate_with_reserves_v2",
            (
                intent.vault_id,
                intent.absorb_id,
                intent.debt_e8s,
                intent.amount,
                intent.ledger,
            ),
        )
        .await;

        let resolution = match liq_result {
            Ok((Ok(success),)) => {
                match resolve_three_usd_absorb_status(
                    protocol_id,
                    &intent,
                )
                .await
                {
                    ThreeUsdReserveAbsorbStatusResolution::Absorbed {
                        verified_payout,
                        ingress_fee_e8s,
                        refund_fee_e8s,
                    } if verified_payout.backend_result == success =>
                    {
                        ThreeUsdReserveAbsorbStatusResolution::Absorbed {
                            verified_payout,
                            ingress_fee_e8s,
                            refund_fee_e8s,
                        }
                    }
                    ThreeUsdReserveAbsorbStatusResolution::Pending(reason) => {
                        ThreeUsdReserveAbsorbStatusResolution::Pending(reason)
                    }
                    _ => ThreeUsdReserveAbsorbStatusResolution::Pending(
                        "successful update reply conflicts with backend terminal status".into(),
                    ),
                }
            }
            Ok((Err(error),)) => {
                log!(INFO, "Backend returned an error for 3USD reserve absorb {}: {:?}; reading durable status",
                    intent.absorb_id, error);
                resolve_three_usd_absorb_status(
                    protocol_id,
                    &intent,
                )
                .await
            }
            Err(error) => {
                log!(INFO, "3USD reserve absorb {} call outcome is unknown: {:?}; reading durable status",
                    intent.absorb_id, error);
                resolve_three_usd_absorb_status(
                    protocol_id,
                    &intent,
                )
                .await
            }
        };

        match resolution {
            ThreeUsdReserveAbsorbStatusResolution::Unseen => {
                log!(INFO, "3USD reserve absorb {} remains unseen after dispatch; preserving identity", intent.absorb_id);
            }
            ThreeUsdReserveAbsorbStatusResolution::TransferSubmittedOrUnknown => {
                log!(INFO, "3USD reserve absorb {} remains ambiguous after one exact-tuple recovery attempt; preserving identity", intent.absorb_id);
            }
            ThreeUsdReserveAbsorbStatusResolution::TransferConfirmed => {
                log!(INFO, "3USD reserve absorb {} has a verified ingress receipt; preserving identity for backend resume", intent.absorb_id);
            }
            ThreeUsdReserveAbsorbStatusResolution::Absorbed {
                verified_payout,
                ingress_fee_e8s,
                refund_fee_e8s,
            } => {
                log!(INFO, "3USD absorb {} has exact collateral payout proof at block {}; durable recovery timer will atomically promote gains (net {}, fees {} + {})", intent.absorb_id, verified_payout.block_index, verified_payout.payout.net_e8s, ingress_fee_e8s, refund_fee_e8s);
                held_absorb = true;
                continue;
            }
            ThreeUsdReserveAbsorbStatusResolution::PreTransferRejected => {
                mutate_state(|s| s.clear_pending_three_usd_absorb(intent.vault_id));
                log!(INFO, "Backend status proves 3USD reserve absorb {} was rejected before transfer", intent.absorb_id);
            }
            ThreeUsdReserveAbsorbStatusResolution::FullyRefunded {
                ingress_fee_e8s,
                refund_fee_e8s,
            } => {
                let fee_total = ingress_fee_e8s.checked_add(refund_fee_e8s);
                let accounted = fee_total.map(|fees| {
                    mutate_state(|state| {
                        let mut next = state.clone();
                        apply_three_usd_absorb_settlement(
                            &mut next,
                            &intent,
                            fees,
                            ThreeUsdSettlementEvidence::TerminalRefund,
                        )?;
                        *state = next;
                        Ok::<(), StabilityPoolError>(())
                    })
                });
                match accounted {
                    Some(Ok(())) => log!(INFO, "Backend status proves 3USD absorb {} fully refunded; exact ingress/refund fees were accounted", intent.absorb_id),
                    Some(Err(error)) => log!(INFO, "3USD absorb {} has a terminal refund receipt but exact fee accounting failed ({:?}); preserving pending identity", intent.absorb_id, error),
                    None => log!(INFO, "3USD absorb {} fee total overflowed; preserving pending identity", intent.absorb_id),
                }
            }
            ThreeUsdReserveAbsorbStatusResolution::Pending(reason) => {
                log!(INFO, "3USD reserve absorb {} remains pending: {}", intent.absorb_id, reason);
            }
        }
    }

    // Step 3: If any liquidation calls succeeded, process gains
    let liquidation_result = if !actual_consumed.is_empty() {
        // Deduct the collateral ledger's transfer fee from gains — the backend reports
        // gross collateral but the transfer to the SP deducts one fee.
        let collateral_fee: u64 = match call::<(), (candid::Nat,)>(
            vault_info.collateral_type,
            "icrc1_fee",
            (),
        )
        .await
        {
            Ok((fee_nat,)) => {
                let fee: u128 = fee_nat.0.try_into().unwrap_or(0);
                fee as u64
            }
            Err(e) => {
                // SP-104 (audit 2026-06-05): do NOT fall back to fee=0. The actual
                // payout transfer deducts the real ledger fee, so crediting the full
                // gross over-credits depositors and leaves the pool short by one fee.
                // Use a conservative fallback so we under- rather than over-credit
                // (solvency-safe); the next successful interaction reconciles.
                log!(INFO, "icrc1_fee query failed for collateral {}: {:?}; using conservative fallback {} e8s",
                    vault_info.collateral_type, e, FALLBACK_COLLATERAL_FEE_E8S);
                FALLBACK_COLLATERAL_FEE_E8S
            }
        };
        let net_collateral = total_collateral_gained.saturating_sub(collateral_fee);

        let accounting = mutate_state(|state| {
            let mut next = state.clone();
            next.process_liquidation_gains(
                vault_info.vault_id,
                vault_info.collateral_type,
                &actual_consumed,
                net_collateral,
                vault_info.collateral_price_e8s,
            );
            *state = next;
            Ok::<(), StabilityPoolError>(())
        });

        match accounting {
            Ok(()) => LiquidationResult {
                vault_id: vault_info.vault_id,
                stables_consumed: actual_consumed,
                collateral_gained: net_collateral,
                collateral_type: vault_info.collateral_type,
                success: true,
                error_message: None,
            },
            Err(error) => {
                log!(INFO, "Liquidation fee accounting failed for vault {}: {:?}; pending absorb identity and pool balances were preserved", vault_info.vault_id, error);
                LiquidationResult {
                    vault_id: vault_info.vault_id,
                    stables_consumed: BTreeMap::new(),
                    collateral_gained: 0,
                    collateral_type: vault_info.collateral_type,
                    success: false,
                    error_message: Some("Exact reserve fee accounting failed; reconciliation required".to_string()),
                }
            }
        }
    } else {
        LiquidationResult {
            vault_id: vault_info.vault_id,
            stables_consumed: BTreeMap::new(),
            collateral_gained: 0,
            collateral_type: vault_info.collateral_type,
            success: false,
            error_message: Some(if held_absorb {
                "Backend absorbed 3USD; Stability Pool settlement awaits collateral payout proof".to_string()
            } else {
                "All liquidation calls failed".to_string()
            }),
        }
    };

    // Record the state-accounting outcome, not merely the backend response. If
    // exact reserve fee accounting failed, the durable absorb identity remains
    // pending and no successful event is emitted.
    let stables_consumed_e8s: u64 = liquidation_result.stables_consumed.values().sum();
    mutate_state(|s| {
        s.push_event(
            s.protocol_canister_id,
            PoolEventType::LiquidationExecuted {
                vault_id: vault_info.vault_id,
                stables_consumed_e8s,
                collateral_gained: liquidation_result.collateral_gained,
                collateral_type: vault_info.collateral_type,
                success: liquidation_result.success,
            },
        );
    });
    liquidation_result
}

/// Select LP draws only when no earlier non-LP leg succeeded or has an
/// ambiguous outcome. A backend call can mutate vault state before its reply,
/// so remaining draws from the original snapshot must wait for a later round.
fn lp_token_draws_after_non_lp_attempts<'a>(
    token_draw: &'a BTreeMap<Principal, u64>,
    stablecoin_configs: &BTreeMap<Principal, StablecoinConfig>,
    actual_consumed: &BTreeMap<Principal, u64>,
    non_lp_outcome_unknown: bool,
) -> Vec<(&'a Principal, &'a u64)> {
    if non_lp_outcome_unknown || !actual_consumed.is_empty() {
        return Vec::new();
    }

    token_draw
        .iter()
        .filter(|(ledger, _)| {
            stablecoin_configs
                .get(*ledger)
                .is_some_and(|config| config.is_lp_token.unwrap_or(false))
        })
        .collect()
}

/// Thin translation layer: map a ledger principal to the backend's StableTokenType enum.
fn determine_stable_token_type(
    ledger: Principal,
    configs: &BTreeMap<Principal, StablecoinConfig>,
) -> Option<rumi_protocol_backend::StableTokenType> {
    let config = configs.get(&ledger)?;
    match config.symbol.as_str() {
        "ckUSDT" => Some(rumi_protocol_backend::StableTokenType::CKUSDT),
        "ckUSDC" => Some(rumi_protocol_backend::StableTokenType::CKUSDC),
        _ => None,
    }
}

/// Backend result type for debt-already-burned liquidations.
type StabilityPoolLiquidationResult = rumi_protocol_backend::StabilityPoolLiquidationResult;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{chain_collateral_sentinel, read_state, replace_state, StabilityPoolState};
    use candid::Nat;
    use icrc_ledger_types::icrc::generic_value::{ICRC3Map, ICRC3Value};
    use icrc_ledger_types::icrc1::transfer::Memo;
    use serde_bytes::ByteBuf;

    #[test]
    fn new_three_usd_absorb_requires_v2_readiness_before_intent_or_approval() {
        assert!(!three_usd_backend_ready_for_new_absorb(None));
        assert!(!three_usd_backend_ready_for_new_absorb(Some(false)));
        assert!(three_usd_backend_ready_for_new_absorb(Some(true)));
    }

    #[test]
    fn three_usd_payout_receipt_requires_every_exact_transfer_field() {
        let backend = principal(40);
        let pool = principal(41);
        let memo = [9u8; 16];
        let payout = rumi_protocol_backend::state::ThreeUsdReserveCollateralPayout {
            operation_id: 10,
            op_nonce: 10,
            collateral_type: principal(42),
            ledger: principal(42),
            source: Account {
                owner: backend,
                subaccount: None,
            },
            destination: Account {
                owner: pool,
                subaccount: None,
            },
            gross_e8s: 101,
            net_e8s: 100,
            expected_fee_e8s: 1,
            memo,
            created_at_time_ns: 77,
            fee_arg_e8s: Some(1),
            candidate_block_index: Some(88),
            observed_fee_e8s: Some(1),
            rearmed_attempts: Vec::new(),
        };
        let block = rumi_protocol_backend::icrc3_proof::DecodedBlock {
            btype: Some("1xfer".into()),
            op: "xfer".into(),
            from: Some(payout.source),
            to: Some(payout.destination),
            spender: None,
            amount: 100,
            fee: Some(1),
            created_at_time: Some(77),
            memo: Some(memo.to_vec()),
        };
        assert!(three_usd_payout_block_matches(&payout, &block));
        let mut wrong = block.clone();
        wrong.amount += 1;
        assert!(!three_usd_payout_block_matches(&payout, &wrong));
        let mut wrong = block.clone();
        wrong.to = Some(Account {
            owner: backend,
            subaccount: None,
        });
        assert!(!three_usd_payout_block_matches(&payout, &wrong));
        let mut wrong = block.clone();
        wrong.fee = Some(2);
        assert!(!three_usd_payout_block_matches(&payout, &wrong));
        let mut wrong = block;
        wrong.spender = Some(Account {
            owner: backend,
            subaccount: None,
        });
        assert!(!three_usd_payout_block_matches(&payout, &wrong));
    }

    #[test]
    fn native_icp_candidate_requires_direct_exact_index_response() {
        use rumi_protocol_backend::native_icp_proof::{
            ArchivedBlocksRange, Block, QueryArchiveFn, QueryBlocksResponse, Timestamp,
            Transaction,
        };
        let block = Block {
            parent_hash: None,
            transaction: Transaction {
                memo: 0,
                icrc1_memo: None,
                operation: None,
                created_at_time: Timestamp { timestamp_nanos: 1 },
            },
            timestamp: Timestamp { timestamp_nanos: 2 },
        };
        let direct = QueryBlocksResponse {
            chain_length: 89,
            certificate: None,
            blocks: vec![block.clone()],
            first_block_index: 88,
            archived_blocks: Vec::new(),
        };
        assert_eq!(native_icp_direct_candidate_block(direct, 88), Ok(block));

        let archive_only = QueryBlocksResponse {
            chain_length: 89,
            certificate: None,
            blocks: Vec::new(),
            first_block_index: 89,
            archived_blocks: vec![ArchivedBlocksRange {
                start: 88,
                length: 1,
                callback: QueryArchiveFn(candid::Func {
                    principal: principal(43),
                    method: "get_blocks".into(),
                }),
            }],
        };
        assert!(native_icp_direct_candidate_block(archive_only, 88).is_err());
    }

    fn principal(byte: u8) -> Principal {
        Principal::from_slice(&[byte])
    }

    fn verified_three_usd_fixture(
        intent: &ThreeUsdReserveAbsorbIntent,
        backend: Principal,
        pool: Principal,
        collateral: Principal,
    ) -> (
        StabilityPoolLiquidationResult,
        rumi_protocol_backend::state::ThreeUsdReserveCollateralPayout,
        rumi_protocol_backend::icrc3_proof::DecodedBlock,
    ) {
        let memo = rumi_protocol_backend::management::nonce_to_memo(10).0;
        let memo_array: [u8; 16] = memo.as_ref().try_into().expect("fixed-size nonce memo");
        let created_at_time_ns =
            rumi_protocol_backend::management::nonce_to_created_at_time(10);
        let result = StabilityPoolLiquidationResult {
            success: true,
            vault_id: intent.vault_id,
            liquidated_debt: 1,
            collateral_received: 8,
            collateral_type: collateral.to_text(),
            block_index: 100,
            fee: 0,
            collateral_price_e8s: 100_000_000,
        };
        let payout = rumi_protocol_backend::state::ThreeUsdReserveCollateralPayout {
            operation_id: 10,
            op_nonce: 10,
            collateral_type: collateral,
            ledger: collateral,
            source: Account { owner: backend, subaccount: None },
            destination: Account { owner: pool, subaccount: None },
            gross_e8s: 8,
            net_e8s: 7,
            expected_fee_e8s: 1,
            memo: memo_array,
            created_at_time_ns,
            fee_arg_e8s: Some(1),
            candidate_block_index: Some(88),
            observed_fee_e8s: Some(1),
            rearmed_attempts: Vec::new(),
        };
        let block = rumi_protocol_backend::icrc3_proof::DecodedBlock {
            btype: Some("1xfer".into()),
            op: "xfer".into(),
            from: Some(payout.source.clone()),
            to: Some(payout.destination.clone()),
            spender: None,
            amount: 7,
            fee: Some(1),
            created_at_time: Some(created_at_time_ns),
            memo: Some(memo_array.to_vec()),
        };
        (result, payout, block)
    }

    fn liquidation_token(ledger: Principal, is_lp_token: bool) -> StablecoinConfig {
        StablecoinConfig {
            ledger_id: ledger,
            symbol: if is_lp_token { "3USD" } else { "icUSD" }.into(),
            decimals: 8,
            priority: 1,
            is_active: true,
            transfer_fee: Some(1),
            is_lp_token: Some(is_lp_token),
            underlying_pool: None,
        }
    }

    #[test]
    fn lp_draws_require_no_success_or_ambiguous_non_lp_outcome() {
        let non_lp = principal(10);
        let lp = principal(11);
        let token_draw = BTreeMap::from([(non_lp, 100), (lp, 200)]);
        let configs = BTreeMap::from([
            (non_lp, liquidation_token(non_lp, false)),
            (lp, liquidation_token(lp, true)),
        ]);

        // An explicit backend rejection or pre-call failure leaves no realized
        // consumption or ambiguity, so LP fallback remains eligible this round.
        let no_success = BTreeMap::new();
        let fallback =
            lp_token_draws_after_non_lp_attempts(&token_draw, &configs, &no_success, false);
        assert_eq!(fallback.len(), 1);
        assert_eq!(*fallback[0].0, lp);
        assert_eq!(*fallback[0].1, 200);

        // A transport error can hide a successful backend mutation, so it must
        // hold LP fallback even though no realized consumption was recorded.
        assert!(
            lp_token_draws_after_non_lp_attempts(&token_draw, &configs, &no_success, true)
                .is_empty()
        );

        // Once a non-LP backend call succeeds, the LP loop receives no draw and
        // therefore cannot make a second liquidation call against stale vault state.
        let successful_non_lp = BTreeMap::from([(non_lp, 100)]);
        assert!(lp_token_draws_after_non_lp_attempts(
            &token_draw,
            &configs,
            &successful_non_lp,
            false,
        )
        .is_empty());

        // With no non-LP draw, the normal LP-only path is still selected.
        let lp_only_draw = BTreeMap::from([(lp, 200)]);
        let lp_only =
            lp_token_draws_after_non_lp_attempts(&lp_only_draw, &configs, &no_success, false);
        assert_eq!(lp_only.len(), 1);
        assert_eq!(*lp_only[0].0, lp);
        assert_eq!(*lp_only[0].1, 200);
    }

    #[test]
    fn lp_liquidation_requires_a_present_nonzero_virtual_price() {
        let lp_ledger = principal(30);
        let amount = 100_000_000;
        let mut state = StabilityPoolState::default();

        assert_eq!(lp_liquidation_equivalent_e8s(&state, lp_ledger, amount), None);

        state
            .cached_virtual_prices
            .as_mut()
            .unwrap()
            .insert(lp_ledger, 0);
        assert_eq!(lp_liquidation_equivalent_e8s(&state, lp_ledger, amount), None);

        state
            .cached_virtual_prices
            .as_mut()
            .unwrap()
            .insert(lp_ledger, 1_000_000_000_000_000_000);
        assert_eq!(
            lp_liquidation_equivalent_e8s(&state, lp_ledger, amount),
            Some(amount)
        );
    }

    fn icusd_ledger() -> Principal {
        Principal::from_slice(&[10])
    }

    fn ambiguous_native_xrp_burn_intent(vault_id: u64) -> NativeXrpAbsorbIntent {
        let mut state = test_state();
        prepare_or_reuse_native_xrp_absorb_intent_in_state(
            &mut state,
            &native_xrp_plan(vault_id, 42_000, 1_000),
            1,
        )
        .expect("persist intent");
        mark_native_xrp_absorb_burn_attempted_in_state(&mut state, vault_id, 2)
            .expect("mark dispatched");
        state.get_pending_native_xrp_absorb(vault_id).unwrap()
    }

    fn decoded_native_xrp_burn_block(
        intent: &NativeXrpAbsorbIntent,
        pool: Principal,
    ) -> rumi_protocol_backend::icrc3_proof::DecodedBlock {
        // Exercise a plausible legacy burn shape consistent with the sampled
        // xfer/mint blocks: top-level ts plus tx.op, no btype or explicit
        // burn `to`. No live burn block was observed. A present
        // tx.created_at_time still has to match the persisted tuple.
        let mut tx: ICRC3Map = std::collections::BTreeMap::new();
        tx.insert("op".into(), ICRC3Value::Text("burn".into()));
        tx.insert(
            "from".into(),
            ICRC3Value::Array(vec![ICRC3Value::Blob(ByteBuf::from(
                pool.as_slice().to_vec(),
            ))]),
        );
        tx.insert(
            "amt".into(),
            ICRC3Value::Nat(Nat::from(intent.icusd_to_burn_e8s)),
        );
        tx.insert(
            "memo".into(),
            ICRC3Value::Blob(ByteBuf::from(encode_chain_writedown_memo(intent.vault_id))),
        );
        tx.insert(
            "created_at_time".into(),
            ICRC3Value::Nat(Nat::from(intent.burn_created_at_time_ns)),
        );
        let mut outer: ICRC3Map = std::collections::BTreeMap::new();
        outer.insert("ts".into(), ICRC3Value::Nat(Nat::from(77u64)));
        outer.insert("tx".into(), ICRC3Value::Map(tx));
        rumi_protocol_backend::icrc3_proof::decode_block(&ICRC3Value::Map(outer))
            .expect("decode legacy ICRC-3 burn shape")
    }

    #[test]
    fn ambiguous_native_xrp_burn_recovery_requires_exact_direct_block_tuple() {
        let intent = ambiguous_native_xrp_burn_intent(903);
        let minting = intent.icusd_minting_account.clone();
        let pool = principal(42);
        let block = decoded_native_xrp_burn_block(&intent, pool);
        assert!(block.btype.is_none());
        assert!(validate_ambiguous_native_xrp_burn_block(&block, &intent, &minting, pool).is_ok());

        let mut wrong_btype = block.clone();
        wrong_btype.btype = Some("1xfer".into());
        assert!(
            validate_ambiguous_native_xrp_burn_block(&wrong_btype, &intent, &minting, pool)
                .is_err()
        );
        let mut wrong_op = block.clone();
        wrong_op.op = "xfer".into();
        assert!(
            validate_ambiguous_native_xrp_burn_block(&wrong_op, &intent, &minting, pool).is_err()
        );

        let mut wrong_amount = block.clone();
        wrong_amount.amount += 1;
        assert!(
            validate_ambiguous_native_xrp_burn_block(&wrong_amount, &intent, &minting, pool)
                .is_err()
        );
        let mut wrong_memo = block.clone();
        wrong_memo.memo = Some(b"unrelated".to_vec());
        assert!(
            validate_ambiguous_native_xrp_burn_block(&wrong_memo, &intent, &minting, pool).is_err()
        );
        let mut wrong_timestamp = block.clone();
        wrong_timestamp.created_at_time = Some(intent.burn_created_at_time_ns + 1);
        assert!(validate_ambiguous_native_xrp_burn_block(
            &wrong_timestamp,
            &intent,
            &minting,
            pool
        )
        .is_err());
        let mut wrong_source = block.clone();
        wrong_source.from.as_mut().unwrap().owner = principal(43);
        assert!(
            validate_ambiguous_native_xrp_burn_block(&wrong_source, &intent, &minting, pool)
                .is_err()
        );
        let mut nonzero_fee = block.clone();
        nonzero_fee.fee = Some(1);
        assert!(
            validate_ambiguous_native_xrp_burn_block(&nonzero_fee, &intent, &minting, pool)
                .is_err()
        );
        assert!(validate_ambiguous_native_xrp_burn_block(
            &block,
            &intent,
            &Account {
                owner: principal(91),
                subaccount: None,
            },
            pool
        )
        .is_err());
    }

    #[test]
    fn canonical_btype_burn_with_tx_ts_requires_the_persisted_timestamp() {
        let intent = ambiguous_native_xrp_burn_intent(906);
        let minting = intent.icusd_minting_account.clone();
        let pool = principal(42);
        let mut tx: ICRC3Map = std::collections::BTreeMap::new();
        tx.insert("op".into(), ICRC3Value::Text("burn".into()));
        tx.insert(
            "from".into(),
            ICRC3Value::Array(vec![ICRC3Value::Blob(ByteBuf::from(
                pool.as_slice().to_vec(),
            ))]),
        );
        tx.insert(
            "amt".into(),
            ICRC3Value::Nat(Nat::from(intent.icusd_to_burn_e8s)),
        );
        tx.insert(
            "memo".into(),
            ICRC3Value::Blob(ByteBuf::from(encode_chain_writedown_memo(intent.vault_id))),
        );
        tx.insert(
            "ts".into(),
            ICRC3Value::Nat(Nat::from(intent.burn_created_at_time_ns)),
        );
        let mut outer: ICRC3Map = std::collections::BTreeMap::new();
        outer.insert("btype".into(), ICRC3Value::Text("1burn".into()));
        outer.insert("ts".into(), ICRC3Value::Nat(Nat::from(77u64)));
        outer.insert("tx".into(), ICRC3Value::Map(tx));

        let block = rumi_protocol_backend::icrc3_proof::decode_block(&ICRC3Value::Map(
            outer.clone(),
        ))
        .expect("decode canonical btype burn block");
        assert_eq!(block.btype.as_deref(), Some("1burn"));
        assert_eq!(block.created_at_time, Some(intent.burn_created_at_time_ns));
        assert!(validate_ambiguous_native_xrp_burn_block(
            &block, &intent, &minting, pool
        )
        .is_ok());

        let mut wrong_outer = outer;
        let ICRC3Value::Map(wrong_tx) = wrong_outer.get_mut("tx").unwrap() else {
            unreachable!();
        };
        wrong_tx.insert(
            "ts".into(),
            ICRC3Value::Nat(Nat::from(intent.burn_created_at_time_ns + 1)),
        );
        let wrong_timestamp = rumi_protocol_backend::icrc3_proof::decode_block(
            &ICRC3Value::Map(wrong_outer),
        )
        .expect("decode burn block with changed tx.ts");
        assert!(validate_ambiguous_native_xrp_burn_block(
            &wrong_timestamp,
            &intent,
            &minting,
            pool,
        )
        .is_err());
    }

    #[test]
    fn recovered_native_xrp_burn_proof_is_attached_once_to_unchanged_intent() {
        let mut state = test_state();
        prepare_or_reuse_native_xrp_absorb_intent_in_state(
            &mut state,
            &native_xrp_plan(904, 42_000, 1_000),
            1,
        )
        .expect("persist intent");
        let expected = mark_native_xrp_absorb_burn_attempted_in_state(&mut state, 904, 2)
            .expect("mark dispatched");
        let proof = build_icusd_burn_proof(321, 904);
        mark_native_xrp_absorb_recovered_burn_proof_in_state(
            &mut state,
            &expected,
            proof.clone(),
            3,
        )
        .expect("attach verified positive proof");
        let recovered = state.get_pending_native_xrp_absorb(904).unwrap();
        assert_eq!(recovered.burn_proof, Some(proof));
        assert_eq!(recovered.status, NativeXrpAbsorbIntentStatus::Burned);
        assert_eq!(recovered.burn_attempted, Some(true));
        assert!(mark_native_xrp_absorb_recovered_burn_proof_in_state(
            &mut state,
            &expected,
            build_icusd_burn_proof(321, 904),
            4,
        )
        .is_err());
        assert_eq!(state.get_pending_native_xrp_absorb(904).unwrap(), recovered);

        let mut legacy_state = test_state();
        let mut legacy = prepare_or_reuse_native_xrp_absorb_intent_in_state(
            &mut legacy_state,
            &native_xrp_plan(905, 42_000, 1_000),
            1,
        )
        .expect("persist legacy-shaped intent");
        legacy.burn_attempted = None;
        legacy_state
            .put_pending_native_xrp_absorb(legacy.clone())
            .expect("persist legacy dispatch marker");
        mark_native_xrp_absorb_recovered_burn_proof_in_state(
            &mut legacy_state,
            &legacy,
            build_icusd_burn_proof(322, 905),
            3,
        )
        .expect("positive exact evidence resolves legacy unknown dispatch state");
        let recovered_legacy = legacy_state.get_pending_native_xrp_absorb(905).unwrap();
        assert_eq!(recovered_legacy.burn_attempted, Some(true));
        assert_eq!(
            recovered_legacy.burn_proof,
            Some(build_icusd_burn_proof(322, 905))
        );
    }

    #[test]
    fn three_usd_terminal_refund_completes_exact_intent_once() {
        let mut state = StabilityPoolState::default();
        let ledger = principal(13);
        let intent = state.prepare_three_usd_absorb(
            42, 500, 1_000, ledger, principal(14), 100_000_000,
        ).expect("persist intent before settlement");
        apply_three_usd_absorb_settlement_at(
            &mut state,
            &intent,
            0,
            ThreeUsdSettlementEvidence::TerminalRefund,
            123,
        )
        .expect("verified terminal outcome settles");
        assert!(state.get_pending_three_usd_absorb(42).is_none());
        assert!(state.completed_three_usd_absorbs.as_ref().unwrap().contains(&intent.absorb_id));
        assert!(apply_three_usd_absorb_settlement_at(
            &mut state,
            &intent,
            0,
            ThreeUsdSettlementEvidence::TerminalRefund,
            123,
        ).is_err(),
            "a timer/notification replay must not settle the same identity twice");
    }

    #[test]
    fn ordinary_liquidation_without_persisted_three_usd_intent_is_held_before_calls() {
        let mut state = test_state();
        let owner = user_a();
        let ledger = icusd_ledger();
        add_deposit_direct(&mut state, owner, ledger, 50_000_000);
        replace_state(state);

        let vault = LiquidatableVaultInfo {
            vault_id: 4242,
            collateral_type: principal(55),
            debt_amount: 20_000_000,
            collateral_amount: 1_000_000,
            recommended_liquidation_amount: 0,
            collateral_price_e8s: 100_000_000,
        };
        // This future must complete without entering any ledger or backend await.
        // The canister call APIs have no test runtime here, so a missed early guard
        // would fail before this assertion.
        let result = futures::executor::block_on(execute_single_liquidation(&vault));

        assert!(!result.success);
        assert!(result.error_message.as_deref().unwrap().contains("ordinary stablecoin liquidation remains held"));
        read_state(|state| {
            assert!(state.get_pending_three_usd_absorb(vault.vault_id).is_none());
            assert_eq!(state.total_stablecoin_balances.get(&ledger), Some(&50_000_000));
            assert_eq!(state.deposits[&owner].stablecoin_balances.get(&ledger), Some(&50_000_000));
        });
        replace_state(StabilityPoolState::default());
    }

    #[test]
    fn second_vault_cannot_start_while_a_three_usd_absorb_is_unresolved() {
        let mut state = test_state();
        let three_usd = principal(56);
        let collateral = principal(55);
        state.register_stablecoin(StablecoinConfig {
            ledger_id: three_usd,
            symbol: "3USD".into(),
            decimals: 8,
            priority: 3,
            is_active: true,
            transfer_fee: Some(0),
            is_lp_token: Some(true),
            underlying_pool: Some(principal(57)),
        });
        state.cached_virtual_prices =
            Some(BTreeMap::from([(three_usd, 1_000_000_000_000_000_000)]));
        add_deposit_direct(&mut state, user_a(), three_usd, 100_000_000);
        let first = state
            .prepare_three_usd_absorb(
                42,
                20_000_000,
                20_000_000,
                three_usd,
                collateral,
                100_000_000,
            )
            .expect("first vault has a durable request");
        replace_state(state);

        let second = LiquidatableVaultInfo {
            vault_id: 43,
            collateral_type: collateral,
            debt_amount: 20_000_000,
            collateral_amount: 1_000_000,
            recommended_liquidation_amount: 0,
            collateral_price_e8s: 100_000_000,
        };
        // No inter-canister runtime is installed. An admission path that
        // reaches approval or the backend would fail this synchronous test.
        let result = futures::executor::block_on(execute_single_liquidation(&second));
        assert!(!result.success);
        assert!(result
            .error_message
            .as_deref()
            .unwrap()
            .contains("SystemBusy"));
        read_state(|state| {
            assert_eq!(state.get_pending_three_usd_absorb(42), Some(first));
            assert!(state.get_pending_three_usd_absorb(43).is_none());
            assert_eq!(state.total_stablecoin_balances.get(&three_usd), Some(&100_000_000));
        });
        replace_state(StabilityPoolState::default());
    }

    #[test]
    fn fresh_three_usd_liquidation_rejects_unpinned_collateral_metadata_before_calls() {
        let mut state = test_state();
        let three_usd = principal(56);
        let collateral = principal(55);
        state.register_stablecoin(StablecoinConfig {
            ledger_id: three_usd,
            symbol: "3USD".into(),
            decimals: 8,
            priority: 3,
            is_active: true,
            transfer_fee: Some(0),
            is_lp_token: Some(true),
            underlying_pool: Some(principal(57)),
        });
        state.register_collateral(CollateralInfo {
            ledger_id: collateral,
            symbol: "ICP".into(),
            decimals: 8,
            status: CollateralStatus::Active,
        });
        state.cached_virtual_prices = Some(BTreeMap::from([(three_usd, 1_000_000_000_000_000_000)]));
        add_deposit_direct(&mut state, user_a(), three_usd, 100_000_000);
        replace_state(state);

        let vault = LiquidatableVaultInfo {
            vault_id: 4243,
            collateral_type: collateral,
            debt_amount: 20_000_000,
            collateral_amount: 1_000_000,
            recommended_liquidation_amount: 0,
            collateral_price_e8s: 0,
        };
        let result = futures::executor::block_on(execute_single_liquidation(&vault));
        assert!(!result.success);
        assert!(result.error_message.as_deref().unwrap().contains("pinned collateral price"));
        read_state(|state| {
            assert!(state.get_pending_three_usd_absorb(vault.vault_id).is_none());
            assert_eq!(state.total_stablecoin_balances.get(&three_usd), Some(&100_000_000));
        });

        mutate_state(|state| {
            state.collateral_registry.remove(&collateral);
        });
        let missing_ledger = LiquidatableVaultInfo {
            collateral_price_e8s: 100_000_000,
            ..vault
        };
        let result = futures::executor::block_on(execute_single_liquidation(&missing_ledger));
        assert!(!result.success);
        assert!(result.error_message.as_deref().unwrap().contains("pinned collateral price"));
        read_state(|state| {
            assert!(state.get_pending_three_usd_absorb(vault.vault_id).is_none());
            assert_eq!(state.total_stablecoin_balances.get(&three_usd), Some(&100_000_000));
        });
    }

    #[test]
    fn mismatched_direct_block_cannot_create_a_payout_capability() {
        let mut state = StabilityPoolState::default();
        let ledger = principal(13);
        let collateral = principal(14);
        let backend = principal(44);
        let pool = principal(45);
        state.protocol_canister_id = backend;
        state.collateral_registry.insert(collateral, crate::types::CollateralInfo {
            ledger_id: collateral,
            symbol: "ICP".into(),
            decimals: 8,
            status: crate::types::CollateralStatus::Active,
        });
        let intent = state.prepare_three_usd_absorb(
            42, 1, 1, ledger, collateral, 100_000_000,
        ).expect("persist intent before recovery");
        let (result, payout, mut block) =
            verified_three_usd_fixture(&intent, backend, pool, collateral);
        block.amount -= 1;
        assert!(VerifiedThreeUsdCollateralPayout::from_direct_icrc3_block(
            &intent, &result, backend, pool, payout, 88, &block,
        ).is_err(), "a candidate index plus a mismatched ledger block is not a receipt");
        assert_eq!(state.get_pending_three_usd_absorb(42), Some(intent));
        assert!(state.completed_three_usd_absorbs.as_ref().unwrap().is_empty());
        assert!(state.total_stablecoin_balances.is_empty());
        assert!(state.deposits.is_empty());
    }

    #[test]
    fn payout_capability_rejects_changed_request_amount_or_ledger_without_promotion() {
        let mut state = StabilityPoolState::default();
        let ledger = principal(13);
        let collateral = principal(14);
        let backend = principal(44);
        let pool = principal(45);
        state.protocol_canister_id = backend;
        state.collateral_registry.insert(collateral, crate::types::CollateralInfo {
            ledger_id: collateral,
            symbol: "ICP".into(),
            decimals: 8,
            status: crate::types::CollateralStatus::Active,
        });
        state.register_stablecoin(liquidation_token(ledger, true));
        state.add_deposit_at(user_a(), ledger, 2, 123);
        let intent = state
            .prepare_three_usd_absorb(42, 2, 2, ledger, collateral, 100_000_000)
            .expect("immutable request persists before backend call");
        let (result, payout, block) = verified_three_usd_fixture(&intent, backend, pool, collateral);
        let verified = VerifiedThreeUsdCollateralPayout::from_direct_icrc3_block(
            &intent, &result, backend, pool, payout, 88, &block,
        ).expect("exact ledger receipt binds to original request");

        let mut changed_amount = intent.clone();
        changed_amount.amount += 1;
        let amount_result = apply_three_usd_absorb_settlement_at(
            &mut state,
            &changed_amount,
            0,
            ThreeUsdSettlementEvidence::CollateralPayout(&verified),
            123,
        );
        assert!(amount_result.is_err());

        let mut changed_ledger = intent.clone();
        changed_ledger.ledger = principal(15);
        let ledger_result = apply_three_usd_absorb_settlement_at(
            &mut state,
            &changed_ledger,
            0,
            ThreeUsdSettlementEvidence::CollateralPayout(&verified),
            123,
        );
        assert!(ledger_result.is_err());
        assert_eq!(state.get_pending_three_usd_absorb(42), Some(intent));
        assert_eq!(state.total_stablecoin_balances.get(&ledger), Some(&2));
        assert_eq!(state.deposits[&user_a()].stablecoin_balances.get(&ledger), Some(&2));
        assert!(state.deposits[&user_a()].collateral_gains.get(&collateral).is_none());
        assert!(state.completed_three_usd_absorbs.as_ref().unwrap().is_empty());
    }

    #[test]
    fn exact_receipt_settlement_promotes_once_with_two_depositors_and_tiny_draw() {
        let mut state = StabilityPoolState::default();
        let ledger = principal(13);
        let collateral = principal(14);
        let backend = principal(44);
        let pool = principal(45);
        state.protocol_canister_id = backend;
        state.collateral_registry.insert(collateral, crate::types::CollateralInfo {
            ledger_id: collateral,
            symbol: "ICP".into(),
            decimals: 8,
            status: crate::types::CollateralStatus::Active,
        });
        state.register_stablecoin(liquidation_token(ledger, true));
        state
            .cached_virtual_prices
            .get_or_insert_with(BTreeMap::new)
            .insert(ledger, 1_000_000_000_000_000_000);
        let first = principal(21);
        let second = principal(22);
        state.add_deposit_at(first, ledger, 1, 123);
        state.add_deposit_at(second, ledger, 1, 123);
        let intent = state
            .prepare_three_usd_absorb(42, 1, 1, ledger, collateral, 100_000_000)
            .expect("immutable request persists");
        let (result, payout, block) = verified_three_usd_fixture(&intent, backend, pool, collateral);
        let verified = VerifiedThreeUsdCollateralPayout::from_direct_icrc3_block(
            &intent, &result, backend, pool, payout, 88, &block,
        ).expect("exact direct ledger transfer creates the opaque receipt");

        apply_three_usd_absorb_settlement_at(
            &mut state,
            &intent,
            0,
            ThreeUsdSettlementEvidence::CollateralPayout(&verified),
            123,
        )
            .expect("exact receipt plus exact debit promotes");
        assert_eq!(state.total_stablecoin_balances.get(&ledger), Some(&1));
        assert_eq!(
            state.deposits[&first].collateral_gains.get(&collateral),
            Some(&7)
        );
        assert_eq!(state.get_pending_three_usd_absorb(42), None);
        assert!(state
            .completed_three_usd_absorbs
            .as_ref()
            .unwrap()
            .contains(&intent.absorb_id));
        assert!(
            apply_three_usd_absorb_settlement_at(
                &mut state,
                &intent,
                0,
                ThreeUsdSettlementEvidence::CollateralPayout(&verified),
                123,
            )
                .is_err(),
            "completed absorb cannot promote twice"
        );
        assert_eq!(
            state.deposits[&first].collateral_gains.get(&collateral),
            Some(&7)
        );
    }

    #[test]
    fn three_usd_refund_requires_exact_terminal_receipt_tuple() {
        let stability_pool = principal(11);
        let protocol = principal(12);
        let mut receipt = rumi_protocol_backend::state::ThreeUsdReserveRefundReceipt {
            block_index: 101,
            tuple: rumi_protocol_backend::state::ThreeUsdReserveRefundTuple {
                source_owner: protocol,
                source_subaccount: None,
                destination: Account {
                    owner: stability_pool,
                    subaccount: None,
                },
                amount_e8s: 1_000,
                charged_fee_e8s: 0,
                fee_e8s: Some(0),
                memo: [8; 16],
                created_at_time_ns: 123,
            },
        };

        assert!(exact_three_usd_refund_receipt_matches(
            protocol, stability_pool, 1_000, 100, &receipt
        ));
        receipt.tuple.amount_e8s -= 1;
        assert!(!exact_three_usd_refund_receipt_matches(
            protocol, stability_pool, 1_000, 100, &receipt
        ));
    }

    fn ckusdc_ledger() -> Principal {
        Principal::from_slice(&[11])
    }

    fn xrp_ledger() -> Principal {
        rumi_protocol_backend::state::xrp_collateral_principal()
    }

    fn valid_xrp_address() -> String {
        "rUn84CUYbNjRoTQ6mSW7BVJPSVJNLb1QLo".to_string()
    }

    fn user_a() -> Principal {
        Principal::from_slice(&[1])
    }

    fn user_b() -> Principal {
        Principal::from_slice(&[2])
    }

    fn test_state() -> StabilityPoolState {
        let mut state = StabilityPoolState::default();
        state.register_stablecoin(StablecoinConfig {
            ledger_id: icusd_ledger(),
            symbol: "icUSD".to_string(),
            decimals: 8,
            priority: 1,
            is_active: true,
            transfer_fee: Some(100_000),
            is_lp_token: None,
            underlying_pool: None,
        });
        state.register_stablecoin(StablecoinConfig {
            ledger_id: ckusdc_ledger(),
            symbol: "ckUSDC".to_string(),
            decimals: 6,
            priority: 2,
            is_active: true,
            transfer_fee: Some(10),
            is_lp_token: None,
            underlying_pool: None,
        });
        state.register_collateral(CollateralInfo {
            ledger_id: xrp_ledger(),
            symbol: "XRP".to_string(),
            decimals: 6,
            status: CollateralStatus::Active,
        });
        state
    }

    fn add_deposit_direct(
        state: &mut StabilityPoolState,
        user: Principal,
        token: Principal,
        amount: u64,
    ) {
        let position = state
            .deposits
            .entry(user)
            .or_insert_with(|| DepositPosition::new(0));
        *position.stablecoin_balances.entry(token).or_insert(0) += amount;
        *state.total_stablecoin_balances.entry(token).or_insert(0) += amount;
    }

    fn chain_vault(debt_e8s: u128, sp_attempted: bool) -> ChainLiquidatableVaultInfo {
        chain_vault_with_id(77, debt_e8s, sp_attempted)
    }

    fn chain_vault_with_id(
        vault_id: u64,
        debt_e8s: u128,
        sp_attempted: bool,
    ) -> ChainLiquidatableVaultInfo {
        ChainLiquidatableVaultInfo {
            vault_id,
            chain_id: rumi_protocol_backend::chains::config::ChainId(1030),
            chain_collateral_sentinel: chain_collateral_sentinel(1030),
            sp_attempted,
            debt_e8s,
            effective_debt_e8s: debt_e8s,
            collateral_native: 1_000_000_000_000_000_000_000,
            cr_e4: 12_000,
            liquidation_threshold_e4: 13_500,
            sized_repay_e8s: debt_e8s,
        }
    }

    fn minting_account() -> Account {
        Account {
            owner: principal(90),
            subaccount: None,
        }
    }

    fn backend_chain_result() -> ChainStabilityPoolLiquidationResult {
        ChainStabilityPoolLiquidationResult {
            success: true,
            vault_id: 77,
            chain_id: rumi_protocol_backend::chains::config::ChainId(1030),
            liquidated_debt_e8s: 100_00000000,
            collateral_received_native: 10_000_000_000_000_000_000u128,
            claim_id: 77,
            custody_address: "0xcustody".to_string(),
            block_index: 44,
            collateral_price_e8s: 5_000_000,
        }
    }

    #[derive(Default)]
    struct FakeNativeXrpAbsorbIo {
        preflight: Option<XrpSpAbsorbPreflight>,
        submit_result: Option<XrpSpAbsorbResult>,
        minting_account: Option<Account>,
        burn_proof: Option<rumi_protocol_backend::icrc3_proof::SpWritedownProof>,
        ambiguous_burn_failure: bool,
        burn_attempts: usize,
        events: Vec<String>,
        submitted_requests: Vec<XrpSpAbsorbRequest>,
    }

    #[async_trait::async_trait(?Send)]
    impl NativeXrpAbsorbIo for FakeNativeXrpAbsorbIo {
        fn now_ns(&self) -> u64 {
            123_456_789
        }

        async fn fetch_icusd_minting_account(
            &mut self,
            _icusd_ledger: Principal,
        ) -> Result<Account, StabilityPoolError> {
            self.events.push("minting_account".to_string());
            Ok(self.minting_account.clone().unwrap_or_else(minting_account))
        }

        async fn preflight_xrp_absorb(
            &mut self,
            _protocol_id: Principal,
            vault_id: u64,
            expected_icusd_burn_e8s: u64,
        ) -> Result<XrpSpAbsorbPreflight, StabilityPoolError> {
            self.events
                .push(format!("preflight:{vault_id}:{expected_icusd_burn_e8s}"));
            self.preflight
                .clone()
                .ok_or_else(|| StabilityPoolError::LiquidationFailed {
                    vault_id,
                    reason: "test preflight missing".to_string(),
                })
        }

        async fn burn_icusd(
            &mut self,
            _icusd_ledger: Principal,
            _minting_account: Account,
            amount_e8s: u64,
            vault_id: u64,
            created_at_time: u64,
        ) -> Result<rumi_protocol_backend::icrc3_proof::SpWritedownProof, IcusdBurnAttemptError>
        {
            self.burn_attempts += 1;
            self.events
                .push(format!("burn:{vault_id}:{amount_e8s}:{created_at_time}"));
            if self.ambiguous_burn_failure {
                return Err(IcusdBurnAttemptError::ambiguous(
                    StabilityPoolError::LiquidationFailed {
                        vault_id,
                        reason: "simulated ambiguous burn result".into(),
                    },
                ));
            }
            Ok(self
                .burn_proof
                .clone()
                .unwrap_or_else(|| build_icusd_burn_proof(44, vault_id)))
        }

        async fn release_xrp_absorb_preflight(
            &mut self,
            _protocol_id: Principal,
            vault_id: u64,
            icusd_burn_e8s: u64,
        ) {
            self.events
                .push(format!("release:{vault_id}:{icusd_burn_e8s}"));
        }

        async fn submit_xrp_absorb(
            &mut self,
            _protocol_id: Principal,
            request: XrpSpAbsorbRequest,
        ) -> Result<XrpSpAbsorbResult, StabilityPoolError> {
            self.events.push(format!(
                "submit:{}:{}:{}",
                request.vault_id,
                request.icusd_burned_e8s,
                request.allocations.len()
            ));
            self.submitted_requests.push(request);
            self.submit_result
                .clone()
                .ok_or_else(|| StabilityPoolError::LiquidationFailed {
                    vault_id: 0,
                    reason: "test submit result missing".to_string(),
                })
        }
    }

    fn xrp_vault(vault_id: u64, debt_amount: u64) -> LiquidatableVaultInfo {
        LiquidatableVaultInfo {
            vault_id,
            collateral_type: xrp_ledger(),
            debt_amount,
            collateral_amount: 5_000_000,
            recommended_liquidation_amount: 0,
            collateral_price_e8s: 50_00000000,
        }
    }

    fn xrp_preflight(
        vault_id: u64,
        icusd_burn_e8s: u64,
        collateral_received_drops: u64,
    ) -> XrpSpAbsorbPreflight {
        XrpSpAbsorbPreflight {
            vault_id,
            icusd_burn_e8s,
            collateral_received_drops,
            collateral_price_e8s: 50_00000000,
            expires_at_ns: 999,
        }
    }

    fn xrp_backend_result(
        vault_id: u64,
        icusd_burned_e8s: u64,
        collateral_received_drops: u64,
    ) -> XrpSpAbsorbResult {
        XrpSpAbsorbResult {
            success: true,
            vault_id,
            liquidated_debt_e8s: icusd_burned_e8s,
            collateral_received_drops,
            payout_claims: vec![XrpSpPayoutClaim {
                claimant: user_a(),
                claim_id: 7001,
                payout_address: valid_xrp_address(),
                destination_tag: Some(7),
                drops: collateral_received_drops,
            }],
            block_index: 1440,
            collateral_price_e8s: 50_00000000,
        }
    }

    fn native_xrp_plan(
        vault_id: u64,
        icusd_to_burn_e8s: u64,
        collateral_received_drops: u64,
    ) -> NativeXrpAbsorbPlan {
        let mut stables_consumed = BTreeMap::new();
        stables_consumed.insert(icusd_ledger(), icusd_to_burn_e8s);
        NativeXrpAbsorbPlan {
            vault_id,
            collateral_type: xrp_ledger(),
            icusd_ledger: icusd_ledger(),
            icusd_minting_account: minting_account(),
            icusd_to_burn_e8s,
            stables_consumed,
            collateral_received_drops,
            collateral_price_e8s: 50_00000000,
            allocations: vec![XrpSpPayoutAllocation {
                claimant: user_a(),
                payout_address: valid_xrp_address(),
                destination_tag: Some(7),
                drops: collateral_received_drops,
            }],
        }
    }

    #[test]
    fn sp_burn_refund_memo_accepts_original_and_nonzero_rotated_attempts_only() {
        let burn_block_index = 0x0102_0304_0506_0708;
        let vault_id = 0x1112_1314_1516_1718;
        let original = expected_sp_burn_refund_memo(burn_block_index, vault_id);
        assert!(sp_burn_refund_memo_matches(
            &original,
            burn_block_index,
            vault_id
        ));

        let mut rotated = original.clone();
        rotated.extend_from_slice(&1u64.to_be_bytes());
        assert!(sp_burn_refund_memo_matches(
            &rotated,
            burn_block_index,
            vault_id
        ));

        let mut zero_attempt = original.clone();
        zero_attempt.extend_from_slice(&0u64.to_be_bytes());
        assert!(!sp_burn_refund_memo_matches(
            &zero_attempt,
            burn_block_index,
            vault_id
        ));
        let mut excessive_attempt = original.clone();
        excessive_attempt.extend_from_slice(
            &((rumi_protocol_backend::state::MAX_SP_BURN_REFUND_ATTEMPTS as u64) + 1)
                .to_be_bytes(),
        );
        assert!(!sp_burn_refund_memo_matches(
            &excessive_attempt,
            burn_block_index,
            vault_id
        ));
        assert!(!sp_burn_refund_memo_matches(
            &rotated[..rotated.len() - 1],
            burn_block_index,
            vault_id
        ));

        let mut wrong_identity = rotated;
        wrong_identity[8] ^= 1;
        assert!(!sp_burn_refund_memo_matches(
            &wrong_identity,
            burn_block_index,
            vault_id
        ));
    }

    #[test]
    fn native_xrp_recovery_selector_uses_pending_ids_even_without_liquidatable_entries() {
        let mut state = test_state();
        // Intentionally leave the liquidatable list empty: recovery is driven
        // by durable burn intents, not by the live liquidation discovery view.
        for vault_id in [12, 24, 36] {
            let intent = prepare_or_reuse_native_xrp_absorb_intent_in_state(
                &mut state,
                &native_xrp_plan(vault_id, 100, 1_000),
                1,
            )
            .unwrap();
            if vault_id != 24 {
                mark_native_xrp_absorb_burn_attempted_in_state(&mut state, vault_id, 2).unwrap();
                mark_native_xrp_absorb_burned_in_state(
                    &mut state,
                    vault_id,
                    build_icusd_burn_proof(44, vault_id),
                    3,
                )
                .unwrap();
            } else {
                // A legacy/ambiguous intent with no exact proof must remain
                // held and must not be sent to automatic refund recovery.
                let mut legacy = intent;
                legacy.burn_attempted = None;
                legacy.status = NativeXrpAbsorbIntentStatus::Burned;
                state.put_pending_native_xrp_absorb(legacy).unwrap();
            }
        }
        replace_state(state);

        assert_eq!(pending_native_xrp_recovery_vault_ids(None, 2), vec![12, 36]);
        assert_eq!(pending_native_xrp_recovery_vault_ids(Some(12), 1), vec![36]);
        assert!(pending_native_xrp_recovery_vault_ids(None, 0).is_empty());
    }

    #[test]
    fn pre_cl08_native_xrp_intent_decodes_unknown_dispatch_and_remains_held() {
        #[derive(serde::Serialize)]
        struct LegacyNativeXrpAbsorbIntent {
            vault_id: u64,
            collateral_type: Principal,
            icusd_ledger: Principal,
            icusd_minting_account: Account,
            icusd_to_burn_e8s: u64,
            stables_consumed: BTreeMap<Principal, u64>,
            collateral_received_drops: u64,
            collateral_price_e8s: u64,
            allocations: Vec<XrpSpPayoutAllocation>,
            burn_created_at_time_ns: u64,
            status: NativeXrpAbsorbIntentStatus,
            burn_proof: Option<rumi_protocol_backend::icrc3_proof::SpWritedownProof>,
            backend_result: Option<XrpSpAbsorbResult>,
            last_error: Option<String>,
            created_at_ns: u64,
            updated_at_ns: u64,
        }

        let mut state = test_state();
        let current = prepare_or_reuse_native_xrp_absorb_intent_in_state(
            &mut state,
            &native_xrp_plan(79, 10_00000000, 500),
            1,
        )
        .unwrap();
        let legacy = LegacyNativeXrpAbsorbIntent {
            vault_id: current.vault_id,
            collateral_type: current.collateral_type,
            icusd_ledger: current.icusd_ledger,
            icusd_minting_account: current.icusd_minting_account,
            icusd_to_burn_e8s: current.icusd_to_burn_e8s,
            stables_consumed: current.stables_consumed,
            collateral_received_drops: current.collateral_received_drops,
            collateral_price_e8s: current.collateral_price_e8s,
            allocations: current.allocations,
            burn_created_at_time_ns: current.burn_created_at_time_ns,
            status: current.status,
            burn_proof: None,
            backend_result: None,
            last_error: None,
            created_at_ns: current.created_at_ns,
            updated_at_ns: current.updated_at_ns,
        };
        let mut snapshot = Vec::new();
        ciborium::ser::into_writer(&legacy, &mut snapshot).expect("encode pre-CL08 intent");
        let decoded: NativeXrpAbsorbIntent =
            ciborium::de::from_reader(snapshot.as_slice()).expect("decode pre-CL08 intent");

        assert_eq!(decoded.burn_attempted, None);
        assert!(decoded.burn_proof.is_none());
        state.put_pending_native_xrp_absorb(decoded).unwrap();
        replace_state(state);

        assert!(crate::pool_token_balance_mutation_blocked(&[icusd_ledger()]));
        assert!(pending_native_xrp_recovery_vault_ids(None, 2).is_empty());
        replace_state(StabilityPoolState::default());
    }

    #[test]
    fn pending_icusd_absorb_allows_unrelated_token_deposit_and_withdraw_without_changing_burn_share(
    ) {
        let mut state = test_state();
        for user in [user_a(), user_b()] {
            add_deposit_direct(&mut state, user, icusd_ledger(), 100_00000000);
            add_deposit_direct(&mut state, user, ckusdc_ledger(), 100_000000);
            state
                .opt_in_native_collateral_with_tag(
                    &user,
                    xrp_ledger(),
                    valid_xrp_address(),
                    Some(7),
                )
                .unwrap();
        }
        let intent = prepare_or_reuse_native_xrp_absorb_intent_in_state(
            &mut state,
            &native_xrp_plan(77, 50_00000000, 1_000),
            1,
        )
        .unwrap();
        mark_native_xrp_absorb_burn_attempted_in_state(&mut state, 77, 2).unwrap();
        mark_native_xrp_absorb_burned_in_state(&mut state, 77, build_icusd_burn_proof(44, 77), 3)
            .unwrap();
        replace_state(state);

        assert!(crate::pool_token_balance_mutation_blocked(
            &[icusd_ledger()]
        ));
        assert!(!crate::pool_token_balance_mutation_blocked(&[
            ckusdc_ledger()
        ]));
        crate::ensure_pool_token_balance_mutation_allowed(&[ckusdc_ledger()]).unwrap();
        mutate_state(|state| {
            state.add_deposit_at(user_a(), ckusdc_ledger(), 20_000000, 6);
        });
        crate::deposits::prepare_withdrawal_after_ledger_check(
            user_b(),
            ckusdc_ledger(),
            30_000000,
            None,
            0,
        )
        .unwrap();

        let still_pending = read_state(|state| state.get_pending_native_xrp_absorb(77).unwrap());
        assert_eq!(still_pending.stables_consumed, intent.stables_consumed);
        assert_eq!(still_pending.icusd_to_burn_e8s, 50_00000000);
        assert_eq!(
            read_state(|state| state.total_stablecoin_balances[&icusd_ledger()]),
            200_00000000,
            "unrelated-token traffic leaves the burn-token denominator unchanged"
        );
        assert!(crate::pool_token_balance_mutation_blocked(
            &[icusd_ledger()]
        ));

        mutate_state(|state| {
            let mut accepted = state.get_pending_native_xrp_absorb(77).unwrap();
            accepted.backend_result = Some(xrp_backend_result(77, 50_00000000, 1_000));
            state
                .put_pending_native_xrp_absorb(accepted.clone())
                .unwrap();
            apply_native_xrp_absorb_success_in_state_at(state, &accepted, 4).unwrap();
        });
        assert_eq!(
            read_state(|state| state.deposits[&user_a()].stablecoin_balances[&icusd_ledger()]),
            75_00000000,
            "unrelated-token changes must not alter user A's exact burn share"
        );
        assert_eq!(
            read_state(|state| state.deposits[&user_b()].stablecoin_balances[&icusd_ledger()]),
            75_00000000,
            "unrelated-token changes must not alter user B's exact burn share"
        );

        // Legacy records with unknown dispatch state remain blocked for the
        // ledger in their durable consumed map even when their proof is absent.
        mutate_state(|state| {
            let mut legacy = prepare_or_reuse_native_xrp_absorb_intent_in_state(
                state,
                &native_xrp_plan(78, 10_00000000, 500),
                5,
            )
            .unwrap();
            legacy.burn_attempted = None;
            legacy.burn_proof = None;
            legacy.status = NativeXrpAbsorbIntentStatus::Burned;
            state.put_pending_native_xrp_absorb(legacy).unwrap();
        });
        assert!(crate::pool_token_balance_mutation_blocked(
            &[icusd_ledger()]
        ));
        assert!(!crate::pool_token_balance_mutation_blocked(&[
            ckusdc_ledger()
        ]));
        replace_state(StabilityPoolState::default());
    }

    #[test]
    fn xrp_absorb_preflights_allocates_burns_submits_and_records_pending_payouts() {
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        add_deposit_direct(&mut state, user_b(), icusd_ledger(), 50_00000000);
        state
            .opt_in_native_collateral_with_tag(
                &user_a(),
                xrp_ledger(),
                valid_xrp_address(),
                Some(7),
            )
            .unwrap();
        replace_state(state);

        let mut io = FakeNativeXrpAbsorbIo {
            preflight: Some(xrp_preflight(144, 60_00000000, 12_000_000)),
            submit_result: Some(XrpSpAbsorbResult {
                success: true,
                vault_id: 144,
                liquidated_debt_e8s: 60_00000000,
                collateral_received_drops: 12_000_000,
                payout_claims: vec![XrpSpPayoutClaim {
                    claimant: user_a(),
                    claim_id: 7001,
                    payout_address: valid_xrp_address(),
                    destination_tag: Some(7),
                    drops: 12_000_000,
                }],
                block_index: 1440,
                collateral_price_e8s: 50_00000000,
            }),
            ..Default::default()
        };

        let result = futures::executor::block_on(execute_native_xrp_absorb_with_io(
            &xrp_vault(144, 60_00000000),
            &mut io,
        ));

        assert!(result.success, "native XRP SP absorb should succeed");
        assert_eq!(
            io.events,
            vec![
                "preflight:144:6000000000",
                "minting_account",
                "burn:144:6000000000:123456789",
                "submit:144:6000000000:1"
            ],
            "preflight/allocation and intent prep must happen before burn, then backend submit",
        );
        assert_eq!(io.submitted_requests.len(), 1);
        assert_eq!(io.submitted_requests[0].allocations.len(), 1);
        assert_eq!(io.submitted_requests[0].allocations[0].claimant, user_a());
        assert_eq!(io.submitted_requests[0].allocations[0].drops, 12_000_000);
        assert_eq!(
            io.submitted_requests[0].allocations[0].destination_tag,
            Some(7)
        );
        assert_eq!(
            read_state(|s| s
                .deposits
                .get(&user_a())
                .and_then(|pos| pos.stablecoin_balances.get(&icusd_ledger()).copied())),
            Some(40_00000000),
            "opted-in depositor burns their pro-rata icUSD",
        );
        assert_eq!(
            read_state(|s| s
                .deposits
                .get(&user_b())
                .and_then(|pos| pos.stablecoin_balances.get(&icusd_ledger()).copied())),
            Some(50_00000000),
            "non-opted-in depositor must not burn for native XRP",
        );
        assert!(
            read_state(|s| s
                .deposits
                .get(&user_a())
                .and_then(|pos| pos.collateral_gains.get(&xrp_ledger()).copied())
                .unwrap_or(0))
                == 0,
            "native XRP must not enter ICRC collateral_gains",
        );
        let pending = read_state(|s| s.native_xrp_pending_payouts_for(&user_a()));
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].claim_id, 7001);
        assert_eq!(pending[0].vault_id, 144);
        assert_eq!(pending[0].drops, 12_000_000);
        assert_eq!(pending[0].destination_tag, Some(7));
        assert!(
            read_state(|s| s.native_xrp_pending_payouts_for(&user_b())).is_empty(),
            "non-opted-in depositor must not receive pending native XRP payouts",
        );

        // The absorb must leave a substantive audit event. Without this the
        // only trace in the Explorer is `LiquidationNotification`, which
        // carries a bare vault COUNT — no vault id, no amounts, no outcome —
        // so a real absorb was indistinguishable from one that did nothing.
        // `LiquidationExecuted` already exists and is already rendered richly
        // by the frontend, so emitting it needs no interface change.
        let executed = read_state(|s| {
            s.pool_events
                .as_ref()
                .map(|events| {
                    events
                        .iter()
                        .filter_map(|e| match &e.event_type {
                            PoolEventType::LiquidationExecuted {
                                vault_id,
                                stables_consumed_e8s,
                                collateral_gained,
                                collateral_type,
                                success,
                            } => Some((
                                *vault_id,
                                *stables_consumed_e8s,
                                *collateral_gained,
                                *collateral_type,
                                *success,
                            )),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        });
        assert_eq!(
            executed.len(),
            1,
            "a successful native-XRP absorb must emit exactly one LiquidationExecuted event"
        );
        let (vault_id, stables_e8s, drops, collateral, success) = executed[0];
        assert_eq!(vault_id, 144);
        assert_eq!(collateral, xrp_ledger());
        assert_eq!(drops, 12_000_000, "collateral_gained is the seized drops");
        assert_eq!(
            stables_e8s, 60_00000000,
            "stables_consumed_e8s is the full icUSD burn this fixture absorbs"
        );
        assert!(success);
    }

    #[test]
    fn native_xrp_unburned_clear_requires_known_not_attempted_state() {
        for (dispatch_state, should_clear) in
            [(Some(false), true), (Some(true), false), (None, false)]
        {
            let mut state = test_state();
            let mut intent = prepare_or_reuse_native_xrp_absorb_intent_in_state(
                &mut state,
                &native_xrp_plan(901, 100, 1_000),
                1,
            )
            .unwrap();
            intent.burn_attempted = dispatch_state;
            state.put_pending_native_xrp_absorb(intent).unwrap();

            assert_eq!(
                clear_unburned_native_xrp_absorb_intent_in_state(&mut state, 901),
                should_clear,
                "dispatch state {dispatch_state:?} must have the expected clear behavior",
            );
            assert_eq!(
                state.get_pending_native_xrp_absorb(901).is_some(),
                !should_clear,
                "only an intent known never to have dispatched may be erased",
            );
        }

        // A definitive first-attempt failure explicitly restores the dispatch
        // marker to false before using the same clear helper.
        let mut state = test_state();
        let expected = prepare_or_reuse_native_xrp_absorb_intent_in_state(
            &mut state,
            &native_xrp_plan(902, 100, 1_000),
            1,
        )
        .unwrap();
        mark_native_xrp_absorb_burn_attempted_in_state(&mut state, 902, 2).unwrap();
        assert!(cancel_first_definitive_native_burn_failure_in_state(
            &mut state, &expected
        ));
        assert!(state.get_pending_native_xrp_absorb(902).is_none());
    }

    #[test]
    fn ambiguous_native_xrp_burn_survives_retry_preflight_error_and_never_reburns() {
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        state
            .opt_in_native_collateral_with_tag(
                &user_a(),
                xrp_ledger(),
                valid_xrp_address(),
                Some(7),
            )
            .unwrap();
        replace_state(state);

        let vault = xrp_vault(903, 60_00000000);
        let mut ambiguous_io = FakeNativeXrpAbsorbIo {
            preflight: Some(xrp_preflight(903, 60_00000000, 12_000_000)),
            ambiguous_burn_failure: true,
            ..Default::default()
        };
        assert!(
            !futures::executor::block_on(execute_native_xrp_absorb_with_io(
                &vault,
                &mut ambiguous_io,
            ))
            .success
        );
        assert_eq!(ambiguous_io.burn_attempts, 1);
        let pending = read_state(|s| s.get_pending_native_xrp_absorb(903)).unwrap();
        assert_eq!(pending.burn_attempted, Some(true));
        assert!(pending.burn_proof.is_none());
        assert!(pending.backend_result.is_none());

        // Simulate the next retry failing before it can rebuild the plan.
        let mut failed_preflight = FakeNativeXrpAbsorbIo::default();
        assert!(
            !futures::executor::block_on(execute_native_xrp_absorb_with_io(
                &vault,
                &mut failed_preflight,
            ))
            .success
        );
        let held = read_state(|s| s.get_pending_native_xrp_absorb(903)).unwrap();
        assert_eq!(held.burn_attempted, Some(true));
        assert!(held.burn_proof.is_none());

        // A later successful preflight reuses the held intent, whose attempted
        // marker refuses another ledger burn without exact proof.
        let mut later_retry = FakeNativeXrpAbsorbIo {
            preflight: Some(xrp_preflight(903, 60_00000000, 12_000_000)),
            ..Default::default()
        };
        assert!(
            !futures::executor::block_on(execute_native_xrp_absorb_with_io(
                &vault,
                &mut later_retry,
            ))
            .success
        );
        assert_eq!(later_retry.burn_attempts, 0);
        assert!(later_retry
            .events
            .iter()
            .all(|event| !event.starts_with("burn:")));
        assert_eq!(
            read_state(|s| s.get_pending_native_xrp_absorb(903).unwrap().burn_attempted),
            Some(true),
        );
        replace_state(StabilityPoolState::default());
    }

    #[test]
    fn xrp_absorb_submit_failure_after_burn_retries_exact_request_to_success() {
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        state
            .opt_in_native_collateral_with_tag(
                &user_a(),
                xrp_ledger(),
                valid_xrp_address(),
                Some(7),
            )
            .unwrap();
        replace_state(state);

        let mut first_io = FakeNativeXrpAbsorbIo {
            preflight: Some(xrp_preflight(147, 60_00000000, 12_000_000)),
            submit_result: None,
            ..Default::default()
        };
        let first = futures::executor::block_on(execute_native_xrp_absorb_with_io(
            &xrp_vault(147, 60_00000000),
            &mut first_io,
        ));

        assert!(
            !first.success,
            "first attempt fails after burn/backend submit",
        );
        assert_eq!(
            first_io.events,
            vec![
                "preflight:147:6000000000",
                "minting_account",
                "burn:147:6000000000:123456789",
                "submit:147:6000000000:1",
            ],
        );
        let pending = read_state(|s| s.get_pending_native_xrp_absorb(147)).unwrap();
        assert_eq!(pending.status, NativeXrpAbsorbIntentStatus::BackendRejected);
        assert!(pending.burn_proof.is_some());
        assert!(pending.backend_result.is_none());
        assert!(read_state(|s| s.native_xrp_pending_payouts_for(&user_a())).is_empty());
        assert_eq!(
            read_state(|s| s
                .deposits
                .get(&user_a())
                .and_then(|pos| pos.stablecoin_balances.get(&icusd_ledger()).copied())),
            Some(100_00000000),
            "local balances are not deducted until backend result is accepted",
        );

        let mut retry_io = FakeNativeXrpAbsorbIo {
            submit_result: Some(xrp_backend_result(147, 60_00000000, 12_000_000)),
            ..Default::default()
        };
        let retry = futures::executor::block_on(execute_native_xrp_absorb_with_io(
            &xrp_vault(147, 60_00000000),
            &mut retry_io,
        ));

        assert!(
            retry.success,
            "retry completes from persisted burned intent"
        );
        assert_eq!(
            retry_io.events,
            vec!["submit:147:6000000000:1"],
            "burned retry must not preflight or burn again",
        );
        assert!(
            read_state(|s| s.get_pending_native_xrp_absorb(147)).is_none(),
            "successful local apply clears XRP absorb journal",
        );
        let payouts = read_state(|s| s.native_xrp_pending_payouts_for(&user_a()));
        assert_eq!(payouts.len(), 1);
        assert_eq!(payouts[0].claim_id, 7001);
        assert_eq!(payouts[0].drops, 12_000_000);
        assert_eq!(
            read_state(|s| s
                .deposits
                .get(&user_a())
                .and_then(|pos| pos.stablecoin_balances.get(&icusd_ledger()).copied())),
            Some(40_00000000),
        );
    }

    #[test]
    fn xrp_absorb_invalid_backend_result_keeps_burned_intent_retryable() {
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        state
            .opt_in_native_collateral_with_tag(
                &user_a(),
                xrp_ledger(),
                valid_xrp_address(),
                Some(7),
            )
            .unwrap();
        replace_state(state);

        let mut invalid_io = FakeNativeXrpAbsorbIo {
            preflight: Some(xrp_preflight(150, 60_00000000, 12_000_000)),
            submit_result: Some(xrp_backend_result(150, 60_00000000, 12_000_001)),
            ..Default::default()
        };
        let invalid = futures::executor::block_on(execute_native_xrp_absorb_with_io(
            &xrp_vault(150, 60_00000000),
            &mut invalid_io,
        ));

        assert!(!invalid.success);
        let pending = read_state(|s| s.get_pending_native_xrp_absorb(150)).unwrap();
        assert_eq!(pending.status, NativeXrpAbsorbIntentStatus::BackendRejected);
        assert!(pending.burn_proof.is_some());
        assert!(
            pending.backend_result.is_none(),
            "invalid backend result must not be persisted as accepted",
        );

        let mut retry_io = FakeNativeXrpAbsorbIo {
            submit_result: Some(xrp_backend_result(150, 60_00000000, 12_000_000)),
            ..Default::default()
        };
        let retry = futures::executor::block_on(execute_native_xrp_absorb_with_io(
            &xrp_vault(150, 60_00000000),
            &mut retry_io,
        ));

        assert!(retry.success);
        assert_eq!(
            retry_io.events,
            vec!["submit:150:6000000000:1"],
            "retry should reuse the stored burn proof and request without another burn",
        );
        assert!(read_state(|s| s.get_pending_native_xrp_absorb(150)).is_none());
        assert_eq!(
            read_state(|s| s.native_xrp_pending_payouts_for(&user_a()).len()),
            1,
        );
    }

    #[test]
    fn xrp_absorb_backend_accepted_intent_retries_local_apply_without_backend_call() {
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        state
            .opt_in_native_collateral_with_tag(
                &user_a(),
                xrp_ledger(),
                valid_xrp_address(),
                Some(7),
            )
            .unwrap();
        let mut stables_consumed = BTreeMap::new();
        stables_consumed.insert(icusd_ledger(), 60_00000000);
        let plan = NativeXrpAbsorbPlan {
            vault_id: 148,
            collateral_type: xrp_ledger(),
            icusd_ledger: icusd_ledger(),
            icusd_minting_account: minting_account(),
            icusd_to_burn_e8s: 60_00000000,
            stables_consumed,
            collateral_received_drops: 12_000_000,
            collateral_price_e8s: 50_00000000,
            allocations: vec![XrpSpPayoutAllocation {
                claimant: user_a(),
                payout_address: valid_xrp_address(),
                destination_tag: Some(7),
                drops: 12_000_000,
            }],
        };
        prepare_or_reuse_native_xrp_absorb_intent_in_state(&mut state, &plan, 111).unwrap();
        mark_native_xrp_absorb_burned_in_state(
            &mut state,
            148,
            build_icusd_burn_proof(44, 148),
            112,
        )
        .unwrap();
        let intent = mark_native_xrp_absorb_backend_result_in_state(
            &mut state,
            148,
            xrp_backend_result(148, 60_00000000, 12_000_000),
            113,
        )
        .unwrap();
        assert_eq!(intent.status, NativeXrpAbsorbIntentStatus::BackendAccepted);
        replace_state(state);

        let mut io = FakeNativeXrpAbsorbIo::default();
        let retry = futures::executor::block_on(execute_native_xrp_absorb_with_io(
            &xrp_vault(148, 60_00000000),
            &mut io,
        ));

        assert!(retry.success);
        assert!(
            io.events.is_empty(),
            "backend-accepted retry must only apply local state",
        );
        assert!(read_state(|s| s.get_pending_native_xrp_absorb(148)).is_none());
        assert_eq!(
            read_state(|s| s.native_xrp_pending_payouts_for(&user_a()).len()),
            1,
        );
    }

    #[test]
    fn prepared_xrp_absorb_rejects_conflicting_retry_allocations() {
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 60_00000000);
        add_deposit_direct(&mut state, user_b(), icusd_ledger(), 60_00000000);
        state
            .opt_in_native_collateral_with_tag(
                &user_a(),
                xrp_ledger(),
                valid_xrp_address(),
                Some(7),
            )
            .unwrap();
        let mut stables_consumed = BTreeMap::new();
        stables_consumed.insert(icusd_ledger(), 60_00000000);
        let plan = NativeXrpAbsorbPlan {
            vault_id: 149,
            collateral_type: xrp_ledger(),
            icusd_ledger: icusd_ledger(),
            icusd_minting_account: minting_account(),
            icusd_to_burn_e8s: 60_00000000,
            stables_consumed,
            collateral_received_drops: 12_000_000,
            collateral_price_e8s: 50_00000000,
            allocations: vec![XrpSpPayoutAllocation {
                claimant: user_a(),
                payout_address: valid_xrp_address(),
                destination_tag: Some(7),
                drops: 12_000_000,
            }],
        };
        prepare_or_reuse_native_xrp_absorb_intent_in_state(&mut state, &plan, 111).unwrap();
        state
            .opt_in_native_collateral_with_tag(&user_b(), xrp_ledger(), valid_xrp_address(), None)
            .unwrap();
        replace_state(state);

        let mut io = FakeNativeXrpAbsorbIo {
            preflight: Some(xrp_preflight(149, 60_00000000, 12_000_000)),
            ..Default::default()
        };
        let retry = futures::executor::block_on(execute_native_xrp_absorb_with_io(
            &xrp_vault(149, 60_00000000),
            &mut io,
        ));

        assert!(
            !retry.success,
            "prepared retry must reject changed allocations",
        );
        assert_eq!(io.events, vec!["preflight:149:6000000000"]);
        let pending = read_state(|s| s.get_pending_native_xrp_absorb(149)).unwrap();
        assert_eq!(pending.status, NativeXrpAbsorbIntentStatus::Prepared);
        assert!(pending.burn_proof.is_none());
    }

    #[test]
    fn pending_native_xrp_absorb_blocks_new_chain_absorb_start() {
        let mut state = test_state();
        let plan = native_xrp_plan(150, 60_00000000, 12_000_000);
        prepare_or_reuse_native_xrp_absorb_intent_in_state(&mut state, &plan, 111).unwrap();

        assert!(matches!(
            ensure_no_other_pending_pool_absorb_for_chain(&state, 77),
            Err(StabilityPoolError::SystemBusy)
        ));
    }

    #[test]
    fn pending_chain_absorb_blocks_new_native_xrp_absorb_start() {
        let mut state = test_state();
        state
            .register_chain_collateral(1030, "CFX".to_string(), 18)
            .unwrap();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        state
            .opt_in_cfx(&user_a(), chain_collateral_sentinel(1030))
            .unwrap();
        state
            .opt_in_native_collateral_with_tag(
                &user_a(),
                xrp_ledger(),
                valid_xrp_address(),
                Some(7),
            )
            .unwrap();
        let chain_plan =
            prepare_chain_absorb_plan_in_state(&state, &chain_vault(100_00000000, true)).unwrap();
        prepare_or_reuse_chain_absorb_intent_in_state(
            &mut state,
            &chain_plan,
            minting_account(),
            111,
        )
        .unwrap();
        replace_state(state);

        let mut io = FakeNativeXrpAbsorbIo {
            preflight: Some(xrp_preflight(151, 60_00000000, 12_000_000)),
            ..Default::default()
        };
        let result = futures::executor::block_on(execute_native_xrp_absorb_with_io(
            &xrp_vault(151, 60_00000000),
            &mut io,
        ));

        assert!(!result.success);
        assert!(
            io.events.is_empty(),
            "native XRP start must reject before preflight when chain absorb is pending",
        );
    }

    #[test]
    fn xrp_absorb_aborts_before_burn_when_preflight_yields_no_allocations() {
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 10_00000000);
        state
            .opt_in_native_collateral_with_tag(&user_a(), xrp_ledger(), valid_xrp_address(), None)
            .unwrap();
        replace_state(state);
        let mut io = FakeNativeXrpAbsorbIo {
            preflight: Some(xrp_preflight(145, 10_00000000, 0)),
            ..Default::default()
        };

        let result = futures::executor::block_on(execute_native_xrp_absorb_with_io(
            &xrp_vault(145, 10_00000000),
            &mut io,
        ));

        assert!(!result.success);
        // Giving up after reserving but before burning must hand the backend
        // reservation back, or the vault stays blocked for every liquidation
        // path (including manual) until the 15-minute TTL expires.
        assert_eq!(
            io.events,
            vec!["preflight:145:1000000000", "release:145:1000000000"]
        );
        assert_eq!(
            read_state(|s| s
                .deposits
                .get(&user_a())
                .and_then(|pos| pos.stablecoin_balances.get(&icusd_ledger()).copied())),
            Some(10_00000000),
            "empty allocation rejection must not burn or mutate pool balances",
        );
    }

    #[test]
    fn xrp_absorb_aborts_before_burn_when_allocation_fanout_exceeds_500() {
        let mut state = test_state();
        for i in 0..501u16 {
            let principal = Principal::from_slice(&i.to_be_bytes());
            add_deposit_direct(&mut state, principal, icusd_ledger(), 1_00000000);
            state
                .opt_in_native_collateral_with_tag(
                    &principal,
                    xrp_ledger(),
                    valid_xrp_address(),
                    None,
                )
                .unwrap();
        }
        replace_state(state);
        let mut io = FakeNativeXrpAbsorbIo {
            preflight: Some(xrp_preflight(146, 501_00000000, 501)),
            ..Default::default()
        };

        let result = futures::executor::block_on(execute_native_xrp_absorb_with_io(
            &xrp_vault(146, 501_00000000),
            &mut io,
        ));

        assert!(!result.success);
        // Same contract as the empty-allocation abort: reserved, gave up
        // before burning, so the reservation goes back rather than blocking
        // the vault for the full TTL.
        assert_eq!(
            io.events,
            vec!["preflight:146:50100000000", "release:146:50100000000"]
        );
        assert_eq!(
            read_state(|s| s.total_stablecoin_balances.get(&icusd_ledger()).copied()),
            Some(501_00000000),
            "over-500 fanout rejection must not burn or mutate pool balances",
        );
    }

    #[test]
    fn chain_writedown_memo_matches_backend_liq_004_shape() {
        let vault_id: u64 = 0x0102_0304_0506_0708;
        let memo = encode_chain_writedown_memo(vault_id);

        assert_eq!(&memo[..13], b"RUMI-LIQ-004:");
        assert_eq!(&memo[13..], &vault_id.to_be_bytes());
        assert_eq!(
            rumi_protocol_backend::icrc3_proof::decode_writedown_memo(&memo),
            Ok(vault_id),
            "SP burn memo must be accepted by backend proof verifier",
        );
    }

    #[test]
    fn icusd_burn_request_targets_minting_account_and_builds_proof() {
        let minting_account = Account {
            owner: principal(90),
            subaccount: None,
        };
        let amount_e8s = 12_345_00000000;
        let vault_id = 77;
        let created_at_time = 123_456_789;
        let block_index = 999;

        let transfer =
            build_icusd_burn_transfer_arg(minting_account, amount_e8s, vault_id, created_at_time);

        assert_eq!(transfer.to, minting_account);
        assert_eq!(transfer.amount, Nat::from(amount_e8s));
        assert_eq!(
            transfer.fee, None,
            "ICRC-1 burns to the minting account have zero fee"
        );
        assert_eq!(transfer.from_subaccount, None);
        assert_eq!(transfer.created_at_time, Some(created_at_time));
        assert_eq!(
            transfer.memo,
            Some(Memo::from(encode_chain_writedown_memo(vault_id))),
        );

        let proof = build_icusd_burn_proof(block_index, vault_id);
        assert_eq!(proof.block_index, block_index);
        assert_eq!(
            proof.ledger_kind,
            rumi_protocol_backend::icrc3_proof::SpProofLedger::IcusdBurn
        );
        assert_eq!(proof.vault_id_memo, vault_id);
    }

    #[test]
    fn registered_chain_ids_decode_from_registered_sentinels() {
        let mut state = test_state();
        state
            .register_chain_collateral(1030, "CFX".to_string(), 18)
            .unwrap();

        assert_eq!(
            registered_chain_ids_from_sentinels(&state),
            vec![rumi_protocol_backend::chains::config::ChainId(1030)],
        );
    }

    #[test]
    fn chain_absorb_preflight_requires_escalation_and_icusd_coverage() {
        let mut state = test_state();
        state
            .register_chain_collateral(1030, "CFX".to_string(), 18)
            .unwrap();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 40_00000000);
        add_deposit_direct(&mut state, user_a(), ckusdc_ledger(), 5_000_000_000);
        add_deposit_direct(&mut state, user_b(), icusd_ledger(), 100_00000000);

        let not_escalated =
            prepare_chain_absorb_plan_in_state(&state, &chain_vault(70_00000000, false))
                .unwrap_err();
        assert!(matches!(
            not_escalated,
            StabilityPoolError::LiquidationFailed { .. }
        ));

        let no_opt_in = prepare_chain_absorb_plan_in_state(&state, &chain_vault(70_00000000, true))
            .unwrap_err();
        assert!(matches!(
            no_opt_in,
            StabilityPoolError::InsufficientPoolBalance
        ));

        state
            .opt_in_cfx(&user_a(), chain_collateral_sentinel(1030))
            .unwrap();
        let undercovered =
            prepare_chain_absorb_plan_in_state(&state, &chain_vault(70_00000000, true))
                .unwrap_err();
        assert!(
            matches!(undercovered, StabilityPoolError::InsufficientPoolBalance),
            "ckUSDC must not count toward chain absorb coverage",
        );

        state
            .opt_in_cfx(&user_b(), chain_collateral_sentinel(1030))
            .unwrap();
        let plan = prepare_chain_absorb_plan_in_state(&state, &chain_vault(70_00000000, true))
            .expect("covered by opted-in icUSD");
        assert_eq!(plan.icusd_to_burn_e8s, 70_00000000);
        assert_eq!(
            plan.stables_consumed.get(&icusd_ledger()).copied(),
            Some(70_00000000)
        );
        assert_eq!(plan.stables_consumed.len(), 1);
    }

    #[test]
    fn chain_absorb_intent_reuses_timestamp_and_rejects_conflicting_recompute() {
        let mut state = test_state();
        state
            .register_chain_collateral(1030, "CFX".to_string(), 18)
            .unwrap();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        state
            .opt_in_cfx(&user_a(), chain_collateral_sentinel(1030))
            .unwrap();

        let plan = prepare_chain_absorb_plan_in_state(&state, &chain_vault(100_00000000, true))
            .expect("covered");
        let intent = prepare_or_reuse_chain_absorb_intent_in_state(
            &mut state,
            &plan,
            minting_account(),
            123,
        )
        .expect("intent is recorded");

        assert_eq!(intent.burn_created_at_time_ns, 123);
        assert!(state.has_pending_chain_absorbs());
        assert_eq!(state.pending_chain_absorb_count(), 1);

        let reused = prepare_or_reuse_chain_absorb_intent_in_state(
            &mut state,
            &plan,
            minting_account(),
            999,
        )
        .expect("same plan reuses existing intent");
        assert_eq!(
            reused.burn_created_at_time_ns, 123,
            "retry must not recompute ledger dedupe timestamp",
        );

        let mut conflicting = plan.clone();
        conflicting.icusd_to_burn_e8s -= 1;
        let err = prepare_or_reuse_chain_absorb_intent_in_state(
            &mut state,
            &conflicting,
            minting_account(),
            1_000,
        )
        .unwrap_err();
        assert!(matches!(err, StabilityPoolError::LiquidationFailed { .. }));
    }

    #[test]
    fn chain_absorb_intent_records_single_burn_proof() {
        let mut state = test_state();
        state
            .register_chain_collateral(1030, "CFX".to_string(), 18)
            .unwrap();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        state
            .opt_in_cfx(&user_a(), chain_collateral_sentinel(1030))
            .unwrap();
        let plan = prepare_chain_absorb_plan_in_state(&state, &chain_vault(100_00000000, true))
            .expect("covered");
        prepare_or_reuse_chain_absorb_intent_in_state(&mut state, &plan, minting_account(), 123)
            .expect("intent is recorded");

        let proof = build_icusd_burn_proof(44, 77);
        let burned = mark_chain_absorb_burned_in_state(&mut state, 77, proof.clone(), 456)
            .expect("proof recorded");
        assert_eq!(burned.status, ChainSpAbsorbIntentStatus::Burned);
        assert_eq!(burned.burn_proof, Some(proof.clone()));

        mark_chain_absorb_burned_in_state(&mut state, 77, proof, 789)
            .expect("same proof is idempotent");
        let conflict =
            mark_chain_absorb_burned_in_state(&mut state, 77, build_icusd_burn_proof(45, 77), 790)
                .unwrap_err();
        assert!(matches!(
            conflict,
            StabilityPoolError::LiquidationFailed { .. }
        ));
    }

    #[test]
    fn unburned_prepared_chain_absorb_intent_can_be_cleared_after_failure() {
        let mut state = test_state();
        state
            .register_chain_collateral(1030, "CFX".to_string(), 18)
            .unwrap();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        state
            .opt_in_cfx(&user_a(), chain_collateral_sentinel(1030))
            .unwrap();
        let plan = prepare_chain_absorb_plan_in_state(&state, &chain_vault(100_00000000, true))
            .expect("covered");
        prepare_or_reuse_chain_absorb_intent_in_state(&mut state, &plan, minting_account(), 123)
            .expect("intent is recorded");

        assert!(state.has_pending_chain_absorbs());
        assert!(clear_unburned_chain_absorb_intent_in_state(&mut state, 77));
        assert!(
            !state.has_pending_chain_absorbs(),
            "a failed pre-burn attempt must not wedge pool balance operations"
        );
    }

    #[test]
    fn burned_chain_absorb_intent_is_not_cleared_as_unburned_failure() {
        let mut state = test_state();
        state
            .register_chain_collateral(1030, "CFX".to_string(), 18)
            .unwrap();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        state
            .opt_in_cfx(&user_a(), chain_collateral_sentinel(1030))
            .unwrap();
        let plan = prepare_chain_absorb_plan_in_state(&state, &chain_vault(100_00000000, true))
            .expect("covered");
        prepare_or_reuse_chain_absorb_intent_in_state(&mut state, &plan, minting_account(), 123)
            .expect("intent is recorded");
        mark_chain_absorb_burned_in_state(&mut state, 77, build_icusd_burn_proof(44, 77), 456)
            .expect("proof recorded");

        assert!(!clear_unburned_chain_absorb_intent_in_state(&mut state, 77));
        assert!(
            state.has_pending_chain_absorbs(),
            "burned intents must remain for backend finalization/retry"
        );
    }

    #[test]
    fn burned_chain_absorb_intent_replays_backend_without_discovery() {
        let mut state = test_state();
        state
            .register_chain_collateral(1030, "CFX".to_string(), 18)
            .unwrap();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        state
            .opt_in_cfx(&user_a(), chain_collateral_sentinel(1030))
            .unwrap();
        let plan = prepare_chain_absorb_plan_in_state(&state, &chain_vault(100_00000000, true))
            .expect("covered");
        prepare_or_reuse_chain_absorb_intent_in_state(&mut state, &plan, minting_account(), 123)
            .expect("intent is recorded");
        let proof = build_icusd_burn_proof(44, 77);
        let burned = mark_chain_absorb_burned_in_state(&mut state, 77, proof.clone(), 456)
            .expect("proof recorded");

        let (replay_plan, replay_proof) =
            burned_chain_absorb_replay_plan(&burned).expect("burned intent replays by proof");
        assert_eq!(replay_plan, plan);
        assert_eq!(replay_proof, proof);
        assert!(
            burned.backend_result.is_none(),
            "this covers backend-accepted/lost-reply state before local result journaling",
        );
    }

    #[test]
    fn other_pending_chain_absorb_blocks_new_burn_plan() {
        let mut state = test_state();
        state
            .register_chain_collateral(1030, "CFX".to_string(), 18)
            .unwrap();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        state
            .opt_in_cfx(&user_a(), chain_collateral_sentinel(1030))
            .unwrap();

        let plan_a = prepare_chain_absorb_plan_in_state(
            &state,
            &chain_vault_with_id(77, 100_00000000, true),
        )
        .expect("vault A covered");
        prepare_or_reuse_chain_absorb_intent_in_state(&mut state, &plan_a, minting_account(), 123)
            .expect("intent A is recorded");
        mark_chain_absorb_burned_in_state(&mut state, 77, build_icusd_burn_proof(44, 77), 456)
            .expect("intent A burn proof recorded");

        assert!(
            ensure_no_other_pending_chain_absorb(&state, 77).is_ok(),
            "the original vault remains retryable",
        );
        assert!(
            matches!(
                ensure_no_other_pending_chain_absorb(&state, 78),
                Err(StabilityPoolError::SystemBusy)
            ),
            "a burned pending intent reserves its icUSD until local finalization",
        );
    }

    #[test]
    fn auto_absorb_selection_prefers_pending_intent_before_new_candidate() {
        let mut state = test_state();
        state
            .register_chain_collateral(1030, "CFX".to_string(), 18)
            .unwrap();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        state
            .opt_in_cfx(&user_a(), chain_collateral_sentinel(1030))
            .unwrap();

        let plan = prepare_chain_absorb_plan_in_state(
            &state,
            &chain_vault_with_id(77, 100_00000000, true),
        )
        .expect("covered");
        prepare_or_reuse_chain_absorb_intent_in_state(&mut state, &plan, minting_account(), 123)
            .expect("pending intent");

        let candidates = vec![ChainSpAbsorbCandidate {
            vault: chain_vault_with_id(88, 100_00000000, true),
            icusd_to_burn_e8s: 100_00000000,
            pending_status: None,
        }];

        assert_eq!(
            select_chain_absorb_auto_vault(&state, &candidates),
            Some(77)
        );
    }

    #[test]
    fn auto_absorb_selection_uses_first_candidate_when_no_pending_intent() {
        let state = test_state();
        let candidates = vec![
            ChainSpAbsorbCandidate {
                vault: chain_vault_with_id(88, 100_00000000, true),
                icusd_to_burn_e8s: 100_00000000,
                pending_status: None,
            },
            ChainSpAbsorbCandidate {
                vault: chain_vault_with_id(99, 100_00000000, true),
                icusd_to_burn_e8s: 100_00000000,
                pending_status: None,
            },
        ];

        assert_eq!(
            select_chain_absorb_auto_vault(&state, &candidates),
            Some(88)
        );
    }

    #[test]
    fn chain_absorb_backend_result_intent_can_finalize_without_discovery() {
        let mut state = test_state();
        state
            .register_chain_collateral(1030, "CFX".to_string(), 18)
            .unwrap();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        state
            .opt_in_cfx(&user_a(), chain_collateral_sentinel(1030))
            .unwrap();
        let plan = prepare_chain_absorb_plan_in_state(&state, &chain_vault(100_00000000, true))
            .expect("covered");
        prepare_or_reuse_chain_absorb_intent_in_state(&mut state, &plan, minting_account(), 123)
            .expect("intent is recorded");
        let backend_result = backend_chain_result();
        let intent = mark_chain_absorb_backend_result_in_state(&mut state, 77, backend_result, 456)
            .expect("backend result is stored");

        let resumed_plan = chain_absorb_plan_from_intent(&intent);
        let absorbed = apply_chain_absorb_success_in_state_at(
            &mut state,
            &resumed_plan,
            intent.backend_result.expect("stored"),
            789,
        )
        .expect("stored backend result finalizes locally");

        assert_eq!(absorbed.icusd_burned_e8s, 100_00000000);
        assert!(
            state.get_pending_chain_absorb(77).is_none(),
            "local finalization clears pending journal entry",
        );
        assert!(state.completed_chain_absorb(77).is_some());
    }

    #[test]
    fn chain_absorb_rejects_partial_backend_result_without_deducting_pool() {
        let mut state = test_state();
        state
            .register_chain_collateral(1030, "CFX".to_string(), 18)
            .unwrap();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        state
            .opt_in_cfx(&user_a(), chain_collateral_sentinel(1030))
            .unwrap();

        let plan = prepare_chain_absorb_plan_in_state(&state, &chain_vault(100_00000000, true))
            .expect("covered");
        let mut partial = backend_chain_result();
        partial.liquidated_debt_e8s = 80_00000000;

        let err =
            apply_chain_absorb_success_in_state_at(&mut state, &plan, partial, 123).unwrap_err();

        assert!(
            matches!(err, StabilityPoolError::LiquidationFailed { .. }),
            "partial backend result must be rejected before pool accounting"
        );
        assert_eq!(
            state
                .total_stablecoin_balances
                .get(&icusd_ledger())
                .copied(),
            Some(100_00000000),
            "pool icUSD balance is unchanged",
        );
        assert!(
            state
                .deposits
                .get(&user_a())
                .unwrap()
                .cfx_claims
                .clone()
                .unwrap_or_default()
                .is_empty(),
            "no CFX claim is credited on a partial backend result",
        );
        assert!(state.completed_chain_absorb(77).is_none());
    }

    #[test]
    fn chain_absorb_success_credits_cfx_claims_and_deducts_burned_icusd() {
        let mut state = test_state();
        state
            .register_chain_collateral(1030, "CFX".to_string(), 18)
            .unwrap();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 40_00000000);
        add_deposit_direct(&mut state, user_b(), icusd_ledger(), 60_00000000);
        state
            .opt_in_cfx(&user_a(), chain_collateral_sentinel(1030))
            .unwrap();
        state
            .opt_in_cfx(&user_b(), chain_collateral_sentinel(1030))
            .unwrap();

        let plan = prepare_chain_absorb_plan_in_state(&state, &chain_vault(100_00000000, true))
            .expect("covered");
        let result = backend_chain_result();

        let absorbed =
            apply_chain_absorb_success_in_state_at(&mut state, &plan, result.clone(), 123)
                .expect("success finalizes");

        assert_eq!(absorbed.icusd_burned_e8s, 100_00000000);
        assert_eq!(
            state
                .total_stablecoin_balances
                .get(&icusd_ledger())
                .copied(),
            Some(0),
            "SP aggregate tracks the burned icUSD",
        );
        let claim_a = state
            .deposits
            .get(&user_a())
            .unwrap()
            .cfx_claims
            .as_ref()
            .unwrap()
            .get(&chain_collateral_sentinel(1030))
            .copied()
            .unwrap_or(0);
        let claim_b = state
            .deposits
            .get(&user_b())
            .unwrap()
            .cfx_claims
            .as_ref()
            .unwrap()
            .get(&chain_collateral_sentinel(1030))
            .copied()
            .unwrap_or(0);
        assert_eq!(claim_a, 4_000_000_000_000_000_000u128);
        assert_eq!(claim_b, 6_000_000_000_000_000_000u128);

        let repeated = apply_chain_absorb_success_in_state_at(&mut state, &plan, result, 124)
            .expect("same result is idempotent");
        assert_eq!(repeated, absorbed);
        assert_eq!(
            state
                .total_stablecoin_balances
                .get(&icusd_ledger())
                .copied(),
            Some(0),
            "replay must not double-deduct icUSD",
        );
        assert_eq!(
            state
                .deposits
                .get(&user_a())
                .unwrap()
                .cfx_claims
                .as_ref()
                .unwrap()
                .get(&chain_collateral_sentinel(1030))
                .copied()
                .unwrap_or(0),
            claim_a,
            "replay must not double-credit user A CFX",
        );
        assert_eq!(
            state
                .deposits
                .get(&user_b())
                .unwrap()
                .cfx_claims
                .as_ref()
                .unwrap()
                .get(&chain_collateral_sentinel(1030))
                .copied()
                .unwrap_or(0),
            claim_b,
            "replay must not double-credit user B CFX",
        );
        assert_eq!(state.completed_chain_absorbs(10).len(), 1);
    }

    #[test]
    fn cfx_claim_payout_deducts_from_user_and_claim_source() {
        let mut state = test_state();
        let sentinel = state
            .register_chain_collateral(1030, "CFX".to_string(), 18)
            .unwrap();
        let mut pos = DepositPosition::new(0);
        pos.cfx_claims
            .get_or_insert_with(BTreeMap::new)
            .insert(sentinel, 12);
        state.deposits.insert(user_a(), pos);
        state.record_chain_claim_source(sentinel, 77, 10);

        let invalid = prepare_cfx_claim_payout_in_state(
            &mut state,
            user_a(),
            sentinel,
            "not-evm".to_string(),
            |_| false,
        )
        .unwrap_err();
        assert!(matches!(
            invalid,
            StabilityPoolError::LiquidationFailed { .. }
        ));
        assert_eq!(
            state.deposits[&user_a()].cfx_claims.as_ref().unwrap()[&sentinel],
            12,
            "destination validation happens before mutation",
        );
        assert_eq!(
            state.chain_claim_sources.as_ref().unwrap()[&sentinel][0].remaining_native,
            10
        );

        let plan = prepare_cfx_claim_payout_in_state(
            &mut state,
            user_a(),
            sentinel,
            "0x000000000000000000000000000000000000c0de".to_string(),
            rumi_protocol_backend::chains::evm::tecdsa::is_valid_evm_address,
        )
        .expect("claim plan")
        .expect("nonzero claim");
        assert_eq!(plan.claim_id, 77);
        assert_eq!(plan.amount_wei, 10);
        assert_eq!(
            state.deposits[&user_a()].cfx_claims.as_ref().unwrap()[&sentinel],
            2,
            "only the covered source amount is deducted",
        );
        assert!(
            state
                .chain_claim_sources
                .as_ref()
                .unwrap()
                .get(&sentinel)
                .is_none(),
            "depleted source is pruned before await",
        );

        rollback_cfx_claim_payout_in_state(&mut state, &plan);
        assert_eq!(
            state.deposits[&user_a()].cfx_claims.as_ref().unwrap()[&sentinel],
            12,
            "rollback restores user claim",
        );
        assert_eq!(
            state.chain_claim_sources.as_ref().unwrap()[&sentinel][0].remaining_native,
            10,
            "rollback restores backend claim source",
        );
    }

    #[test]
    fn failed_cfx_claim_payout_recredit_restores_user_claim_and_source_once() {
        let mut state = test_state();
        let sentinel = state
            .register_chain_collateral(1030, "CFX".to_string(), 18)
            .unwrap();
        let mut pos = DepositPosition::new(0);
        pos.cfx_claims
            .get_or_insert_with(BTreeMap::new)
            .insert(sentinel, 12);
        state.deposits.insert(user_a(), pos);
        state.record_chain_claim_source(sentinel, 77, 10);
        let plan = prepare_cfx_claim_payout_in_state(
            &mut state,
            user_a(),
            sentinel,
            "0x000000000000000000000000000000000000c0de".to_string(),
            rumi_protocol_backend::chains::evm::tecdsa::is_valid_evm_address,
        )
        .expect("claim plan")
        .expect("nonzero claim");

        let recovered = recredit_failed_cfx_claim_payout_in_state(
            &mut state,
            CfxClaimPayoutRecovery {
                chain_sentinel: sentinel,
                op_id: 42,
                claim_id: plan.claim_id,
                claimant: user_a(),
                amount_wei: plan.amount_wei,
                reason: "tx reverted".to_string(),
                failed_at_ns: 123,
            },
        )
        .expect("first recovery succeeds");

        assert!(recovered, "first recovery mutates state");
        assert_eq!(
            state.deposits[&user_a()].cfx_claims.as_ref().unwrap()[&sentinel],
            12,
            "failed payout recredits the user claim",
        );
        assert_eq!(
            state.chain_claim_sources.as_ref().unwrap()[&sentinel][0].remaining_native,
            10,
            "failed payout restores backend claim source",
        );

        let replay = recredit_failed_cfx_claim_payout_in_state(
            &mut state,
            CfxClaimPayoutRecovery {
                chain_sentinel: sentinel,
                op_id: 42,
                claim_id: plan.claim_id,
                claimant: user_a(),
                amount_wei: plan.amount_wei,
                reason: "same failed op replay".to_string(),
                failed_at_ns: 124,
            },
        )
        .expect("same recovery replay succeeds without mutation");

        assert!(!replay, "replay is recognized as already recovered");
        assert_eq!(
            state.deposits[&user_a()].cfx_claims.as_ref().unwrap()[&sentinel],
            12,
            "replay must not double-credit the user claim",
        );
        assert_eq!(
            state.chain_claim_sources.as_ref().unwrap()[&sentinel][0].remaining_native,
            10,
            "replay must not double-restore backend claim source",
        );
    }

    #[test]
    fn failed_cfx_claim_payout_replay_is_idempotent() {
        let mut state = test_state();
        let sentinel = state
            .register_chain_collateral(1030, "CFX".to_string(), 18)
            .unwrap();
        let mut pos = DepositPosition::new(0);
        pos.cfx_claims
            .get_or_insert_with(BTreeMap::new)
            .insert(sentinel, 10);
        state.deposits.insert(user_a(), pos);
        state.record_chain_claim_source(sentinel, 77, 10);
        let plan = prepare_cfx_claim_payout_in_state(
            &mut state,
            user_a(),
            sentinel,
            "0x000000000000000000000000000000000000c0de".to_string(),
            rumi_protocol_backend::chains::evm::tecdsa::is_valid_evm_address,
        )
        .expect("claim plan")
        .expect("nonzero claim");
        let recovery = CfxClaimPayoutRecovery {
            chain_sentinel: sentinel,
            op_id: 43,
            claim_id: plan.claim_id,
            claimant: user_a(),
            amount_wei: plan.amount_wei,
            reason: "tx reverted".to_string(),
            failed_at_ns: 123,
        };

        assert!(
            recredit_failed_cfx_claim_payout_in_state(&mut state, recovery.clone())
                .expect("first recovery succeeds")
        );
        assert!(
            !recredit_failed_cfx_claim_payout_in_state(&mut state, recovery)
                .expect("exact replay succeeds")
        );
        assert_eq!(
            state
                .completed_cfx_claim_payout_recoveries
                .as_ref()
                .map(|m| m.len()),
            Some(1),
            "journal stores one durable recovery record",
        );
    }

    #[test]
    fn evicted_failed_cfx_claim_payout_replay_remains_idempotent() {
        let mut state = test_state();
        let sentinel = state
            .register_chain_collateral(1030, "CFX".to_string(), 18)
            .unwrap();

        for op_id in 0..=(crate::state::MAX_COMPLETED_CFX_CLAIM_PAYOUT_RECOVERIES as u64) {
            assert!(
                recredit_failed_cfx_claim_payout_in_state(
                    &mut state,
                    CfxClaimPayoutRecovery {
                        chain_sentinel: sentinel,
                        op_id,
                        claim_id: op_id,
                        claimant: user_a(),
                        amount_wei: 1,
                        reason: "tx reverted".to_string(),
                        failed_at_ns: op_id,
                    },
                )
                .expect("recovery succeeds"),
                "first recovery for op {op_id} mutates state",
            );
        }

        assert!(
            !state
                .completed_cfx_claim_payout_recoveries
                .as_ref()
                .unwrap()
                .contains_key(&CfxClaimPayoutRecoveryKey {
                    chain_sentinel: sentinel,
                    op_id: 0,
                }),
            "oldest recovery record should be evicted at the bound",
        );
        let before_claims = state.deposits[&user_a()].cfx_claims.clone();
        let before_sources = state.chain_claim_sources.clone();

        let replay = recredit_failed_cfx_claim_payout_in_state(
            &mut state,
            CfxClaimPayoutRecovery {
                chain_sentinel: sentinel,
                op_id: 0,
                claim_id: 0,
                claimant: user_a(),
                amount_wei: 1,
                reason: "old op replay after eviction".to_string(),
                failed_at_ns: 999_999,
            },
        )
        .expect("evicted replay succeeds without mutation");

        assert!(!replay, "evicted replay is treated as already recovered");
        assert_eq!(state.deposits[&user_a()].cfx_claims, before_claims);
        assert_eq!(state.chain_claim_sources, before_sources);
    }

    #[test]
    fn failed_cfx_claim_payout_conflicting_replay_rejects_without_mutation() {
        let mut state = test_state();
        let sentinel = state
            .register_chain_collateral(1030, "CFX".to_string(), 18)
            .unwrap();
        let mut pos = DepositPosition::new(0);
        pos.cfx_claims
            .get_or_insert_with(BTreeMap::new)
            .insert(sentinel, 10);
        state.deposits.insert(user_a(), pos);
        state.record_chain_claim_source(sentinel, 77, 10);
        let plan = prepare_cfx_claim_payout_in_state(
            &mut state,
            user_a(),
            sentinel,
            "0x000000000000000000000000000000000000c0de".to_string(),
            rumi_protocol_backend::chains::evm::tecdsa::is_valid_evm_address,
        )
        .expect("claim plan")
        .expect("nonzero claim");
        assert!(recredit_failed_cfx_claim_payout_in_state(
            &mut state,
            CfxClaimPayoutRecovery {
                chain_sentinel: sentinel,
                op_id: 44,
                claim_id: plan.claim_id,
                claimant: user_a(),
                amount_wei: plan.amount_wei,
                reason: "tx reverted".to_string(),
                failed_at_ns: 123,
            },
        )
        .expect("first recovery succeeds"));
        let before_claims = state.deposits[&user_a()].cfx_claims.clone();
        let before_sources = state.chain_claim_sources.clone();

        let err = recredit_failed_cfx_claim_payout_in_state(
            &mut state,
            CfxClaimPayoutRecovery {
                chain_sentinel: sentinel,
                op_id: 44,
                claim_id: plan.claim_id,
                claimant: user_b(),
                amount_wei: plan.amount_wei,
                reason: "conflicting replay".to_string(),
                failed_at_ns: 124,
            },
        )
        .unwrap_err();

        assert!(matches!(err, StabilityPoolError::LiquidationFailed { .. }));
        assert_eq!(
            state.deposits[&user_a()].cfx_claims,
            before_claims,
            "conflict must not mutate user claims",
        );
        assert_eq!(
            state.chain_claim_sources, before_sources,
            "conflict must not mutate backend claim sources",
        );
    }

    #[test]
    fn distinct_failed_cfx_claim_payout_ops_can_recredit_separate_payouts() {
        let mut state = test_state();
        let sentinel = state
            .register_chain_collateral(1030, "CFX".to_string(), 18)
            .unwrap();
        let mut pos = DepositPosition::new(0);
        pos.cfx_claims
            .get_or_insert_with(BTreeMap::new)
            .insert(sentinel, 20);
        state.deposits.insert(user_a(), pos);
        state.record_chain_claim_source(sentinel, 77, 20);

        let first = prepare_cfx_claim_payout_in_state(
            &mut state,
            user_a(),
            sentinel,
            "0x000000000000000000000000000000000000c0de".to_string(),
            rumi_protocol_backend::chains::evm::tecdsa::is_valid_evm_address,
        )
        .expect("first claim plan")
        .expect("nonzero first claim");
        assert!(recredit_failed_cfx_claim_payout_in_state(
            &mut state,
            CfxClaimPayoutRecovery {
                chain_sentinel: sentinel,
                op_id: 45,
                claim_id: first.claim_id,
                claimant: user_a(),
                amount_wei: first.amount_wei,
                reason: "first tx reverted".to_string(),
                failed_at_ns: 123,
            },
        )
        .expect("first recovery succeeds"));

        let second = prepare_cfx_claim_payout_in_state(
            &mut state,
            user_a(),
            sentinel,
            "0x000000000000000000000000000000000000c0de".to_string(),
            rumi_protocol_backend::chains::evm::tecdsa::is_valid_evm_address,
        )
        .expect("second claim plan")
        .expect("nonzero second claim");
        assert!(recredit_failed_cfx_claim_payout_in_state(
            &mut state,
            CfxClaimPayoutRecovery {
                chain_sentinel: sentinel,
                op_id: 46,
                claim_id: second.claim_id,
                claimant: user_a(),
                amount_wei: second.amount_wei,
                reason: "second tx reverted".to_string(),
                failed_at_ns: 124,
            },
        )
        .expect("second recovery succeeds"));

        assert_eq!(
            state.deposits[&user_a()].cfx_claims.as_ref().unwrap()[&sentinel],
            20,
            "separate failed ops restore separate payout attempts",
        );
        assert_eq!(
            state
                .completed_cfx_claim_payout_recoveries
                .as_ref()
                .map(|m| m.len()),
            Some(2),
            "journal keeps one record per failed backend op",
        );
    }

    #[test]
    fn failed_cfx_claim_payout_recovery_rejects_invalid_input_without_mutation() {
        let mut state = test_state();
        let sentinel = state
            .register_chain_collateral(1030, "CFX".to_string(), 18)
            .unwrap();
        let mut pos = DepositPosition::new(0);
        pos.cfx_claims
            .get_or_insert_with(BTreeMap::new)
            .insert(sentinel, 5);
        state.deposits.insert(user_a(), pos);
        state.record_chain_claim_source(sentinel, 77, 5);
        let before_claims = state.deposits[&user_a()].cfx_claims.clone();
        let before_sources = state.chain_claim_sources.clone();

        let zero = recredit_failed_cfx_claim_payout_in_state(
            &mut state,
            CfxClaimPayoutRecovery {
                chain_sentinel: sentinel,
                op_id: 47,
                claim_id: 77,
                claimant: user_a(),
                amount_wei: 0,
                reason: "zero amount".to_string(),
                failed_at_ns: 123,
            },
        )
        .unwrap_err();
        assert!(matches!(zero, StabilityPoolError::AmountTooLow { .. }));

        let missing_sentinel = Principal::from_slice(&[99]);
        let invalid_sentinel = recredit_failed_cfx_claim_payout_in_state(
            &mut state,
            CfxClaimPayoutRecovery {
                chain_sentinel: missing_sentinel,
                op_id: 48,
                claim_id: 77,
                claimant: user_a(),
                amount_wei: 5,
                reason: "bad sentinel".to_string(),
                failed_at_ns: 124,
            },
        )
        .unwrap_err();
        assert!(matches!(
            invalid_sentinel,
            StabilityPoolError::CollateralNotFound { .. }
        ));
        assert_eq!(state.deposits[&user_a()].cfx_claims, before_claims);
        assert_eq!(state.chain_claim_sources, before_sources);
        assert!(
            state
                .completed_cfx_claim_payout_recoveries
                .clone()
                .unwrap_or_default()
                .is_empty(),
            "invalid input must not journal a recovery",
        );
    }

    #[test]
    fn duplicate_chain_claim_error_is_not_rolled_back() {
        let duplicate = rumi_protocol_backend::ProtocolError::ChainAdmin(
            "Duplicate chain collateral claim payout idempotency key chain-collateral-claim-77"
                .to_string(),
        );
        let ordinary = rumi_protocol_backend::ProtocolError::ChainAdmin(
            "chain collateral claim: unknown claim 77".to_string(),
        );

        assert!(is_duplicate_chain_claim_error(&duplicate));
        assert!(!is_duplicate_chain_claim_error(&ordinary));
    }

    // ─── Native-XRP auto-settlement sweep ───

    fn payout(claim_id: u64, drops: u64, created_at_ns: u64) -> NativeXrpPendingPayout {
        NativeXrpPendingPayout {
            claim_id,
            collateral_type: xrp_ledger(),
            vault_id: 195,
            drops,
            payout_address: valid_xrp_address(),
            destination_tag: Some(7),
            created_at_ns,
        }
    }

    #[derive(Default)]
    struct FakeSettleSweepIo {
        outstanding: std::collections::BTreeMap<u64, bool>,
        outstanding_errors: std::collections::BTreeSet<u64>,
        settle_errors: std::collections::BTreeSet<u64>,
        settle_error_messages: std::collections::BTreeMap<u64, String>,
        settle_calls: Vec<(u64, Principal, String, Option<u32>)>,
        outstanding_calls: Vec<(u64, Principal)>,
    }

    #[async_trait::async_trait(?Send)]
    impl NativeXrpSettleSweepIo for FakeSettleSweepIo {
        async fn claim_outstanding(
            &mut self,
            _protocol: Principal,
            claim_id: u64,
            claimant: Principal,
        ) -> Result<bool, StabilityPoolError> {
            self.outstanding_calls.push((claim_id, claimant));
            if self.outstanding_errors.contains(&claim_id) {
                return Err(StabilityPoolError::InterCanisterCallFailed {
                    target: "Protocol".to_string(),
                    method: "stability_pool_xrp_claim_outstanding".to_string(),
                });
            }
            Ok(*self.outstanding.get(&claim_id).unwrap_or(&true))
        }

        async fn settle_on_behalf(
            &mut self,
            _protocol: Principal,
            claim_id: u64,
            claimant: Principal,
            destination: String,
            destination_tag: Option<u32>,
        ) -> Result<String, StabilityPoolError> {
            self.settle_calls
                .push((claim_id, claimant, destination, destination_tag));
            if let Some(message) = self.settle_error_messages.get(&claim_id) {
                return Err(StabilityPoolError::LiquidationFailed {
                    vault_id: claim_id,
                    reason: format!("backend rejected settle-on-behalf: {message}"),
                });
            }
            if self.settle_errors.contains(&claim_id) {
                return Err(StabilityPoolError::InterCanisterCallFailed {
                    target: "Protocol".to_string(),
                    method: "stability_pool_settle_xrp_claim".to_string(),
                });
            }
            Ok(format!("TXHASH{claim_id}"))
        }
    }

    fn sweep_state_with_payouts(payouts: Vec<(Principal, NativeXrpPendingPayout)>) -> StabilityPoolState {
        let mut state = test_state();
        for (user, p) in payouts {
            add_deposit_direct(&mut state, user, icusd_ledger(), 1_00000000);
            state.record_native_xrp_pending_payout(user, p).unwrap();
        }
        state
    }

    #[test]
    fn settle_sweep_settles_outstanding_claim_with_stored_address_and_tag() {
        // The sweep must hand the backend exactly what the depositor registered
        // (address + destination tag) for the oldest pending payout, and must
        // NOT remove the local record yet: the claim is only removed after a
        // later tick observes the settlement validated (claim no longer
        // outstanding) — that mirrors the manual settle flow's two phases.
        let state = sweep_state_with_payouts(vec![(user_a(), payout(3, 11_529, 100))]);
        replace_state(state);
        let mut io = FakeSettleSweepIo::default();

        let summary = futures::executor::block_on(run_native_xrp_settle_sweep_with_io(
            &mut io, None, 2,
        ));

        assert_eq!(summary.examined, 1);
        assert_eq!(summary.submitted, 1);
        assert_eq!(summary.acked, 0);
        assert_eq!(
            io.settle_calls,
            vec![(3, user_a(), valid_xrp_address(), Some(7))]
        );
        assert_eq!(
            read_state(|s| s.native_xrp_pending_payouts_for(&user_a()).len()),
            1,
            "record stays until a later tick confirms the claim is gone"
        );
    }

    #[test]
    fn settle_sweep_acks_payout_whose_claim_is_gone() {
        // A claim that the backend no longer knows (settled + validated, by the
        // sweep or by the user clicking settle) must have its SP-side reminder
        // removed, and must not be re-settled.
        let state = sweep_state_with_payouts(vec![(user_a(), payout(3, 11_529, 100))]);
        replace_state(state);
        let mut io = FakeSettleSweepIo::default();
        io.outstanding.insert(3, false);

        let summary = futures::executor::block_on(run_native_xrp_settle_sweep_with_io(
            &mut io, None, 2,
        ));

        assert_eq!(summary.acked, 1);
        assert!(io.settle_calls.is_empty());
        assert!(read_state(|s| s.native_xrp_pending_payouts_for(&user_a()).is_empty()));
    }

    #[test]
    fn settle_sweep_is_bounded_and_rotates_across_ticks() {
        // Bounded work per tick, and the cursor must rotate so one
        // perpetually-failing claim cannot head-of-line block the others.
        let state = sweep_state_with_payouts(vec![
            (user_a(), payout(1, 10, 100)),
            (user_a(), payout(2, 20, 110)),
            (user_b(), payout(5, 50, 120)),
        ]);
        replace_state(state);
        let mut io = FakeSettleSweepIo::default();

        let first = futures::executor::block_on(run_native_xrp_settle_sweep_with_io(
            &mut io, None, 2,
        ));
        assert_eq!(first.examined, 2);
        assert_eq!(first.last_claim_id, Some(2));
        assert_eq!(
            io.settle_calls.iter().map(|c| c.0).collect::<Vec<_>>(),
            vec![1, 2]
        );

        let second = futures::executor::block_on(run_native_xrp_settle_sweep_with_io(
            &mut io,
            first.last_claim_id,
            2,
        ));
        assert_eq!(
            io.settle_calls.iter().map(|c| c.0).collect::<Vec<_>>(),
            vec![1, 2, 5, 1],
            "second tick continues after the cursor and wraps around"
        );
        assert_eq!(second.last_claim_id, Some(1));
    }

    #[test]
    fn settle_sweep_skips_entirely_when_emergency_paused() {
        let mut state = sweep_state_with_payouts(vec![(user_a(), payout(3, 11_529, 100))]);
        state.configuration.emergency_pause = true;
        replace_state(state);
        let mut io = FakeSettleSweepIo::default();

        let summary = futures::executor::block_on(run_native_xrp_settle_sweep_with_io(
            &mut io, None, 2,
        ));

        assert_eq!(summary.examined, 0);
        assert!(io.settle_calls.is_empty() && io.outstanding_calls.is_empty());
    }

    #[test]
    fn settle_sweep_counts_past_seq_submit_as_pending_confirmation() {
        // XRPL `submit` over IC https-outcalls is fan-out: every replica POSTs
        // the SAME signed blob, the first arrival applies, and the losers get
        // tefPAST_SEQ ("sequence already used" -- by our own tx). Consensus can
        // land on the losers' answer, so a SUCCESSFUL payment surfaces as a
        // submit error. Observed live 2026-08-17: all four vault-195 payouts
        // logged tefPAST_SEQ and all four were tesSUCCESS on-ledger.
        //
        // Counting these as `failed` makes a healthy sweep read like an
        // incident. They are pending-confirmation: the claim keeps its recorded
        // settlement and the next tick confirms it.
        let state = sweep_state_with_payouts(vec![(user_a(), payout(3, 11_529, 100))]);
        replace_state(state);
        let mut io = FakeSettleSweepIo::default();
        io.settle_error_messages.insert(
            3,
            "xrp claim submit failed (call settle again to confirm or retry): \
             submit rejected: tefPAST_SEQ"
                .to_string(),
        );

        let summary = futures::executor::block_on(run_native_xrp_settle_sweep_with_io(
            &mut io, None, 2,
        ));

        assert_eq!(summary.failed, 0, "a landed-but-noisy submit is not a failure");
        assert_eq!(
            summary.pending_confirmation, 1,
            "tefPAST_SEQ submits must be counted as awaiting confirmation"
        );
        assert_eq!(
            read_state(|s| s.native_xrp_pending_payouts_for(&user_a()).len()),
            1,
            "the reminder stays until a later tick confirms the settlement"
        );
    }

    #[test]
    fn settle_sweep_tolerates_errors_and_continues() {
        // An outstanding-check error or settle error on one claim must not
        // abort the tick or drop the record; the next claims still process.
        let state = sweep_state_with_payouts(vec![
            (user_a(), payout(1, 10, 100)),
            (user_b(), payout(2, 20, 110)),
            (user_b(), payout(4, 40, 120)),
        ]);
        replace_state(state);
        let mut io = FakeSettleSweepIo::default();
        io.outstanding_errors.insert(1);
        io.settle_errors.insert(2);

        let summary = futures::executor::block_on(run_native_xrp_settle_sweep_with_io(
            &mut io, None, 3,
        ));

        assert_eq!(summary.examined, 3);
        assert_eq!(summary.failed, 2);
        assert_eq!(summary.submitted, 1);
        assert_eq!(io.settle_calls.iter().map(|c| c.0).collect::<Vec<_>>(), vec![2, 4]);
        assert_eq!(
            read_state(|s| s.native_xrp_pending_payouts_for(&user_a()).len())
                + read_state(|s| s.native_xrp_pending_payouts_for(&user_b()).len()),
            3,
            "no record may be dropped on errors"
        );
    }
}
