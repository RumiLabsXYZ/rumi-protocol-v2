//! Generic lifecycle for persisted ledger operations (pulls, releases, refunds, sweeps).

use crate::accounting::Transition;
use crate::env;
use crate::ledger::{self, CallOutcome, LedgerRejection, Resolution, SearchOutcome};
use crate::state::{self, AuditCtx, DepositState, LedgerOp, LedgerOpKind, LedgerOpStatus, ReleaseState};

/// Retry the identical tuple only while comfortably inside the ledger's 24 h dedup
/// window; after that, resolve by ICRC-3 search.
pub const DEDUP_SAFE_WINDOW_NS: u64 = (24 * 3_600 - 30 * 60) * 1_000_000_000;
/// An attempt without a recorded outcome is treated as ambiguous after this long.
pub const STALE_ATTEMPT_NS: u64 = 120 * 1_000_000_000;
pub const SEARCH_BLOCKS_PER_TICK: u64 = 500;

/// Where an operation is stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpRef {
    Pull(u64),
    Refund(u64),
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
        OpRef::Sweep(id) => state::read_core(|c| c.sweeps.iter().find(|s| s.id == *id).map(|s| s.op.clone())),
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
        OpRef::Sweep(id) => state::mutate_core(|c| {
            if let Some(s) = c.sweeps.iter_mut().find(|s| s.id == *id) {
                s.op = op;
            }
        }),
    }
}

pub fn is_terminal(op: &LedgerOp) -> bool {
    matches!(op.status, LedgerOpStatus::Succeeded { .. } | LedgerOpStatus::Failed { .. })
}

/// Send one attempt of the persisted tuple. Returns the classified resolution, or
/// `None` when there is nothing to send.
pub async fn attempt(r: &OpRef) -> Option<Resolution> {
    let mut op = load_op(r)?;
    if is_terminal(&op) {
        return None;
    }
    let now = env::now_ns();
    // A previous attempt whose outcome was never recorded (trap after send, or a call
    // still in flight elsewhere) may have executed.
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
    store_op(r, op.clone());

    let ledger_id = state::read_core(|c| c.config.icusd_ledger);
    let outcome = ledger::execute(ledger_id, &op).await;
    Some(ledger::classify(&op, &outcome))
}

/// Completion bucket changes for a proven success, by kind. `None` means the parent
/// record is not in a state that expects this completion.
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

fn mark_parent_succeeded(r: &OpRef) {
    let now = env::now_ns();
    match r {
        OpRef::Pull(id) => {
            if let Some(mut d) = state::get_deposit(*id) {
                d.state = DepositState::Escrowed;
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
            mark_parent_succeeded(r);
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
    state::mutate_core(|c| {
        c.windows.admitted_in_window = c.windows.admitted_in_window.saturating_sub(d.amount_e8s as u128);
    });
    d.pull = Some(op);
    d.state = DepositState::PullFailed { reason };
    d.updated_at_ns = env::now_ns();
    state::put_deposit(&d);
}

/// Replace an escrow transfer tuple after a proven no-effect outcome (new timestamp,
/// possibly a new ledger fee). The escrow outflow is unchanged by construction.
pub fn rebuild_escrow_tuple(r: &OpRef, mut op: LedgerOp, rej: Option<LedgerRejection>, now: u64) {
    if let Some(LedgerRejection::BadFee { expected_fee }) = rej {
        let outflow = op.escrow_outflow();
        state::mutate_core(|c| c.ledger_fee_e8s = Some(expected_fee));
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
    op.search_from_block = state::read_core(|c| c.ledger_tip_lower_bound);
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

/// Drive an operation whose outcome is unknown (or whose attempt went stale) toward a
/// proven result. Within the dedup window the identical tuple is resent; afterwards the
/// ledger history is searched.
pub async fn reconcile(r: &OpRef) {
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
        if let Some(res) = attempt(r).await {
            apply_resolution(r, res);
        }
        return;
    }
    // Outside the safe dedup window: search for the exact block.
    let ledger_id = state::read_core(|c| c.config.icusd_ledger);
    let from = op.search_next_block.unwrap_or(op.search_from_block);
    match ledger::search(ledger_id, &op, from, SEARCH_BLOCKS_PER_TICK, now).await {
        SearchOutcome::Found { block_index } => apply_resolution(r, Resolution::Succeeded { block_index }),
        SearchOutcome::NotFoundYet { next_block, .. } => {
            if let Some(mut cur) = load_op(r) {
                cur.search_next_block = Some(next_block);
                store_op(r, cur);
            }
        }
        SearchOutcome::ProvenAbsent => {
            let Some(mut cur) = load_op(r) else { return };
            if is_terminal(&cur) {
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

/// Outcome for a freshly sent call, used by tests to drive `apply_resolution`.
pub fn resolution_for(op: &LedgerOp, outcome: &CallOutcome) -> Resolution {
    ledger::classify(op, outcome)
}
