//! Bounded growth of the lane's state: retention of terminal operations, compaction of
//! the dedup records, per-sender aggregation of unattributed value, and the refund
//! budget. Everything here is pure (time is a parameter) so it is host-testable.
//!
//! # Dedup correctness under compaction
//!
//! A value-bearing return is consumed once per `(network, issuer, tx hash)`. Two doors
//! can present a transaction: the paged history scan (monotone cursor; it never
//! revisits a ledger below `observer.next_ledger_min`, the fully-scanned frontier) and
//! the wallet hint (any hash). Dropping a dedup record is therefore safe exactly when
//! the transaction can no longer be presented as NEW:
//!
//! * the scan can't: its frontier is already past the record's ledger;
//! * the hint can't, because `apply_verified_return` refuses any receipt at a ledger
//!   below `dedup_floor_ledger` (treated as already settled by the scan).
//!
//! The floor is only ever raised, and only to a ledger strictly below the frontier, so
//! it can never cover an unscanned (hence possibly unprocessed) return. Records at or
//! above the frontier (e.g. a hinted return ahead of the scan) are never compacted.

use super::state::XrplIcusdState;
use super::types::*;
use crate::chains::xrp::network::XrplNetwork;

/// Operations examined per prune sweep (the sweep is round-robin, never a full scan).
const PRUNE_SCAN_PER_PASS: usize = 500;

/// Key of the aggregate for `(network, sender)`.
pub fn aggregate_key(network: XrplNetwork, sender: &str) -> (String, String) {
    (network.name().to_string(), sender.to_string())
}

/// Cap on remembered proven escrow-mint blocks (8 bytes each). Eviction drops the
/// lowest index; that is safe because a rebuilt borrow's proof must also carry a
/// timestamp at or after its reservation (see `borrow::apply_rebuilt_mint_proof`).
pub const MAX_PROVEN_MINT_BLOCKS: usize = 50_000;

impl XrplIcusdState {
    /// Fold unattributed value from `sender` into its aggregate. Never fails for
    /// capacity: when the sender slots are full, the smallest sub-threshold (dust)
    /// aggregate is merged into the overflow aggregate so a LARGER new sender still gets
    /// its own attributed slot; otherwise the value joins the overflow aggregate (held,
    /// operator-visible, never auto-refunded) and, if it is at least the auto-refund
    /// threshold, its per-transaction evidence is kept so a controller can later move
    /// exactly that receipt to its verified sender (`reconcile_xrpl_overflow_receipt`).
    #[allow(clippy::too_many_arguments)]
    pub fn fold_unattributed(
        &mut self,
        network: XrplNetwork,
        sender: &str,
        amount: &WideAmount,
        tx_hash: &str,
        ledger_index: u32,
        reason: &str,
        now_ns: u64,
    ) -> Result<(), XrplIcusdError> {
        self.fold_unattributed_inner(network, sender, amount, tx_hash, ledger_index, reason, now_ns, true)
            .map(|_| ())
    }

