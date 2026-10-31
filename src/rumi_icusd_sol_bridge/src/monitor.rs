//! The periodic invariant monitors (49c, `accounting.rs` module docs): C1 coverage of the ICP escrow
//! and C2 the supply bound on Solana. Both are READS compared with the books, and both follow the same
//! two rules, learned from the Base instance:
//!
//! * a reading is compared with the books on BOTH sides of the await (before and after), and a
//!   violation needs both to fail, so an operation that completed while the read was in flight can
//!   never be mistaken for a violation (a pull that landed after the balance was read, a burn applied
//!   after the supply was read);
//! * a violation is confirmed by a second consecutive reading before it halts anything
//!   ([`CONFIRMATIONS`]); the first is recorded in the status and the audit log.
//!
//! C3 is not enforced: a holder may burn, so `supply == O` does not hold (note section 12). The gap
//! `O - supply`, when quiescent, is REPORTED as unattributed and never halts, credits, sweeps or
//! refunds (blocker `solana-unattributed-burn-policy`; the `UB` seam is implementation missing and is
//! passed as `None`). C4 does not exist under the memo plus durable-nonce guard.

use crate::accounting::{check_coverage, check_destination_supply, Buckets};
use crate::icp::IcpLedger;
use crate::mint::parse_mint;
use crate::sol_rpc::SolRpc;
use crate::state::{self, AuditCtx, EscrowSnapshot, SupplySnapshot};

/// How often the monitors read.
pub const MONITOR_INTERVAL_NS: u64 = 5 * 60 * 1_000_000_000;
/// Consecutive violating readings that halt the bridge.
pub const CONFIRMATIONS: u8 = 2;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MonitorReport {
    /// `Some(ok)` when C1 was read.
    pub c1: Option<bool>,
    /// `Some(within_bound)` when C2 was read.
    pub c2: Option<bool>,
    /// A violation was confirmed and the bridge halted.
    pub halted: bool,
    pub notes: Vec<String>,
}

fn is_due(now: u64) -> bool {
    state::read_core(|c| c.monitor.last_run_ns).is_none_or(|t| now.saturating_sub(t) >= MONITOR_INTERVAL_NS)
}

fn idle() -> bool {
    state::read_core(|c| c.observer.caught_up && !c.observer.walk_active) && state::observer_queue_len() == 0
}

/// Run the monitors if they are due.
pub async fn run_if_due<R: SolRpc, L: IcpLedger>(rpc: &R, ledger: &L, now: u64) -> MonitorReport {
    if !state::read_core(|c| c.initialized) || !is_due(now) {
        return MonitorReport::default();
    }
    run(rpc, ledger, now).await
}

/// Read C1 and C2 now.
pub async fn run<R: SolRpc, L: IcpLedger>(rpc: &R, ledger: &L, now: u64) -> MonitorReport {
    let mut report = MonitorReport::default();
    let Ok(profile) = state::read_core(|c| c.tx_profile()) else {
        report.notes.push("the escrow mint is not bound: nothing to monitor".into());
        return report;
    };
    state::mutate_core(|c| c.monitor.last_run_ns = Some(now));
    c1(ledger, now, &mut report).await;
    c2(rpc, &profile, now, &mut report).await;
    report
}

/// Record one reading's verdict: reset the strikes on success, count one on a violation and halt on
/// the confirming one.
fn judge(label: &str, ok: bool, detail: String, report: &mut MonitorReport) {
    let strikes = state::mutate_core(|c| {
        let s = if label == "C1" { &mut c.monitor.c1_strikes } else { &mut c.monitor.c2_strikes };
        *s = if ok { 0 } else { s.saturating_add(1) };
        *s
    });
    if ok {
        return;
    }
    report.notes.push(format!("{label} violated ({strikes} of {CONFIRMATIONS}): {detail}"));
    state::mutate_core(|c| c.monitor.last_note = Some(format!("{label} violated: {detail}")));
    state::append_audit(None, AuditCtx::note(format!("MONITOR {label} violated ({strikes} of {CONFIRMATIONS}): {detail}")));
    if strikes >= CONFIRMATIONS {
        state::halt(format!("{label} violated on {CONFIRMATIONS} consecutive readings: {detail}"));
        report.halted = true;
    }
}

async fn c1<L: IcpLedger>(ledger: &L, now: u64, report: &mut MonitorReport) {
    let canister = crate::env::canister_id();
    let escrow = state::read_core(|c| c.escrow_account(canister));
    let before: Buckets = state::read_core(|c| c.buckets);
    let balance = match ledger.balance_of(escrow).await {
        Ok(b) => b,
        Err(e) => {
            report.notes.push(format!("C1: escrow balance unavailable: {e}"));
            return;
        }
    };
    let after: Buckets = state::read_core(|c| c.buckets);
    let (r0, r1) = match (check_coverage(&before, balance), check_coverage(&after, balance)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            judge("C1", false, format!("the books overflow: {e}"), report);
            return;
        }
    };
    let ok = r0.ok || r1.ok;
    // The snapshot keeps the reading against the books as they stand now.
    state::mutate_core(|c| c.monitor.escrow = Some(EscrowSnapshot { balance, required: r1.required, ok, at_ns: now }));
    report.c1 = Some(ok);
    judge("C1", ok, format!("escrow balance {balance} below required {} (before) and {} (after)", r0.required, r1.required), report);
}

async fn c2<R: SolRpc>(rpc: &R, profile: &crate::tx::builders::TxProfile, now: u64, report: &mut MonitorReport) {
    let before = (state::read_core(|c| c.buckets), state::inflight_mint_e8s());
    let snapshot = match rpc.get_account(&profile.mint).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            judge("C2", false, "the bound mint account does not exist at finalized".into(), report);
            return;
        }
        Err(e) => {
            report.notes.push(format!("C2: mint read failed: {e:?}"));
            return;
        }
    };
    if snapshot.owner != profile.token_program.id() {
        judge("C2", false, "the mint account is not owned by the token program".into(), report);
        return;
    }
    let supply = match parse_mint(&snapshot.data, profile.token_program) {
        Ok(v) => v.supply as u128,
        Err(e) => {
            judge("C2", false, format!("the mint account does not parse: {e}"), report);
            return;
        }
    };
    let after = (state::read_core(|c| c.buckets), state::inflight_mint_e8s());
    let idle = idle();
    let (r0, r1) = match (
        check_destination_supply(&before.0, supply, before.1, idle, None),
        check_destination_supply(&after.0, supply, after.1, idle, None),
    ) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            judge("C2", false, format!("the books are inconsistent: {e}"), report);
            return;
        }
    };
    let within = r0.within_bound || r1.within_bound;
    state::mutate_core(|c| {
        c.monitor.supply = Some(SupplySnapshot {
            supply,
            bound: r1.bound.max(r0.bound),
            inflight_mint: after.1,
            within_bound: within,
            quiescent: r1.quiescent,
            unattributed_gap: r1.unattributed_gap,
            at_ns: now,
        })
    });
    report.c2 = Some(within);
    judge("C2", within, format!("supply {supply} above the bound {} (before) and {} (after)", r0.bound, r1.bound), report);
}
