use candid::{Nat, Principal};
use ic_canister_log::log;
use ic_cdk::call;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::{Memo, TransferArg, TransferError};
use icrc_ledger_types::icrc2::approve::{ApproveArgs, ApproveError};
use num_traits::ToPrimitive;
use rumi_protocol_backend::chains::config::ChainId;
use std::cell::Cell;
use std::collections::BTreeMap;

use crate::logs::INFO;
use crate::state::{mutate_state, read_state, StabilityPoolState};
use crate::types::*;

thread_local! {
    /// Runtime-only cursor for bounded exact-proof burn recovery.
    static CHAIN_BURN_RECOVERY_CURSOR: Cell<u64> = const { Cell::new(0) };
}

// Keep new SP V2 approvals disabled until the coordinated backend/SP end-to-end
// path passes with the official ledger. The ledger operation shapes are now
// covered by PIC evidence; recovery of existing rows remains available.
const SP_LIQUIDATION_V2_ADMISSION_ENABLED: bool = false;
// Keep 3USD reserve ingress closed until coordinated cross-canister tests and
// refund candidate recovery are complete. Existing-row reconciliation remains
// available while this new-row admission switch is false.
#[cfg(feature = "test-three-usd-reserve-ingress-v2-admission")]
const THREE_USD_RESERVE_INGRESS_V2_ADMISSION_ENABLED: bool = true;
#[cfg(not(feature = "test-three-usd-reserve-ingress-v2-admission"))]
const THREE_USD_RESERVE_INGRESS_V2_ADMISSION_ENABLED: bool = false;

const THREE_USD_E8S_SCALE: u128 = 1_000_000_000_000_000_000;

fn bounded_three_usd_absorb_amounts(
    debt_target_e8s: u64,
    opted_in_available_e8s: u64,
    draw_cap_e8s: u64,
    virtual_price_e18: u128,
) -> Result<(u64, u64), StabilityPoolError> {
    if debt_target_e8s == 0
        || opted_in_available_e8s == 0
        || draw_cap_e8s == 0
        || virtual_price_e18 == 0
    {
        return Err(StabilityPoolError::InsufficientPoolBalance);
    }
    let target_lp = (u128::from(debt_target_e8s))
        .checked_mul(THREE_USD_E8S_SCALE)
        .ok_or(StabilityPoolError::SystemBusy)?
        / virtual_price_e18;
    let target_lp = u64::try_from(target_lp).map_err(|_| StabilityPoolError::SystemBusy)?;
    let lp_amount = target_lp.min(opted_in_available_e8s).min(draw_cap_e8s);
    let debt_covered = u64::try_from(
        u128::from(lp_amount)
            .checked_mul(virtual_price_e18)
            .ok_or(StabilityPoolError::SystemBusy)?
            / THREE_USD_E8S_SCALE,
    )
    .map_err(|_| StabilityPoolError::SystemBusy)?;
    if lp_amount == 0 || debt_covered == 0 || debt_covered > debt_target_e8s {
        return Err(StabilityPoolError::InsufficientPoolBalance);
    }
    Ok((lp_amount, debt_covered))
}

fn three_usd_terminal_outcome(
    terminal: Option<&SpThreeUsdTerminalEvidence>,
) -> Result<(bool, Option<String>), StabilityPoolError> {
    match terminal {
        Some(SpThreeUsdTerminalEvidence::Absorbed { .. }) => Ok((true, None)),
        Some(SpThreeUsdTerminalEvidence::PreTransferRejected { reason, .. }) => Ok((
            false,
            Some(format!("backend rejected before 3USD transfer: {reason}")),
        )),
        Some(SpThreeUsdTerminalEvidence::FailedAfterTransfer { error, .. }) => Ok((
            false,
            Some(format!("backend refunded failed 3USD liquidation: {error}")),
        )),
        None => Err(StabilityPoolError::SystemBusy),
    }
}

fn sp_liquidation_v2_token_supported(token: SpLiquidationToken) -> bool {
    // Backend's current V2 executor only supports the verified icUSD minter
    // burn/mint path. CK-stable routes remain rejected before approval.
    token == SpLiquidationToken::IcUsd
}

fn sp_v2_approval_preflight_amounts(
    principal_cap: u64,
    approval_fee: u64,
    pull_fee: u64,
) -> Option<(u64, u64)> {
    let allowance = principal_cap.checked_add(pull_fee)?;
    let required_balance = allowance.checked_add(approval_fee)?;
    Some((allowance, required_balance))
}

/// Acknowledge backend V2 state only after the local journal contains a
/// completed row. Local completion is written only after independently
/// validating the collateral ICRC-3 receipt, so backend status alone can never
/// release its replay fence.
pub(crate) async fn acknowledge_completed_sp_liquidation_v2(
    request_id: u64,
) -> Result<(), StabilityPoolError> {
    let row = read_state(|state| {
        state
            .completed_sp_liquidations_v2
            .as_ref()
            .and_then(|rows| rows.get(&request_id))
            .cloned()
    })
    .ok_or(StabilityPoolError::SystemBusy)?;
    let payout_completed = row.phase == SpLiquidationV2LocalPhase::Complete
        && row.payout_receipt.is_some()
        && row.stable_debit_applied;
    let refund_completed = row.phase == SpLiquidationV2LocalPhase::Rejected
        && row.stable_refund_applied
        && row.stable_refund_receipt.is_some();
    if !payout_completed && !refund_completed {
        return Err(StabilityPoolError::SystemBusy);
    }
    let protocol = read_state(|state| state.protocol_canister_id);
    let (status,): (Result<SpLiquidationV2StatusView, rumi_protocol_backend::ProtocolError>,) =
        call(
            protocol,
            "get_stability_pool_liquidation_v2_status",
            (request_id,),
        )
        .await
        .map_err(|_| StabilityPoolError::InterCanisterCallFailed {
            target: "Protocol".into(),
            method: "get_stability_pool_liquidation_v2_status".into(),
        })?;
    let view = status.map_err(|_| StabilityPoolError::SystemBusy)?;
    if view.stability_pool != ic_cdk::api::id()
        || view.request_id != request_id
        || row.backend_request.is_none()
        || view
            .request
            .as_ref()
            .is_some_and(|request| row.backend_request.as_ref() != Some(request))
    {
        return Err(StabilityPoolError::SystemBusy);
    }
    match view.status {
        SpLiquidationV2Status::Complete {
            stable_pull_receipt,
            result,
            payout_receipt,
        } => {
            if !payout_completed || view.request.as_ref() != row.backend_request.as_ref() {
                return Err(StabilityPoolError::SystemBusy);
            }
            if row.stable_pull_receipt.as_ref() != Some(&stable_pull_receipt)
                || row.result.as_ref() != Some(&result)
                || row.payout_receipt.as_ref() != Some(&payout_receipt)
            {
                return Err(StabilityPoolError::SystemBusy);
            }
            let (result,): (Result<(), rumi_protocol_backend::ProtocolError>,) =
                call(protocol, "ack_stability_pool_liquidation_v2", (request_id,))
                    .await
                    .map_err(|_| StabilityPoolError::InterCanisterCallFailed {
                        target: "Protocol".into(),
                        method: "ack_stability_pool_liquidation_v2".into(),
                    })?;
            result.map_err(|_| StabilityPoolError::SystemBusy)?;
        }
        SpLiquidationV2Status::StablePullRefunded {
            stable_pull_receipt,
            refund_receipt,
            ..
        } => {
            if !refund_completed
                || view.request.as_ref() != row.backend_request.as_ref()
                || row.stable_pull_receipt != stable_pull_receipt
                || row.stable_refund_receipt.as_ref() != Some(&refund_receipt)
            {
                return Err(StabilityPoolError::SystemBusy);
            }
            let (result,): (Result<(), rumi_protocol_backend::ProtocolError>,) =
                call(protocol, "ack_stability_pool_liquidation_v2", (request_id,))
                    .await
                    .map_err(|_| StabilityPoolError::InterCanisterCallFailed {
                        target: "Protocol".into(),
                        method: "ack_stability_pool_liquidation_v2".into(),
                    })?;
            result.map_err(|_| StabilityPoolError::SystemBusy)?;
        }
        SpLiquidationV2Status::Acknowledged => {
            if view.request.is_some() {
                return Err(StabilityPoolError::SystemBusy);
            }
        }
        _ => return Err(StabilityPoolError::SystemBusy),
    }
    mutate_state(|state| state.acknowledge_sp_liquidation_v2(request_id))
}

pub(crate) async fn verify_sp_liquidation_v2_approval_receipt(
    receipt: &SpLiquidationApprovalReceipt,
) -> Result<(), StabilityPoolError> {
    let backend_receipt = rumi_protocol_backend::SpLiquidationApprovalReceipt {
        block_index: receipt.block_index,
        tuple: rumi_protocol_backend::SpLiquidationApprovalTuple {
            ledger: receipt.tuple.ledger,
            owner: receipt.tuple.owner.clone(),
            spender: receipt.tuple.spender.clone(),
            allowance_raw: receipt.tuple.allowance_raw,
            fee_raw: receipt.tuple.fee_raw,
            memo: receipt.tuple.memo.clone(),
            created_at_time_ns: receipt.tuple.created_at_time_ns,
            expires_at_ns: receipt.tuple.expires_at_ns,
        },
    };
    rumi_protocol_backend::icrc3_proof::verify_icrc3_approval_block(&backend_receipt)
        .await
        .map_err(|reason| StabilityPoolError::LedgerTransferFailed { reason })
}

fn backend_sp_pull_tuple(
    tuple: &SpLiquidationStablePullTuple,
) -> rumi_protocol_backend::SpLiquidationStablePullTuple {
    rumi_protocol_backend::SpLiquidationStablePullTuple {
        op_nonce: tuple.op_nonce,
        ledger: tuple.ledger,
        from: tuple.from.clone(),
        spender: tuple.spender.clone(),
        to: tuple.to.clone(),
        amount_raw: tuple.amount_raw,
        fee_raw: tuple.fee_raw,
        memo: tuple.memo.clone(),
        created_at_time_ns: tuple.created_at_time_ns,
    }
}

fn backend_sp_payout_tuple(
    tuple: &SpLiquidationPayoutTuple,
) -> rumi_protocol_backend::SpLiquidationPayoutTuple {
    rumi_protocol_backend::SpLiquidationPayoutTuple {
        op_nonce: tuple.op_nonce,
        ledger: tuple.ledger,
        source: tuple.source.clone(),
        destination: tuple.destination.clone(),
        gross_amount_raw: tuple.gross_amount_raw,
        net_amount_raw: tuple.net_amount_raw,
        fee_raw: tuple.fee_raw,
        memo: tuple.memo.clone(),
        created_at_time_ns: tuple.created_at_time_ns,
        collateral_type: tuple.collateral_type,
    }
}

async fn accept_sp_liquidation_v2_payout_supersession(
    request_id: u64,
    generation: u32,
    predecessor: &SpLiquidationPayoutTuple,
    replacement: &SpLiquidationPayoutTuple,
) -> Result<(), StabilityPoolError> {
    let protocol = read_state(|state| state.protocol_canister_id);
    let result: Result<(Result<(), rumi_protocol_backend::ProtocolError>,), _> = call(
        protocol,
        "accept_stability_pool_liquidation_v2_payout_supersession",
        (
            request_id,
            generation,
            backend_sp_payout_tuple(predecessor),
            backend_sp_payout_tuple(replacement),
        ),
    )
    .await;
    match result {
        Ok((Ok(()),)) => Ok(()),
        Ok((Err(_),)) => Err(StabilityPoolError::SystemBusy),
        Err(_) => Err(StabilityPoolError::InterCanisterCallFailed {
            target: "Protocol".into(),
            method: "accept_stability_pool_liquidation_v2_payout_supersession".into(),
        }),
    }
}

async fn verify_sp_liquidation_v2_stable_receipt(
    receipt: &SpLiquidationStablePullReceipt,
    token: SpLiquidationToken,
) -> Result<(), StabilityPoolError> {
    let tuple = backend_sp_pull_tuple(&receipt.tuple);
    let block = rumi_protocol_backend::icrc3_proof::fetch_icrc3_block(
        receipt.tuple.ledger,
        receipt.block_index,
    )
    .await
    .map_err(|reason| StabilityPoolError::LedgerTransferFailed { reason })?;
    let proof = match token {
        SpLiquidationToken::IcUsd => validate_sp_v2_icusd_burn_block(&block, &tuple),
        // The current backend V2 executor only supports icUSD. Do not infer
        // CK-stable minting or burn semantics from a configured token symbol.
        SpLiquidationToken::CKUSDT | SpLiquidationToken::CKUSDC => {
            Err("CK stable V2 pull proof is not enabled".to_string())
        }
    };
    proof.map_err(|reason| StabilityPoolError::LedgerTransferFailed { reason })
}

fn validate_sp_v2_icusd_burn_block(
    block: &rumi_protocol_backend::icrc3_proof::DecodedBlock,
    tuple: &rumi_protocol_backend::SpLiquidationStablePullTuple,
) -> Result<(), String> {
    if tuple.fee_raw != 0
        || block.op != "burn"
        || block.btype.as_deref().is_some_and(|kind| kind != "1burn")
        || !block.from.as_ref().is_some_and(|actual| {
            rumi_protocol_backend::icrc3_proof::accounts_match(actual, &tuple.from)
        })
        || !block.spender.as_ref().is_some_and(|actual| {
            rumi_protocol_backend::icrc3_proof::accounts_match(actual, &tuple.spender)
        })
        || block.to.is_some()
        || block.amount != u128::from(tuple.amount_raw)
        || block.transaction_fee.is_some()
        || block.fee.is_some()
        || block.memo.as_deref() != Some(tuple.memo.as_slice())
        || block.created_at_time != Some(tuple.created_at_time_ns)
        || block.expected_allowance.is_some()
        || block.expires_at.is_some()
    {
        return Err(
            "ICRC-3 block does not prove the exact fee-free SP-to-minter burn tuple".into(),
        );
    }
    Ok(())
}

async fn verify_sp_liquidation_v2_payout_receipt(
    receipt: &SpLiquidationPayoutReceipt,
) -> Result<(), StabilityPoolError> {
    let tuple = &receipt.tuple;
    let total = tuple
        .net_amount_raw
        .checked_add(tuple.fee_raw)
        .ok_or(StabilityPoolError::SystemBusy)?;
    if total != tuple.gross_amount_raw {
        return Err(StabilityPoolError::SystemBusy);
    }
    let block =
        rumi_protocol_backend::icrc3_proof::fetch_icrc3_block(tuple.ledger, receipt.block_index)
            .await
            .map_err(|reason| StabilityPoolError::LedgerTransferFailed { reason })?;
    rumi_protocol_backend::icrc3_proof::validate_icrc3_transfer_block_with_fee(
        &block,
        tuple.source.clone(),
        tuple.destination.clone(),
        tuple.net_amount_raw,
        tuple.fee_raw,
        &tuple.memo,
        tuple.created_at_time_ns,
    )
    .map_err(|reason| StabilityPoolError::LedgerTransferFailed { reason })
}

