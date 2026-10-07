//! Receipt-safe building blocks for the separate 3USD reserve-ingress saga.
//!
//! This module contains only the durable 3USD V2 driver and reconciliation
//! helpers. Admission remains closed in the public liquidation entrypoint.

use candid::Nat;
use candid::Principal;
use ic_cdk::call;
use icrc_ledger_types::icrc1::transfer::Memo;
use icrc_ledger_types::icrc2::approve::{ApproveArgs, ApproveError};
use num_traits::ToPrimitive;

use crate::liquidation::verify_sp_liquidation_v2_approval_receipt;
use crate::pool_guard::SpLiquidationGuard;
use crate::state::{mutate_state, read_state};
use crate::types::{
    SpLiquidationApprovalReceipt, SpLiquidationApprovalTuple, SpThreeUsdAbsorbPhase,
    SpThreeUsdTerminalEvidence, StabilityPoolError,
};

fn approval_receipt_for_three_usd_absorb(
    absorb_id: u64,
    block_index: u64,
) -> Result<SpLiquidationApprovalReceipt, StabilityPoolError> {
    let row = read_state(|state| {
        state
            .pending_sp_three_usd_absorbs
            .as_ref()
            .and_then(|rows| rows.get(&absorb_id))
            .cloned()
    })
    .ok_or(StabilityPoolError::SystemBusy)?;
    if !row.approval_dispatch_may_have_happened
        || row.approval.ledger != row.ledger
        || row.approval.allowance != row.three_usd_amount
        || row.protocol_canister_id != read_state(|state| state.protocol_canister_id)
        || row.stability_pool != ic_cdk::api::id()
    {
        return Err(StabilityPoolError::SystemBusy);
    }
    Ok(SpLiquidationApprovalReceipt {
        block_index,
        tuple: SpLiquidationApprovalTuple {
            ledger: row.ledger,
            owner: icrc_ledger_types::icrc1::account::Account {
                owner: row.stability_pool,
                subaccount: None,
            },
            spender: icrc_ledger_types::icrc1::account::Account {
                owner: row.protocol_canister_id,
                subaccount: None,
            },
            allowance_raw: row.approval.allowance,
            fee_raw: row.approval.fee,
            memo: row.approval.memo,
            created_at_time_ns: row.approval.created_at_time_ns,
            expires_at_ns: row.approval.expires_at_ns,
        },
    })
}

/// Attach an operator-discovered approval block to the existing pending row.
/// The candidate index carries no authority until the exact pinned ICRC-2
/// approval tuple is proven by the ledger's ICRC-3 block.
pub(crate) async fn prove_three_usd_approval_candidate(
    absorb_id: u64,
    block_index: u64,
) -> Result<(), StabilityPoolError> {
    let _guard = SpLiquidationGuard::new_three_usd_resume(absorb_id)?;
    let receipt = approval_receipt_for_three_usd_absorb(absorb_id, block_index)?;
    verify_sp_liquidation_v2_approval_receipt(&receipt).await?;
    mutate_state(|state| {
        state.record_sp_three_usd_approval_receipt(absorb_id, block_index, receipt.tuple.fee_raw)
    })
}

/// Submit the exact approval pinned in the journal. The ambiguity bit is
/// persisted before the ledger call; a lost reply never causes a fresh
/// approval. Recovery must attach and verify the original block instead.
pub(crate) async fn submit_three_usd_approval(absorb_id: u64) -> Result<(), StabilityPoolError> {
    let _guard = SpLiquidationGuard::new_three_usd_resume(absorb_id)?;
    submit_three_usd_approval_inner(absorb_id).await
}

