//! The reconciliation monitors (53c): C1 to C4 evaluated from INDEPENDENT observations (the Gate at one
//! finalized checkpoint through the provider quorum, and the icUSD ledger balance of the escrow) against
//! the SELF-REPORTED books. Pure: [`evaluate`] reads nothing and changes nothing; [`apply`] halts on a breach.
//!
//! * **C1** `E >= L - U` (`accounting::check_coverage`): the escrow, as the LEDGER reports it, covers every
//!   liability not in doubt. A breach halts the bridge.
//! * **C2** `supply <= O + M_inflight` (`check_destination_supply`): the Gate's wrapped `TreasuryCap` supply
//!   is within what the books can explain. A breach (more coin than the books know of) halts the bridge:
//!   returns of coin nobody minted could drain the escrow.
//! * **C3** `supply == O` when quiescent (no mint in flight, the observer caught up at this checkpoint,
//!   this checkpoint at or after our last finalized mint). Sui has no unattributed burn, so any difference is
//!   real: more than `O` halts the bridge, less halts the mint lane.
//! * **C4** counters: `mint_count`/`minted_total` equal the canister's own cumulative delivered mints (an
//!   in-flight mint may account for one extra); `bridge_out_count`/`burned_total` equal the verified returns
//!   when caught up and never fall behind them. A mint the canister never made (a foreign mint id) shows as
//!   more mints or more minted than the books plus the operation in flight: the bridge halts. Books ahead of
//!   the chain halt only the mint lane.
//!
//! A check that cannot be evaluated is `NotEvaluated` with its reason, never a pass. D6 remains a disclosed
//! root of trust: nothing here asserts that a mutable package or its controllers cannot change issuance.

use crate::accounting::{check_coverage, check_destination_supply};
use crate::state::{self, AuditCtx, BreachScope, ChainObservation, CheckOutcome, CheckResult, MonitorReport, OwnBooks};

pub struct Inputs<'a> {
    pub chain: &'a ChainObservation,
    pub checkpoint: u64,
    pub escrow_balance: Option<u128>,
    /// Why there is no balance to compare, when the caller knows (default text otherwise).
    pub escrow_unread: Option<&'a str>,
    pub books: &'a OwnBooks,
    pub last_final_checkpoint: u64,
    pub caught_up: bool,
    pub gap: Option<&'a str>,
    pub active_op: bool,
}

fn result(name: &str, outcome: CheckOutcome) -> CheckResult {
    CheckResult { name: name.to_string(), outcome }
}

fn breach(detail: String, scope: BreachScope) -> CheckOutcome {
    CheckOutcome::Breach { detail, scope }
}

fn not_evaluated(reason: impl Into<String>) -> CheckOutcome {
    CheckOutcome::NotEvaluated { reason: reason.into() }
}

