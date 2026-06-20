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
    /// Return native gas-asset (MON) collateral to `recipient`. `amount_e18`
    /// is wei (1e18 scale, NOT e8s — it goes straight into the EIP-1559
    /// `value`). `vault_id` lets the settlement worker emit `WithdrawalSigned`
    /// with the real id and flip the vault `Closing -> Closed` on confirmation
    /// (replaces the old placeholder `Withdrawal { recipient, amount_e8s }`,
    /// which was never enqueued anywhere and carried the wrong unit + no id).
    NativeWithdrawal { recipient: String, amount_e18: u128, vault_id: u64 },
    Burn { amount_e8s: u128 },
    /// Task 12 (Option B): realize accrued interest by minting `amount_e8s`
    /// icUSD to the per-chain interest-treasury address. `mint_id` is a FRESH,
    /// globally-unique id (from `chain_vault_id_counter`) used as the on-chain
    /// `IcUSD.mint` vault_id arg + the Mint-log match key — the REAL vault
    /// already minted once at open and `IcUSD.mint` reverts a repeat vault_id.
    /// `vault_id` is the REAL vault the interest is charged to; the confirm path
    /// grows ITS `debt_e8s` and advances its `last_interest_accrual_ns` to
    /// `accrual_through_ns` (the harvest snapshot time). `recipient` is the
    /// per-chain interest-treasury address (resolved once at harvest time), used
    /// as the `IcUSD.mint` `to:` and recipient-verified on confirm. EVM-only in
    /// Phase 1b.
    InterestMint {
        vault_id: u64,
        mint_id: u64,
        amount_e8s: u128,
        accrual_through_ns: u64,
        recipient: String,
    },
    /// Tier-1 bot PSM swap: sell `collateral_in_native` seized collateral -> the
    /// settle-stable (USDC) reserve to retire `debt_to_clear_e8s` (spec §4.8/§8).
    /// Enqueued by `begin_liquidation_in_state` in Increment 2 but INERT until
    /// Increment 3 wires the submit/confirm/revert arms: `select_next_op` SKIPS
    /// it, so it never executes and never head-of-line-blocks user ops.
    /// `min_usdc_out_native` is advisory — recomputed just-in-time at submit from
    /// live reserves (Inc 3).
    LiquidationSwap {
        vault_id: u64,
        collateral_in_native: u128,
        min_usdc_out_native: u128,
        debt_to_clear_e8s: u128,
        router: String,
        pair: String,
        path: Vec<String>,
        reserve_recipient: String,
        deadline_secs: u64,
    },
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
    /// EVM nonce this op was first submitted at, recorded by the Task-11
    /// settlement worker on the submit path. The stuck-tx replace-by-fee path
    /// re-signs and rebroadcasts on THIS nonce (never a fresh `latest` read) so
    /// a bumped-gas resubmit replaces the stuck tx rather than minting a second
    /// time. `None` until first submitted. `#[serde(default)]` keeps any
    /// pre-existing snapshot decoding cleanly.
    #[serde(default)]
    pub submit_nonce: Option<u64>,
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
            submit_nonce: None,
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

    /// True iff the queue has any NON-terminal op (`Queued` or `Inflight`) — i.e.
    /// settlement work the worker will submit or confirm. Terminal ops
    /// (`Succeeded`/`Failed`, before `prune_terminal` reaps them) do not count.
    ///
    /// Used by the observer to gate the per-tick hot-wallet balance refresh: the
    /// cached balance only feeds the submit-path gas gate, so there is no reason
    /// to spend an `eth_getBalance` outcall every tick when nothing is in flight.
    pub fn has_active_op(&self) -> bool {
        self.pending.values().any(|op| {
            matches!(
                op.status,
                SettlementOpStatus::Queued | SettlementOpStatus::Inflight { .. }
            )
        })
    }

    /// True iff the queue has a NON-terminal supply-increasing mint op (`Mint`
    /// or, Task 12, `InterestMint`) in `Queued`/`Inflight`. Distinct from
    /// `has_active_op` (which also counts `NativeWithdrawal`): only a mint changes
    /// on-chain icUSD `totalSupply`, so only a live mint can mask a burn from the
    /// observer's supply-equality backstop (`backstop_should_scan`). An interest
    /// mint also raises `totalSupply`, so it counts too. Terminal
    /// (`Succeeded`/`Failed`) ops never count.
    pub fn has_active_mint_op(&self) -> bool {
        self.pending.values().any(|op| {
            matches!(op.kind, SettlementOpKind::Mint { .. } | SettlementOpKind::InterestMint { .. })
                && matches!(
                    op.status,
                    SettlementOpStatus::Queued | SettlementOpStatus::Inflight { .. }
                )
        })
    }

    /// Reap terminal (`Succeeded`/`Failed`) ops so `pending` does not grow
    /// monotonically (the Task-10 review flagged `select_next_op`'s per-tick
    /// scan over an ever-growing `pending` as a cycle-cost anti-pattern this
    /// protocol must avoid). Called once per `run_settlement` tick.
    ///
    /// - Removes terminal ops from `pending` and drops their ids from
    ///   `drain_order`.
    /// - Advances `head` to the lowest op_id still pending (or to `tail` when
    ///   `pending` is now empty), matching `head`'s documented meaning
    ///   ("lowest enqueued op_id still pending").
    /// - Leaves `seen_idempotency_keys` INTACT: it is the replay/duplicate
    ///   guard, and pruning it would re-admit an already-processed op.
    ///   `select_next_op` only ever returns Inflight/Queued ops, so removing
    ///   terminal ops cannot change which live op it selects.
    pub fn prune_terminal(&mut self) {
        let terminal: Vec<u64> = self
            .pending
            .iter()
            .filter(|(_, op)| {
                matches!(
                    op.status,
                    SettlementOpStatus::Succeeded { .. } | SettlementOpStatus::Failed { .. }
                )
            })
            .map(|(&id, _)| id)
            .collect();

        if terminal.is_empty() {
            return;
        }

        for id in &terminal {
            self.pending.remove(id);
        }
        self.drain_order.retain(|id| !terminal.contains(id));

        // `pending` is a BTreeMap, so its first key is the lowest remaining
        // op_id. With nothing left, head meets tail (the next id to assign).
        self.head = self.pending.keys().next().copied().unwrap_or(self.tail);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mint_op(key: &str) -> SettlementOp {
        SettlementOp::new(
            SettlementOpKind::Mint {
                recipient: "0xabc".to_string(),
                amount_e8s: 1_000,
                vault_id: 7,
            },
            key.to_string(),
            0,
        )
    }

    #[test]
    fn prune_terminal_keeps_live_op_advances_head_and_preserves_idempotency() {
        let mut q = SettlementQueueV1::default();
        let id0 = q.enqueue(mint_op("k0")).unwrap(); // 0
        let id1 = q.enqueue(mint_op("k1")).unwrap(); // 1
        let id2 = q.enqueue(mint_op("k2")).unwrap(); // 2
        assert_eq!((id0, id1, id2), (0, 1, 2));
        assert_eq!(q.tail, 3);
        assert_eq!(q.pending_len(), 3);

        // Mark op 0 Succeeded and op 1 Failed; op 2 stays Queued (live).
        q.pending.get_mut(&id0).unwrap().mark_succeeded("0xhash".to_string(), 10);
        q.pending.get_mut(&id1).unwrap().mark_failed("reverted".to_string(), 11);

        q.prune_terminal();

        // Only the non-terminal op remains.
        assert_eq!(q.pending_len(), 1);
        assert!(q.pending.contains_key(&id2));
        assert!(!q.pending.contains_key(&id0));
        assert!(!q.pending.contains_key(&id1));

        // head advanced to the lowest remaining op_id (2); tail unchanged.
        assert_eq!(q.head, 2);
        assert_eq!(q.tail, 3);

        // drain_order dropped the pruned ids, kept the live one.
        assert_eq!(q.drain_order.iter().copied().collect::<Vec<_>>(), vec![2]);

        // seen_idempotency_keys is INTACT: a previously-seen key is still rejected.
        let dup = q.enqueue(mint_op("k0"));
        assert!(matches!(dup, Err(SettlementQueueError::DuplicateIdempotencyKey(k)) if k == "k0"));

        // select_next_op still returns the live Queued op (semantics unchanged).
        let live = q.pending.values().find(|o| matches!(o.status, SettlementOpStatus::Queued));
        assert!(live.is_some());
    }

    #[test]
    fn prune_terminal_empties_pending_and_meets_tail() {
        let mut q = SettlementQueueV1::default();
        let id0 = q.enqueue(mint_op("a")).unwrap();
        let id1 = q.enqueue(mint_op("b")).unwrap();
        q.pending.get_mut(&id0).unwrap().mark_succeeded("0x1".to_string(), 1);
        q.pending.get_mut(&id1).unwrap().mark_succeeded("0x2".to_string(), 2);

        q.prune_terminal();

        assert_eq!(q.pending_len(), 0);
        // With nothing pending, head meets tail (next id to assign).
        assert_eq!(q.head, q.tail);
        assert_eq!(q.tail, 2);
        // Idempotency guard survives even a full drain.
        assert!(q.enqueue(mint_op("a")).is_err());
    }

    #[test]
    fn prune_terminal_noop_when_no_terminal_ops() {
        let mut q = SettlementQueueV1::default();
        q.enqueue(mint_op("x")).unwrap();
        let before_head = q.head;
        q.prune_terminal();
        assert_eq!(q.pending_len(), 1);
        assert_eq!(q.head, before_head);
    }
}