/// Submit approval while the caller already holds the pool liquidation guard.
pub(crate) async fn submit_three_usd_approval_inner(
    absorb_id: u64,
) -> Result<(), StabilityPoolError> {
    let row = read_state(|state| {
        state
            .pending_sp_three_usd_absorbs
            .as_ref()
            .and_then(|rows| rows.get(&absorb_id))
            .cloned()
    })
    .ok_or(StabilityPoolError::SystemBusy)?;
    if row.phase == SpThreeUsdAbsorbPhase::ApprovalProven
        && row.approval_receipt_block_index.is_some()
    {
        return Ok(());
    }
    if row.phase != SpThreeUsdAbsorbPhase::ApprovalPending
        || row.approval_dispatch_may_have_happened
        || row.approval_receipt_block_index.is_some()
        || row.approval.ledger != row.ledger
        || row.approval.allowance != row.three_usd_amount
        || row.protocol_canister_id != read_state(|state| state.protocol_canister_id)
        || row.stability_pool != ic_cdk::api::id()
    {
        return Err(StabilityPoolError::SystemBusy);
    }

    mutate_state(|state| state.mark_sp_three_usd_approval_dispatch(absorb_id))?;
    let args = ApproveArgs {
        from_subaccount: None,
        spender: icrc_ledger_types::icrc1::account::Account {
            owner: row.protocol_canister_id,
            subaccount: None,
        },
        amount: Nat::from(row.approval.allowance),
        expected_allowance: None,
        expires_at: Some(row.approval.expires_at_ns),
        fee: Some(Nat::from(row.approval.fee)),
        memo: Some(Memo::from(row.approval.memo.clone())),
        created_at_time: Some(row.approval.created_at_time_ns),
    };
    let reply: Result<(Result<Nat, ApproveError>,), _> =
        call(row.ledger, "icrc2_approve", (args,)).await;
    let block_index = match reply {
        Ok((Ok(index),)) => index.0.to_u64().ok_or(StabilityPoolError::SystemBusy)?,
        Ok((Err(ApproveError::Duplicate { duplicate_of }),)) => duplicate_of
            .0
            .to_u64()
            .ok_or(StabilityPoolError::SystemBusy)?,
        Ok((Err(error),)) => {
            mutate_state(|state| {
                state.hold_sp_three_usd_absorb(
                    absorb_id,
                    &format!("approval returned without a proven receipt: {error:?}"),
                )
            })?;
            return Err(StabilityPoolError::LedgerTransferFailed {
                reason: format!("3USD approval did not produce a receipt: {error:?}"),
            });
        }
        Err(_) => {
            mutate_state(|state| {
                state.hold_sp_three_usd_absorb(
                    absorb_id,
                    "approval reply was lost; exact ledger block required",
                )
            })?;
            return Err(StabilityPoolError::InterCanisterCallFailed {
                target: row.ledger.to_text(),
                method: "icrc2_approve".into(),
            });
        }
    };

    let receipt = approval_receipt_for_three_usd_absorb(absorb_id, block_index)?;
    if let Err(error) = verify_sp_liquidation_v2_approval_receipt(&receipt).await {
        mutate_state(|state| {
            state.hold_sp_three_usd_absorb(
                absorb_id,
                &format!("approval block failed exact ICRC-3 proof: {error:?}"),
            )
        })?;
        return Err(error);
    }
    mutate_state(|state| {
        state.record_sp_three_usd_approval_receipt(absorb_id, block_index, receipt.tuple.fee_raw)
    })
}

/// Dispatch the pinned 3USD request under its durable absorb identity. The
/// row is marked ambiguous before the inter-canister call. A returned result
/// is diagnostic only; terminal accounting must wait for the exact backend
/// status and receipts.
pub(crate) async fn dispatch_three_usd_absorb(
    absorb_id: u64,
) -> Result<rumi_protocol_backend::StabilityPoolLiquidationResult, StabilityPoolError> {
    let _guard = SpLiquidationGuard::new_three_usd_resume(absorb_id)?;
    dispatch_three_usd_absorb_inner(absorb_id).await
}