async fn verify_sp_liquidation_v2_refund_receipt(
    receipt: &SpLiquidationStableRefundReceipt,
    token: SpLiquidationToken,
) -> Result<(), StabilityPoolError> {
    let tuple = &receipt.tuple;
    let components = tuple
        .principal_refund_raw
        .checked_add(tuple.approval_fee_refund_raw)
        .and_then(|value| value.checked_add(tuple.pull_fee_refund_raw))
        .ok_or(StabilityPoolError::SystemBusy)?;
    if components != tuple.amount_raw {
        return Err(StabilityPoolError::SystemBusy);
    }
    if token != SpLiquidationToken::IcUsd {
        return Err(StabilityPoolError::LedgerTransferFailed {
            reason: "CK stable V2 refund proof is not enabled".into(),
        });
    }
    let block =
        rumi_protocol_backend::icrc3_proof::fetch_icrc3_block(tuple.ledger, receipt.block_index)
            .await
            .map_err(|reason| StabilityPoolError::LedgerTransferFailed { reason })?;
    validate_sp_v2_icusd_mint_refund_block(&block, tuple)
        .map_err(|reason| StabilityPoolError::LedgerTransferFailed { reason })
}

fn validate_sp_v2_icusd_mint_refund_block(
    block: &rumi_protocol_backend::icrc3_proof::DecodedBlock,
    tuple: &SpLiquidationStableRefundTuple,
) -> Result<(), String> {
    if tuple.fee_raw != 0
        || block.op != "mint"
        || block.btype.as_deref().is_some_and(|kind| kind != "1mint")
        || block.from.is_some()
        || block.spender.is_some()
        || !block.to.as_ref().is_some_and(|actual| {
            rumi_protocol_backend::icrc3_proof::accounts_match(actual, &tuple.destination)
        })
        || block.amount != u128::from(tuple.amount_raw)
        || block.transaction_fee.is_some()
        || block.fee.is_some()
        || block.memo.as_deref() != Some(tuple.memo.as_slice())
        || block.created_at_time != Some(tuple.created_at_time_ns)
        || block.expected_allowance.is_some()
        || block.expires_at.is_some()
    {
        return Err(
            "ICRC-3 block does not prove the exact fee-free icUSD mint refund tuple".into(),
        );
    }
    Ok(())
}

fn default_account(owner: Principal) -> Account {
    Account {
        owner,
        subaccount: None,
    }
}

async fn dispatch_or_resume_sp_liquidation_v2(
    request: SpLiquidationV2Request,
) -> Result<(), StabilityPoolError> {
    let protocol = read_state(|state| state.protocol_canister_id);
    let result: Result<
        (Result<SpLiquidationV2StatusView, rumi_protocol_backend::ProtocolError>,),
        _,
    > = call(protocol, "stability_pool_liquidate_v2", (request.clone(),)).await;
    match result {
        Ok((Ok(view),))
            if view.stability_pool == ic_cdk::api::id()
                && view.request_id == request.request_id
                && view.request.as_ref() == Some(&request) =>
        {
            Ok(())
        }
        Ok((Ok(_),)) => Err(StabilityPoolError::SystemBusy),
        Ok((Err(_),)) | Err(_) => {
            mutate_state(|state| {
                state.mark_sp_liquidation_v2_error(
                    request.request_id,
                    "backend liquidation call is ambiguous; durable request retained".into(),
                )
            })?;
            Err(StabilityPoolError::InterCanisterCallFailed {
                target: "Protocol".into(),
                method: "stability_pool_liquidate_v2".into(),
            })
        }
    }
}

/// Advance one durable V2 request by querying the authenticated backend
/// journal, proving any candidate blocks locally, and applying each accounting
/// transition once. All ambiguous paths retain the row and its balance fence.
pub(crate) async fn recover_sp_liquidation_v2(request_id: u64) -> Result<(), StabilityPoolError> {
    let Some(mut row) = read_state(|state| state.pending_sp_liquidation_v2(request_id)) else {
        return acknowledge_completed_sp_liquidation_v2(request_id).await;
    };
    let request = if let Some(request) = row.backend_request.clone() {
        request
    } else {
        if let Some(block_index) = row.approval_candidate_block_index {
            let receipt = approval_receipt_for_row(&row, block_index);
            if let Err(error) = verify_sp_liquidation_v2_approval_receipt(&receipt).await {
                let _ = mutate_state(|state| {
                    state.mark_sp_liquidation_v2_approval_dispatch(request_id, false, true, None)
                });
                let _ = mutate_state(|state| {
                    state.mark_sp_liquidation_v2_error(
                        request_id,
                        format!("approval candidate did not prove the exact tuple: {error:?}"),
                    )
                });
                return Err(error);
            }
            mutate_state(|state| {
                state.account_sp_liquidation_v2_approval_fee(request_id, receipt)
            })?;
            row = read_state(|state| state.pending_sp_liquidation_v2(request_id))
                .ok_or(StabilityPoolError::SystemBusy)?;
            row.backend_request
                .clone()
                .ok_or(StabilityPoolError::SystemBusy)?
        } else if row.approval_ambiguous_seen {
            // No candidate index means the approval outcome cannot be cleared
            // from absence. The public exact-block reconciliation path is
            // required before any retry.
            return Err(StabilityPoolError::SystemBusy);
        } else {
            let request = submit_sp_v2_approval(request_id).await?;
            dispatch_or_resume_sp_liquidation_v2(request.clone()).await?;
            return Ok(());
        }
    };
    let protocol = read_state(|state| state.protocol_canister_id);
    let (response,): (Result<SpLiquidationV2StatusView, rumi_protocol_backend::ProtocolError>,) =
        call(
            protocol,
            "get_stability_pool_liquidation_v2_status",
            (request_id,),
        )
        .await
        .map_err(|_| StabilityPoolError::InterCanisterCallFailed {
            target: "Protocol".into(),
            method: "get_stability_pool_liquidation_v2_status".into(),
        })?;
    let view = response.map_err(|_| StabilityPoolError::SystemBusy)?;
    if view.stability_pool != ic_cdk::api::id() || view.request_id != request_id {
        return Err(StabilityPoolError::SystemBusy);
    }
    match view.status {
        SpLiquidationV2Status::Unseen => {
            if view.request.is_some() {
                return Err(StabilityPoolError::SystemBusy);
            }
            dispatch_or_resume_sp_liquidation_v2(request).await
        }
        SpLiquidationV2Status::StablePullPending {
            tuple,
            candidate_block_index,
            last_error: _,
        } => {
            if view.request.as_ref() != Some(&request)
                || tuple.to != tuple.spender
                || tuple.ledger != row.stablecoin_ledger
                || tuple.from != row.approval.owner
                || tuple.spender != row.approval.spender
                || tuple.amount_raw > row.request.amount
                || tuple
                    .amount_raw
                    .checked_add(tuple.fee_raw)
                    .is_none_or(|total| total > request.approval.tuple.allowance_raw)
            {
                return Err(StabilityPoolError::SystemBusy);
            }
            mutate_state(|state| {
                state.record_sp_liquidation_v2_stable_pull_candidate(
                    request_id,
                    tuple.clone(),
                    candidate_block_index,
                )
            })?;
            if let Some(block_index) = candidate_block_index {
                let receipt = SpLiquidationStablePullReceipt { block_index, tuple };
                verify_sp_liquidation_v2_stable_receipt(&receipt, request.token).await?;
            }
            dispatch_or_resume_sp_liquidation_v2(request).await
        }
        SpLiquidationV2Status::CollateralPayoutSupersessionPending {
            stable_pull_receipt,
            result,
            predecessor,
            replacement,
            evidence,
            generation,
        } => {
            validate_sp_v2_collateral_tuple(&row, &predecessor, protocol)?;
            validate_sp_v2_collateral_tuple(&row, &replacement, protocol)?;
            if view.request.as_ref() != Some(&request)
                || generation != 1
                || replacement.gross_amount_raw != predecessor.gross_amount_raw
                || result.collateral_amount_received != Some(predecessor.gross_amount_raw)
                || row.payout_candidate_block_index.is_some()
                || row.payout_receipt.is_some()
                || row.payout_supersession_generation.is_none()
            {
                return Err(StabilityPoolError::SystemBusy);
            }

            // Prove the already-committed stable pull before recording its
            // accounting effect alongside the local tuple transition.
            verify_sp_liquidation_v2_stable_receipt(&stable_pull_receipt, request.token).await?;
            mutate_state(|state| {
                state.adopt_sp_liquidation_v2_payout_supersession(
                    request_id,
                    &stable_pull_receipt,
                    predecessor.clone(),
                    replacement.clone(),
                    &result,
                    evidence.clone(),
                    generation,
                )
            })?;

            // Adoption is durable before this await. A lost accept reply is
            // recovered by replaying the same generation and exact tuple pair.
            accept_sp_liquidation_v2_payout_supersession(
                request_id,
                generation,
                &predecessor,
                &replacement,
            )
            .await?;
            dispatch_or_resume_sp_liquidation_v2(request).await
        }
        SpLiquidationV2Status::CollateralPayoutPending {
            stable_pull_receipt,
            result,
            tuple,
            candidate_block_index,
            last_error: _,
        } => {
            validate_sp_v2_collateral_tuple(&row, &tuple, protocol)?;
            if view.request.as_ref() != Some(&request)
                || result.collateral_amount_received != Some(tuple.gross_amount_raw)
            {
                return Err(StabilityPoolError::SystemBusy);
            }
            mutate_state(|state| {
                state.record_sp_liquidation_v2_backend_receipts(
                    request_id,
                    stable_pull_receipt.clone(),
                    tuple.clone(),
                    candidate_block_index,
                    result.clone(),
                )
            })?;
            verify_sp_liquidation_v2_stable_receipt(&stable_pull_receipt, request.token).await?;
            mutate_state(|state| {
                state.apply_sp_liquidation_v2_stable_receipt(
                    request_id,
                    stable_pull_receipt,
                    tuple.clone(),
                    result,
                )
            })?;
            let Some(block_index) = candidate_block_index else {
                return dispatch_or_resume_sp_liquidation_v2(request).await;
            };
            let receipt = SpLiquidationPayoutReceipt { block_index, tuple };
            mutate_state(|state| {
                state.record_sp_liquidation_v2_payout_receipt(request_id, receipt.clone())
            })?;
            verify_sp_liquidation_v2_payout_receipt(&receipt).await?;
            mutate_state(|state| {
                state.finalize_sp_liquidation_v2_payout(request_id, ic_cdk::api::time())
            })?;
            acknowledge_completed_sp_liquidation_v2(request_id).await
        }
        SpLiquidationV2Status::Complete {
            stable_pull_receipt,
            result,
            payout_receipt,
        } => {
            validate_sp_v2_collateral_tuple(&row, &payout_receipt.tuple, protocol)?;
            if view.request.as_ref() != Some(&request)
                || result.collateral_amount_received != Some(payout_receipt.tuple.gross_amount_raw)
            {
                return Err(StabilityPoolError::SystemBusy);
            }
            mutate_state(|state| {
                state.record_sp_liquidation_v2_backend_receipts(
                    request_id,
                    stable_pull_receipt.clone(),
                    payout_receipt.tuple.clone(),
                    Some(payout_receipt.block_index),
                    result.clone(),
                )
            })?;
            verify_sp_liquidation_v2_stable_receipt(&stable_pull_receipt, request.token).await?;
            mutate_state(|state| {
                state.apply_sp_liquidation_v2_stable_receipt(
                    request_id,
                    stable_pull_receipt,
                    payout_receipt.tuple.clone(),
                    result,
                )
            })?;
            verify_sp_liquidation_v2_payout_receipt(&payout_receipt).await?;
            mutate_state(|state| {
                state.record_sp_liquidation_v2_payout_receipt(request_id, payout_receipt)
            })?;
            mutate_state(|state| {
                state.finalize_sp_liquidation_v2_payout(request_id, ic_cdk::api::time())
            })?;
            acknowledge_completed_sp_liquidation_v2(request_id).await
        }
        SpLiquidationV2Status::StablePullRefundPending {
            stable_pull_receipt,
            tuple,
            candidate_block_index,
            last_error: _,
        } => {
            validate_sp_v2_refund_tuple(&row, stable_pull_receipt.as_ref(), &tuple, protocol)?;
            if view.request.as_ref() != Some(&request) {
                return Err(StabilityPoolError::SystemBusy);
            }
            if let Some(pull) = &stable_pull_receipt {
                mutate_state(|state| {
                    state.record_sp_liquidation_v2_stable_pull_candidate(
                        request_id,
                        pull.tuple.clone(),
                        Some(pull.block_index),
                    )
                })?;
                verify_sp_liquidation_v2_stable_receipt(pull, request.token).await?;
                mutate_state(|state| {
                    state.apply_sp_liquidation_v2_refundable_stable_pull(request_id, pull.clone())
                })?;
            }
            mutate_state(|state| {
                state.record_sp_liquidation_v2_refund_tuple(
                    request_id,
                    tuple.clone(),
                    candidate_block_index,
                )
            })?;
            let Some(block_index) = candidate_block_index else {
                return dispatch_or_resume_sp_liquidation_v2(request).await;
            };
            let receipt = SpLiquidationStableRefundReceipt { block_index, tuple };
            verify_sp_liquidation_v2_refund_receipt(&receipt, request.token).await?;
            mutate_state(|state| {
                state.apply_sp_liquidation_v2_refund_receipt(request_id, receipt)
            })?;
            acknowledge_completed_sp_liquidation_v2(request_id).await
        }
        SpLiquidationV2Status::StablePullRefunded {
            stable_pull_receipt,
            refund_receipt,
            ..
        } => {
            validate_sp_v2_refund_tuple(
                &row,
                stable_pull_receipt.as_ref(),
                &refund_receipt.tuple,
                protocol,
            )?;
            if view.request.as_ref() != Some(&request) {
                return Err(StabilityPoolError::SystemBusy);
            }
            if let Some(pull) = &stable_pull_receipt {
                mutate_state(|state| {
                    state.record_sp_liquidation_v2_stable_pull_candidate(
                        request_id,
                        pull.tuple.clone(),
                        Some(pull.block_index),
                    )
                })?;
                verify_sp_liquidation_v2_stable_receipt(pull, request.token).await?;
                mutate_state(|state| {
                    state.apply_sp_liquidation_v2_refundable_stable_pull(request_id, pull.clone())
                })?;
            }
            mutate_state(|state| {
                state.record_sp_liquidation_v2_refund_tuple(
                    request_id,
                    refund_receipt.tuple.clone(),
                    Some(refund_receipt.block_index),
                )
            })?;
            verify_sp_liquidation_v2_refund_receipt(&refund_receipt, request.token).await?;
            mutate_state(|state| {
                state.apply_sp_liquidation_v2_refund_receipt(request_id, refund_receipt)
            })?;
            acknowledge_completed_sp_liquidation_v2(request_id).await
        }
        SpLiquidationV2Status::Rejected { reason } => {
            mutate_state(|state| state.mark_sp_liquidation_v2_error(request_id, reason))?;
            Err(StabilityPoolError::SystemBusy)
        }
        SpLiquidationV2Status::Acknowledged => Err(StabilityPoolError::SystemBusy),
    }
}

