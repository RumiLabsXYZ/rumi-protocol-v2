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
        delivered: d.delivered.as_ref().map(|m| SuiInclusion { tx_digest: format_digest_base58(&m.tx_digest), checkpoint: m.checkpoint }),
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