pub fn evaluate(i: &Inputs) -> Vec<CheckResult> {
    let b = &i.books.buckets;
    let mut out = Vec::with_capacity(6);

    // C1: escrow coverage, from the ledger's own balance.
    out.push(result(
        "C1 escrow covers liabilities",
        match i.escrow_balance {
            None => not_evaluated(i.escrow_unread.unwrap_or("the escrow balance could not be read from the ledger")),
            Some(balance) => match check_coverage(b, balance) {
                Ok(r) if r.ok => CheckOutcome::Ok,
                Ok(r) => breach(format!("the ledger shows {} in escrow but {} is required ({} short)", r.escrow_balance, r.required, r.deficit.unwrap_or(0)), BreachScope::Bridge),
                Err(e) => breach(format!("the books do not add up: {e}"), BreachScope::Bridge),
            },
        },
    ));

    // C2: the chain's supply is within what the books can explain.
    let supply = i.chain.total_supply as u128;
    out.push(result(
        "C2 supply within outstanding plus in flight",
        match check_destination_supply(b, supply, i.books.inflight_mint_e8s, false) {
            Ok(r) if r.within_bound => CheckOutcome::Ok,
            Ok(r) => breach(format!("the Gate's supply {} exceeds outstanding {} plus in flight {}", r.supply, r.outstanding, r.inflight_mint), BreachScope::Bridge),
            Err(e) => breach(format!("the books do not add up: {e}"), BreachScope::Bridge),
        },
    ));

    // C3: exact match, only when quiescent and fully observed.
    let behind_floor = i.checkpoint < i.last_final_checkpoint;
    let mut why_not: Vec<&str> = vec![];
    if !i.caught_up {
        why_not.push("the return observer is not caught up at this checkpoint");
    }
    if i.active_op || i.books.inflight_mint_e8s != 0 {
        why_not.push("a mint operation is in flight");
    }
    if behind_floor {
        why_not.push("the checkpoint is behind our last finalized mint");
    }
    out.push(result(
        "C3 supply equals outstanding when quiescent",
        if !why_not.is_empty() {
            not_evaluated(why_not.join("; "))
        } else {
            match check_destination_supply(b, supply, 0, true) {
                Ok(r) if r.exact_match == Some(true) => CheckOutcome::Ok,
                Ok(r) if r.supply > r.outstanding => breach(format!("quiescent, but the Gate's supply {} exceeds outstanding {}", r.supply, r.outstanding), BreachScope::Bridge),
                Ok(r) => breach(format!("quiescent, but the Gate's supply {} is below outstanding {}: the books claim more than the chain", r.supply, r.outstanding), BreachScope::MintLane),
                Err(e) => breach(format!("the books do not add up: {e}"), BreachScope::Bridge),
            }
        },
    ));

    // C4 (mints): the Gate's own counters against the canister's.
    let c = i.chain;
    let (n, m) = (i.books.minted_count, i.books.minted_e8s);
    out.push(result(
        "C4 mint counters",
        if behind_floor {
            not_evaluated("the checkpoint is behind our last finalized mint")
        } else if c.mint_count < n || c.minted_total < m {
            breach(format!("the Gate counts {} mints / {} minted but the books have delivered {} / {}", c.mint_count, c.minted_total, n, m), BreachScope::MintLane)
        } else {
            let (extra_count, extra_amount) = (c.mint_count - n, c.minted_total - m);
            if (extra_count == 0 && extra_amount == 0) || (extra_count == 1 && i.books.inflight_ops == 1 && extra_amount == i.books.inflight_mint_e8s) {
                CheckOutcome::Ok
            } else {
                breach(
                    format!(
                        "the Gate shows {extra_count} mint(s) / {extra_amount} e8s beyond the books' {n} / {m}, and the operation in flight accounts for {} / {} (a foreign mint id)",
                        i.books.inflight_ops, i.books.inflight_mint_e8s
                    ),
                    BreachScope::Bridge,
                )
            }
        },
    ));

    // C4 (returns): the Gate's return counters against the verified returns.
    let v = i.books.verified_through;
    out.push(result(
        "C4 return counters",
        if c.bridge_out_count < v || c.burned_total < i.books.returned_e8s {
            breach(format!("the Gate counts {} returns / {} burned but the books have verified {} / {}", c.bridge_out_count, c.burned_total, v, i.books.returned_e8s), BreachScope::Bridge)
        } else if i.caught_up && (c.bridge_out_count != v || c.burned_total != i.books.returned_e8s) {
            breach(format!("caught up, but the Gate counts {} returns / {} burned against the books' {} / {}", c.bridge_out_count, c.burned_total, v, i.books.returned_e8s), BreachScope::Bridge)
        } else {
            CheckOutcome::Ok
        },
    ));

    // Completeness of the return discovery.
    out.push(result(
        "returns complete",
        match i.gap {
            Some(g) => not_evaluated(format!("hold: {g}")),
            None if i.caught_up => CheckOutcome::Ok,
            None => not_evaluated("the observer has not yet reached this checkpoint"),
        },
    ));
    out
}