fn validate_sp_v2_collateral_tuple(
    row: &PendingSpLiquidationV2,
    tuple: &SpLiquidationPayoutTuple,
    protocol: Principal,
) -> Result<(), StabilityPoolError> {
    if tuple.ledger != row.collateral_type
        || tuple.collateral_type != row.collateral_type
        || tuple.source != default_account(protocol)
        || tuple.destination != default_account(ic_cdk::api::id())
        || tuple.gross_amount_raw
            != tuple
                .net_amount_raw
                .checked_add(tuple.fee_raw)
                .ok_or(StabilityPoolError::SystemBusy)?
    {
        return Err(StabilityPoolError::SystemBusy);
    }
    Ok(())
}

fn validate_sp_v2_refund_tuple(
    row: &PendingSpLiquidationV2,
    pull: Option<&SpLiquidationStablePullReceipt>,
    tuple: &SpLiquidationStableRefundTuple,
    protocol: Principal,
) -> Result<(), StabilityPoolError> {
    let principal = pull.map_or(0, |receipt| receipt.tuple.amount_raw);
    let pull_fee = pull.map_or(0, |receipt| receipt.tuple.fee_raw);
    let expected = principal
        .checked_add(pull_fee)
        .and_then(|amount| amount.checked_add(row.approval.fee_raw))
        .ok_or(StabilityPoolError::SystemBusy)?;
    if tuple.ledger != row.stablecoin_ledger
        || tuple.source != default_account(protocol)
        || tuple.destination != default_account(ic_cdk::api::id())
        || tuple.principal_refund_raw != principal
        || tuple.pull_fee_refund_raw != pull_fee
        || tuple.approval_fee_refund_raw != row.approval.fee_raw
        || tuple.amount_raw != expected
    {
        return Err(StabilityPoolError::SystemBusy);
    }
    Ok(())
}

async fn query_exact_ledger_fee(ledger: Principal) -> Result<u64, StabilityPoolError> {
    let (fee,): (Nat,) = call(ledger, "icrc1_fee", ()).await.map_err(|_| {
        StabilityPoolError::LedgerTransferFailed {
            reason: "could not query exact ICRC-1 fee before SP V2 approval".into(),
        }
    })?;
    fee.0
        .to_u64()
        .ok_or_else(|| StabilityPoolError::LedgerTransferFailed {
            reason: "ICRC-1 fee exceeds the supported u64 accounting range".into(),
        })
}

fn make_sp_v2_approval_tuple(
    ledger: Principal,
    pool: Principal,
    protocol: Principal,
    allowance_raw: u64,
    fee_raw: u64,
    vault_id: u64,
    created_at_time_ns: u64,
    expires_at_ns: u64,
) -> SpLiquidationV2ApprovalTuple {
    let memo = format!("sp-v2-approval/{vault_id}/{created_at_time_ns}").into_bytes();
    SpLiquidationV2ApprovalTuple {
        ledger,
        owner: default_account(pool),
        spender: default_account(protocol),
        allowance_raw,
        fee_raw,
        memo,
        created_at_time_ns,
        expires_at_ns,
        fee_accounted: false,
    }
}

async fn submit_sp_v2_approval(
    request_id: u64,
) -> Result<SpLiquidationV2Request, StabilityPoolError> {
    let mut row = read_state(|state| state.pending_sp_liquidation_v2(request_id))
        .ok_or(StabilityPoolError::SystemBusy)?;
    if row.backend_request.is_some() {
        return Ok(row.backend_request.unwrap());
    }
    if row.approval_proven_no_effect {
        let fee = query_exact_ledger_fee(row.stablecoin_ledger).await?;
        let pull_fee = match row.request.token {
            SpLiquidationToken::IcUsd => 0,
            SpLiquidationToken::CKUSDT | SpLiquidationToken::CKUSDC => {
                return Err(StabilityPoolError::SystemBusy);
            }
        };
        let (allowance, required_pool_balance) =
            sp_v2_approval_preflight_amounts(row.request.amount, fee, pull_fee)
                .ok_or(StabilityPoolError::SystemBusy)?;
        let available = read_state(|state| {
            state.available_stablecoin_for_collateral(row.stablecoin_ledger, &row.collateral_type)
        })?;
        if available < required_pool_balance {
            return Err(StabilityPoolError::InsufficientPoolBalance);
        }
        let now =
            mutate_state(|state| state.allocate_outbound_payout_timestamp(ic_cdk::api::time()))?;
        let expires = now
            .checked_add(300_000_000_000)
            .ok_or(StabilityPoolError::SystemBusy)?;
        let approval = make_sp_v2_approval_tuple(
            row.stablecoin_ledger,
            ic_cdk::api::id(),
            read_state(|state| state.protocol_canister_id),
            allowance,
            fee,
            row.request.vault_id,
            now,
            expires,
        );
        mutate_state(|state| {
            state.reprice_sp_liquidation_v2_approval_after_no_effect(request_id, approval)
        })?;
        row = read_state(|state| state.pending_sp_liquidation_v2(request_id))
            .ok_or(StabilityPoolError::SystemBusy)?;
    }
    let tuple = &row.approval;
    let args = ApproveArgs {
        from_subaccount: None,
        spender: tuple.spender.clone(),
        amount: Nat::from(tuple.allowance_raw),
        expected_allowance: None,
        expires_at: Some(tuple.expires_at_ns),
        fee: Some(Nat::from(tuple.fee_raw)),
        memo: Some(Memo::from(tuple.memo.clone())),
        created_at_time: Some(tuple.created_at_time_ns),
    };
    mutate_state(|state| {
        state.mark_sp_liquidation_v2_approval_dispatch(request_id, true, false, None)
    })?;
    let result: Result<(Result<Nat, ApproveError>,), _> =
        call(tuple.ledger, "icrc2_approve", (args,)).await;
    let block_index = match result {
        Ok((Ok(index),)) => match index.0.to_u64() {
            Some(index) => index,
            None => {
                mutate_state(|state| {
                    state.mark_sp_liquidation_v2_approval_dispatch(request_id, false, true, None)
                })?;
                return Err(StabilityPoolError::SystemBusy);
            }
        },
        Ok((Err(ApproveError::Duplicate { duplicate_of }),)) => {
            let index = duplicate_of.0.to_u64();
            mutate_state(|state| {
                state.mark_sp_liquidation_v2_approval_dispatch(request_id, false, true, None)
            })?;
            let Some(index) = index else {
                return Err(StabilityPoolError::SystemBusy);
            };
            index
        }
        Ok((Err(error),)) => {
            let no_effect = matches!(
                error,
                ApproveError::BadFee { .. }
                    | ApproveError::InsufficientFunds { .. }
                    | ApproveError::AllowanceChanged { .. }
                    | ApproveError::Expired { .. }
                    | ApproveError::TooOld
                    | ApproveError::CreatedInFuture { .. }
            );
            if no_effect {
                mutate_state(|state| {
                    state.mark_sp_liquidation_v2_approval_no_effect(
                        request_id,
                        format!("approval had typed no-effect result: {error:?}"),
                    )
                })?;
            } else {
                mutate_state(|state| {
                    state.mark_sp_liquidation_v2_approval_dispatch(request_id, false, true, None)
                })?;
            }
            return Err(StabilityPoolError::LedgerTransferFailed {
                reason: format!("ICRC-2 approval failed: {error:?}"),
            });
        }
        Err(_) => {
            mutate_state(|state| {
                state.mark_sp_liquidation_v2_approval_dispatch(request_id, false, true, None)
            })?;
            return Err(StabilityPoolError::InterCanisterCallFailed {
                target: tuple.ledger.to_text(),
                method: "icrc2_approve".into(),
            });
        }
    };
    let receipt = SpLiquidationApprovalReceipt {
        block_index,
        tuple: SpLiquidationApprovalTuple {
            ledger: tuple.ledger,
            owner: tuple.owner.clone(),
            spender: tuple.spender.clone(),
            allowance_raw: tuple.allowance_raw,
            fee_raw: tuple.fee_raw,
            memo: tuple.memo.clone(),
            created_at_time_ns: tuple.created_at_time_ns,
            expires_at_ns: tuple.expires_at_ns,
        },
    };
    if let Err(error) = verify_sp_liquidation_v2_approval_receipt(&receipt).await {
        mutate_state(|state| {
            state.mark_sp_liquidation_v2_approval_dispatch(request_id, false, true, None)
        })?;
        mutate_state(|state| {
            state.mark_sp_liquidation_v2_error(
                request_id,
                format!("approval receipt proof failed: {error:?}"),
            )
        })?;
        return Err(error);
    }
    mutate_state(|state| state.account_sp_liquidation_v2_approval_fee(request_id, receipt))?;
    read_state(|state| {
        state
            .pending_sp_liquidation_v2(request_id)
            .and_then(|row| row.backend_request)
    })
    .ok_or(StabilityPoolError::SystemBusy)
}

fn approval_receipt_for_row(
    row: &PendingSpLiquidationV2,
    block_index: u64,
) -> SpLiquidationApprovalReceipt {
    SpLiquidationApprovalReceipt {
        block_index,
        tuple: SpLiquidationApprovalTuple {
            ledger: row.approval.ledger,
            owner: row.approval.owner.clone(),
            spender: row.approval.spender.clone(),
            allowance_raw: row.approval.allowance_raw,
            fee_raw: row.approval.fee_raw,
            memo: row.approval.memo.clone(),
            created_at_time_ns: row.approval.created_at_time_ns,
            expires_at_ns: row.approval.expires_at_ns,
        },
    }
}

/// Additive ICRC-collateral liquidation entrypoint. The SP chooses a monotonic
/// local request ID and persists its exact intent before approval; retries
/// resume that same ID and payload.
pub async fn execute_liquidation_v2(
    vault_id: u64,
    token: SpLiquidationToken,
    max_principal_pull_raw: u64,
) -> Result<u64, StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    if caller == Principal::anonymous() {
        return Err(StabilityPoolError::Unauthorized);
    }
    if read_state(|state| state.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }
    if max_principal_pull_raw == 0 {
        return Err(StabilityPoolError::InsufficientPoolBalance);
    }
    if let Some(existing) = read_state(|state| state.pending_sp_liquidation_v2_for_vault(vault_id))
    {
        if existing.request.amount != max_principal_pull_raw || existing.request.token != token {
            return Err(StabilityPoolError::SystemBusy);
        }
        let _guard =
            crate::pool_guard::SpLiquidationGuard::new_v2_resume(existing.request.request_id)?;
        let request = if let Some(request) = existing.backend_request.clone() {
            request
        } else if let Some(block_index) = existing.approval_candidate_block_index {
            let receipt = approval_receipt_for_row(&existing, block_index);
            verify_sp_liquidation_v2_approval_receipt(&receipt).await?;
            mutate_state(|state| {
                state.account_sp_liquidation_v2_approval_fee(existing.request.request_id, receipt)
            })?;
            read_state(|state| {
                state
                    .pending_sp_liquidation_v2(existing.request.request_id)
                    .and_then(|row| row.backend_request)
            })
            .ok_or(StabilityPoolError::SystemBusy)?
        } else if existing.approval_ambiguous_seen {
            return Err(StabilityPoolError::SystemBusy);
        } else {
            match submit_sp_v2_approval(existing.request.request_id).await {
                Ok(request) => request,
                Err(_) => return Ok(existing.request.request_id),
            }
        };
        let _ = dispatch_or_resume_sp_liquidation_v2(request.clone()).await;
        let _ = recover_sp_liquidation_v2(request.request_id).await;
        return Ok(request.request_id);
    }

    if !sp_liquidation_v2_token_supported(token) {
        return Err(StabilityPoolError::SystemBusy);
    }

    // An old V1 marker survives upgrades and has no V2 receipt binding. Do not
    // admit a second liquidation for that vault while it remains unresolved.
    if read_state(|state| state.in_flight_liquidations.contains(&vault_id)) {
        return Err(StabilityPoolError::SystemBusy);
    }
    // The backend executor is fail-closed pending real-ledger receipt proof.
    // This gate is deliberately after the existing-row branch so recovery,
    // status, and ACK handling for durable rows continue to work.
    if !SP_LIQUIDATION_V2_ADMISSION_ENABLED {
        return Err(StabilityPoolError::SystemBusy);
    }

    let _guard = crate::pool_guard::SpLiquidationGuard::new()?;
    let protocol = read_state(|state| state.protocol_canister_id);
    let (vaults,): (Vec<rumi_protocol_backend::vault::CandidVault>,) =
        call(protocol, "get_liquidatable_vaults", ())
            .await
            .map_err(|_| StabilityPoolError::InterCanisterCallFailed {
                target: "Protocol".into(),
                method: "get_liquidatable_vaults".into(),
            })?;
    let vault = vaults
        .into_iter()
        .find(|vault| vault.vault_id == vault_id)
        .ok_or(StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: "vault is not currently liquidatable".into(),
        })?;
    let (collateral_ok, token_ledger, stable_config) = read_state(|state| {
        let collateral_ok = !state.collateral_requires_payout_address(&vault.collateral_type)
            && !state.is_chain_collateral_sentinel(&vault.collateral_type)
            && state
                .collateral_registry
                .get(&vault.collateral_type)
                .is_some_and(|collateral| collateral.status == CollateralStatus::Active);
        let expected_symbol = match token {
            SpLiquidationToken::IcUsd => "icUSD",
            SpLiquidationToken::CKUSDT => "ckUSDT",
            SpLiquidationToken::CKUSDC => "ckUSDC",
        };
        let stable = state
            .stablecoin_registry
            .values()
            .find(|config| {
                config.symbol == expected_symbol
                    && config.is_active
                    && !config.is_lp_token.unwrap_or(false)
            })
            .cloned();
        (
            collateral_ok,
            stable.as_ref().map(|config| config.ledger_id),
            stable,
        )
    });
    if !collateral_ok {
        return Err(StabilityPoolError::SystemBusy);
    }
    let config = stable_config.ok_or(StabilityPoolError::SystemBusy)?;
    let ledger = token_ledger.ok_or(StabilityPoolError::SystemBusy)?;
    if crate::pool_balance_mutation_blocked_for_ledger(ledger) {
        return Err(StabilityPoolError::SystemBusy);
    }
    let fee = query_exact_ledger_fee(ledger).await?;
    let (allowance, required_pool_balance) = sp_v2_approval_preflight_amounts(
        max_principal_pull_raw,
        fee,
        0, // verified icUSD minting-account pull is a fee-free burn
    )
    .ok_or(StabilityPoolError::SystemBusy)?;
    let available = read_state(|state| {
        state.available_stablecoin_for_collateral(ledger, &vault.collateral_type)
    })?;
    if available < required_pool_balance {
        return Err(StabilityPoolError::InsufficientPoolBalance);
    }
    let created_at =
        mutate_state(|state| state.allocate_outbound_payout_timestamp(ic_cdk::api::time()))?;
    let expires_at = created_at
        .checked_add(300_000_000_000)
        .ok_or(StabilityPoolError::SystemBusy)?;
    let approval = make_sp_v2_approval_tuple(
        ledger,
        ic_cdk::api::id(),
        protocol,
        allowance,
        fee,
        vault_id,
        created_at,
        expires_at,
    );
    let request_id = mutate_state(|state| {
        state
            .prepare_sp_liquidation_v2(
                vault_id,
                vault.collateral_type,
                0,
                ledger,
                max_principal_pull_raw,
                token,
                approval,
            )
            .map(|row| row.request.request_id)
    })?;
    let request = match submit_sp_v2_approval(request_id).await {
        Ok(request) => request,
        Err(_) => return Ok(request_id),
    };
    let _ = dispatch_or_resume_sp_liquidation_v2(request.clone()).await;
    let _ = recover_sp_liquidation_v2(request_id).await;
    Ok(request_id)
}

