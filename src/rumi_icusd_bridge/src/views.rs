//! Projections from stable records to public Candid views.

use crate::accounting::Buckets;
use crate::env;
use crate::ids::{self, LaneKind};
use crate::state::{
    self, BridgeCore, DepositKind, DepositRecord, DepositState, LaneState, LedgerOpStatus, ReleaseRecord,
    ReleaseState, TxStatus,
};
use crate::types::*;
use candid::Nat;

pub fn deposit_view(d: &DepositRecord) -> DepositView {
    let txs: Vec<state::EvmTxRecord> = d.tx_ids.iter().filter_map(|id| state::get_tx(*id)).collect();
    let mint_tx_hashes = txs.iter().flat_map(|t| t.attempts.iter().map(|a| ids::hex0x(&a.tx_hash))).collect();
    let mint_stage = d.active_tx.and_then(state::get_tx).map(|t| match t.status {
        TxStatus::Unsigned => BaseTxStage::Queued,
        TxStatus::Pending => BaseTxStage::Submitted,
        TxStatus::Included { block_number, .. } => BaseTxStage::Included { block_number },
        TxStatus::Finalized { block_number, success, .. } => BaseTxStage::Finalized { block_number, success },
    });
    DepositView {
        id: d.id,
        kind: match &d.kind {
            DepositKind::UserDeposit => DepositKindView::UserDeposit,
            DepositKind::Remint { transfer_id } => DepositKindView::Remint { transfer_id: ids::hex0x(transfer_id) },
        },
        owner: d.owner,
        from: d.from,
        base_recipient: ids::hex0x(&d.base_recipient),
        amount_e8s: d.amount_e8s,
        deposit_fee_e8s: d.deposit_fee_e8s,
        mint_amount_e8s: d.mint_amount_e8s,
        mint_id: ids::hex0x(&d.mint_id),
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
        mint_tx_hashes,
        mint_stage,
        delivered: d.delivered.as_ref().map(|i| BaseInclusion {
            tx_hash: ids::hex0x(&i.tx_hash),
            block_number: i.block_number,
            block_hash: ids::hex0x(&i.block_hash),
        }),
        created_at_ns: d.created_at_ns,
        updated_at_ns: d.updated_at_ns,
    }
}

fn succeeded_block(op: &state::LedgerOp) -> Option<u64> {
    match op.status {
        LedgerOpStatus::Succeeded { block_index } => Some(block_index),
        _ => None,
    }
}

