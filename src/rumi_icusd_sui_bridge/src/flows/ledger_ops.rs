//! Generic lifecycle for persisted ledger operations (pulls, releases, refunds, sweeps).
//!
//! Ported from the Base instance's `flows/ledger_ops.rs` with the guarantees unchanged:
//!
//! * the exact transfer tuple (accounts, amount, fee, memo, `created_at_time`) is stored BEFORE the
//!   first send and every retry sends that same tuple, so the ledger's own deduplication applies;
//! * an escrow outflow is reserved in `unknown_outflow` BEFORE the first send of a tuple, so the
//!   coverage check can never see escrow that an unproven transfer may already have spent;
//! * any attempt whose outcome was not recorded is ambiguous, and an ambiguous operation is
//!   resolved only by `Ok`/`Duplicate` or by a complete ICRC-3 search, never by a typed error;
//! * an absence proof is valid only for the tuple and send history it looked at.
//!
//! The only change is the seam: every call goes through [`IcpLedger`] so host tests can drive the
//! whole lifecycle against a ledger that replays deduplication.

use crate::accounting::Transition;
use crate::env;
use crate::icp::IcpLedger;
use crate::ledger::{self, CallOutcome, LedgerRejection, Resolution, SearchOutcome};
use crate::state::{self, AuditCtx, DepositState, LedgerOp, LedgerOpKind, LedgerOpStatus, ReleaseState};
use icrc_ledger_types::icrc1::account::Account;

/// Retry the identical tuple only while comfortably inside the ledger's 24 h dedup window; after
/// that, resolve by ICRC-3 search.
pub const DEDUP_SAFE_WINDOW_NS: u64 = (24 * 3_600 - 30 * 60) * 1_000_000_000;
/// An attempt without a recorded outcome is treated as ambiguous after this long.
pub const STALE_ATTEMPT_NS: u64 = 120 * 1_000_000_000;
pub const SEARCH_BLOCKS_PER_TICK: u64 = 500;

/// Where an operation is stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpRef {
    Pull(u64),
    Refund(u64),
    /// Keyed by the return id.
    Release([u8; 32]),
    Sweep(u64),
}

impl OpRef {
    fn ctx(&self, note: impl Into<String>) -> AuditCtx {
        match self {
            OpRef::Pull(id) | OpRef::Refund(id) => AuditCtx::deposit(*id, note),
            OpRef::Release(t) => AuditCtx::release(*t, note),
            OpRef::Sweep(id) => AuditCtx::note(format!("sweep {id}: {}", note.into())),
        }
    }
}

pub fn load_op(r: &OpRef) -> Option<LedgerOp> {
    match r {
        OpRef::Pull(id) => state::get_deposit(*id).and_then(|d| d.pull),
        OpRef::Refund(id) => state::get_deposit(*id).and_then(|d| d.refund),
        OpRef::Release(t) => state::get_release(t).and_then(|x| x.transfer),
        OpRef::Sweep(id) => state::read_runtime(|c| c.sweeps.iter().find(|s| s.id == *id).map(|s| s.op.clone())),
    }
}

pub fn store_op(r: &OpRef, op: LedgerOp) {
    let now = env::now_ns();
    match r {
        OpRef::Pull(id) => {
            if let Some(mut d) = state::get_deposit(*id) {
                d.pull = Some(op);
                d.updated_at_ns = now;
                state::put_deposit(&d);
            }
        }
        OpRef::Refund(id) => {
            if let Some(mut d) = state::get_deposit(*id) {
                d.refund = Some(op);
                d.updated_at_ns = now;
                state::put_deposit(&d);
            }
        }
        OpRef::Release(t) => {
            if let Some(mut x) = state::get_release(t) {
                x.transfer = Some(op);
                x.updated_at_ns = now;
                state::put_release(&x);
            }
        }
        OpRef::Sweep(id) => state::mutate_runtime(|c| {
            if let Some(s) = c.sweeps.iter_mut().find(|s| s.id == *id) {
                s.op = op;
            }
        }),
    }
}