pub async fn retry_sp_liquidation_v2(request_id: u64) -> Result<(), StabilityPoolError> {
    if ic_cdk::api::caller() == Principal::anonymous() {
        return Err(StabilityPoolError::Unauthorized);
    }
    if let Some(row) = read_state(|state| state.pending_sp_liquidation_v2(request_id)) {
        execute_liquidation_v2(row.request.vault_id, row.request.token, row.request.amount)
            .await
            .map(|_| ())
    } else {
        recover_sp_liquidation_v2(request_id).await
    }
}

pub async fn reconcile_sp_liquidation_v2_approval(
    request_id: u64,
    block_index: u64,
) -> Result<(), StabilityPoolError> {
    if ic_cdk::api::caller() == Principal::anonymous() {
        return Err(StabilityPoolError::Unauthorized);
    }
    let _guard = crate::pool_guard::SpLiquidationGuard::new_v2_resume(request_id)?;
    let row = read_state(|state| state.pending_sp_liquidation_v2(request_id))
        .ok_or(StabilityPoolError::SystemBusy)?;
    if row.backend_request.is_some()
        || row.approval.fee_accounted
        || row.approval_dispatch_in_flight
        || !row.approval_ambiguous_seen
        || row
            .approval_candidate_block_index
            .is_some_and(|saved| saved != block_index)
    {
        return Err(StabilityPoolError::SystemBusy);
    }
    let receipt = approval_receipt_for_row(&row, block_index);
    verify_sp_liquidation_v2_approval_receipt(&receipt).await?;
    // Candidate indexes are caller supplied. Persist/account one only after
    // the ledger has proved the exact immutable approval tuple, and ensure the
    // row did not change while the archive-aware proof was fetched.
    if read_state(|state| state.pending_sp_liquidation_v2(request_id)) != Some(row) {
        return Err(StabilityPoolError::SystemBusy);
    }
    mutate_state(|state| state.account_sp_liquidation_v2_approval_fee(request_id, receipt))?;
    let request = read_state(|state| {
        state
            .pending_sp_liquidation_v2(request_id)
            .and_then(|saved| saved.backend_request)
    })
    .ok_or(StabilityPoolError::SystemBusy)?;
    dispatch_or_resume_sp_liquidation_v2(request).await?;
    recover_sp_liquidation_v2(request_id).await
}

/// Attach an operator-discovered ICRC-3 block to a stable pull whose dispatch
/// had an ambiguous outcome. The backend remains the authoritative verifier;
/// this canister independently checks the same exact tuple before persisting
/// the candidate so a lost backend reply can be retried idempotently.
pub async fn reconcile_sp_liquidation_v2_stable_pull(
    request_id: u64,
    vault_id: u64,
    block_index: u64,
) -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    if caller == Principal::anonymous() || !read_state(|state| state.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    let _guard = crate::pool_guard::SpLiquidationGuard::new_v2_resume(request_id)?;
    let row = read_state(|state| state.pending_sp_liquidation_v2(request_id))
        .ok_or(StabilityPoolError::SystemBusy)?;
    if row.request.vault_id != vault_id
        || !row.ambiguous_seen
        || row.stable_debit_applied
        || row.stable_pull_receipt.is_some()
        || row
            .stable_pull_candidate_block_index
            .is_some_and(|saved| saved != block_index)
    {
        return Err(StabilityPoolError::SystemBusy);
    }
    let request = row
        .backend_request
        .clone()
        .ok_or(StabilityPoolError::SystemBusy)?;
    let protocol = read_state(|state| state.protocol_canister_id);
    let (status,): (Result<SpLiquidationV2StatusView, rumi_protocol_backend::ProtocolError>,) =
        call(
            protocol,
            "get_stability_pool_liquidation_v2_status",
            (request_id,),
        )
        .await
        .map_err(|_| StabilityPoolError::InterCanisterCallFailed {
            target: "Protocol".into(),
            method: "get_stability_pool_liquidation_v2_status".into(),
        })?;
    let view = status.map_err(|_| StabilityPoolError::SystemBusy)?;
    let tuple = match view.status {
        SpLiquidationV2Status::StablePullPending {
            tuple,
            candidate_block_index,
            ..
        } if view.stability_pool == ic_cdk::api::id()
            && view.request_id == request_id
            && view.request.as_ref() == Some(&request)
            && candidate_block_index.is_none_or(|saved| saved == block_index) =>
        {
            tuple
        }
        _ => return Err(StabilityPoolError::SystemBusy),
    };
    if tuple.ledger != row.stablecoin_ledger
        || tuple.from != row.approval.owner
        || tuple.spender != row.approval.spender
        || tuple.to != tuple.spender
        || tuple.amount_raw == 0
        || tuple.amount_raw > row.request.amount
        || tuple
            .amount_raw
            .checked_add(tuple.fee_raw)
            .is_none_or(|total| total > request.approval.tuple.allowance_raw)
    {
        return Err(StabilityPoolError::SystemBusy);
    }
    let receipt = SpLiquidationStablePullReceipt {
        block_index,
        tuple: SpLiquidationStablePullTuple {
            op_nonce: tuple.op_nonce,
            ledger: tuple.ledger,
            from: tuple.from.clone(),
            spender: tuple.spender.clone(),
            to: tuple.to.clone(),
            amount_raw: tuple.amount_raw,
            fee_raw: tuple.fee_raw,
            memo: tuple.memo.clone(),
            created_at_time_ns: tuple.created_at_time_ns,
        },
    };
    verify_sp_liquidation_v2_stable_receipt(&receipt, request.token).await?;
    if read_state(|state| state.pending_sp_liquidation_v2(request_id)) != Some(row.clone()) {
        return Err(StabilityPoolError::SystemBusy);
    }
    mutate_state(|state| {
        state.record_sp_liquidation_v2_stable_pull_candidate(
            request_id,
            receipt.tuple.clone(),
            Some(block_index),
        )
    })?;

    let (attached,): (Result<(), rumi_protocol_backend::ProtocolError>,) = call(
        protocol,
        "attach_stability_pool_liquidation_v2_receipt",
        (
            request_id,
            rumi_protocol_backend::SpLiquidationV2ReceiptKind::StablePull,
            block_index,
        ),
    )
    .await
    .map_err(|_| StabilityPoolError::InterCanisterCallFailed {
        target: "Protocol".into(),
        method: "attach_stability_pool_liquidation_v2_receipt".into(),
    })?;
    attached.map_err(|_| StabilityPoolError::SystemBusy)?;
    recover_sp_liquidation_v2(request_id).await
}

/// Conservative fallback for a collateral ledger's transfer fee, used only when
/// the live `icrc1_fee` query fails (SP-104). Set to the common ICRC fee
/// (10_000 e8s, as on ICP/ckBTC-class ledgers). Over-estimating the fee
/// under-credits depositors slightly (solvency-safe) rather than over-crediting
/// them as a fee=0 fallback would. The next successful liquidation reconciles.
/// Shared with `claim_collateral`'s fee lookup (ICRC-004 / SP-203).
pub(crate) const FALLBACK_COLLATERAL_FEE_E8S: u64 = 10_000;

#[derive(Clone, Debug)]
pub(crate) struct IcusdBurnAttemptError {
    pub error: StabilityPoolError,
    pub definitive_no_effect: bool,
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

pub(crate) const CHAIN_WRITEDOWN_MEMO_PREFIX: &[u8] = b"RUMI-LIQ-004:";

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

/// Validate a ledger-history hit against the complete persisted burn identity.
/// The canonical icUSD ledger exposes ICRC-1 burns as `1burn`; requiring that
/// type prevents a transfer or ICRC-2 operation from being mistaken for the
/// pool's burn.
pub(crate) fn validate_icusd_burn_reconciliation_block(
    block: &rumi_protocol_backend::icrc3_proof::DecodedBlock,
    sp_principal: Principal,
    minting_account: Account,
    amount_e8s: u64,
    vault_id: u64,
    created_at_time_ns: u64,
) -> Result<(), String> {
    // The pinned icUSD ledger's ICRC-3 blocks use the legacy `op=burn`
    // shape without top-level `btype`; standard ledgers use `1burn`. Both
    // identify a burn, while any other explicit block type remains rejected.
    if block.op != "burn" || block.btype.as_deref().is_some_and(|btype| btype != "1burn") {
        return Err(format!(
            "block is not a canonical ICRC-1 burn (btype={:?}, op={})",
            block.btype, block.op
        ));
    }
    if block.from.as_ref().map_or(true, |account| {
        account.owner != sp_principal || account.subaccount.unwrap_or([0; 32]) != [0; 32]
    }) {
        return Err("burn source is not the stability pool default account".into());
    }
    if block.to.is_some() || block.spender.is_some() {
        return Err("burn has an unexpected destination or spender".into());
    }
    if block.amount != amount_e8s as u128 {
        return Err("burn amount does not match the pending intent".into());
    }
    if block.memo.as_deref() != Some(encode_chain_writedown_memo(vault_id).as_slice()) {
        return Err("burn memo does not match the pending vault".into());
    }
    if block.created_at_time != Some(created_at_time_ns) {
        return Err("burn timestamp does not match the pending intent".into());
    }
    // The minting account is accepted as part of the caller's captured
    // identity for symmetry with ledger configuration, but an ICRC-1 burn
    // block has no destination to compare against it.
    let _ = minting_account;
    Ok(())
}

fn expected_sp_burn_refund_memo(burn_block_index: u64, vault_id: u64) -> Vec<u8> {
    let mut memo = b"RSPRFND:".to_vec();
    memo.extend_from_slice(&burn_block_index.to_be_bytes());
    memo.extend_from_slice(&vault_id.to_be_bytes());
    memo
}

/// Accept the original refund identity and backend retry identities derived
/// from it. The backend appends a nonzero big-endian ordinal after a proven
/// TooOld rejection; all other memo shapes remain invalid.
fn sp_burn_refund_memo_matches(memo: &[u8], burn_block_index: u64, vault_id: u64) -> bool {
    let expected = expected_sp_burn_refund_memo(burn_block_index, vault_id);
    if memo == expected {
        return true;
    }
    if memo.len() != expected.len() + 8 || !memo.starts_with(&expected) {
        return false;
    }
    u64::from_be_bytes(
        memo[expected.len()..]
            .try_into()
            .expect("validated eight-byte retry ordinal"),
    ) != 0
}

fn validate_sp_burn_refund_receipt(
    intent: &ChainSpAbsorbIntent,
    receipt: &rumi_protocol_backend::sp_burn_refund::SpBurnRefundReceipt,
    sp_principal: Principal,
) -> Result<(), StabilityPoolError> {
    let proof = intent.burn_proof.as_ref();
    if receipt.vault_id != intent.vault_id
        || receipt.amount_e8s != intent.icusd_to_burn_e8s
        || receipt.ledger != intent.icusd_ledger
        || receipt.recipient != sp_principal
        || receipt.burn_block_index != proof.map(|proof| proof.block_index).unwrap_or(u64::MAX)
        || !sp_burn_refund_memo_matches(
            &receipt.refund_memo,
            receipt.burn_block_index,
            intent.vault_id,
        )
    {
        return Err(StabilityPoolError::LedgerTransferFailed {
            reason: "backend burn refund receipt does not match the exact pending chain intent"
                .into(),
        });
    }
    Ok(())
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
            reason: "backend burn refund receipt does not match original icUSD burn".into(),
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
    .map_err(|reason| StabilityPoolError::LedgerTransferFailed {
        reason: format!("icUSD refund block failed independent verification: {reason}"),
    })?;
    Ok(receipt)
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

pub(crate) async fn burn_icusd_for_chain_writedown_with_account(
    icusd_ledger: Principal,
    minting_account: Account,
    amount_e8s: u64,
    vault_id: u64,
    created_at_time: u64,
) -> Result<rumi_protocol_backend::icrc3_proof::SpWritedownProof, IcusdBurnAttemptError> {
    if amount_e8s == 0 {
        return Err(IcusdBurnAttemptError::definite(
            StabilityPoolError::AmountTooLow { minimum_e8s: 1 },
        ));
    }
    let transfer_arg =
        build_icusd_burn_transfer_arg(minting_account, amount_e8s, vault_id, created_at_time);

    let result: Result<(Result<Nat, TransferError>,), _> =
        call(icusd_ledger, "icrc1_transfer", (transfer_arg,)).await;

    let block_index = match result {
        Ok((Ok(block_index),)) => {
            nat_block_index_to_u64(block_index).map_err(IcusdBurnAttemptError::ambiguous)?
        }
        Ok((Err(TransferError::Duplicate { duplicate_of }),)) => {
            nat_block_index_to_u64(duplicate_of).map_err(IcusdBurnAttemptError::ambiguous)?
        }
        Ok((Err(error),)) => {
            let failure = StabilityPoolError::LedgerTransferFailed {
                reason: format!("{:?}", error),
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
                    target: format!("{}", icusd_ledger),
                    method: "icrc1_transfer".to_string(),
                },
            ));
        }
    };

    Ok(build_icusd_burn_proof(block_index, vault_id))
}

