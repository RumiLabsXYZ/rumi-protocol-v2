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
//! passed as `None`). A holder who burns from their own account without a return memo lowers the
//! supply and nothing else: it never touches the return account, so the observer never sees it,
//! nothing is credited, and nothing halts. C4 does not exist under the memo plus durable-nonce guard.
//!
//! **Custody (49d).** `custody_measured == RC + BU + stray` is read as `supply, custody, supply` at
//! `finalized`, only when the observer is at the head of the return account, and the pair is used only
//! if both supply reads are equal (a burn or a mint that landed between the reads changes the supply,
//! so the pair is retried). Then:
//! * an EXCESS (tokens the books cannot explain) halts credits, not the bridge, once a second reading
//!   confirms it, and lifts when a reading finds none; a stray the observer has since recorded
//!   explains it;
//! * a DEFICIT no larger than what the bridge's own unresolved burns and refunds can explain
//!   (`BU` plus refunds in flight) is held for reconciliation: it is not OK, and it halts nothing;
//! * any other deficit is never OK, and halts the bridge once a second reading confirms it.

use crate::accounting::{check_coverage, check_custody, check_destination_supply, Buckets};
use crate::icp::IcpLedger;
use crate::mint::{parse_mint, parse_token_account};
use crate::sol_rpc::SolRpc;
use crate::state::{self, AuditCtx, CustodySnapshot, CustodyStatus, EscrowSnapshot, SupplySnapshot};

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
    /// What the custody reading concluded, when a valid pair was read.
    pub custody: Option<CustodyStatus>,
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
    custody(rpc, &profile, now, &mut report).await;
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

/// The supply of the bound mint at `finalized`, or why it could not be read.
async fn read_supply<R: SolRpc>(rpc: &R, profile: &crate::tx::builders::TxProfile) -> Result<u128, String> {
    let snapshot = rpc.get_account(&profile.mint).await.map_err(|e| format!("mint read failed: {e:?}"))?.ok_or("the bound mint account does not exist")?;
    if snapshot.owner != profile.token_program.id() {
        return Err("the mint account is not owned by the token program".into());
    }
    parse_mint(&snapshot.data, profile.token_program).map(|v| v.supply as u128)
}

/// The custody reading: `supply, custody, supply` (see the module docs).
async fn custody<R: SolRpc>(rpc: &R, profile: &crate::tx::builders::TxProfile, now: u64, report: &mut MonitorReport) {
    if !idle() {
        report.notes.push("custody: the observer is not at the head of the return account; not read".into());
        return;
    }
    let Ok(ata) = profile.return_ata() else { return };
    let first = match read_supply(rpc, profile).await {
        Ok(v) => v,
        Err(e) => {
            report.notes.push(format!("custody: {e}"));
            return;
        }
    };
    let measured = match rpc.get_account(&ata).await {
        Err(e) => {
            report.notes.push(format!("custody: return account read failed: {e:?}"));
            return;
        }
        // No account holds no tokens; whether that is expected is decided by the books below.
        Ok(None) => 0u128,
        Ok(Some(snap)) => {
            if snap.owner != profile.token_program.id() {
                report.notes.push("custody: the return account is not owned by the token program".into());
                return;
            }
            match parse_token_account(&snap.data, profile.token_program) {
                Ok(v) if v.mint == profile.mint && v.owner == profile.return_owner => v.amount as u128,
                Ok(_) => {
                    report.notes.push("custody: the return account is not this mint's account of the return owner".into());
                    return;
                }
                Err(e) => {
                    report.notes.push(format!("custody: the return account does not parse: {e}"));
                    return;
                }
            }
        }
    };
    let second = match read_supply(rpc, profile).await {
        Ok(v) => v,
        Err(e) => {
            report.notes.push(format!("custody: {e}"));
            return;
        }
    };
    if first != second {
        report.notes.push("custody: the supply changed during the reading (a burn or a mint landed); retried next interval".into());
        return;
    }
    let buckets: Buckets = state::read_core(|c| c.buckets);
    let verdict = match check_custody(&buckets, measured) {
        Ok(v) => v,
        Err(e) => {
            judge_custody(CustodyStatus::Deficit { deficit: 0 }, format!("the books overflow: {e}"), report);
            return;
        }
    };
    // What the bridge's own operations that are not yet applied can explain: a landed burn is still
    // `BU` and a landed refund is still `stray` until the saga applies them.
    let explained = buckets.burn_unknown.saturating_add(state::refunding_claims_total());
    let status = match (verdict.excess, verdict.deficit) {
        (Some(excess), _) => CustodyStatus::Excess { excess },
        (None, Some(deficit)) if deficit <= explained => CustodyStatus::Reconciling { deficit },
        (None, Some(deficit)) => CustodyStatus::Deficit { deficit },
        (None, None) => CustodyStatus::Ok,
    };
    state::mutate_core(|c| {
        c.monitor.custody = Some(CustodySnapshot { custody: measured, expected: verdict.expected, supply: first, status: status.clone(), at_ns: now })
    });
    report.custody = Some(status.clone());
    judge_custody(status, format!("custody {measured} against expected {}", verdict.expected), report);
}

/// Apply one custody verdict: strikes, the credits halt and the halt.
fn judge_custody(status: CustodyStatus, detail: String, report: &mut MonitorReport) {
    let bad = matches!(status, CustodyStatus::Excess { .. } | CustodyStatus::Deficit { .. });
    let strikes = state::mutate_core(|c| {
        c.monitor.custody_strikes = if bad { c.monitor.custody_strikes.saturating_add(1) } else { 0 };
        c.monitor.custody_strikes
    });
    match &status {
        // Nothing unexplained: a credits halt from an earlier excess lifts.
        CustodyStatus::Ok | CustodyStatus::Reconciling { .. } => {
            if state::mutate_core(|c| c.observer.credits_halted.take()).is_some() {
                state::append_audit(None, AuditCtx::note("custody check: no excess any more; credits resume"));
            }
            if let CustodyStatus::Reconciling { deficit } = status {
                report.notes.push(format!("custody: {deficit} e8s below the books, within the bridge's own unresolved burns and refunds; held for reconciliation"));
            }
        }
        CustodyStatus::Excess { excess } => {
            report.notes.push(format!("custody excess {excess} ({strikes} of {CONFIRMATIONS}): {detail}"));
            state::append_audit(None, AuditCtx::note(format!("MONITOR custody excess {excess} ({strikes} of {CONFIRMATIONS}): {detail}")));
            if strikes >= CONFIRMATIONS {
                let why = format!("{excess} e8s in the return account that the books cannot explain");
                state::mutate_core(|c| c.observer.credits_halted = Some(why));
            }
        }
        CustodyStatus::Deficit { deficit } => {
            report.notes.push(format!("custody deficit {deficit} ({strikes} of {CONFIRMATIONS}): {detail}"));
            state::append_audit(None, AuditCtx::note(format!("MONITOR custody deficit {deficit} ({strikes} of {CONFIRMATIONS}): {detail}")));
            if strikes >= CONFIRMATIONS {
                state::halt(format!("custody deficit of {deficit} e8s that no unresolved burn or refund of ours explains: {detail}"));
                report.halted = true;
            }
        }
    }
}