pub fn is_terminal(op: &LedgerOp) -> bool {
    matches!(op.status, LedgerOpStatus::Succeeded { .. } | LedgerOpStatus::Failed { .. })
}

/// The raw result of one send, tagged with the attempt number it belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SentAttempt {
    pub attempt: u32,
    pub outcome: CallOutcome,
}

/// Send one attempt of the persisted tuple. Returns the raw outcome tagged with its attempt
/// number, or `None` when there is nothing to send. Classify it with [`apply_outcome`], which uses
/// the op as stored AFTER the await.
pub async fn attempt<L: IcpLedger>(ledger: &L, r: &OpRef) -> Option<SentAttempt> {
    let mut op = load_op(r)?;
    if is_terminal(&op) {
        return None;
    }
    let now = env::now_ns();
    // A previous attempt whose outcome was never recorded (trap after send, or a call still in
    // flight elsewhere) may have executed.
    if op.attempts > 0 && matches!(op.status, LedgerOpStatus::Pending) {
        op.ambiguous_seen = true;
        op.status = LedgerOpStatus::Unknown { last_error: "previous attempt outcome was not recorded".into() };
    }
    if op.kind != LedgerOpKind::Pull && !op.outflow_reserved {
        let amount = op.escrow_outflow();
        if let Err(e) = state::apply_transition(Transition::OutflowUnknown { amount }, r.ctx("reserve outflow before send")) {
            state::halt(format!("cannot reserve escrow outflow: {e}"));
            return None;
        }
        op.outflow_reserved = true;
    }
    op.attempts = op.attempts.saturating_add(1);
    op.last_attempt_ns = Some(now);
    // Persisted before the call: if the call traps or the reply is lost, the next pass finds an
    // attempt without an outcome and treats it as ambiguous.
    store_op(r, op.clone());

    let attempt_no = op.attempts;
    let outcome = ledger.execute(&op).await;
    Some(SentAttempt { attempt: attempt_no, outcome })
}

/// Apply the outcome of a send. The classification uses the op as stored now, not the copy taken
/// before the await, so a slow reply cannot use stale `ambiguous_seen`. A non-success reply from an
/// older attempt (a newer send of the same tuple has started since) proves nothing about the newer
/// send, so it only marks the op unknown.
pub fn apply_outcome(r: &OpRef, sent: SentAttempt) {
    let Some(mut op) = load_op(r) else { return };
    if is_terminal(&op) {
        return;
    }
    let res = ledger::classify(&op, &sent.outcome);
    if sent.attempt != op.attempts && !matches!(res, Resolution::Succeeded { .. }) {
        op.ambiguous_seen = true;
        op.status = LedgerOpStatus::Unknown {
            last_error: format!("late reply from attempt {} while attempt {} is outstanding", sent.attempt, op.attempts),
        };
        store_op(r, op);
        return;
    }
    apply_resolution(r, res);
}

/// Completion bucket changes for a proven success, by kind. `None` means the parent record is not
/// in a state that expects this completion.
fn completion(r: &OpRef, op: &LedgerOp) -> Option<Vec<Transition>> {
    match r {
        OpRef::Pull(id) => {
            let d = state::get_deposit(*id)?;
            matches!(d.state, DepositState::PullPending | DepositState::PullUnknown)
                .then(|| vec![Transition::PullSucceeded { amount: d.amount_e8s }])
        }
        OpRef::Refund(id) => {
            let d = state::get_deposit(*id)?;
            matches!(d.state, DepositState::Refunding).then(|| vec![Transition::RefundCompleted { amount: d.amount_e8s }])
        }
        OpRef::Release(t) => {
            let x = state::get_release(t)?;
            matches!(x.state, ReleaseState::Pending)
                .then(|| vec![Transition::ReleaseCompleted { amount: x.amount_e8s, release_fee: x.release_fee_e8s }])
        }
        OpRef::Sweep(_) => Some(vec![Transition::SweepCompleted { amount: op.escrow_outflow() }]),
    }
}