    /// `allow_overflow = false` refuses (state untouched) instead of using the overflow
    /// aggregate: used when value is being MOVED OUT of overflow to its own sender.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn fold_unattributed_inner(
        &mut self,
        network: XrplNetwork,
        sender: &str,
        amount: &WideAmount,
        tx_hash: &str,
        ledger_index: u32,
        reason: &str,
        now_ns: u64,
        allow_overflow: bool,
    ) -> Result<bool, XrplIcusdError> {
        let mut key = aggregate_key(network, sender);
        let mut overflowed = false;
        if !self.unattributed.contains_key(&key) && self.unattributed.len() >= MAX_UNATTRIBUTED_SENDERS
        {
            if let Some(victim) = self.evictable_dust_slot(amount) {
                self.merge_into_overflow(&victim, now_ns)?;
            } else if allow_overflow {
                key = aggregate_key(network, OVERFLOW_SENDER);
                overflowed = true;
            } else {
                return Err(XrplIcusdError::Unavailable(
                    "no free sender aggregate slot (all are above the dust threshold); retry once \
                     refunds drain"
                        .into(),
                ));
            }
        }
        let entry = self
            .unattributed
            .entry(key.clone())
            .or_insert_with(|| UnattributedReturns {
                network,
                sender: key.1.clone(),
                total: WideAmount::ZERO,
                tx_count: 0,
                first_ledger: ledger_index,
                last_ledger: ledger_index,
                last_tx: String::new(),
                last_reason: String::new(),
                updated_at_ns: now_ns,
                refunds_issued: 0,
            });
        entry.total = entry
            .total
            .checked_add(amount)
            .map_err(|e| XrplIcusdError::Internal(format!("aggregate overflow: {e:?}")))?;
        entry.tx_count = entry.tx_count.saturating_add(1);
        entry.first_ledger = entry.first_ledger.min(ledger_index);
        entry.last_ledger = entry.last_ledger.max(ledger_index);
        entry.last_tx = tx_hash.to_ascii_uppercase();
        entry.last_reason = reason.chars().take(MAX_NOTE_LEN).collect();
        entry.updated_at_ns = now_ns;
        if overflowed {
            self.record_overflow_evidence(OverflowEvidence {
                network,
                kind: OverflowKind::Unattributed,
                tx_hash: tx_hash.to_ascii_uppercase(),
                sender: sender.to_string(),
                ledger_index,
                destination_tag: None,
                amount: *amount,
                at_ns: now_ns,
            });
        }
        Ok(overflowed)
    }

    /// The smallest per-sender aggregate that is below the auto-refund threshold (dust
    /// that would never be paid out) and strictly smaller than `incoming`. The overflow
    /// aggregate itself is never a candidate.
    fn evictable_dust_slot(&self, incoming: &WideAmount) -> Option<(String, String)> {
        let min = u128::from(self.config.min_auto_refund_e8s);
        self.unattributed
            .iter()
            .filter(|((_, sender), u)| {
                sender != OVERFLOW_SENDER && u.total.whole_e8s < min && u.total < *incoming
            })
            .min_by(|(_, a), (_, b)| a.total.cmp(&b.total))
            .map(|(k, _)| k.clone())
    }

    /// Merge a dust aggregate into its network's overflow aggregate (value exact).
    fn merge_into_overflow(
        &mut self,
        victim: &(String, String),
        now_ns: u64,
    ) -> Result<(), XrplIcusdError> {
        let Some(v) = self.unattributed.get(victim).cloned() else {
            return Ok(());
        };
        let okey = aggregate_key(v.network, OVERFLOW_SENDER);
        let entry = self
            .unattributed
            .entry(okey.clone())
            .or_insert_with(|| UnattributedReturns {
                network: v.network,
                sender: OVERFLOW_SENDER.to_string(),
                total: WideAmount::ZERO,
                tx_count: 0,
                first_ledger: v.first_ledger,
                last_ledger: v.last_ledger,
                last_tx: v.last_tx.clone(),
                last_reason: "dust aggregates merged to free sender slots".into(),
                updated_at_ns: now_ns,
                refunds_issued: 0,
            });
        entry.total = entry
            .total
            .checked_add(&v.total)
            .map_err(|e| XrplIcusdError::Internal(format!("aggregate overflow: {e:?}")))?;
        entry.tx_count = entry.tx_count.saturating_add(v.tx_count);
        entry.first_ledger = entry.first_ledger.min(v.first_ledger);
        entry.last_ledger = entry.last_ledger.max(v.last_ledger);
        entry.updated_at_ns = now_ns;
        self.unattributed.remove(victim);
        Ok(())
    }

    /// Keep per-transaction evidence for an overflow contribution at or above the
    /// auto-refund threshold. Bounded: when full, a larger entry replaces the smallest.
    fn record_overflow_evidence(&mut self, ev: OverflowEvidence) {
        if ev.amount.whole_e8s < u128::from(self.config.min_auto_refund_e8s) {
            return;
        }
        if self.overflow_evidence.len() >= MAX_OVERFLOW_EVIDENCE
            && !self.overflow_evidence.contains_key(&ev.tx_hash)
        {
            let smallest = self
                .overflow_evidence
                .iter()
                .min_by(|(_, a), (_, b)| a.amount.cmp(&b.amount))
                .map(|(k, e)| (k.clone(), e.amount));
            match smallest {
                Some((k, amt)) if ev.amount > amt => {
                    self.overflow_evidence.remove(&k);
                }
                _ => return,
            }
        }
        self.overflow_evidence.insert(ev.tx_hash.clone(), ev);
    }

    /// Hold a value-bearing receipt that failed strict evidence checks. Its value has
    /// already been taken out of `O` by the caller.
    pub fn hold_quarantined(
        &mut self,
        network: XrplNetwork,
        rec: QuarantinedReceipt,
    ) -> Result<(), XrplIcusdError> {
        let overflowed = self.quarantined_receipts.len() >= MAX_QUARANTINED_RECEIPTS
            && !self.quarantined_receipts.contains_key(&rec.tx_hash);
        let key = if overflowed {
            // Overflow entry: summed, still exact, still operator-visible.
            OVERFLOW_SENDER.to_string()
        } else {
            rec.tx_hash.clone()
        };
        let evidence = overflowed.then(|| OverflowEvidence {
            network,
            kind: OverflowKind::Quarantine,
            tx_hash: rec.tx_hash.to_ascii_uppercase(),
            sender: rec.sender.clone(),
            ledger_index: rec.ledger_index,
            destination_tag: rec.destination_tag,
            amount: rec.amount,
            at_ns: rec.at_ns,
        });
        match self.quarantined_receipts.get_mut(&key) {
            Some(e) => {
                e.amount = e
                    .amount
                    .checked_add(&rec.amount)
                    .map_err(|e| XrplIcusdError::Internal(format!("quarantine overflow: {e:?}")))?;
                e.reason = format!("{} (+ {})", e.reason, rec.tx_hash)
                    .chars()
                    .take(MAX_NOTE_LEN)
                    .collect();
            }
            None => {
                let mut rec = rec;
                rec.tx_hash = key.clone();
                self.quarantined_receipts.insert(key, rec);
            }
        }
        if let Some(ev) = evidence {
            self.record_overflow_evidence(ev);
        }
        Ok(())
    }

    /// Remember a proven escrow-mint block (bounded; the lowest index is dropped first).
    pub fn remember_proven_mint_block(&mut self, block: u64) {
        self.proven_escrow_mint_blocks.insert(block);
        while self.proven_escrow_mint_blocks.len() > MAX_PROVEN_MINT_BLOCKS {
            let Some(low) = self.proven_escrow_mint_blocks.iter().next().copied() else {
                break;
            };
            self.proven_escrow_mint_blocks.remove(&low);
        }
    }

    /// Whole-e8s refund amount an aggregate may release now, if any.
    fn refundable_e8s(&self, u: &UnattributedReturns) -> Option<u64> {
        if u.sender == OVERFLOW_SENDER {
            return None;
        }
        if u.total.whole_e8s < u128::from(self.config.min_auto_refund_e8s) {
            return None;
        }
        u64::try_from(u.total.whole_e8s).ok()
    }

    /// True when an aggregate could be cut into a refund operation right now.
    pub fn refund_drain_possible(&self) -> bool {
        self.config.enabled
            && self.system_refund_ops.len() < MAX_SYSTEM_REFUND_OPS
            && self
                .unattributed
                .values()
                .any(|u| self.refundable_e8s(u).is_some())
    }

    /// Cut refund operations out of aggregates that reached the threshold: at most
    /// [`REFUNDS_PER_PASS`] per call and never past [`MAX_SYSTEM_REFUND_OPS`] live
    /// system refund records (value beyond the budget simply stays in its aggregate).
    /// Each refund pays the whole-e8s part of the aggregate; the sub-e8s remainder
    /// stays aggregated, exactly. Returns the created operation ids.
    pub fn drain_refunds(&mut self, now_ns: u64) -> Vec<OperationId> {
        let mut created = Vec::new();
        if !self.config.enabled {
            return created;
        }
        let keys: Vec<(String, String)> = self
            .unattributed
            .iter()
            .filter(|(_, u)| self.refundable_e8s(u).is_some())
            .map(|(k, _)| k.clone())
            .collect();
        for key in keys {
            if created.len() >= REFUNDS_PER_PASS || self.system_refund_ops.len() >= MAX_SYSTEM_REFUND_OPS
            {
                break;
            }
            let Some(u) = self.unattributed.get(&key).cloned() else {
                continue;
            };
            let Some(e8s) = self.refundable_e8s(&u) else {
                continue;
            };
            let Ok(amount) = crate::chains::xrp::issued_amount::ExactIssuedAmount::try_from_e8s(e8s)
            else {
                continue;
            };
            // Creation is one synchronous step, so uniqueness (not replay idempotency)
            // is all the nonce needs: it is bound to the never-reused operation id.
            let id = self.allocate_operation_id();
            let nonce = super::repay::aggregate_refund_nonce(u.network, &u.sender, id);
            let owner = system_owner();
            let mut op = Operation {
                id,
                owner,
                created_at_ns: now_ns,
                updated_at_ns: now_ns,
                request_nonce: nonce,
                network: u.network,
                kind: OperationKind::Refund {
                    source_tx: format!("aggregate:{}:{}", u.last_tx, u.refunds_issued),
                    recipient: u.sender.clone(),
                    amount,
                },
                phase: Phase::RefundPending,
                ic_jobs: vec![],
                xrpl_attempts: vec![],
                notes: vec![],
                resolution: None,
                attempt_budget_base: 0,
                delivery_halted: false,
            };
            op.push_note(format!(
                "refund of {} unattributed return(s) from {} (last: {})",
                u.tx_count, u.sender, u.last_reason
            ));
            self.request_nonce_index.insert((owner, nonce), id);
            self.next_operation_id = self.next_operation_id.max(id + 1);
            self.operations.insert(id, op);
            self.register_op(id);
            // Move the value out of the aggregate in the same step it joins the
            // refund operation (the liability sum never changes).
            let remaining = u
                .total
                .checked_sub(&WideAmount::from_e8s(e8s))
                .unwrap_or(WideAmount::ZERO);
            if remaining.is_zero() {
                self.unattributed.remove(&key);
            } else if let Some(e) = self.unattributed.get_mut(&key) {
                e.total = remaining;
                e.refunds_issued = e.refunds_issued.saturating_add(1);
                e.tx_count = 0;
            }
            created.push(id);
        }
        created
    }

    // ---- retention --------------------------------------------------------------------

    /// One bounded maintenance pass: normalise stale in-flight calls, drop settled
    /// operations from the live set, prune old terminal operations and compact the
    /// dedup records. Returns the number of operations pruned.
    pub fn maintain(&mut self, now_ns: u64) -> usize {
        self.expire_stale_in_flight(now_ns);
        let settled: Vec<OperationId> = self
            .live_ops
            .iter()
            .copied()
            .filter(|id| self.is_settled(*id))
            .collect();
        for id in settled {
            self.live_ops.remove(&id);
        }
        let pruned = self.prune_terminal(now_ns);
        self.compact_processed_returns();
        pruned
    }

    /// Can this operation be deleted? Terminal, past retention, settled, every ledger
    /// job resolved AND proven, no issuer attempt or lease, no queued work.
    fn prunable(&self, op: &Operation, now_ns: u64) -> bool {
        op.phase.is_terminal()
            && now_ns.saturating_sub(op.updated_at_ns) > OP_RETENTION_NS
            && op.unresolved_attempt().is_none()
            && self.issuer_lease != Some(LeaseHolder::Operation(op.id))
            && op
                .ic_jobs
                .iter()
                .all(|j| j.is_resolved() && (j.done_block().is_none() || j.proof_verified))
            // A borrow whose fee-remainder mint is held, queued or awaiting retry still
            // owes the treasury value and the operator needs its record.
            && !super::borrow::fee_mint_unresolved(op)
            && self.is_settled(op.id)
    }

    /// Round-robin retention sweep over at most [`PRUNE_SCAN_PER_PASS`] operations.
    /// The nonce of a Borrow/Import is kept (a replayed request must never create a
    /// second operation); repayment/refund nonces and destination tags are released
    /// (an unknown tag or nonce is handled as unattributed / a fresh intent).
    pub fn prune_terminal(&mut self, now_ns: u64) -> usize {
        let start = self.prune_cursor;
        let mut examined: Vec<OperationId> = self
            .operations
            .range(start..)
            .map(|(id, _)| *id)
            .take(PRUNE_SCAN_PER_PASS)
            .collect();
        if examined.len() < PRUNE_SCAN_PER_PASS {
            // Wrap around.
            let room = PRUNE_SCAN_PER_PASS - examined.len();
            examined.extend(self.operations.range(..start).map(|(id, _)| *id).take(room));
            self.prune_cursor = 0;
        } else {
            self.prune_cursor = examined.last().map(|i| i + 1).unwrap_or(0);
        }
        let mut pruned = 0;
        for id in examined {
            let ok = self
                .operations
                .get(&id)
                .is_some_and(|op| self.prunable(op, now_ns));
            if !ok {
                continue;
            }
            self.remove_operation(id);
            pruned += 1;
            if pruned >= PRUNE_PER_PASS {
                break;
            }
        }
        pruned
    }

    pub fn remove_operation(&mut self, id: OperationId) {
        let Some(op) = self.operations.remove(&id) else {
            return;
        };
        // Its proven escrow-mint blocks stay spent after the record is gone.
        for j in &op.ic_jobs {
            if j.purpose == IcJobPurpose::EscrowMint {
                if let Some(b) = j.done_block() {
                    self.remember_proven_mint_block(b);
                }
            }
        }
        self.live_ops.remove(&id);
        self.reservations.remove(&id);
        self.system_refund_ops.remove(&id);
        if let Some(v) = op.kind.vault_id() {
            if let Some(set) = self.vault_ops.get_mut(&v) {
                set.remove(&id);
                if set.is_empty() {
                    self.vault_ops.remove(&v);
                }
            }
        }
        match &op.kind {
            OperationKind::Repayment {
                destination_tag, ..
            } => {
                self.tag_index.remove(destination_tag);
                self.request_nonce_index.remove(&(op.owner, op.request_nonce));
            }
            OperationKind::Refund { .. } => {
                self.request_nonce_index.remove(&(op.owner, op.request_nonce));
            }
            // Borrow / Import keep their nonce entry: the request stays spent.
            _ => {}
        }
    }

    /// Drop dedup records the scan can no longer re-present (see the module doc) and
    /// raise `dedup_floor_ledger` accordingly. Gentle (retention window) normally;
    /// aggressive (everything behind the frontier) when near the hard cap.
    pub fn compact_processed_returns(&mut self) {
        let Some(frontier) = self.observer.next_ledger_min else {
            return;
        };
        let near_cap = self.processed_returns.len() >= MAX_PROCESSED_RETURNS / 10 * 9;
        let cutoff = if near_cap {
            frontier
        } else {
            frontier.saturating_sub(PROCESSED_RETAIN_LEDGERS)
        };
        if cutoff <= self.dedup_floor_ledger {
            return;
        }
        self.processed_returns.retain(|_, p| p.ledger_index >= cutoff);
        self.dedup_floor_ledger = cutoff;
    }
}