/// Dispatch while the caller already holds the pool liquidation guard.
pub(crate) async fn dispatch_three_usd_absorb_inner(
    absorb_id: u64,
) -> Result<rumi_protocol_backend::StabilityPoolLiquidationResult, StabilityPoolError> {
    let row = read_state(|state| {
        state
            .pending_sp_three_usd_absorbs
            .as_ref()
            .and_then(|rows| rows.get(&absorb_id))
            .cloned()
    })
    .ok_or(StabilityPoolError::SystemBusy)?;
    if row.approval_receipt_block_index.is_none()
        || !matches!(
            row.phase,
            SpThreeUsdAbsorbPhase::ApprovalProven
                | SpThreeUsdAbsorbPhase::BackendPending
                | SpThreeUsdAbsorbPhase::Held
        )
        || row.protocol_canister_id != read_state(|state| state.protocol_canister_id)
        || row.stability_pool != ic_cdk::api::id()
    {
        return Err(StabilityPoolError::SystemBusy);
    }
    mutate_state(|state| state.mark_sp_three_usd_backend_dispatch(absorb_id))?;
    let result: Result<
        (
            Result<
                rumi_protocol_backend::StabilityPoolLiquidationResult,
                rumi_protocol_backend::ProtocolError,
            >,
        ),
        _,
    > = call(
        row.protocol_canister_id,
        "stability_pool_liquidate_with_reserves_v2",
        (
            row.vault_id,
            row.absorb_id,
            row.debt_covered_e8s,
            row.three_usd_amount,
            row.ledger,
        ),
    )
    .await;
    match result {
        Ok((Ok(result),)) => Ok(result),
        Ok((Err(error),)) => {
            mutate_state(|state| {
                state.hold_sp_three_usd_absorb(
                    absorb_id,
                    &format!("backend result requires status reconciliation: {error:?}"),
                )
            })?;
            Err(StabilityPoolError::LiquidationFailed {
                vault_id: row.vault_id,
                reason: format!("backend call requires status reconciliation: {error:?}"),
            })
        }
        Err(_) => {
            mutate_state(|state| {
                state.hold_sp_three_usd_absorb(
                    absorb_id,
                    "backend reply was lost; exact status reconciliation required",
                )
            })?;
            Err(StabilityPoolError::InterCanisterCallFailed {
                target: row.protocol_canister_id.to_text(),
                method: "stability_pool_liquidate_with_reserves_v2".into(),
            })
        }
    }
}

async fn verify_three_usd_refund_receipt(
    ledger: Principal,
    receipt: &rumi_protocol_backend::state::ThreeUsdReserveIngressRefundReceipt,
    expected_source: Principal,
    expected_destination: Principal,
    expected_amount: u64,
) -> Result<(), StabilityPoolError> {
    let tuple = &receipt.tuple;
    if tuple.amount_e8s != expected_amount
        || tuple.source_owner != expected_source
        || tuple.source_subaccount.is_some()
        || tuple.destination.owner != expected_destination
        || tuple.destination.subaccount.is_some()
        || tuple.created_at_time_ns == 0
    {
        return Err(StabilityPoolError::SystemBusy);
    }
    let block = rumi_protocol_backend::icrc3_proof::fetch_icrc3_block(ledger, receipt.block_index)
        .await
        .map_err(|reason| StabilityPoolError::LedgerTransferFailed { reason })?;
    validate_three_usd_refund_block(&block, tuple)
        .map_err(|reason| StabilityPoolError::LedgerTransferFailed { reason })
}

fn validate_three_usd_refund_block(
    block: &rumi_protocol_backend::icrc3_proof::DecodedBlock,
    tuple: &rumi_protocol_backend::state::ThreeUsdRefundTransferTuple,
) -> Result<(), String> {
    if block.fee != Some(u128::from(tuple.fee_e8s)) {
        return Err("3USD refund ICRC-3 fee does not match the exact receipt tuple".into());
    }
    rumi_protocol_backend::icrc3_proof::validate_three_usd_default_source_refund_block(block, tuple)
}