fn mark_parent_succeeded(r: &OpRef, block_index: u64) {
    let now = env::now_ns();
    match r {
        OpRef::Pull(id) => {
            if let Some(mut d) = state::get_deposit(*id) {
                d.state = DepositState::Escrowed;
                // The mint id contains this block index, so it exists from this moment.
                d.mint_id = super::deposit::mint_id_for(&d, block_index);
                d.updated_at_ns = now;
                state::put_deposit(&d);
            }
        }
        OpRef::Refund(id) => {
            if let Some(mut d) = state::get_deposit(*id) {
                d.state = DepositState::Refunded;
                d.updated_at_ns = now;
                state::put_deposit(&d);
            }
        }
        OpRef::Release(t) => {
            if let Some(mut x) = state::get_release(t) {
                x.state = ReleaseState::Released;
                x.updated_at_ns = now;
                state::put_release(&x);
            }
        }
        OpRef::Sweep(_) => {}
    }
}

/// Apply a resolution to the stored op and its parent. Synchronous: no awaits.
pub fn apply_resolution(r: &OpRef, res: Resolution) {
    let Some(mut op) = load_op(r) else { return };
    if is_terminal(&op) {
        return;
    }
    let now = env::now_ns();
    match res {
        Resolution::Succeeded { block_index } => {
            let Some(mut ts) = completion(r, &op) else {
                state::halt(format!("{r:?}: ledger success but parent record is not awaiting it"));
                op.status = LedgerOpStatus::Succeeded { block_index };
                store_op(r, op);
                return;
            };
            if op.outflow_reserved {
                ts.insert(0, Transition::OutflowResolved { amount: op.escrow_outflow() });
            }
            op.status = LedgerOpStatus::Succeeded { block_index };
            op.outflow_reserved = false;
            if let Err(e) = state::apply_transitions(&ts, r.ctx(format!("ledger block {block_index}"))) {
                // The ledger moved funds; record that truth, then stop everything.
                state::halt(format!("{r:?}: accounting rejected a proven ledger success: {e}"));
                store_op(r, op);
                return;
            }
            store_op(r, op);
            mark_parent_succeeded(r, block_index);
        }
        Resolution::RetrySameTuple(note) => {
            if op.ambiguous_seen {
                op.status = LedgerOpStatus::Unknown { last_error: note };
            } else {
                // A definite no-effect reply: no earlier send is unresolved.
                op.status = LedgerOpStatus::Pending;
                op.attempts = 0;
            }
            // Keep the outflow reservation: the same tuple will be sent again.
            store_op(r, op);
        }
        Resolution::RebuildTuple(rej) => {
            // Only reachable without any ambiguous attempt: nothing moved.
            release_reservation(r, &mut op);
            match r {
                OpRef::Pull(_) => fail_pull(r, op, format!("ledger rejected the pull: {rej:?}")),
                _ => rebuild_escrow_tuple(r, op, Some(rej), now),
            }
        }
        Resolution::Failed(reason) => {
            release_reservation(r, &mut op);
            match r {
                OpRef::Pull(_) => fail_pull(r, op, reason),
                _ => {
                    // The escrow cannot lack funds or allowance while coverage holds.
                    state::halt(format!("{r:?}: escrow transfer rejected: {reason}"));
                    op.status = LedgerOpStatus::Pending;
                    op.attempts = 0;
                    store_op(r, op);
                }
            }
        }
        Resolution::Unknown(reason) => {
            op.ambiguous_seen = true;
            op.status = LedgerOpStatus::Unknown { last_error: reason };
            store_op(r, op);
        }
    }
}

fn release_reservation(r: &OpRef, op: &mut LedgerOp) {
    if op.outflow_reserved {
        let amount = op.escrow_outflow();
        if let Err(e) = state::apply_transition(Transition::OutflowResolved { amount }, r.ctx("outflow proven not executed")) {
            state::halt(format!("cannot release outflow reservation: {e}"));
        }
        op.outflow_reserved = false;
    }
}