pub(crate) async fn burn_icusd_for_chain_writedown(
    icusd_ledger: Principal,
    amount_e8s: u64,
    vault_id: u64,
) -> Result<rumi_protocol_backend::icrc3_proof::SpWritedownProof, IcusdBurnAttemptError> {
    let minting_account = fetch_icusd_minting_account(icusd_ledger)
        .await
        .map_err(IcusdBurnAttemptError::definite)?;
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
    intent.status = NativeXrpAbsorbIntentStatus::Burned;
    intent.last_error = None;
    intent.updated_at_ns = now_ns;
    state.put_pending_native_xrp_absorb(intent.clone())?;
    Ok(intent)
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
    let safe_to_release = mutate_state(|s| match s.get_pending_native_xrp_absorb(vault_id) {
        None => true,
        Some(_) => clear_unburned_native_xrp_absorb_intent_in_state(s, vault_id),
    });
    if safe_to_release {
        io.release_xrp_absorb_preflight(protocol_id, vault_id, icusd_burn_e8s)
            .await;
    }
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

pub(crate) fn mark_native_xrp_absorb_burn_attempted_in_state(
    state: &mut StabilityPoolState,
    vault_id: u64,
    now_ns: u64,
) -> Result<NativeXrpAbsorbIntent, StabilityPoolError> {
    let mut intent = state
        .get_pending_native_xrp_absorb(vault_id)
        .ok_or_else(|| StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: "missing pending native XRP intent".into(),
        })?;
    match intent.burn_attempted {
        Some(false) => intent.burn_attempted = Some(true),
        Some(true) => {},
        None => return Err(StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: "legacy prepared native XRP burn has unknown dispatch history; held for ledger evidence".into(),
        }),
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
    let same_first_attempt = expected.burn_attempted == Some(false)
        && current.burn_attempted == Some(true)
        && current.burn_proof.is_none()
        && current.backend_result.is_none()
        && current.icusd_ledger == expected.icusd_ledger
        && current.icusd_to_burn_e8s == expected.icusd_to_burn_e8s
        && current.burn_created_at_time_ns == expected.burn_created_at_time_ns;
    if !same_first_attempt {
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
    let matches = current.vault_id == expected.vault_id
        && current.icusd_ledger == expected.icusd_ledger
        && current.icusd_to_burn_e8s == expected.icusd_to_burn_e8s
        && current.burn_created_at_time_ns == expected.burn_created_at_time_ns
        && current.burn_proof == expected.burn_proof
        && current.backend_result.is_none();
    if matches && current.burn_proof.is_some() {
        state.take_pending_native_xrp_absorb(expected.vault_id);
        true
    } else {
        false
    }
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

    async fn refund_and_verify_burn(
        &mut self,
        intent: &NativeXrpAbsorbIntent,
    ) -> Result<rumi_protocol_backend::sp_burn_refund::SpBurnRefundReceipt, StabilityPoolError>
    {
        let _ = intent;
        Err(StabilityPoolError::LiquidationFailed {
            vault_id: 0,
            reason: "exact SP burn refund is unavailable in this driver".into(),
        })
    }

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
        burn_icusd_for_chain_writedown_with_account(
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

    async fn refund_and_verify_burn(
        &mut self,
        intent: &NativeXrpAbsorbIntent,
    ) -> Result<rumi_protocol_backend::sp_burn_refund::SpBurnRefundReceipt, StabilityPoolError>
    {
        let proof =
            intent
                .burn_proof
                .clone()
                .ok_or_else(|| StabilityPoolError::LiquidationFailed {
                    vault_id: intent.vault_id,
                    reason: "cannot refund a burn without its exact proof".into(),
                })?;
        refund_and_verify_sp_burn(
            read_state(|s| s.protocol_canister_id),
            intent.vault_id,
            intent.icusd_to_burn_e8s,
            intent.icusd_ledger,
            proof,
        )
        .await
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

fn collateral_fee_from_nat(fee: candid::Nat) -> Result<u64, ()> {
    fee.0.try_into().map_err(|_| ())
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

async fn compensate_rejected_native_xrp_absorb(
    intent: &NativeXrpAbsorbIntent,
    absorb_error: StabilityPoolError,
    io: &mut dyn NativeXrpAbsorbIo,
) -> StabilityPoolError {
    match io.refund_and_verify_burn(intent).await {
        Ok(receipt) => {
            let proof = intent.burn_proof.as_ref();
            let matches = receipt.vault_id == intent.vault_id
                && receipt.amount_e8s == intent.icusd_to_burn_e8s
                && receipt.ledger == intent.icusd_ledger
                && receipt.recipient == ic_cdk::id()
                && receipt.burn_block_index == proof.map(|proof| proof.block_index).unwrap_or(u64::MAX)
                && sp_burn_refund_memo_matches(
                    &receipt.refund_memo,
                    receipt.burn_block_index,
                    intent.vault_id,
                );
            if !matches {
                return StabilityPoolError::LiquidationFailed {
                    vault_id: intent.vault_id,
                    reason: "verified refund receipt does not match original native XRP burn; intent remains held".into(),
                };
            }
            if mutate_state(|s| clear_refunded_native_xrp_absorb_in_state(s, intent)) {
                StabilityPoolError::LiquidationFailed {
                    vault_id: intent.vault_id,
                    reason: format!(
                        "icUSD burn was refunded in verified ledger block {}; no liquidation or depositor loss was applied (absorb result: {:?})",
                        receipt.refund_block_index, absorb_error
                    ),
                }
            } else {
                StabilityPoolError::LiquidationFailed {
                    vault_id: intent.vault_id,
                    reason: "refund was verified but pending native XRP intent changed; reconciliation required".into(),
                }
            }
        }
        Err(refund_error) => StabilityPoolError::LiquidationFailed {
            vault_id: intent.vault_id,
            reason: format!(
                "native XRP absorb failed ({absorb_error:?}) and exact burn refund remains pending ({refund_error:?})"
            ),
        },
    }
}

pub(crate) async fn execute_native_xrp_absorb_with_io(
    vault_info: &LiquidatableVaultInfo,
    io: &mut dyn NativeXrpAbsorbIo,
) -> LiquidationResult {
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
                Err(error) => {
                    let compensated =
                        compensate_rejected_native_xrp_absorb(&intent, error, io).await;
                    return liquidation_failure(vault_info, compensated);
                }
            };
            return match mutate_state(|s| {
                apply_native_xrp_absorb_success_in_state_at(s, &accepted, io.now_ns())
            }) {
                Ok(result) => result,
                Err(error) => liquidation_failure(vault_info, error),
            };
        }
        if intent.burn_attempted.is_none() {
            return liquidation_failure(vault_info, StabilityPoolError::LiquidationFailed {
                vault_id: vault_info.vault_id,
                reason: "legacy prepared native XRP burn has unknown dispatch history; held for exact ledger evidence".into(),
            });
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
        abandon_unburned_native_xrp_absorb(io, protocol_id, vault_info.vault_id, icusd_to_burn_e8s)
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
                        )
                    });
                    false
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
        Err(error) => {
            let compensated = compensate_rejected_native_xrp_absorb(&intent, error, io).await;
            return liquidation_failure(vault_info, compensated);
        }
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
        burn_attempted: Some(false),
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
        && intent.burn_attempted == Some(false)
        && intent.burn_proof.is_none()
        && intent.backend_result.is_none()
    {
        state.take_pending_chain_absorb(vault_id);
        return true;
    }
    false
}

pub(crate) fn mark_chain_absorb_burn_attempted_in_state(
    state: &mut StabilityPoolState,
    vault_id: u64,
    now_ns: u64,
) -> Result<ChainSpAbsorbIntent, StabilityPoolError> {
    let mut intent = state.get_pending_chain_absorb(vault_id).ok_or_else(|| {
        StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: "missing pending chain absorb intent".into(),
        }
    })?;
    match intent.burn_attempted {
        Some(false) => intent.burn_attempted = Some(true),
        Some(true) => {}
        None => return Err(StabilityPoolError::LiquidationFailed {
            vault_id,
            reason:
                "legacy prepared chain burn has unknown dispatch history; held for ledger evidence"
                    .into(),
        }),
    }
    intent.updated_at_ns = now_ns;
    state.put_pending_chain_absorb(intent.clone())?;
    Ok(intent)
}

fn cancel_first_definitive_chain_burn_failure_in_state(
    state: &mut StabilityPoolState,
    expected: &ChainSpAbsorbIntent,
) -> bool {
    let Some(mut current) = state.get_pending_chain_absorb(expected.vault_id) else {
        return false;
    };
    let same_first_attempt = expected.burn_attempted == Some(false)
        && current.burn_attempted == Some(true)
        && current.burn_proof.is_none()
        && current.backend_result.is_none()
        && current.icusd_ledger == expected.icusd_ledger
        && current.icusd_to_burn_e8s == expected.icusd_to_burn_e8s
        && current.burn_created_at_time_ns == expected.burn_created_at_time_ns;
    if !same_first_attempt {
        return false;
    }
    current.burn_attempted = Some(false);
    if state.put_pending_chain_absorb(current).is_err() {
        return false;
    }
    clear_unburned_chain_absorb_intent_in_state(state, expected.vault_id)
}