/// Halt what a breach requires. A halt is a decision for a human to clear, never a retry.
pub fn apply(report: &MonitorReport) {
    for check in &report.checks {
        if let CheckOutcome::Breach { detail, scope } = &check.outcome {
            let why = format!("monitor {} at checkpoint {}: {detail}", check.name, report.checkpoint);
            match scope {
                BreachScope::Bridge => {
                    state::halt(why.clone());
                    state::halt_mint_lane(why);
                }
                BreachScope::MintLane => state::halt_mint_lane(why),
            }
        }
    }
    let breaches = report.checks.iter().filter(|c| matches!(c.outcome, CheckOutcome::Breach { .. })).count();
    state::append_audit(
        None,
        AuditCtx::note(format!(
            "monitors at checkpoint {}: {} ok, {} breached, {} not evaluated",
            report.checkpoint,
            report.checks.iter().filter(|c| c.outcome == CheckOutcome::Ok).count(),
            breaches,
            report.checks.iter().filter(|c| matches!(c.outcome, CheckOutcome::NotEvaluated { .. })).count()
        )),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounting::Buckets;

    fn chain() -> ChainObservation {
        ChainObservation { total_supply: 500, supply_cap: 1_000_000, minted_total: 500, burned_total: 0, mint_count: 1, bridge_out_count: 0, gate_version: 1 }
    }

    fn books() -> OwnBooks {
        OwnBooks {
            buckets: Buckets { outstanding: 500, ..Buckets::default() },
            minted_count: 1,
            minted_e8s: 500,
            verified_through: 0,
            returned_e8s: 0,
            inflight_mint_e8s: 0,
            inflight_ops: 0,
        }
    }

    fn run(c: &ChainObservation, b: &OwnBooks, escrow: Option<u128>, caught_up: bool) -> Vec<CheckResult> {
        evaluate(&Inputs { chain: c, checkpoint: 1_000, escrow_balance: escrow, escrow_unread: None, books: b, last_final_checkpoint: 900, caught_up, gap: None, active_op: false })
    }

    fn outcome<'a>(r: &'a [CheckResult], prefix: &str) -> &'a CheckOutcome {
        &r.iter().find(|c| c.name.starts_with(prefix)).unwrap().outcome
    }

    #[test]
    fn a_consistent_quiescent_state_passes_every_check() {
        let r = run(&chain(), &books(), Some(500), true);
        assert!(r.iter().all(|c| c.outcome == CheckOutcome::Ok), "{r:?}");
    }

    #[test]
    fn c1_uses_the_ledger_balance_and_a_missing_read_is_not_a_pass() {
        assert!(matches!(outcome(&run(&chain(), &books(), Some(499), true), "C1"), CheckOutcome::Breach { scope: BreachScope::Bridge, .. }));
        assert!(matches!(outcome(&run(&chain(), &books(), None, true), "C1"), CheckOutcome::NotEvaluated { .. }));
        // Funds in doubt (an unknown outflow) do not count against coverage.
        let mut b = books();
        b.buckets.unknown_outflow = 100;
        b.buckets.outstanding = 400;
        let mut c = chain();
        c.total_supply = 400;
        c.minted_total = 400;
        b.minted_e8s = 400;
        assert_eq!(outcome(&run(&c, &b, Some(400), true), "C1"), &CheckOutcome::Ok);
    }

    #[test]
    fn a_balance_the_caller_could_not_match_with_the_books_is_not_evaluated_with_its_reason_and_never_a_breach() {
        let why = "the books changed while the escrow balance was being read";
        let r = evaluate(&Inputs {
            chain: &chain(),
            checkpoint: 1_000,
            escrow_balance: None,
            escrow_unread: Some(why),
            books: &books(),
            last_final_checkpoint: 900,
            caught_up: true,
            gap: None,
            active_op: false,
        });
        assert!(matches!(outcome(&r, "C1"), CheckOutcome::NotEvaluated { reason } if reason == why));
        // The other checks do not depend on the balance.
        assert!(r.iter().filter(|c| !c.name.starts_with("C1")).all(|c| c.outcome == CheckOutcome::Ok), "{r:?}");
    }

    #[test]
    fn c2_halts_the_bridge_when_the_chain_holds_more_than_the_books_explain() {
        let mut c = chain();
        c.total_supply = 501;
        c.minted_total = 501;
        let r = run(&c, &books(), Some(500), true);
        assert!(matches!(outcome(&r, "C2"), CheckOutcome::Breach { scope: BreachScope::Bridge, .. }));
        // An operation in flight explains its own amount, so the bound moves with it.
        let mut b = books();
        b.inflight_mint_e8s = 1;
        b.inflight_ops = 1;
        b.buckets.pending_mint = 1;
        assert_eq!(outcome(&run(&c, &b, Some(501), false), "C2"), &CheckOutcome::Ok);
    }

    #[test]
    fn c3_needs_quiescence_and_then_demands_equality_in_both_directions() {
        // Not caught up, a mint in flight, or a checkpoint behind our last mint: not evaluated.
        assert!(matches!(outcome(&run(&chain(), &books(), Some(500), false), "C3"), CheckOutcome::NotEvaluated { .. }));
        let mut flying = books();
        flying.inflight_mint_e8s = 10;
        flying.inflight_ops = 1;
        flying.buckets.pending_mint = 10;
        assert!(matches!(outcome(&run(&chain(), &flying, Some(510), true), "C3"), CheckOutcome::NotEvaluated { reason } if reason.contains("in flight")));
        let behind = evaluate(&Inputs { chain: &chain(), checkpoint: 800, escrow_balance: Some(500), escrow_unread: None, books: &books(), last_final_checkpoint: 900, caught_up: true, gap: None, active_op: false });
        assert!(matches!(outcome(&behind, "C3"), CheckOutcome::NotEvaluated { reason } if reason.contains("behind our last finalized mint")));
        assert!(matches!(outcome(&behind, "C4 mint"), CheckOutcome::NotEvaluated { .. }));
        // Quiescent: more supply than outstanding halts the bridge, less halts only the lane.
        let mut over = chain();
        over.total_supply = 600;
        over.minted_total = 600;
        assert!(matches!(outcome(&run(&over, &books(), Some(500), true), "C3"), CheckOutcome::Breach { scope: BreachScope::Bridge, .. }));
        let mut under = chain();
        under.total_supply = 400;
        under.minted_total = 400;
        assert!(matches!(outcome(&run(&under, &books(), Some(500), true), "C3"), CheckOutcome::Breach { scope: BreachScope::MintLane, .. }));
    }

    #[test]
    fn c4_names_a_foreign_mint_by_its_counters_and_allows_only_the_operation_in_flight() {
        // One extra mint with nothing in flight: foreign.
        let mut c = chain();
        c.mint_count = 2;
        c.minted_total = 507;
        c.total_supply = 507;
        let r = run(&c, &books(), Some(500), false);
        assert!(matches!(outcome(&r, "C4 mint"), CheckOutcome::Breach { scope: BreachScope::Bridge, detail } if detail.contains("foreign mint id")));
        // The same extra mint is explained when it is exactly the operation in flight.
        let mut b = books();
        b.inflight_ops = 1;
        b.inflight_mint_e8s = 7;
        b.buckets.pending_mint = 7;
        assert_eq!(outcome(&run(&c, &b, Some(507), false), "C4 mint"), &CheckOutcome::Ok);
        // A different amount than the operation in flight is not explained.
        c.minted_total = 508;
        assert!(matches!(outcome(&run(&c, &b, Some(507), false), "C4 mint"), CheckOutcome::Breach { scope: BreachScope::Bridge, .. }));
        // The books ahead of the chain halt only the lane.
        let mut behind = chain();
        behind.mint_count = 0;
        behind.minted_total = 0;
        behind.total_supply = 0;
        assert!(matches!(outcome(&run(&behind, &books(), Some(500), true), "C4 mint"), CheckOutcome::Breach { scope: BreachScope::MintLane, .. }));
    }

    #[test]
    fn c4_returns_demand_equality_only_when_caught_up_and_never_fall_behind_the_books() {
        let mut b = books();
        b.verified_through = 2;
        b.returned_e8s = 300;
        b.buckets.outstanding = 200;
        b.buckets.pending_release = 300;
        let mut c = chain();
        c.bridge_out_count = 2;
        c.burned_total = 300;
        c.total_supply = 200;
        assert_eq!(outcome(&run(&c, &b, Some(500), true), "C4 return"), &CheckOutcome::Ok);
        // Not caught up: the Gate may be ahead.
        c.bridge_out_count = 3;
        c.burned_total = 350;
        c.total_supply = 150;
        assert_eq!(outcome(&run(&c, &b, Some(500), false), "C4 return"), &CheckOutcome::Ok);
        // Caught up but the Gate disagrees on the total burned: a breach.
        c.bridge_out_count = 2;
        c.burned_total = 301;
        assert!(matches!(outcome(&run(&c, &b, Some(500), true), "C4 return"), CheckOutcome::Breach { scope: BreachScope::Bridge, .. }));
        // The Gate behind the books is impossible.
        c.bridge_out_count = 1;
        c.burned_total = 100;
        assert!(matches!(outcome(&run(&c, &b, Some(500), false), "C4 return"), CheckOutcome::Breach { scope: BreachScope::Bridge, .. }));
    }

    #[test]
    fn a_gap_is_a_hold_not_a_pass() {
        let r = evaluate(&Inputs { chain: &chain(), checkpoint: 1_000, escrow_balance: Some(500), escrow_unread: None, books: &books(), last_final_checkpoint: 0, caught_up: false, gap: Some("sequence 3 is missing"), active_op: false });
        assert!(matches!(outcome(&r, "returns complete"), CheckOutcome::NotEvaluated { reason } if reason.contains("sequence 3")));
    }

    #[test]
    fn apply_halts_what_the_scope_says_and_nothing_else() {
        state::reset_for_tests();
        state::mutate_runtime(|r| r.initialized = true);
        let report = |scope| MonitorReport {
            at_ns: 1,
            checkpoint: 77,
            checkpoint_digest: [1; 32],
            epoch: 1,
            chain: chain(),
            escrow_balance: Some(500),
            books: books(),
            caught_up: true,
            checks: vec![CheckResult { name: "C9 test".into(), outcome: CheckOutcome::Breach { detail: "d".into(), scope } }, CheckResult { name: "ok".into(), outcome: CheckOutcome::Ok }],
        };
        apply(&report(BreachScope::MintLane));
        assert!(state::is_mint_lane_halted() && !state::is_halted());
        apply(&report(BreachScope::Bridge));
        assert!(state::is_halted());
        assert!(state::read_runtime(|r| r.halted.clone().unwrap().reason).contains("checkpoint 77"));
    }
}