fn fail_pull(r: &OpRef, mut op: LedgerOp, reason: String) {
    let OpRef::Pull(id) = r else { return };
    let Some(mut d) = state::get_deposit(*id) else { return };
    if !matches!(d.state, DepositState::PullPending | DepositState::PullUnknown) {
        return;
    }
    op.status = LedgerOpStatus::Failed { reason: reason.clone() };
    if let Err(e) = state::apply_transition(Transition::PullFailed { amount: d.amount_e8s }, r.ctx(&reason)) {
        state::halt(format!("pull failure accounting: {e}"));
        return;
    }
    state::mutate_runtime(|c| {
        c.windows.admitted_in_window = c.windows.admitted_in_window.saturating_sub(d.amount_e8s as u128);
    });
    d.pull = Some(op);
    d.state = DepositState::PullFailed { reason };
    d.updated_at_ns = env::now_ns();
    state::put_deposit(&d);
}

/// Replace an escrow transfer tuple after a proven no-effect outcome (new timestamp, possibly a
/// new ledger fee). The escrow outflow is unchanged by construction.
pub fn rebuild_escrow_tuple(r: &OpRef, mut op: LedgerOp, rej: Option<LedgerRejection>, now: u64) {
    if let Some(LedgerRejection::BadFee { expected_fee }) = rej {
        let outflow = op.escrow_outflow();
        state::mutate_runtime(|c| c.ledger_fee_e8s = Some(expected_fee));
        if outflow <= expected_fee {
            match r {
                OpRef::Release(t) => {
                    hold_release_for_fees(t, outflow, expected_fee);
                    return;
                }
                _ => {
                    state::halt(format!("{r:?}: ledger fee {expected_fee} consumes the whole transfer"));
                    return;
                }
            }
        }
        op.fee_e8s = Some(expected_fee);
        op.amount_e8s = outflow - expected_fee;
    }
    op.created_at_time = now;
    op.attempts = 0;
    op.ambiguous_seen = false;
    op.status = LedgerOpStatus::Pending;
    op.search_from_block = state::read_runtime(|c| c.ledger_tip_lower_bound);
    op.search_next_block = None;
    op.outflow_reserved = false;
    store_op(r, op);
    state::append_audit(None, r.ctx("ledger tuple rebuilt after proven no-effect outcome"));
}

fn hold_release_for_fees(t: &[u8; 32], outflow: u64, fee: u64) {
    let Some(mut x) = state::get_release(t) else { return };
    if !matches!(x.state, ReleaseState::Pending) {
        return;
    }
    if let Err(e) = state::apply_transition(
        Transition::ReleaseHeld { amount: x.amount_e8s },
        AuditCtx::release(*t, "release cannot cover fees; held for re-mint"),
    ) {
        state::halt(format!("release hold accounting: {e}"));
        return;
    }
    x.transfer = None;
    x.state = ReleaseState::Held { reason: format!("amount after bridge fee ({outflow} e8s) does not cover the ledger fee ({fee} e8s)") };
    x.updated_at_ns = env::now_ns();
    state::put_release(&x);
}

/// Drive an operation whose outcome is unknown (or whose attempt went stale) toward a proven
/// result. Within the dedup window the identical tuple is resent; afterwards the ledger history is
/// searched.
pub async fn reconcile<L: IcpLedger>(ledger: &L, r: &OpRef) {
    let Some(op) = load_op(r) else { return };
    if is_terminal(&op) {
        return;
    }
    let now = env::now_ns();
    let stale_pending = matches!(op.status, LedgerOpStatus::Pending)
        && op.attempts > 0
        && op.last_attempt_ns.map(|t| now.saturating_sub(t) >= STALE_ATTEMPT_NS).unwrap_or(true);
    let unknown = matches!(op.status, LedgerOpStatus::Unknown { .. });
    let unsent = matches!(op.status, LedgerOpStatus::Pending) && op.attempts == 0;
    if !(stale_pending || unknown || unsent) {
        return;
    }
    if unsent || now < op.created_at_time.saturating_add(DEDUP_SAFE_WINDOW_NS) {
        if let Some(sent) = attempt(ledger, r).await {
            apply_outcome(r, sent);
        }
        return;
    }
    // Outside the safe dedup window: search for the exact block. The search awaits, so note
    // exactly which tuple and send history it is about; the result is applied only to an op that is
    // still that tuple (see `apply_search_outcome`).
    let basis = SearchBasis::of(&op);
    let from = op.search_next_block.unwrap_or(op.search_from_block);
    let outcome = ledger.search(&op, from, SEARCH_BLOCKS_PER_TICK, now).await;
    apply_search_outcome(r, &basis, outcome, env::now_ns());
}