/// Convert the status projection to the stable evidence DTO without dropping
/// any backend result field. The stable planner performs the remaining exact
/// request, collateral, and allocation checks before persisting it.
fn checked_absorbed_result(
    result: rumi_protocol_backend::StabilityPoolLiquidationResult,
    expected_vault_id: u64,
    maximum_debt: u64,
) -> Result<rumi_protocol_backend::state::ThreeUsdReserveIngressResult, StabilityPoolError> {
    let evidence = rumi_protocol_backend::state::ThreeUsdReserveIngressResult {
        success: result.success,
        vault_id: result.vault_id,
        liquidated_debt: result.liquidated_debt,
        collateral_received: result.collateral_received,
        collateral_type: result.collateral_type,
        block_index: result.block_index,
        fee: result.fee,
        collateral_price_e8s: result.collateral_price_e8s,
    };
    if !evidence.success
        || evidence.vault_id != expected_vault_id
        || evidence.liquidated_debt == 0
        || evidence.liquidated_debt > maximum_debt
        || evidence.collateral_type.parse::<Principal>().is_err()
    {
        return Err(StabilityPoolError::SystemBusy);
    }
    Ok(evidence)
}

#[cfg(test)]
mod tests {
    use super::validate_three_usd_refund_block;
    use candid::Principal;
    use icrc_ledger_types::icrc1::account::Account;
    use rumi_protocol_backend::{icrc3_proof::DecodedBlock, state::ThreeUsdRefundTransferTuple};

    #[test]
    fn three_usd_refund_rejects_mismatched_normalized_block_fee() {
        let source = Principal::from_slice(&[1]);
        let destination = Principal::from_slice(&[2]);
        let memo = [7; 16];
        let tuple = ThreeUsdRefundTransferTuple {
            source_owner: source,
            source_subaccount: None,
            destination: Account {
                owner: destination,
                subaccount: None,
            },
            amount_e8s: 50,
            fee_e8s: 2,
            memo: memo.clone(),
            created_at_time_ns: 8,
        };
        let mut block = DecodedBlock {
            btype: Some("1xfer".into()),
            op: "xfer".into(),
            from: Some(Account {
                owner: source,
                subaccount: None,
            }),
            to: Some(Account {
                owner: destination,
                subaccount: None,
            }),
            spender: None,
            amount: 50,
            transaction_fee: Some(2),
            fee: Some(1),
            memo: Some(memo.to_vec()),
            created_at_time: Some(8),
            expected_allowance: None,
            expires_at: None,
        };

        assert!(validate_three_usd_refund_block(&block, &tuple).is_err());
        block.fee = Some(2);
        assert!(validate_three_usd_refund_block(&block, &tuple).is_ok());
    }
}

/// Fetch the replicated backend status and accept only exact terminal states.
/// The ordinary query status projection must never be used to release this
/// journal, especially for PreTransferRejected. All positive ledger receipts
/// are independently checked by the SP before local accounting.
pub(crate) async fn reconcile_three_usd_absorb(absorb_id: u64) -> Result<(), StabilityPoolError> {
    let _guard = SpLiquidationGuard::new_three_usd_resume(absorb_id)?;
    reconcile_three_usd_absorb_inner(absorb_id).await
}

