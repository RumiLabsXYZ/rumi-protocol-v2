//! Per-chain settlement queue.
//!
//! Each registered chain owns one `SettlementQueueV1` carrying outbound ops
//! the canister still needs to sign and submit. Phase 1a defines the shape
//! and the enqueue/idempotency rules. Phase 1b adds the Timer-D worker that
//! actually drains the queue against the Monad adapter.
//!
//! Versioned per the spec Section 3. Adding a field bumps to V2 plus a
//! migration in `chains::supply::migrate_multi_chain_state`.

use candid::{CandidType, Deserialize};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub enum SettlementOpKind {
    Mint { recipient: String, amount_e8s: u128, vault_id: u64 },
    Withdrawal { recipient: String, amount_e8s: u128 },
    Burn { amount_e8s: u128 },
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub enum SettlementOpStatus {
    Queued,
    Inflight { tries: u32, last_attempt_ns: u64 },
    Succeeded { tx_hash: String, confirmed_ns: u64 },
    Failed { reason: String, failed_ns: u64 },
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct SettlementOp {
    pub op_id: u64,
    pub kind: SettlementOpKind,
    pub idempotency_key: String,
    pub enqueued_at_ns: u64,
    pub status: SettlementOpStatus,
    /// Hash of the most recent on-chain submission for this op, set by the
    /// Phase-1b Timer-D worker when the op goes Inflight. Read back on the
    /// Confirm path to fetch the receipt. `#[serde(default)]` keeps any
    /// pre-existing (V1) snapshot decoding cleanly into the V2 state.
    #[serde(default)]
    pub last_tx_hash: Option<String>,
}

impl SettlementOp {
    pub fn new(kind: SettlementOpKind, idempotency_key: String, now_ns: u64) -> Self {
        Self {
            op_id: 0,
            kind,
            idempotency_key,
            enqueued_at_ns: now_ns,
            status: SettlementOpStatus::Queued,
            last_tx_hash: None,
        }
    }

    pub fn mark_inflight(&mut self, now_ns: u64) {
        let tries = match &self.status {
            SettlementOpStatus::Inflight { tries, .. } => tries.saturating_add(1),
            _ => 1,
        };
        self.status = SettlementOpStatus::Inflight { tries, last_attempt_ns: now_ns };
    }

    pub fn mark_succeeded(&mut self, tx_hash: String, now_ns: u64) {
        self.status = SettlementOpStatus::Succeeded { tx_hash, confirmed_ns: now_ns };
    }

    pub fn mark_failed(&mut self, reason: String, now_ns: u64) {
        self.status = SettlementOpStatus::Failed { reason, failed_ns: now_ns };
    }
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, Default)]
pub struct SettlementQueueV1 {
    /// Lowest enqueued op_id still pending. Advances as ops complete.
    pub head: u64,
    /// Next op_id to assign. Always >= head.
    pub tail: u64,
    /// Pending ops indexed by op_id. Drained head-first by Phase-1b's Timer D.
    pub pending: BTreeMap<u64, SettlementOp>,
    /// Idempotency keys seen on this queue. Enqueue rejects duplicates.
    pub seen_idempotency_keys: BTreeSet<String>,
    /// FIFO ordering hint for the drain loop. Phase 1a never reads it; kept
    /// so Phase 1b can drain in enqueue order without scanning `pending`.
    pub drain_order: VecDeque<u64>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SettlementQueueError {
    DuplicateIdempotencyKey(String),
}

impl SettlementQueueV1 {
    pub fn enqueue(&mut self, mut op: SettlementOp) -> Result<u64, SettlementQueueError> {
        if self.seen_idempotency_keys.contains(&op.idempotency_key) {
            return Err(SettlementQueueError::DuplicateIdempotencyKey(
                op.idempotency_key,
            ));
        }
        let assigned = self.tail;
        op.op_id = assigned;
        self.seen_idempotency_keys.insert(op.idempotency_key.clone());
        self.drain_order.push_back(assigned);
        self.pending.insert(assigned, op);
        self.tail = self.tail.saturating_add(1);
        Ok(assigned)
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }
}