pub(crate) fn clear_refunded_chain_absorb_in_state(
    state: &mut StabilityPoolState,
    expected: &ChainSpAbsorbIntent,
) -> bool {
    let Some(current) = state.get_pending_chain_absorb(expected.vault_id) else {
        return false;
    };
    let matches = current.vault_id == expected.vault_id
        && current.icusd_ledger == expected.icusd_ledger
        && current.icusd_to_burn_e8s == expected.icusd_to_burn_e8s
        && current.burn_created_at_time_ns == expected.burn_created_at_time_ns
        && current.burn_proof == expected.burn_proof
        && current.backend_result.is_none();
    if matches && current.burn_proof.is_some() {
        state.take_pending_chain_absorb(expected.vault_id);
        true
    } else {
        false
    }
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
    if !state
        .can_process_chain_liquidation_debits(vault.chain_collateral_sentinel, &stables_consumed)
    {
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

    state.process_chain_liquidation_gains_at(
        plan.vault_id,
        plan.chain_sentinel,
        &plan.stables_consumed,
        absorbed.collateral_received_native,
        absorbed.collateral_price_e8s,
        timestamp,
    )?;
    state.record_chain_claim_source(
        plan.chain_sentinel,
        absorbed.claim_id,
        absorbed.collateral_received_native,
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

    if read_state(|s| s.in_flight_liquidations.contains(&vault_id)) {
        return Err(StabilityPoolError::SystemBusy);
    }

    // SP-102: hold the per-pool liquidation lock across snapshot -> await ->
    // apportion so deposit/withdraw/claim cannot race the apportionment.
    let _liq_guard = crate::pool_guard::SpLiquidationGuard::new()?;

    // Fetch the backend-priced and backend-sized liquidation payload. The
    // public CandidVault listing omits both fields and cannot safely supply
    // them to receipt-bound absorb paths.
    let protocol_id = read_state(|s| s.protocol_canister_id);
    let (target_info,): (Option<rumi_protocol_backend::LiquidatableVaultInfo>,) =
        call(protocol_id, "get_liquidatable_vault_info", (vault_id,))
            .await
            .map_err(|_e| StabilityPoolError::InterCanisterCallFailed {
                target: "Protocol".to_string(),
                method: "get_liquidatable_vault_info".to_string(),
            })?;
    let target_info = target_info.ok_or_else(|| StabilityPoolError::LiquidationFailed {
        vault_id,
        reason: "Vault not found in liquidatable list".to_string(),
    })?;
    let vault_info = LiquidatableVaultInfo {
        vault_id: target_info.vault_id,
        collateral_type: target_info.collateral_type,
        debt_amount: target_info.debt_amount,
        collateral_amount: target_info.collateral_amount,
        recommended_liquidation_amount: target_info.recommended_liquidation_amount,
        collateral_price_e8s: target_info.collateral_price_e8s,
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

async fn compensate_rejected_chain_absorb(
    intent: &ChainSpAbsorbIntent,
    absorb_error: StabilityPoolError,
) -> StabilityPoolError {
    let Some(proof) = intent.burn_proof.clone() else {
        return StabilityPoolError::LiquidationFailed {
            vault_id: intent.vault_id,
            reason: "chain absorb failed without a persisted burn proof; intent remains held"
                .into(),
        };
    };
    match refund_and_verify_sp_burn(
        read_state(|s| s.protocol_canister_id),
        intent.vault_id,
        intent.icusd_to_burn_e8s,
        intent.icusd_ledger,
        proof,
    )
    .await
    {
        Ok(receipt) => {
            if validate_sp_burn_refund_receipt(intent, &receipt, ic_cdk::id()).is_err() {
                return StabilityPoolError::LiquidationFailed {
                    vault_id: intent.vault_id,
                    reason: "verified refund receipt does not match pending chain intent; obligation remains held".into(),
                };
            }
            if mutate_state(|s| clear_refunded_chain_absorb_in_state(s, intent)) {
                StabilityPoolError::LiquidationFailed {
                    vault_id: intent.vault_id,
                    reason: format!(
                        "icUSD burn was refunded in verified ledger block {}; no liquidation or depositor loss was applied (absorb result: {:?})",
                        receipt.refund_block_index, absorb_error
                    ),
                }
            } else {
                StabilityPoolError::LiquidationFailed {
                    vault_id: intent.vault_id,
                    reason: "refund verified but pending chain intent changed; reconciliation required".into(),
                }
            }
        }
        Err(refund_error) => StabilityPoolError::LiquidationFailed {
            vault_id: intent.vault_id,
            reason: format!(
                "chain absorb failed ({absorb_error:?}) and exact burn refund remains pending ({refund_error:?})"
            ),
        },
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
            let result = match submit_chain_absorb_to_backend(protocol_id, vault_id, &plan, proof)
                .await
            {
                Ok(result) => result,
                Err(error) => return Err(compensate_rejected_chain_absorb(&intent, error).await),
            };
            return mutate_state(|s| apply_chain_absorb_success_in_state(s, &plan, result));
        }
        if intent.burn_attempted.is_none() {
            return Err(StabilityPoolError::LiquidationFailed {
                vault_id,
                reason: "legacy prepared chain burn has unknown dispatch history; held for exact ledger evidence".into(),
            });
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

        let first_attempt = intent.burn_attempted == Some(false);
        let attempt_identity = intent.clone();
        let intent = mutate_state(|s| {
            mark_chain_absorb_burn_attempted_in_state(s, vault_id, ic_cdk::api::time())
        })?;
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
            Err(failure) => {
                if first_attempt && failure.definitive_no_effect {
                    mutate_state(|s| {
                        cancel_first_definitive_chain_burn_failure_in_state(s, &attempt_identity);
                    });
                } else {
                    mutate_state(|s| {
                        mark_chain_absorb_error_in_state(
                            s,
                            vault_id,
                            ChainSpAbsorbIntentStatus::Prepared,
                            format!("ambiguous icUSD burn result: {:?}", failure.error),
                            ic_cdk::api::time(),
                        )
                    });
                }
                let _ = attempt_identity;
                return Err(failure.error);
            }
        };

        let result = match submit_chain_absorb_to_backend(protocol_id, vault_id, &plan, proof).await
        {
            Ok(result) => result,
            Err(error) => {
                let intent =
                    read_state(|s| s.get_pending_chain_absorb(vault_id)).ok_or_else(|| {
                        StabilityPoolError::LiquidationFailed {
                            vault_id,
                            reason: "chain absorb failed and exact burn intent disappeared".into(),
                        }
                    })?;
                return Err(compensate_rejected_chain_absorb(&intent, error).await);
            }
        };
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
        if intent.burn_attempted.is_none() {
            return Err(StabilityPoolError::LiquidationFailed {
                vault_id,
                reason: "legacy prepared chain burn has unknown dispatch history; held for exact ledger evidence".into(),
            });
        }
        let first_attempt = intent.burn_attempted == Some(false);
        let attempt_identity = intent.clone();
        intent = mutate_state(|s| {
            mark_chain_absorb_burn_attempted_in_state(s, vault_id, ic_cdk::api::time())
        })?;
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
            Err(failure) => {
                if first_attempt && failure.definitive_no_effect {
                    mutate_state(|s| {
                        cancel_first_definitive_chain_burn_failure_in_state(s, &attempt_identity);
                    });
                } else {
                    mutate_state(|s| {
                        mark_chain_absorb_error_in_state(
                            s,
                            vault_id,
                            ChainSpAbsorbIntentStatus::Prepared,
                            format!("ambiguous icUSD burn result: {:?}", failure.error),
                            ic_cdk::api::time(),
                        )
                    });
                }
                let _ = attempt_identity;
                return Err(failure.error);
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

/// Retry only retained chain intents with an exact burn proof. The normal
/// core path reuses that proof for absorption or proof-backed compensation;
/// no new burn can be dispatched by this recovery tick.
pub(crate) async fn run_chain_burn_recovery_tick(max_per_tick: usize) -> usize {
    let pending = read_state(|state| state.pending_chain_absorbs());
    let cursor = CHAIN_BURN_RECOVERY_CURSOR.with(Cell::get);
    let batch = select_chain_burn_recovery_batch(&pending, cursor, max_per_tick);
    if let Some(last) = batch.last().copied() {
        CHAIN_BURN_RECOVERY_CURSOR.with(|value| value.set(last));
    }

    for vault_id in &batch {
        if let Err(error) = sp_absorb_chain_vault_core(*vault_id).await {
            log!(
                INFO,
                "chain burn recovery {} remains pending: {:?}",
                vault_id,
                error
            );
        }
    }
    batch.len()
}

fn select_chain_burn_recovery_batch(
    pending: &[ChainSpAbsorbIntent],
    cursor: u64,
    limit: usize,
) -> Vec<u64> {
    if limit == 0 {
        return Vec::new();
    }
    let mut eligible = pending
        .iter()
        .filter(|intent| intent.burn_proof.is_some())
        .map(|intent| intent.vault_id)
        .collect::<Vec<_>>();
    eligible.sort_unstable();
    let mut batch = eligible
        .iter()
        .copied()
        .filter(|vault_id| *vault_id > cursor)
        .take(limit)
        .collect::<Vec<_>>();
    if batch.len() < limit {
        batch.extend(
            eligible
                .iter()
                .copied()
                .filter(|vault_id| *vault_id <= cursor)
                .take(limit - batch.len()),
        );
    }
    batch
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LegacyLiquidationDispatch {
    NativeXrpAbsorb,
    DedicatedChainAbsorb,
    DisabledGenericIcrc,
}

fn legacy_liquidation_dispatch(
    state: &StabilityPoolState,
    collateral: &Principal,
) -> LegacyLiquidationDispatch {
    if state.collateral_requires_payout_address(collateral) {
        LegacyLiquidationDispatch::NativeXrpAbsorb
    } else if state.is_chain_collateral_sentinel(collateral) {
        LegacyLiquidationDispatch::DedicatedChainAbsorb
    } else {
        LegacyLiquidationDispatch::DisabledGenericIcrc
    }
}

fn legacy_generic_icrc_liquidation_enabled() -> bool {
    // Re-enable only after the receipt-bound allocation path is deployed and
    // its official-ledger proof tests pass.
    false
}

async fn submit_legacy_approval_with_receipt(
    vault_id: u64,
    ledger: Principal,
    amount: u64,
    protocol: Principal,
) -> Result<u64, StabilityPoolError> {
    let existing = read_state(|state| state.pending_sp_legacy_approval_fee(vault_id, ledger));
    let row = if let Some(row) = existing {
        row
    } else {
        ensure_ledger_has_icrc3_approval_blocks(ledger).await?;
        let fee = crate::deposits::fresh_ledger_transfer_fee(ledger)
            .await
            .ok_or_else(|| StabilityPoolError::LedgerTransferFailed {
                reason: "could not query a representable fresh fee before legacy SP approval"
                    .into(),
            })?;
        let allowance = amount
            .checked_mul(2)
            .ok_or(StabilityPoolError::SystemBusy)?;
        if !read_state(|state| state.can_deduct_fee_from_pool(ledger, fee)) {
            return Err(StabilityPoolError::InsufficientPoolBalance);
        }
        let created_at_time_ns =
            mutate_state(|state| state.allocate_outbound_payout_timestamp(ic_cdk::api::time()))?;
        let expires_at_ns = created_at_time_ns
            .checked_add(300_000_000_000)
            .ok_or(StabilityPoolError::SystemBusy)?;
        let memo = format!("sp-legacy-approval/{vault_id}/{created_at_time_ns}").into_bytes();
        let row = PendingSpLegacyApprovalFee {
            vault_id,
            approval: SpLiquidationApprovalTuple {
                ledger,
                owner: default_account(ic_cdk::api::id()),
                spender: default_account(protocol),
                allowance_raw: allowance,
                fee_raw: fee,
                memo,
                created_at_time_ns,
                expires_at_ns,
            },
            dispatch_in_flight: false,
            ambiguous_seen: false,
        };
        mutate_state(|state| state.begin_sp_legacy_approval_fee(row))?
    };
    let tuple = &row.approval;
    if tuple.ledger != ledger
        || tuple.owner != default_account(ic_cdk::api::id())
        || tuple.spender != default_account(protocol)
        || tuple.allowance_raw
            != amount
                .checked_mul(2)
                .ok_or(StabilityPoolError::SystemBusy)?
    {
        return Err(StabilityPoolError::SystemBusy);
    }
    if !read_state(|state| state.can_deduct_fee_from_pool(ledger, tuple.fee_raw)) {
        return Err(StabilityPoolError::InsufficientPoolBalance);
    }

    let previously_ambiguous = row.ambiguous_seen || row.dispatch_in_flight;
    let args = ApproveArgs {
        from_subaccount: None,
        spender: tuple.spender.clone(),
        amount: Nat::from(tuple.allowance_raw),
        expected_allowance: None,
        expires_at: Some(tuple.expires_at_ns),
        fee: Some(Nat::from(tuple.fee_raw)),
        memo: Some(Memo::from(tuple.memo.clone())),
        created_at_time: Some(tuple.created_at_time_ns),
    };
    mutate_state(|state| state.mark_sp_legacy_approval_dispatch(vault_id, ledger, true, false))?;
    let result: Result<(Result<Nat, ApproveError>,), _> =
        call(ledger, "icrc2_approve", (args,)).await;
    let block_index = match result {
        Ok((Ok(index),)) => index.0.to_u64().ok_or(StabilityPoolError::SystemBusy)?,
        Ok((Err(ApproveError::Duplicate { duplicate_of }),)) => duplicate_of
            .0
            .to_u64()
            .ok_or(StabilityPoolError::SystemBusy)?,
        Ok((Err(error),)) => {
            let typed_no_effect = matches!(
                error,
                ApproveError::BadFee { .. }
                    | ApproveError::InsufficientFunds { .. }
                    | ApproveError::AllowanceChanged { .. }
                    | ApproveError::Expired { .. }
                    | ApproveError::TooOld
                    | ApproveError::CreatedInFuture { .. }
            );
            if matches!(&error, ApproveError::BadFee { .. }) {
                crate::deposits::invalidate_cached_ledger_fee_after_approve_bad_fee(ledger);
            }
            if typed_no_effect && !previously_ambiguous {
                mutate_state(|state| {
                    state.mark_sp_legacy_approval_dispatch(vault_id, ledger, false, false)?;
                    state.clear_sp_legacy_approval_fee_after_no_effect(vault_id, ledger)
                })?;
            } else {
                mutate_state(|state| {
                    state.mark_sp_legacy_approval_dispatch(vault_id, ledger, false, true)
                })?;
            }
            return Err(StabilityPoolError::LedgerTransferFailed {
                reason: format!("legacy ICRC-2 approval had unresolved result: {error:?}"),
            });
        }
        Err(error) => {
            mutate_state(|state| {
                state.mark_sp_legacy_approval_dispatch(vault_id, ledger, false, true)
            })?;
            return Err(StabilityPoolError::InterCanisterCallFailed {
                target: ledger.to_text(),
                method: format!("icrc2_approve ({error:?})"),
            });
        }
    };

    let receipt = SpLiquidationApprovalReceipt {
        block_index,
        tuple: tuple.clone(),
    };
    // Keep the row durable until the exact ledger block is verified and fee
    // accounting is committed atomically. A lost reply retries this exact tuple;
    // Duplicate supplies the original block index.
    mutate_state(|state| state.mark_sp_legacy_approval_dispatch(vault_id, ledger, false, true))?;
    verify_sp_liquidation_v2_approval_receipt(&receipt).await?;
    mutate_state(|state| state.account_sp_legacy_approval_fee(vault_id, receipt))?;
    Ok(tuple.fee_raw)
}

#[derive(candid::CandidType, serde::Deserialize)]
struct Icrc3SupportedBlockType {
    block_type: String,
}

async fn ensure_ledger_has_icrc3_approval_blocks(
    ledger: Principal,
) -> Result<(), StabilityPoolError> {
    let supported: Result<(Vec<Icrc3SupportedBlockType>,), _> =
        call(ledger, "icrc3_supported_block_types", ()).await;
    let (supported,) = supported.map_err(|_| StabilityPoolError::LedgerTransferFailed {
        reason: format!("ledger {ledger} does not expose queryable ICRC-3 approval block support"),
    })?;
    if supported.iter().any(|block| block.block_type == "2approve") {
        rumi_protocol_backend::icrc3_proof::icrc3_log_length(ledger)
            .await
            .map_err(|reason| StabilityPoolError::LedgerTransferFailed {
                reason: format!(
                    "ledger {ledger} advertises 2approve blocks but ICRC-3 history query failed: {reason}"
                ),
            })?;
        return Ok(());
    }
    Err(StabilityPoolError::LedgerTransferFailed {
        reason: format!("ledger {ledger} does not advertise ICRC-3 2approve blocks"),
    })
}

pub(crate) async fn recover_sp_legacy_approval_fee(
    vault_id: u64,
    ledger: Principal,
) -> Result<(), StabilityPoolError> {
    let row = read_state(|state| state.pending_sp_legacy_approval_fee(vault_id, ledger))
        .ok_or(StabilityPoolError::SystemBusy)?;
    let amount = row
        .approval
        .allowance_raw
        .checked_div(2)
        .filter(|amount| amount.checked_mul(2) == Some(row.approval.allowance_raw))
        .ok_or(StabilityPoolError::SystemBusy)?;
    let protocol = read_state(|state| state.protocol_canister_id);
    submit_legacy_approval_with_receipt(vault_id, ledger, amount, protocol)
        .await
        .map(|_| ())
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
async fn execute_single_liquidation(vault_info: &LiquidatableVaultInfo) -> LiquidationResult {
    let dispatch =
        read_state(|state| legacy_liquidation_dispatch(state, &vault_info.collateral_type));
    if dispatch == LegacyLiquidationDispatch::NativeXrpAbsorb {
        return execute_native_xrp_absorb_with_io(vault_info, &mut CdkNativeXrpAbsorbIo).await;
    }
    if dispatch == LegacyLiquidationDispatch::DedicatedChainAbsorb {
        return liquidation_failure(
            vault_info,
            StabilityPoolError::LiquidationFailed {
                vault_id: vault_info.vault_id,
                reason: "chain collateral must use the dedicated receipt-reconciled absorb route"
                    .into(),
            },
        );
    }
    let protocol_id = read_state(|s| s.protocol_canister_id);

    // Step 1: Compute token draw
    // Use recommended_liquidation_amount (partial cap) if available, otherwise full debt
    let draw_amount = if vault_info.recommended_liquidation_amount > 0 {
        vault_info.recommended_liquidation_amount
    } else {
        vault_info.debt_amount
    };
    let token_draw = read_state(|s| s.compute_token_draw(draw_amount, &vault_info.collateral_type));

    if token_draw.is_empty() {
        return LiquidationResult {
            vault_id: vault_info.vault_id,
            stables_consumed: BTreeMap::new(),
            collateral_gained: 0,
            collateral_type: vault_info.collateral_type,
            success: false,
            error_message: Some("No stablecoins available for liquidation".to_string()),
        };
    }

    let stablecoin_configs: BTreeMap<Principal, StablecoinConfig> =
        read_state(|s| s.stablecoin_registry.clone());
    if let Some(ledger) = disabled_lp_reserve_route_ledger(&token_draw, &stablecoin_configs) {
        let draw_cap = token_draw.get(&ledger).copied().unwrap_or(0);
        return start_three_usd_absorb_v2(vault_info, ledger, draw_cap).await;
    }

    if !legacy_generic_icrc_liquidation_enabled() {
        return liquidation_failure(
            vault_info,
            StabilityPoolError::LiquidationFailed {
                vault_id: vault_info.vault_id,
                reason: "legacy generic ICRC liquidation is disabled until receipt-bound collateral allocation is available".into(),
            },
        );
    }

    if let Some(ledger) = token_draw
        .keys()
        .find(|ledger| crate::pool_balance_mutation_blocked_for_ledger(**ledger))
    {
        return liquidation_failure(
            vault_info,
            StabilityPoolError::LiquidationFailed {
                vault_id: vault_info.vault_id,
                reason: format!(
                    "stable ledger {} is fenced by an unresolved obligation",
                    ledger
                ),
            },
        );
    }

    log!(
        INFO,
        "Token draw for vault {}: {:?}",
        vault_info.vault_id,
        token_draw
    );

    // Step 2: Process each token in the draw
    let mut total_collateral_gained: u64 = 0;
    let mut actual_consumed: BTreeMap<Principal, u64> = BTreeMap::new();

    // Query the collateral fee before any approve or backend liquidation call.
    // If the fee is outside our accounting range, no stablecoin may be consumed
    // until a durable collateral-liability journal exists for that receipt.
    let collateral_fee = match call::<(), (candid::Nat,)>(
        vault_info.collateral_type,
        "icrc1_fee",
        (),
    )
    .await
    {
        Ok((fee_nat,)) => match collateral_fee_from_nat(fee_nat) {
            Ok(fee) => fee,
            Err(_) => {
                return liquidation_failure(
                    vault_info,
                    StabilityPoolError::LiquidationFailed {
                        vault_id: vault_info.vault_id,
                        reason: "collateral ledger fee exceeds supported u64 range; liquidation held before any ledger or backend mutation".into(),
                    },
                );
            }
        },
        Err(e) => {
            // Preserve the existing conservative fallback when the query is
            // unavailable. This exact value is reused for accounting after the
            // backend call instead of querying a potentially different fee.
            log!(INFO, "icrc1_fee query failed for collateral {} before liquidation: {:?}; using conservative fallback {} e8s",
                vault_info.collateral_type, e, FALLBACK_COLLATERAL_FEE_E8S);
            FALLBACK_COLLATERAL_FEE_E8S
        }
    };

    let icusd_ledger = stablecoin_configs
        .iter()
        .find(|(_, c)| c.symbol == "icUSD")
        .map(|(id, _)| *id);

    // --- Non-LP tokens: approve + liquidate_vault_partial ---
    for (token_ledger, amount) in &token_draw {
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

        // The exact approval tuple is persisted before dispatch and remains
        // fenced until a verified ICRC-3 receipt accounts its fee.
        let ledger_fee = match submit_legacy_approval_with_receipt(
            vault_info.vault_id,
            *token_ledger,
            *amount,
            protocol_id,
        )
        .await
        {
            Ok(fee) => fee,
            Err(error) => return liquidation_failure(vault_info, error),
        };

        // No pre-deduct of depositor balances: `process_liquidation_gains` is the
        // single point of truth for stablecoin bookkeeping on a successful
        // liquidation (SP-001 regression fix, audit 2026-04-22-28e9896). Calling
        // `deduct_burned_lp_from_balances` here previously caused depositor balances
        // and the aggregate total to be decremented twice per liquidation — once
        // pre-call, once inside `process_liquidation_gains_at` — leaving phantom
        // tokens in the pool account per liquidation.

        let principal_draw = BTreeMap::from([(*token_ledger, *amount)]);
        let post_approve_fee_reserve = amount.checked_add(ledger_fee);
        if !read_state(|s| {
            s.can_process_liquidation_debits(vault_info.collateral_type, &principal_draw)
                && post_approve_fee_reserve
                    .map(|reserve| s.can_deduct_fee_from_pool(*token_ledger, reserve))
                    .unwrap_or(false)
        }) {
            log!(INFO, "Skipping backend liquidation for {}: exact principal and transfer fee are not covered by tracked balances", token_ledger);
            continue;
        }

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
                    if let Err(error) =
                        mutate_state(|s| s.deduct_fee_from_pool(*token_ledger, ledger_fee))
                    {
                        return liquidation_failure(vault_info, error);
                    }
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
                // Inter-canister call failed; outcome is unknown. We do NOT mutate
                // depositor bookkeeping here — the previous "conservative deduct" path
                // (SP-005) caused permanent depositor loss when the backend was in
                // fact a no-op. If the backend rolled forward (took the tokens via
                // transfer_from but failed to reply), the next liquidation or a manual
                // `correct_balance` reconciliation against `icrc1_balance_of(pool)`
                // will reconcile the divergence. Log loudly so operators notice.
                log!(
                    INFO,
                    "Liquidation call failed for vault {} with token {}: {:?}. \
                      No bookkeeping change; ledger balance should be reconciled if \
                      tokens moved silently.",
                    vault_info.vault_id,
                    token_ledger,
                    call_error
                );
            }
        }
    }

    // --- LP tokens (3USD): approve + backend pull (atomic) ---
    for (token_ledger, amount) in &token_draw {
        match stablecoin_configs.get(token_ledger) {
            Some(c) if c.is_lp_token.unwrap_or(false) => (),
            _ => continue,
        }

        // Calculate icUSD equivalent using cached virtual price
        let vp = read_state(|s| {
            s.virtual_prices()
                .get(token_ledger)
                .copied()
                .unwrap_or(1_000_000_000_000_000_000)
        });
        let icusd_equiv_e8s = lp_to_usd_e8s(*amount, vp);

        if icusd_equiv_e8s < 10_000_000 {
            log!(
                INFO,
                "Skipping LP token {}: icUSD equivalent {} e8s below backend minimum",
                token_ledger,
                icusd_equiv_e8s
            );
            continue;
        }

        // Step A: approve backend to pull 3USD with the same durable receipt
        // protocol used by the non-LP legacy path.
        if let Err(error) = submit_legacy_approval_with_receipt(
            vault_info.vault_id,
            *token_ledger,
            *amount,
            protocol_id,
        )
        .await
        {
            return liquidation_failure(vault_info, error);
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
            "stability_pool_liquidate_with_reserves",
            (vault_info.vault_id, icusd_equiv_e8s, *amount, *token_ledger),
        )
        .await;

        match liq_result {
            Ok((Ok(success),)) => {
                // VER-002 (audit 2026-06-05): the backend caps the writedown to
                // the vault's current debt and refunds the proportional excess
                // 3USD (see stability_pool_liquidate_with_reserves). Record only
                // the REALIZED 3USD using the SAME floor formula the backend
                // refund uses, so the SP's tracked aggregate and its ledger
                // balance both net to exactly the realized amount (no drift).
                // `icusd_equiv_e8s` here equals the `icusd_debt_covered_e8s` the
                // backend received, so the two formulas are identical.
                let realized_3usd =
                    if icusd_equiv_e8s > 0 && success.liquidated_debt < icusd_equiv_e8s {
                        ((*amount as u128).saturating_mul(success.liquidated_debt as u128)
                            / icusd_equiv_e8s as u128) as u64
                    } else {
                        *amount
                    };
                actual_consumed.insert(*token_ledger, realized_3usd);
                total_collateral_gained += success.collateral_received;
                log!(INFO, "3USD reserves liquidation succeeded for vault {}: {} collateral, {} 3USD consumed (requested {})",
                    vault_info.vault_id, success.collateral_received, realized_3usd, amount);
                break; // one token per vault per round
            }
            Ok((Err(e),)) => {
                // Backend explicitly rejected; approval expires harmlessly and nothing
                // was pre-deducted, so there is no bookkeeping to roll back.
                log!(
                    INFO,
                    "Backend rejected 3USD reserves liquidation for vault {}: {:?}",
                    vault_info.vault_id,
                    e
                );
            }
            Err(e) => {
                // Inter-canister call failed; outcome unknown. We do NOT mutate
                // depositor bookkeeping (SP-005 regression fix). If the backend
                // pulled the 3USD silently, operator reconciliation against
                // `icrc1_balance_of(pool)` will reconcile.
                log!(
                    INFO,
                    "3USD reserves liquidation call failed for vault {}: {:?}. \
                      No bookkeeping change; ledger balance should be reconciled if \
                      tokens moved silently.",
                    vault_info.vault_id,
                    e
                );
            }
        }
    }

    // Record liquidation event
    let stables_consumed_e8s: u64 = actual_consumed.values().sum();
    let liq_success = !actual_consumed.is_empty() && total_collateral_gained > 0;
    mutate_state(|s| {
        s.push_event(
            s.protocol_canister_id,
            PoolEventType::LiquidationExecuted {
                vault_id: vault_info.vault_id,
                stables_consumed_e8s,
                collateral_gained: total_collateral_gained,
                collateral_type: vault_info.collateral_type,
                success: liq_success,
            },
        );
    });

    // Step 3: If any liquidation calls succeeded, process gains
    if !actual_consumed.is_empty() && total_collateral_gained > 0 {
        // Deduct the collateral ledger's transfer fee from gains — the backend reports
        // gross collateral but the transfer to the SP deducts one fee.
        // This is the preflight fee used before dispatch. Never turn an
        // unrepresentable post-dispatch fee into zero collateral credit: the
        // preflight rejects that state before irreversible backend work.
        let net_collateral = total_collateral_gained.saturating_sub(collateral_fee);

        if let Err(error) = mutate_state(|s| {
            s.process_liquidation_gains(
                vault_info.vault_id,
                vault_info.collateral_type,
                &actual_consumed,
                net_collateral,
                vault_info.collateral_price_e8s,
            )
        }) {
            return liquidation_failure(vault_info, error);
        }

        LiquidationResult {
            vault_id: vault_info.vault_id,
            stables_consumed: actual_consumed,
            collateral_gained: net_collateral,
            collateral_type: vault_info.collateral_type,
            success: true,
            error_message: None,
        }
    } else {
        LiquidationResult {
            vault_id: vault_info.vault_id,
            stables_consumed: BTreeMap::new(),
            collateral_gained: 0,
            collateral_type: vault_info.collateral_type,
            success: false,
            error_message: Some("All liquidation calls failed".to_string()),
        }
    }
}

fn disabled_lp_reserve_route_ledger(
    token_draw: &BTreeMap<Principal, u64>,
    configs: &BTreeMap<Principal, StablecoinConfig>,
) -> Option<Principal> {
    token_draw.keys().find_map(|ledger| {
        configs
            .get(ledger)
            .is_some_and(|config| {
                config.is_lp_token.unwrap_or(false)
                    || config.symbol == "3USD"
                    || canonical_three_usd_ledger() == Some(*ledger)
            })
            .then_some(*ledger)
    })
}

/// Start one bounded 3USD reserve ingress. The entire allocation input is
/// durably pinned before the first approval call; no legacy mutable allocator
/// is used for this route.
async fn start_three_usd_absorb_v2(
    vault_info: &LiquidatableVaultInfo,
    ledger: Principal,
    draw_cap_e8s: u64,
) -> LiquidationResult {
    if !THREE_USD_RESERVE_INGRESS_V2_ADMISSION_ENABLED {
        return liquidation_failure(
            vault_info,
            StabilityPoolError::LiquidationFailed {
                vault_id: vault_info.vault_id,
                reason: "3USD reserve-ingress V2 admission is default-off pending coordinated cross-canister evidence".into(),
            },
        );
    }

    let result = async {
        if vault_info.collateral_price_e8s == 0
            || !crate::pool_guard::liquidation_in_progress()
            || !read_state(|state| state.in_flight_liquidations.contains(&vault_info.vault_id))
        {
            return Err(StabilityPoolError::SystemBusy);
        }

        let (virtual_price, available) = read_state(|state| {
            let config = state
                .stablecoin_registry
                .get(&ledger)
                .ok_or(StabilityPoolError::SystemBusy)?;
            if ledger != canonical_three_usd_ledger().ok_or(StabilityPoolError::SystemBusy)?
                || config.symbol != "3USD"
                || config.is_lp_token != Some(true)
                || !config.is_active
            {
                return Err(StabilityPoolError::SystemBusy);
            }
            let virtual_price = state
                .virtual_prices()
                .get(&ledger)
                .copied()
                .filter(|price| *price > 0)
                .ok_or(StabilityPoolError::SystemBusy)?;
            let available =
                state.available_stablecoin_for_collateral(ledger, &vault_info.collateral_type)?;
            Ok((virtual_price, available))
        })?;

        let debt_target = if vault_info.recommended_liquidation_amount > 0 {
            vault_info
                .recommended_liquidation_amount
                .min(vault_info.debt_amount)
        } else {
            vault_info.debt_amount
        };
        let (three_usd_amount, debt_covered) =
            bounded_three_usd_absorb_amounts(debt_target, available, draw_cap_e8s, virtual_price)?;
        if debt_covered < 10_000_000 {
            return Err(StabilityPoolError::AmountTooLow {
                minimum_e8s: 10_000_000,
            });
        }

        let approval_fee = query_exact_ledger_fee(ledger).await?;
        let created_at =
            mutate_state(|state| state.allocate_outbound_payout_timestamp(ic_cdk::api::time()))?;
        let expires_at = created_at
            .checked_add(300_000_000_000)
            .ok_or(StabilityPoolError::SystemBusy)?;
        let mut memo = Vec::with_capacity(16);
        memo.extend_from_slice(&vault_info.vault_id.to_be_bytes());
        memo.extend_from_slice(&created_at.to_be_bytes());
        let approval = SpThreeUsdApprovalIntent {
            ledger,
            allowance: three_usd_amount,
            fee: approval_fee,
            memo,
            created_at_time_ns: created_at,
            expires_at_ns: expires_at,
        };
        let absorb = mutate_state(|state| {
            state.prepare_sp_three_usd_absorb(
                vault_info.vault_id,
                ic_cdk::api::id(),
                ledger,
                vault_info.collateral_type,
                vault_info.collateral_price_e8s,
                created_at,
                debt_covered,
                three_usd_amount,
                virtual_price,
                approval,
            )
        })?;

        // Approval ambiguity remains attached to this absorb ID. Recovery
        // candidate attachment is the only continuation after a lost reply.
        crate::three_usd_v2::submit_three_usd_approval_inner(absorb.absorb_id).await?;
        let _ = crate::three_usd_v2::dispatch_three_usd_absorb_inner(absorb.absorb_id).await;
        crate::three_usd_v2::reconcile_three_usd_absorb_inner(absorb.absorb_id).await?;

        let completed = read_state(|state| {
            state
                .completed_sp_three_usd_absorbs
                .as_ref()
                .and_then(|rows| rows.get(&absorb.absorb_id))
                .filter(|row| row.phase == SpThreeUsdAbsorbPhase::Complete)
                .cloned()
        })
        .ok_or(StabilityPoolError::SystemBusy)?;
        let allocation = completed.allocation.ok_or(StabilityPoolError::SystemBusy)?;
        let (success, error_message) = three_usd_terminal_outcome(completed.terminal.as_ref())?;
        let stables_consumed = if allocation.principal_consumed > 0 {
            BTreeMap::from([(ledger, allocation.principal_consumed)])
        } else {
            BTreeMap::new()
        };
        Ok(LiquidationResult {
            vault_id: vault_info.vault_id,
            stables_consumed,
            collateral_gained: allocation.collateral_received,
            collateral_type: vault_info.collateral_type,
            success,
            error_message,
        })
    }
    .await;

    result.unwrap_or_else(|error| liquidation_failure(vault_info, error))
}

fn canonical_three_usd_ledger() -> Option<Principal> {
    Principal::from_text("fohh4-yyaaa-aaaap-qtkpa-cai").ok()
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
#[derive(candid::CandidType, candid::Deserialize, Debug)]
struct StabilityPoolLiquidationResult {
    pub success: bool,
    pub vault_id: u64,
    pub liquidated_debt: u64,
    pub collateral_received: u64,
    pub collateral_type: String,
    pub block_index: u64,
    pub fee: u64,
    pub collateral_price_e8s: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{chain_collateral_sentinel, read_state, replace_state, StabilityPoolState};
    use candid::Nat;
    use icrc_ledger_types::icrc1::transfer::Memo;

    fn principal(byte: u8) -> Principal {
        Principal::from_slice(&[byte])
    }

    fn icusd_ledger() -> Principal {
        Principal::from_slice(&[10])
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

    fn decoded_icrc3_block(op: &str) -> rumi_protocol_backend::icrc3_proof::DecodedBlock {
        rumi_protocol_backend::icrc3_proof::DecodedBlock {
            btype: None,
            op: op.into(),
            from: None,
            to: None,
            spender: None,
            amount: 5,
            transaction_fee: None,
            fee: None,
            memo: Some(vec![7]),
            created_at_time: Some(8),
            expected_allowance: None,
            expires_at: None,
        }
    }

    #[test]
    fn sp_burn_refund_memo_accepts_legacy_and_nonzero_rotated_identity() {
        let burn_block = 17;
        let vault_id = 44;
        let legacy = expected_sp_burn_refund_memo(burn_block, vault_id);
        assert!(sp_burn_refund_memo_matches(&legacy, burn_block, vault_id));

        let mut rotated = legacy.clone();
        rotated.extend_from_slice(&1u64.to_be_bytes());
        assert!(sp_burn_refund_memo_matches(&rotated, burn_block, vault_id));

        assert!(!sp_burn_refund_memo_matches(
            &legacy,
            burn_block + 1,
            vault_id
        ));
        assert!(!sp_burn_refund_memo_matches(
            &legacy,
            burn_block,
            vault_id + 1
        ));

        let mut zero_ordinal = legacy.clone();
        zero_ordinal.extend_from_slice(&0u64.to_be_bytes());
        assert!(!sp_burn_refund_memo_matches(
            &zero_ordinal,
            burn_block,
            vault_id
        ));

        let mut malformed_prefix = rotated;
        malformed_prefix[0] ^= 1;
        assert!(!sp_burn_refund_memo_matches(
            &malformed_prefix,
            burn_block,
            vault_id
        ));
    }

    #[test]
    fn v2_new_admission_only_accepts_currently_supported_icusd_route() {
        assert!(sp_liquidation_v2_token_supported(SpLiquidationToken::IcUsd));
        assert!(!sp_liquidation_v2_token_supported(
            SpLiquidationToken::CKUSDT
        ));
        assert!(!sp_liquidation_v2_token_supported(
            SpLiquidationToken::CKUSDC
        ));
    }

    #[test]
    fn v2_ic_usd_preflight_reserves_principal_and_one_approval_fee_only() {
        let cap = 200_000_000;
        let approval_fee = 10_000;
        assert_eq!(
            sp_v2_approval_preflight_amounts(cap, approval_fee, 0),
            Some((cap, cap + approval_fee)),
        );
        assert_eq!(
            sp_v2_approval_preflight_amounts(cap, approval_fee, 3),
            Some((cap + 3, cap + 3 + approval_fee)),
        );
        assert_eq!(sp_v2_approval_preflight_amounts(u64::MAX, 1, 0), None);
    }

    #[test]
    fn v2_icusd_burn_proof_binds_legacy_icrc3_operation_and_every_tuple_field() {
        let pool = principal(40);
        let protocol = principal(41);
        let account = |owner| Account {
            owner,
            subaccount: None,
        };
        let tuple = rumi_protocol_backend::SpLiquidationStablePullTuple {
            op_nonce: 1,
            ledger: icusd_ledger(),
            from: account(pool),
            spender: account(protocol),
            to: account(protocol),
            amount_raw: 5,
            fee_raw: 0,
            memo: vec![7],
            created_at_time_ns: 8,
        };
        let mut valid = decoded_icrc3_block("burn");
        valid.from = Some(account(pool));
        valid.spender = Some(account(protocol));
        assert!(validate_sp_v2_icusd_burn_block(&valid, &tuple).is_ok());

        let mut wrong = valid.clone();
        wrong.op = "xfer".into();
        assert!(validate_sp_v2_icusd_burn_block(&wrong, &tuple).is_err());
        let mut wrong = valid.clone();
        wrong.spender = Some(account(principal(42)));
        assert!(validate_sp_v2_icusd_burn_block(&wrong, &tuple).is_err());
        let mut wrong = valid.clone();
        wrong.memo = Some(vec![9]);
        assert!(validate_sp_v2_icusd_burn_block(&wrong, &tuple).is_err());
        let mut wrong = valid.clone();
        wrong.created_at_time = Some(9);
        assert!(validate_sp_v2_icusd_burn_block(&wrong, &tuple).is_err());
        let mut wrong = valid.clone();
        wrong.fee = Some(0);
        assert!(validate_sp_v2_icusd_burn_block(&wrong, &tuple).is_err());
        let mut wrong = valid;
        wrong.btype = Some("2xfer".into());
        assert!(validate_sp_v2_icusd_burn_block(&wrong, &tuple).is_err());
    }

    #[test]
    fn v2_icusd_mint_refund_proof_binds_mint_and_exact_reimbursement() {
        let pool = principal(40);
        let account = |owner| Account {
            owner,
            subaccount: None,
        };
        let tuple = SpLiquidationStableRefundTuple {
            op_nonce: 2,
            ledger: icusd_ledger(),
            source: account(principal(41)),
            destination: account(pool),
            principal_refund_raw: 5,
            approval_fee_refund_raw: 10,
            pull_fee_refund_raw: 0,
            amount_raw: 15,
            fee_raw: 0,
            memo: vec![7],
            created_at_time_ns: 8,
        };
        let mut valid = decoded_icrc3_block("mint");
        valid.amount = 15;
        valid.to = Some(account(pool));
        assert!(validate_sp_v2_icusd_mint_refund_block(&valid, &tuple).is_ok());

        let mut wrong = valid.clone();
        wrong.op = "xfer".into();
        assert!(validate_sp_v2_icusd_mint_refund_block(&wrong, &tuple).is_err());
        let mut wrong = valid.clone();
        wrong.spender = Some(account(principal(42)));
        assert!(validate_sp_v2_icusd_mint_refund_block(&wrong, &tuple).is_err());
        let mut wrong = valid.clone();
        wrong.memo = Some(vec![9]);
        assert!(validate_sp_v2_icusd_mint_refund_block(&wrong, &tuple).is_err());
        let mut wrong = valid.clone();
        wrong.created_at_time = Some(9);
        assert!(validate_sp_v2_icusd_mint_refund_block(&wrong, &tuple).is_err());
        let mut wrong = valid.clone();
        wrong.transaction_fee = Some(0);
        assert!(validate_sp_v2_icusd_mint_refund_block(&wrong, &tuple).is_err());
        let mut wrong = valid;
        wrong.btype = Some("1xfer".into());
        assert!(validate_sp_v2_icusd_mint_refund_block(&wrong, &tuple).is_err());
    }

    #[test]
    fn collateral_fee_nat_overflow_is_rejected_before_liquidation_dispatch() {
        assert_eq!(collateral_fee_from_nat(Nat::from(u64::MAX)), Ok(u64::MAX));
        assert!(collateral_fee_from_nat(Nat::from(u128::from(u64::MAX) + 1)).is_err());
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

    #[test]
    fn legacy_generic_icrc_route_is_blocked_before_approval_but_special_routes_remain_routed() {
        let mut state = test_state();
        let ordinary_icrc_collateral = principal(50);
        let chain_sentinel = principal(51);
        state.register_chain_collateral_sentinel(chain_sentinel);

        assert_eq!(
            legacy_liquidation_dispatch(&state, &ordinary_icrc_collateral),
            LegacyLiquidationDispatch::DisabledGenericIcrc,
        );
        assert!(!legacy_generic_icrc_liquidation_enabled());
        assert_eq!(
            legacy_liquidation_dispatch(&state, &xrp_ledger()),
            LegacyLiquidationDispatch::NativeXrpAbsorb,
        );
        assert_eq!(
            legacy_liquidation_dispatch(&state, &chain_sentinel),
            LegacyLiquidationDispatch::DedicatedChainAbsorb,
        );
    }

    #[test]
    fn mixed_draw_with_lp_reserve_selects_the_dedicated_pre_effect_route() {
        let icusd = icusd_ledger();
        let lp = principal(12);
        let mut configs = BTreeMap::new();
        configs.insert(
            icusd,
            StablecoinConfig {
                ledger_id: icusd,
                symbol: "icUSD".into(),
                decimals: 8,
                priority: 1,
                is_active: true,
                transfer_fee: Some(100_000),
                is_lp_token: None,
                underlying_pool: None,
            },
        );
        configs.insert(
            lp,
            StablecoinConfig {
                ledger_id: lp,
                symbol: "3USD".into(),
                decimals: 8,
                priority: 2,
                is_active: true,
                transfer_fee: Some(0),
                is_lp_token: Some(true),
                underlying_pool: None,
            },
        );
        let draw = BTreeMap::from([(icusd, 10_000_000), (lp, 10_000_000)]);

        // execute_single_liquidation routes LP-containing draws before the
        // generic legacy gate and before any token approval.
        assert_eq!(disabled_lp_reserve_route_ledger(&draw, &configs), Some(lp));
        assert_eq!(
            disabled_lp_reserve_route_ledger(&BTreeMap::from([(lp, 10_000_000)]), &configs),
            Some(lp)
        );
        let misconfigured_symbol = principal(13);
        let canonical = canonical_three_usd_ledger().expect("canonical 3USD principal");
        configs.insert(
            misconfigured_symbol,
            StablecoinConfig {
                ledger_id: misconfigured_symbol,
                symbol: "3USD".into(),
                decimals: 8,
                priority: 3,
                is_active: true,
                transfer_fee: Some(0),
                is_lp_token: None,
                underlying_pool: None,
            },
        );
        configs.insert(
            canonical,
            StablecoinConfig {
                ledger_id: canonical,
                symbol: "legacy-stable".into(),
                decimals: 8,
                priority: 4,
                is_active: true,
                transfer_fee: Some(0),
                is_lp_token: Some(false),
                underlying_pool: None,
            },
        );
        assert_eq!(
            disabled_lp_reserve_route_ledger(
                &BTreeMap::from([(misconfigured_symbol, 10_000_000)]),
                &configs
            ),
            Some(misconfigured_symbol),
            "the 3USD symbol must be fail-closed when LP metadata is absent"
        );
        assert_eq!(
            disabled_lp_reserve_route_ledger(&BTreeMap::from([(canonical, 10_000_000)]), &configs),
            Some(canonical),
            "the canonical ledger must be fail-closed even with conflicting registry metadata"
        );
        assert_eq!(
            disabled_lp_reserve_route_ledger(&BTreeMap::from([(icusd, 10_000_000)]), &configs),
            None
        );
    }

    #[test]
    fn three_usd_v2_admission_gate_and_amounts_are_explicit() {
        #[cfg(not(feature = "test-three-usd-reserve-ingress-v2-admission"))]
        assert!(!THREE_USD_RESERVE_INGRESS_V2_ADMISSION_ENABLED);
        #[cfg(feature = "test-three-usd-reserve-ingress-v2-admission")]
        assert!(THREE_USD_RESERVE_INGRESS_V2_ADMISSION_ENABLED);
        assert_eq!(
            bounded_three_usd_absorb_amounts(100, 1_000, 1_000, THREE_USD_E8S_SCALE).unwrap(),
            (100, 100)
        );
        assert_eq!(
            bounded_three_usd_absorb_amounts(100, 50, 1_000, THREE_USD_E8S_SCALE).unwrap(),
            (50, 50)
        );
        // Flooring the LP amount never asks the backend to write down more
        // debt than the pinned request's target.
        let (lp, covered) =
            bounded_three_usd_absorb_amounts(100, 1_000, 1_000, THREE_USD_E8S_SCALE * 3 / 2)
                .unwrap();
        assert_eq!(lp, 66);
        assert_eq!(covered, 99);
        assert_eq!(
            bounded_three_usd_absorb_amounts(100, 1_000, 25, THREE_USD_E8S_SCALE).unwrap(),
            (25, 25)
        );
        assert!(bounded_three_usd_absorb_amounts(100, 100, 100, 0).is_err());
    }

    #[test]
    fn three_usd_pretransfer_rejection_is_reported_as_no_liquidation() {
        let terminal = SpThreeUsdTerminalEvidence::PreTransferRejected {
            backend_vault_id: 77,
            backend_absorb_id: 1,
            request: rumi_protocol_backend::state::ThreeUsdReserveIngressRequest {
                icusd_debt_covered_e8s: 600,
                three_usd_amount_e8s: 600,
                ledger: principal(43),
            },
            reason: "vault recovered".into(),
        };
        let (success, reason) = three_usd_terminal_outcome(Some(&terminal)).unwrap();
        assert!(!success);
        assert!(reason.unwrap().contains("vault recovered"));
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
            self.events
                .push(format!("burn:{vault_id}:{amount_e8s}:{created_at_time}"));
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
            collateral_price_e8s: 0,
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

    fn sweep_state_with_payouts(
        payouts: Vec<(Principal, NativeXrpPendingPayout)>,
    ) -> StabilityPoolState {
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

        let summary =
            futures::executor::block_on(run_native_xrp_settle_sweep_with_io(&mut io, None, 2));

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

        let summary =
            futures::executor::block_on(run_native_xrp_settle_sweep_with_io(&mut io, None, 2));

        assert_eq!(summary.acked, 1);
        assert!(io.settle_calls.is_empty());
        assert!(read_state(|s| s
            .native_xrp_pending_payouts_for(&user_a())
            .is_empty()));
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

        let first =
            futures::executor::block_on(run_native_xrp_settle_sweep_with_io(&mut io, None, 2));
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

        let summary =
            futures::executor::block_on(run_native_xrp_settle_sweep_with_io(&mut io, None, 2));

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

        let summary =
            futures::executor::block_on(run_native_xrp_settle_sweep_with_io(&mut io, None, 2));

        assert_eq!(
            summary.failed, 0,
            "a landed-but-noisy submit is not a failure"
        );
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

        let summary =
            futures::executor::block_on(run_native_xrp_settle_sweep_with_io(&mut io, None, 3));

        assert_eq!(summary.examined, 3);
        assert_eq!(summary.failed, 2);
        assert_eq!(summary.submitted, 1);
        assert_eq!(
            io.settle_calls.iter().map(|c| c.0).collect::<Vec<_>>(),
            vec![2, 4]
        );
        assert_eq!(
            read_state(|s| s.native_xrp_pending_payouts_for(&user_a()).len())
                + read_state(|s| s.native_xrp_pending_payouts_for(&user_b()).len()),
            3,
            "no record may be dropped on errors"
        );
    }
}
