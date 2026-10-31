//! Projections from stable records to public Candid views (53a).

use crate::ids::Hash32;
use crate::state::{self, AuditRecord, DepositKind, DepositRecord, DepositState, LedgerOp, LedgerOpStatus, ReleaseRecord, ReleaseState};
use crate::sui::{format_address, format_digest_base58};
use crate::types::*;

fn hex32(h: &Hash32) -> String {
    hex::encode(h)
}

fn succeeded_block(op: &LedgerOp) -> Option<u64> {
    match op.status {
        LedgerOpStatus::Succeeded { block_index } => Some(block_index),
        _ => None,
    }
}

pub fn deposit_view(d: &DepositRecord) -> DepositView {
    DepositView {
        id: d.id,
        kind: match &d.kind {
            DepositKind::UserDeposit => DepositKindView::UserDeposit,
            DepositKind::Remint { return_id } => DepositKindView::Remint { return_id: hex32(return_id) },
        },
        owner: d.owner,
        from: d.from,
        sui_recipient: format_address(&d.sui_recipient),
        amount_e8s: d.amount_e8s,
        deposit_fee_e8s: d.deposit_fee_e8s,
        mint_amount_e8s: d.mint_amount_e8s,
        mint_id: d.mint_id.as_ref().map(hex32),
        status: match &d.state {
            DepositState::PullPending => DepositStatus::PullPending,
            DepositState::PullUnknown => DepositStatus::PullUnknown,
            DepositState::PullFailed { reason } => DepositStatus::PullFailed { reason: reason.clone() },
            DepositState::Escrowed => DepositStatus::Escrowed,
            DepositState::MintInFlight => DepositStatus::MintInFlight,
            DepositState::Delivered => DepositStatus::Delivered,
            DepositState::MintHeld { reason } => DepositStatus::MintHeld { reason: reason.clone() },
            DepositState::Refunding => DepositStatus::Refunding,
            DepositState::Refunded => DepositStatus::Refunded,
        },
        client_request_id: serde_bytes::ByteBuf::from(d.client_request_id.clone()),
        pull_block_index: d.pull.as_ref().and_then(succeeded_block),
        refund_block_index: d.refund.as_ref().and_then(succeeded_block),
        refund_amount_e8s: d.refund.as_ref().map(|r| r.amount_e8s),
        mint_op_id: d.mint_op,
        delivered: d.delivered.as_ref().map(|m| SuiInclusion { tx_digest: m.tx_digest.as_ref().map(format_digest_base58), checkpoint: m.checkpoint, sequence: m.sequence }),
        created_at_ns: d.created_at_ns,
        updated_at_ns: d.updated_at_ns,
    }
}

pub fn release_view(r: &ReleaseRecord) -> ReleaseView {
    let unknown = matches!(r.transfer.as_ref().map(|t| &t.status), Some(LedgerOpStatus::Unknown { .. }));
    ReleaseView {
        return_id: hex32(&r.return_id),
        seq: r.seq,
        gate_sequence: r.gate_sequence,
        sui_sender: format_address(&r.sui_sender),
        client_nonce: r.client_nonce,
        amount_e8s: r.amount_e8s,
        release_fee_e8s: r.release_fee_e8s,
        ledger_fee_e8s: r.transfer.as_ref().and_then(|t| t.fee_e8s),
        amount_to_receive_e8s: r.transfer.as_ref().map(|t| t.amount_e8s),
        destination: r.destination,
        tx_digest: format_digest_base58(&r.tx_digest),
        checkpoint: r.checkpoint,
        status: match &r.state {
            ReleaseState::Pending if unknown => ReleaseStatus::TransferUnknown,
            ReleaseState::Pending => ReleaseStatus::Pending,
            ReleaseState::Released => ReleaseStatus::Released,
            ReleaseState::Held { reason } => ReleaseStatus::Held { reason: reason.clone() },
            ReleaseState::Reminting { deposit_id } => ReleaseStatus::Reminting { deposit_id: *deposit_id },
            ReleaseState::Reminted { deposit_id } => ReleaseStatus::Reminted { deposit_id: *deposit_id },
        },
        release_block_index: r.transfer.as_ref().and_then(succeeded_block),
        created_at_ns: r.created_at_ns,
        updated_at_ns: r.updated_at_ns,
    }
}

pub fn audit_entry(a: &AuditRecord) -> AuditEntry {
    AuditEntry {
        seq: a.seq,
        at_ns: a.at_ns,
        transition: a.transition.clone(),
        deposit_id: a.deposit_id,
        return_id: a.return_id.as_ref().map(hex32),
        op_id: a.op_id,
        note: a.note.clone(),
    }
}