/// Identity of what an ICRC-3 search looked at: the tuple that defines the ledger transaction and
/// the send history at the moment the search started.
///
/// A proof that the tuple never executed is only a statement about THAT tuple, up to the ledger tip
/// the search reached. If the op changed while the search awaited (a new send of the same tuple, a
/// rebuilt tuple with a new `created_at_time`, a reset), the proof no longer covers the op as it
/// stands and must be discarded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchBasis {
    kind: LedgerOpKind,
    from: Account,
    to: Account,
    spender: Option<Account>,
    amount_e8s: u64,
    fee_e8s: Option<u64>,
    memo: Vec<u8>,
    created_at_time: u64,
    /// Send history: a send after the search started may execute after the tip it read.
    attempts: u32,
    last_attempt_ns: Option<u64>,
    /// Where the search window of this tuple starts (reset by a rebuild).
    search_from_block: u64,
}

impl SearchBasis {
    pub fn of(op: &LedgerOp) -> SearchBasis {
        SearchBasis {
            kind: op.kind,
            from: op.from,
            to: op.to,
            spender: op.spender,
            amount_e8s: op.amount_e8s,
            fee_e8s: op.fee_e8s,
            memo: op.memo.clone(),
            created_at_time: op.created_at_time,
            attempts: op.attempts,
            last_attempt_ns: op.last_attempt_ns,
            search_from_block: op.search_from_block,
        }
    }

    /// True when `cur` is still the searched tuple with the same send history, and still in a
    /// state that allows a search (not terminal; unknown, or sent without a recorded outcome).
    pub fn still_describes(&self, cur: &LedgerOp) -> bool {
        !is_terminal(cur) && searchable(cur) && *self == SearchBasis::of(cur)
    }
}

/// The states `reconcile` searches from: an unknown outcome, or a sent attempt whose outcome was
/// never recorded. An op that was never sent has nothing to search for.
fn searchable(op: &LedgerOp) -> bool {
    matches!(op.status, LedgerOpStatus::Unknown { .. }) || (matches!(op.status, LedgerOpStatus::Pending) && op.attempts > 0)
}

/// Apply the result of an ICRC-3 search to the op as stored now. Synchronous. A "never executed"
/// proof, and the progress of an unfinished search, are applied only when the op is still what the
/// search looked at (`basis`); otherwise they are dropped and the next pass decides afresh. A found
/// block is applied regardless: the ledger executed that transfer, which is a fact, not an
/// inference about the op's current shape.
pub fn apply_search_outcome(r: &OpRef, basis: &SearchBasis, outcome: SearchOutcome, now: u64) {
    match outcome {
        SearchOutcome::Found { block_index } => apply_resolution(r, Resolution::Succeeded { block_index }),
        SearchOutcome::NotFoundYet { next_block, .. } => {
            if let Some(mut cur) = load_op(r) {
                if basis.still_describes(&cur) {
                    cur.search_next_block = Some(next_block);
                    store_op(r, cur);
                }
            }
        }
        SearchOutcome::ProvenAbsent => {
            let Some(mut cur) = load_op(r) else { return };
            if is_terminal(&cur) {
                return;
            }
            if !basis.still_describes(&cur) {
                state::append_audit(None, r.ctx("ICRC-3 absence proof discarded: the operation changed while the search was running"));
                return;
            }
            state::append_audit(None, r.ctx("ICRC-3 search proved the tuple never executed"));
            release_reservation(r, &mut cur);
            match r {
                OpRef::Pull(_) => fail_pull(r, cur, "pull never executed (proven by ledger history search)".into()),
                _ => rebuild_escrow_tuple(r, cur, None, now),
            }
        }
        SearchOutcome::Error(e) => {
            state::append_audit(None, r.ctx(format!("ledger search error: {e}")));
        }
    }
}
