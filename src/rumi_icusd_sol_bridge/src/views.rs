//! Projections from stable records to public Candid views (49b).

use crate::ids::Hash32;
use crate::records::{OpKind, OpState, SolOp};
use crate::state::{self, AuditRecord, DepositKind, DepositRecord, DepositState, Hint, LedgerOp, LedgerOpStatus, ReleaseRecord, ReleaseState};
use crate::tx::{pubkey_to_base58, signature_to_base58};
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
        solana_recipient: pubkey_to_base58(&d.solana_recipient),
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
        delivered: d.delivered.as_ref().map(|m| SolanaInclusion { signature: signature_to_base58(&m.signature.0), slot: m.slot }),
        created_at_ns: d.created_at_ns,
        updated_at_ns: d.updated_at_ns,
    }
}

pub fn release_view(r: &ReleaseRecord) -> ReleaseView {
    let unknown = matches!(r.transfer.as_ref().map(|t| &t.status), Some(LedgerOpStatus::Unknown { .. }));
    ReleaseView {
        return_id: hex32(&r.return_id),
        seq: r.seq,
        burn_op_id: r.burn_op_id,
        solana_sender: pubkey_to_base58(&r.solana_sender),
        amount_e8s: r.amount_e8s,
        release_fee_e8s: r.release_fee_e8s,
        ledger_fee_e8s: r.transfer.as_ref().and_then(|t| t.fee_e8s),
        amount_to_receive_e8s: r.transfer.as_ref().map(|t| t.amount_e8s),
        destination: r.destination,
        return_signature: signature_to_base58(&r.return_signature.0),
        return_slot: r.return_slot,
        burn_signature: signature_to_base58(&r.burn_signature.0),
        burn_slot: r.burn_slot,
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

pub fn op_view(o: &SolOp) -> OpView {
    OpView {
        id: o.id,
        kind: match o.kind {
            OpKind::Mint => OpKindView::Mint,
            OpKind::Burn => OpKindView::Burn,
            OpKind::Refund => OpKindView::Refund,
        },
        lane: o.lane,
        external_id: hex32(&o.external_id),
        amount: o.amount,
        state: match &o.state {
            OpState::Active => OpStateView::Active,
            OpState::Delivered { signature, slot, adopted } => {
                OpStateView::Delivered { signature: signature_to_base58(&signature.0), slot: *slot, adopted: *adopted }
            }
            OpState::NeedsReissue { reason } => OpStateView::NeedsReissue { reason: format!("{reason:?}") },
            OpState::Held { reason } => OpStateView::Held { reason: reason.clone() },
            OpState::Abandoned { reason, .. } => OpStateView::Abandoned { reason: reason.clone() },
        },
        generation: o.generation,
        slot_index: o.slot_index,
        tx_ids: o.attempts.iter().flat_map(|a| a.tx_ids.iter().map(|s| signature_to_base58(&s.0))).collect(),
        note: o.note.clone(),
        created_at_ns: o.created_at_ns,
        updated_at_ns: o.updated_at_ns,
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

pub fn hint_view(h: &Hint) -> HintView {
    HintView { signature: signature_to_base58(&h.signature.0), submitted_by: h.submitted_by, submitted_at_ns: h.submitted_at_ns }
}

/// Operations that are not terminal, ascending by id after `start_after`, at most `limit`.
pub fn unresolved_ops(start_after: Option<u64>, limit: u32) -> Vec<OpView> {
    let limit = limit.clamp(1, state::MAX_PAGE) as usize;
    let mut out = Vec::new();
    let mut cursor = start_after;
    // The non-terminal index is bounded by the pool and the admission limits.
    for id in state::active_op_page(cursor, 1024) {
        if cursor.is_some_and(|c| id <= c) {
            break; // wrapped around
        }
        cursor = Some(id);
        if let Some(op) = state::get_op(id) {
            out.push(op_view(&op));
            if out.len() >= limit {
                break;
            }
        }
    }
    out
}