pub fn release_view(r: &ReleaseRecord) -> ReleaseView {
    let unknown = matches!(r.transfer.as_ref().map(|t| &t.status), Some(LedgerOpStatus::Unknown { .. }));
    ReleaseView {
        transfer_id: ids::hex0x(&r.transfer_id),
        sequence: r.sequence,
        base_sender: ids::hex0x(&r.base_sender),
        amount_e8s: r.amount_e8s,
        release_fee_e8s: r.release_fee_e8s,
        ledger_fee_e8s: r.transfer.as_ref().and_then(|t| t.fee_e8s),
        amount_to_receive_e8s: r.transfer.as_ref().map(|t| t.amount_e8s),
        destination: r.destination,
        raw_icp_principal: serde_bytes::ByteBuf::from(r.raw_icp_principal.clone()),
        icp_subaccount: serde_bytes::ByteBuf::from(r.icp_subaccount.to_vec()),
        base_tx_hash: ids::hex0x(&r.base_tx_hash),
        base_block_number: r.base_block_number,
        log_index: r.log_index,
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

fn lane_status(kind: LaneKind, l: &LaneState) -> LaneStatus {
    LaneStatus {
        lane: kind,
        address: l.address.map(|a| ids::hex0x(&a)),
        next_nonce: l.next_nonce,
        unincluded_tx: l.unincluded_tx,
        unfinalized_txs: l.unfinalized_txs.len() as u32,
        awaiting_gas: l.awaiting_gas,
        halted: l.halted.clone(),
    }
}

pub fn gas_view(c: &BridgeCore) -> GasConfig {
    let g = &c.config.gas;
    GasConfig {
        max_fee_per_gas_wei: Nat::from(g.max_fee_per_gas_wei),
        max_priority_fee_per_gas_wei: Nat::from(g.max_priority_fee_per_gas_wei),
        mint_gas_limit: g.mint_gas_limit,
        admin_gas_limit: g.admin_gas_limit,
        min_gas_reserve_wei: Nat::from(g.min_gas_reserve_wei),
        replace_after_secs: g.replace_after_secs,
    }
}

pub fn observer_config_view(c: &BridgeCore) -> ObserverConfig {
    let o = &c.config.observer;
    ObserverConfig {
        logs_chunk_blocks: o.logs_chunk_blocks,
        logs_max_response_bytes: o.logs_max_response_bytes,
        default_max_response_bytes: o.default_max_response_bytes,
        idle_interval_secs: o.idle_interval_secs,
        active_interval_secs: o.active_interval_secs,
        cycles_per_rpc_call: Nat::from(o.cycles_per_rpc_call),
    }
}

pub fn bridge_info() -> BridgeInfo {
    let canister = env::canister_id();
    state::read_core(|c| BridgeInfo {
        version: ids::CONTRACT_VERSION.to_string(),
        chain_id: c.config.chain_id,
        icusd_ledger: c.config.icusd_ledger,
        escrow_account: c.escrow_account(canister),
        contract: c.config.contract.as_ref().map(|b| ContractBindingView {
            address: ids::hex0x(&b.address),
            bound_at_ns: b.bound_at_ns,
            start_block: b.start_block,
            supply_cap_e8s: Nat::from(b.supply_cap),
            mint_window_limit_e8s: Nat::from(b.mint_window_limit),
            mint_window_seconds: b.mint_window_seconds,
            min_bridge_out_e8s: Nat::from(b.min_bridge_out),
        }),
        limits: c.config.limits.clone(),
        fees: c.config.fees.clone(),
        gas: gas_view(c),
        observer_config: observer_config_view(c),
        pauses: c.config.pauses.clone(),
        halted: c.halted.as_ref().map(|h| h.reason.clone()),
        finality_policy: format!(
            "Base `finalized` head (L1 batch finality), lowest value agreed by {} of {} providers, block hash confirmed by the same quorum",
            c.config.min_quorum,
            c.config.rpc_services.len()
        ),
        min_quorum: c.config.min_quorum,
        rpc_service_count: c.config.rpc_services.len() as u32,
        observer: ObserverStatus {
            cursor_block: c.observer.cursor_block,
            processed_count: c.observer.processed_count,
            last_finalized_block: c.observer.last_finalized.map(|(n, _)| n),
            last_error: c.observer.last_error.clone(),
            last_run_ns: c.observer.last_run_ns,
            pending_hints: c
                .hints
                .iter()
                .map(|h| HintView {
                    tx_hash: ids::hex0x(&h.tx_hash),
                    submitted_by: h.submitted_by,
                    submitted_at_ns: h.submitted_at_ns,
                })
                .collect(),
        },
        lanes: vec![lane_status(LaneKind::Minter, &c.minter_lane), lane_status(LaneKind::Admin, &c.admin_lane)],
        ledger_fee_e8s: c.ledger_fee_e8s,
        active_deposits: state::active_deposit_count() as u32,
        active_releases: state::active_release_count() as u32,
        mint_window_used_e8s: c.windows.admitted_in_window.min(u64::MAX as u128) as u64,
        release_window_used_e8s: c.windows.released_in_window.min(u64::MAX as u128) as u64,
        window_resets_at_ns: c.window_resets_at_ns(),
    })
}

pub const REPORTING_NOTE: &str = "Escrowed icUSD is already part of the ICP ledger total supply. \
    total_icusd = icp_ledger_total_supply + debt_backed_chain_supplies (backend get_global_icusd_supply). \
    circulating_on_icp = icp_ledger_total_supply - escrow_balance. circulating_on_base = base_total_supply. \
    Never add Base supply to total_icusd.";

pub fn supply_audit() -> SupplyAudit {
    let canister = env::canister_id();
    let inflight = state::inflight_mint_e8s();
    state::read_core(|c| {
        let b: &Buckets = &c.buckets;
        let liabilities = b.liabilities().unwrap_or(u128::MAX);
        let required = liabilities.saturating_sub(b.unknown_outflow);
        // Report the verdict and figures of the last reconciliation, not a recomputation
        // against buckets that may have moved since the balance was read.
        let snap = c.escrow_snapshot.as_ref();
        let surplus = snap.and_then(|s| s.balance.checked_sub(s.required));
        let deficit = snap.and_then(|s| s.required.checked_sub(s.balance).filter(|d| *d > 0));
        let ledger_total = c.escrow_snapshot.as_ref().and_then(|s| s.ledger_total_supply);
        let escrow = c.escrow_snapshot.as_ref().map(|s| s.balance);
        SupplyAudit {
            escrow_account: c.escrow_account(canister),
            icp_ledger_total_supply_e8s: ledger_total.map(Nat::from),
            circulating_on_icp_e8s: match (ledger_total, escrow) {
                (Some(t), Some(e)) => Some(Nat::from(t.saturating_sub(e))),
                _ => None,
            },
            circulating_on_base_e8s: c.base_supply_snapshot.as_ref().map(|s| Nat::from(s.total_supply)),
            escrow_balance_e8s: c.escrow_snapshot.as_ref().map(|s| Nat::from(s.balance)),
            escrow_balance_checked_at_ns: c.escrow_snapshot.as_ref().map(|s| s.at_ns),
            outstanding_base_e8s: Nat::from(b.outstanding_base),
            pending_mint_e8s: Nat::from(b.pending_mint),
            pending_release_e8s: Nat::from(b.pending_release),
            pending_refund_e8s: Nat::from(b.pending_refund),
            held_e8s: Nat::from(b.held),
            fees_e8s: Nat::from(b.fees),
            reserved_pulls_e8s: Nat::from(b.reserved_pulls),
            unknown_outflow_e8s: Nat::from(b.unknown_outflow),
            liabilities_e8s: Nat::from(liabilities),
            required_escrow_e8s: Nat::from(required),
            surplus_e8s: surplus.map(Nat::from),
            deficit_e8s: deficit.map(Nat::from),
            coverage_ok: snap.map(|s| s.coverage_ok),
            base_total_supply_e8s: c.base_supply_snapshot.as_ref().map(|s| Nat::from(s.total_supply)),
            base_supply_block: c.base_supply_snapshot.as_ref().map(|s| s.block_number),
            inflight_mint_e8s: Nat::from(inflight),
            base_supply_within_bound: c.base_supply_snapshot.as_ref().map(|s| s.within_bound),
            base_supply_exact_match: c.base_supply_snapshot.as_ref().and_then(|s| s.exact_match),
            reporting_note: REPORTING_NOTE.to_string(),
        }
    })
}

pub fn audit_entry(r: &state::AuditRecord) -> AuditEntry {
    AuditEntry {
        seq: r.seq,
        at_ns: r.at_ns,
        transition: r.transition.clone(),
        deposit_id: r.deposit_id,
        transfer_id: r.transfer_id.map(|t| ids::hex0x(&t)),
        tx_id: r.tx_id,
        note: r.note.clone(),
    }
}