/// Reconcile while the caller already holds the pool liquidation guard.
pub(crate) async fn reconcile_three_usd_absorb_inner(
    absorb_id: u64,
) -> Result<(), StabilityPoolError> {
    let row = read_state(|state| {
        state
            .pending_sp_three_usd_absorbs
            .as_ref()
            .and_then(|rows| rows.get(&absorb_id))
            .cloned()
    })
    .ok_or(StabilityPoolError::SystemBusy)?;
    if !row.backend_dispatch_may_have_happened
        || row.approval_receipt_block_index.is_none()
        || row.stability_pool != ic_cdk::api::id()
        || row.protocol_canister_id != read_state(|state| state.protocol_canister_id)
    {
        return Err(StabilityPoolError::SystemBusy);
    }
    let (status,): (
        Result<
            rumi_protocol_backend::ThreeUsdReserveIngressV2StatusView,
            rumi_protocol_backend::ProtocolError,
        >,
    ) = call(
        row.protocol_canister_id,
        "get_three_usd_reserve_ingress_v2_status_for_reconciliation",
        (row.vault_id, row.absorb_id),
    )
    .await
    .map_err(|_| StabilityPoolError::InterCanisterCallFailed {
        target: row.protocol_canister_id.to_text(),
        method: "get_three_usd_reserve_ingress_v2_status_for_reconciliation".into(),
    })?;
    let view = status.map_err(|_| StabilityPoolError::SystemBusy)?;
    let expected_request = rumi_protocol_backend::state::ThreeUsdReserveIngressRequest {
        icusd_debt_covered_e8s: row.debt_covered_e8s,
        three_usd_amount_e8s: row.three_usd_amount,
        ledger: row.ledger,
    };
    if view.stability_pool != row.stability_pool
        || view.vault_id != row.vault_id
        || view.absorb_id != row.absorb_id
        || view.request.as_ref() != Some(&expected_request)
    {
        mutate_state(|state| {
            state.hold_sp_three_usd_absorb(absorb_id, "backend status identity/request mismatch")
        })?;
        return Err(StabilityPoolError::SystemBusy);
    }

    let evidence = match view.status {
        rumi_protocol_backend::ThreeUsdReserveIngressV2Status::PreTransferRejected { reason } => {
            SpThreeUsdTerminalEvidence::PreTransferRejected {
                backend_vault_id: view.vault_id,
                backend_absorb_id: view.absorb_id,
                request: expected_request,
                reason,
            }
        }
        rumi_protocol_backend::ThreeUsdReserveIngressV2Status::Absorbed {
            transfer_block_index,
            transfer_tuple,
            proof,
            result,
            proportional_refund,
            collateral_payout_receipt,
        } => {
            let result = checked_absorbed_result(result, row.vault_id, row.debt_covered_e8s)?;
            if proof.block_index != transfer_block_index
                || proof.ledger_kind
                    != rumi_protocol_backend::icrc3_proof::SpProofLedger::ThreePoolTransferDefault
                || proof.vault_id_memo != row.vault_id
            {
                return Err(StabilityPoolError::SystemBusy);
            }
            rumi_protocol_backend::icrc3_proof::verify_three_usd_reserve_ingress_block(
                row.ledger,
                transfer_block_index,
                proof.block_index,
                &transfer_tuple,
            )
            .await
            .map_err(|reason| StabilityPoolError::LedgerTransferFailed { reason })?;
            let payout = &collateral_payout_receipt.tuple;
            if payout.gross_amount_e8s == 0
                || payout.net_amount_e8s == 0
                || payout.net_amount_e8s.checked_add(payout.fee_e8s)
                    != Some(payout.gross_amount_e8s)
                || payout.created_at_time_ns == 0
            {
                return Err(StabilityPoolError::SystemBusy);
            }
            match payout.proof_kind {
                rumi_protocol_backend::state::PayoutProofKind::NativeIcp => {
                    if payout.source.subaccount.is_some() || payout.destination.subaccount.is_some()
                    {
                        return Err(StabilityPoolError::SystemBusy);
                    }
                    rumi_protocol_backend::treasury::verify_native_icp_transfer_receipt(
                        payout.ledger,
                        payout.source.owner,
                        payout.destination.owner,
                        payout.net_amount_e8s,
                        payout.fee_e8s,
                        &payout.memo,
                        payout.created_at_time_ns,
                        collateral_payout_receipt.block_index,
                    )
                    .await
                    .map_err(|reason| StabilityPoolError::LedgerTransferFailed { reason })?;
                }
                rumi_protocol_backend::state::PayoutProofKind::Icrc3 => {
                    let block = rumi_protocol_backend::icrc3_proof::fetch_icrc3_block(
                        payout.ledger,
                        collateral_payout_receipt.block_index,
                    )
                    .await
                    .map_err(|reason| StabilityPoolError::LedgerTransferFailed { reason })?;
                    rumi_protocol_backend::icrc3_proof::validate_icrc3_transfer_block_with_fee(
                        &block,
                        payout.source.clone(),
                        payout.destination.clone(),
                        payout.net_amount_e8s,
                        payout.fee_e8s,
                        &payout.memo,
                        payout.created_at_time_ns,
                    )
                    .map_err(|reason| StabilityPoolError::LedgerTransferFailed { reason })?;
                }
            }
            let consumed = u64::try_from(
                (row.three_usd_amount as u128)
                    .checked_mul(result.liquidated_debt as u128)
                    .ok_or(StabilityPoolError::SystemBusy)?
                    / row.debt_covered_e8s as u128,
            )
            .map_err(|_| StabilityPoolError::SystemBusy)?;
            let expected_refund = row
                .three_usd_amount
                .checked_sub(consumed)
                .ok_or(StabilityPoolError::SystemBusy)?;
            match proportional_refund.as_ref() {
                Some(receipt) if expected_refund > 0 => {
                    verify_three_usd_refund_receipt(
                        row.ledger,
                        receipt,
                        row.protocol_canister_id,
                        row.stability_pool,
                        expected_refund,
                    )
                    .await?;
                }
                None if expected_refund == 0 => {}
                _ => return Err(StabilityPoolError::SystemBusy),
            }
            let observed_transfer_fee = transfer_tuple.fee_e8s.unwrap_or(0);
            SpThreeUsdTerminalEvidence::Absorbed {
                backend_vault_id: view.vault_id,
                backend_absorb_id: view.absorb_id,
                request: expected_request,
                transfer_tuple,
                transfer_block_index,
                observed_transfer_fee,
                proof,
                result,
                proportional_refund,
                payout_receipt: collateral_payout_receipt,
            }
        }
        rumi_protocol_backend::ThreeUsdReserveIngressV2Status::FailedAfterTransfer {
            transfer_block_index,
            transfer_tuple,
            proof,
            error,
            full_refund,
        } => {
            if proof.block_index != transfer_block_index
                || proof.ledger_kind
                    != rumi_protocol_backend::icrc3_proof::SpProofLedger::ThreePoolTransferDefault
                || proof.vault_id_memo != row.vault_id
            {
                return Err(StabilityPoolError::SystemBusy);
            }
            rumi_protocol_backend::icrc3_proof::verify_three_usd_reserve_ingress_block(
                row.ledger,
                transfer_block_index,
                proof.block_index,
                &transfer_tuple,
            )
            .await
            .map_err(|reason| StabilityPoolError::LedgerTransferFailed { reason })?;
            verify_three_usd_refund_receipt(
                row.ledger,
                &full_refund,
                row.protocol_canister_id,
                row.stability_pool,
                row.three_usd_amount,
            )
            .await?;
            SpThreeUsdTerminalEvidence::FailedAfterTransfer {
                backend_vault_id: view.vault_id,
                backend_absorb_id: view.absorb_id,
                request: expected_request,
                transfer_tuple,
                transfer_block_index,
                observed_transfer_fee: 0,
                proof,
                error,
                full_refund,
            }
        }
        rumi_protocol_backend::ThreeUsdReserveIngressV2Status::Unseen
        | rumi_protocol_backend::ThreeUsdReserveIngressV2Status::AdmissionPending
        | rumi_protocol_backend::ThreeUsdReserveIngressV2Status::TransferPending { .. } => {
            mutate_state(|state| {
                state.hold_sp_three_usd_absorb(
                    absorb_id,
                    "backend ingress remains nonterminal; exact candidate recovery required",
                )
            })?;
            return Err(StabilityPoolError::SystemBusy);
        }
    };

    mutate_state(|state| state.plan_sp_three_usd_terminal(absorb_id, evidence))?;
    mutate_state(|state| state.apply_sp_three_usd_terminal(absorb_id))
}