pub fn buckets_view() -> BucketsView {
    state::read_runtime(|c| BucketsView::from(&c.buckets))
}

// ---------------------------------------------------------------------------
// Mint lane (53b)
// ---------------------------------------------------------------------------

use crate::state::{AttemptDisposition, DeliveryVia, MintAttempt, MintOp, MintOpState, ObservedDeployment};

pub fn observed_view(o: &ObservedDeployment) -> ObservedDeploymentView {
    ObservedDeploymentView {
        checkpoint: o.checkpoint,
        checkpoint_digest: format_digest_base58(&o.checkpoint_digest),
        epoch: o.epoch,
        observed_at_ns: o.observed_at_ns,
        gate_version: o.gate_version,
        initial_shared_version: o.initial_shared_version,
        supply_cap: o.supply_cap,
        min_bridge_out: o.min_bridge_out,
        total_supply: o.total_supply,
        minted_total: o.minted_total,
        burned_total: o.burned_total,
        mint_count: o.mint_count,
        bridge_out_count: o.bridge_out_count,
        decimals: o.decimals,
        symbol: o.symbol.clone(),
        mint_cap_id: format_address(&o.mint_cap.id),
        mint_cap_version: o.mint_cap.version,
        mint_cap_owner: format_address(&o.mint_cap_owner),
        admin_cap_id: format_address(&o.admin_cap.id),
        admin_cap_owner: format_address(&o.admin_cap_owner),
        admin_cap_role: o.admin_cap_role,
        module_digest_observed: o.module_digest_observed,
    }
}

fn attempt_view(a: &MintAttempt) -> MintAttemptView {
    MintAttemptView {
        generation: a.generation,
        tx_digest: format_digest_base58(&a.tx_digest),
        signing_digest: hex::encode(a.signing_digest),
        epoch: a.epoch,
        gas_price: a.gas_price,
        gas_budget: a.gas_budget,
        mint_cap_version: a.mint_cap.version,
        gas_coin_version: a.gas_coin.version,
        treasury_signed: a.treasury_sig.is_some(),
        gas_signed: a.gas_sig.is_some(),
        treasury_sig_unknown: a.treasury_sig_unknown,
        gas_sig_unknown: a.gas_sig_unknown,
        sends: a.sends,
        status: match &a.disposition {
            AttemptDisposition::Open => AttemptStatusView::Open,
            AttemptDisposition::NeverSigned { reason, .. } => AttemptStatusView::NeverSigned { reason: reason.clone() },
            AttemptDisposition::FailedFinal { checkpoint, error } => AttemptStatusView::FailedFinal { checkpoint: *checkpoint, error: error.clone() },
            AttemptDisposition::LandedFinal { checkpoint } => AttemptStatusView::LandedFinal { checkpoint: *checkpoint },
        },
    }
}

pub fn mint_op_view(op: &MintOp) -> MintOpView {
    MintOpView {
        id: op.id,
        deposit_id: op.deposit_id,
        mint_id: hex32(&op.mint_id),
        sui_recipient: format_address(&op.recipient),
        amount_e8s: op.amount_e8s,
        status: match &op.state {
            MintOpState::Active => MintOpStatusView::Active,
            MintOpState::Delivered { tx_digest, checkpoint, sequence, via } => MintOpStatusView::Delivered {
                tx_digest: tx_digest.as_ref().map(format_digest_base58),
                checkpoint: *checkpoint,
                sequence: *sequence,
                via: match via {
                    DeliveryVia::Receipt => DeliveryViaView::Receipt,
                    DeliveryVia::ConsumedRecord => DeliveryViaView::ConsumedRecord,
                },
            },
            MintOpState::Failed { reason, .. } => MintOpStatusView::Failed { reason: reason.clone() },
        },
        hold_reason: op.hold_reason.clone(),
        attempts: op.attempts.iter().map(attempt_view).collect(),
        created_at_ns: op.created_at_ns,
        updated_at_ns: op.updated_at_ns,
    }
}

pub fn mint_lane_view() -> MintLaneView {
    let blocked_by = crate::flows::mint::blocked_by();
    state::read_runtime(|c| MintLaneView {
        ready: blocked_by.is_empty(),
        blocked_by,
        active_op: c.active_mint_op,
        lane_halt: c.mint_lane_halted.as_ref().map(|h| h.reason.clone()),
        gas_coin: c.config.lane.gas_coin.as_ref().map(format_address),
        gas_budget: c.config.lane.gas_budget,
        max_attempts: c.config.lane.max_attempts,
        last_wait: c.lane_wait.clone(),
        last_final_checkpoint: c.last_final_checkpoint,
        observed: c.observed.as_ref().map(observed_view),
    })
}
