//! Escrow accounting for one Sui destination: disjoint liability buckets and the only functions
//! allowed to change them. Adapted from the frozen Solana instance (`rumi_icusd_sol_bridge` at
//! 4394b472, itself copied from the Base instance) with the custodied-return lifecycle REMOVED:
//! the Move gate's `bridge_out` burns the coin atomically (`move/rumi_icusd_gate` at 23f75ef1), so
//! a return is never custodied and `RC`, `BU` and `stray` do not exist here. Nothing in this module
//! touches the backend's debt books or `chain_supplies`: escrow-backed Sui supply is ordinary ICP
//! ledger supply, kept apart by construction.
//!
//! Every amount is in e8s. The buckets are liabilities of the escrow account
//! `(bridge canister, ESCROW_SUBACCOUNT)`; none of them is borrowing debt.
//!
//! | Field            | Symbol | Meaning |
//! |------------------|--------|---------|
//! | `outstanding`    | O      | Finalized delivered mints minus finalized valid returns |
//! | `pending_mint`   | PM     | Escrowed deposit amounts (and re-mints) not yet delivered on Sui |
//! | `pending_release`| PR     | Returns whose coin is already burned (the return is final), not yet released on ICP |
//! | `pending_refund` | RF     | Deposits being refunded to their ICP source account |
//! | `held`           | H      | Finalized returns that cannot be released as requested |
//! | `fees`           | F      | Earned bridge fees not yet swept |
//!
//! `reserved_pulls` is admission capacity for pulls whose outcome is not proven yet (no funds
//! moved, not a liability). `unknown_outflow` (U) is the escrow decrease that transfers with an
//! unknown outcome would cause if they executed.
//!
//! # Lifecycle (the only way into `PR`)
//!
//! ```text
//! PM --MintDelivered--> O --ReturnFinalized--> PR --ReleaseCompleted--> (ICP)
//!                                               |
//!                                               +--ReleaseHeld--> H --RemintStarted--> PM
//! ```
//!
//! `ReturnFinalized` is applied by exactly one caller, in the same synchronous step that opens the
//! release (`flows::release::open_release`), and only for a return verified from the Gate's durable
//! `ReturnRecord` at a finalized checkpoint (53c). There is no transition from `O`, `PM` or `H`
//! straight to a payout, so no ICP release is reachable without a final return.
//!
//! # Invariants
//!
//! * C1 (coverage): `escrow_balance >= L - U`, `L = O + PM + PR + RF + H + F`.
//! * C2 (supply bound): `supply(final checkpoint) <= O + M_inflight`, `M_inflight <= PM`.
//! * C3 (quiescent match): with nothing in flight and the observer caught up, `supply == O`
//!   exactly. The unattributed-burn term is zero on Sui: burning needs the Gate's wrapped
//!   `TreasuryCap`, so the only burn path is `bridge_out`, which the observer sees.
//! * C4 (cumulative mints) needs the Gate's `minted_total`; its monitor lands in 53c.

use candid::CandidType;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Buckets {
    #[serde(default)]
    pub outstanding: u128,
    #[serde(default)]
    pub pending_mint: u128,
    #[serde(default)]
    pub pending_release: u128,
    #[serde(default)]
    pub pending_refund: u128,
    #[serde(default)]
    pub held: u128,
    #[serde(default)]
    pub fees: u128,
    #[serde(default)]
    pub reserved_pulls: u128,
    #[serde(default)]
    pub unknown_outflow: u128,
}

/// The complete set of bucket changes. Each variant is one row of the lifecycle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, CandidType)]
pub enum Transition {
    /// A deposit was admitted; reserve capacity for its pull.
    AdmitDeposit { amount: u64 },
    /// The pull was proven not to have happened.
    PullFailed { amount: u64 },
    /// The pull was proven (`Ok` or `Duplicate`); the escrow now holds `amount` more.
    PullSucceeded { amount: u64 },
    /// A finalized, verified Sui mint delivered `amount - fee` for a deposit of `amount`.
    MintDelivered { amount: u64, fee: u64 },
    /// Non-delivery was proven (the mint was never built or signed) and a refund of the full
    /// deposit started.
    RefundStarted { amount: u64 },
    /// The refund transfer was proven; the escrow decreased by `amount`.
    RefundCompleted { amount: u64 },
    /// A final, verified `bridge_out` return of `amount`: `O -> PR`. The coin was burned by the
    /// same transaction, so the supply already fell.
    ReturnFinalized { amount: u64 },
    /// An accepted release became unreleasable (for example the ledger fee rose above it) and
    /// moves to the held bucket for re-mint.
    ReleaseHeld { amount: u64 },
    /// The release transfer was proven; the escrow decreased by `amount - release_fee`.
    ReleaseCompleted { amount: u64, release_fee: u64 },
    /// A held claim is being re-minted to its Sui sender.
    RemintStarted { amount: u64 },
    /// The re-mint was finalized and verified on Sui.
    RemintDelivered { amount: u64 },
    /// A fee sweep was proven; the escrow decreased by `amount`.
    SweepCompleted { amount: u64 },
    /// An escrow outflow of `amount` (amount sent plus ledger fee) has an unknown outcome.
    OutflowUnknown { amount: u64 },
    /// A previously unknown escrow outflow of `amount` was resolved (either way).
    OutflowResolved { amount: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, CandidType)]
pub enum AccountingError {
    Underflow { bucket: String, have: u128, need: u128 },
    Overflow { bucket: String },
    InvalidAmounts(String),
}

impl std::fmt::Display for AccountingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AccountingError::Underflow { bucket, have, need } => {
                write!(f, "accounting underflow in {bucket}: have {have}, need {need}")
            }
            AccountingError::Overflow { bucket } => write!(f, "accounting overflow in {bucket}"),
            AccountingError::InvalidAmounts(m) => write!(f, "invalid accounting amounts: {m}"),
        }
    }
}

fn sub(bucket: &str, value: u128, amount: u128) -> Result<u128, AccountingError> {
    value.checked_sub(amount).ok_or_else(|| AccountingError::Underflow {
        bucket: bucket.to_string(),
        have: value,
        need: amount,
    })
}

fn add(bucket: &str, value: u128, amount: u128) -> Result<u128, AccountingError> {
    value.checked_add(amount).ok_or_else(|| AccountingError::Overflow { bucket: bucket.to_string() })
}

impl Buckets {
    /// `O + PM + PR + RF + H + F`.
    pub fn liabilities(&self) -> Result<u128, AccountingError> {
        let mut total = 0u128;
        for (name, v) in [
            ("outstanding", self.outstanding),
            ("pending_mint", self.pending_mint),
            ("pending_release", self.pending_release),
            ("pending_refund", self.pending_refund),
            ("held", self.held),
            ("fees", self.fees),
        ] {
            total = add(name, total, v)?;
        }
        Ok(total)
    }

    /// Value that exists on Sui, or will, for capacity checks: `O + PM`.
    pub fn destination_exposure(&self) -> Result<u128, AccountingError> {
        add("destination_exposure", self.outstanding, self.pending_mint)
    }

    /// Apply one transition atomically. On error `self` is unchanged.
    pub fn apply(&self, t: &Transition) -> Result<Buckets, AccountingError> {
        let mut b = *self;
        match *t {
            Transition::AdmitDeposit { amount } => {
                b.reserved_pulls = add("reserved_pulls", b.reserved_pulls, amount as u128)?;
            }
            Transition::PullFailed { amount } => {
                b.reserved_pulls = sub("reserved_pulls", b.reserved_pulls, amount as u128)?;
            }
            Transition::PullSucceeded { amount } => {
                b.reserved_pulls = sub("reserved_pulls", b.reserved_pulls, amount as u128)?;
                b.pending_mint = add("pending_mint", b.pending_mint, amount as u128)?;
            }
            Transition::MintDelivered { amount, fee } => {
                if fee > amount {
                    return Err(AccountingError::InvalidAmounts(format!("fee {fee} exceeds amount {amount}")));
                }
                b.pending_mint = sub("pending_mint", b.pending_mint, amount as u128)?;
                b.outstanding = add("outstanding", b.outstanding, (amount - fee) as u128)?;
                b.fees = add("fees", b.fees, fee as u128)?;
            }
            Transition::RefundStarted { amount } => {
                b.pending_mint = sub("pending_mint", b.pending_mint, amount as u128)?;
                b.pending_refund = add("pending_refund", b.pending_refund, amount as u128)?;
            }
            Transition::RefundCompleted { amount } => {
                b.pending_refund = sub("pending_refund", b.pending_refund, amount as u128)?;
            }
            Transition::ReturnFinalized { amount } => {
                b.outstanding = sub("outstanding", b.outstanding, amount as u128)?;
                b.pending_release = add("pending_release", b.pending_release, amount as u128)?;
            }
            Transition::ReleaseHeld { amount } => {
                b.pending_release = sub("pending_release", b.pending_release, amount as u128)?;
                b.held = add("held", b.held, amount as u128)?;
            }
            Transition::ReleaseCompleted { amount, release_fee } => {
                if release_fee > amount {
                    return Err(AccountingError::InvalidAmounts(format!(
                        "release fee {release_fee} exceeds amount {amount}"
                    )));
                }
                b.pending_release = sub("pending_release", b.pending_release, amount as u128)?;
                b.fees = add("fees", b.fees, release_fee as u128)?;
            }
            Transition::RemintStarted { amount } => {
                b.held = sub("held", b.held, amount as u128)?;
                b.pending_mint = add("pending_mint", b.pending_mint, amount as u128)?;
            }
            Transition::RemintDelivered { amount } => {
                b.pending_mint = sub("pending_mint", b.pending_mint, amount as u128)?;
                b.outstanding = add("outstanding", b.outstanding, amount as u128)?;
            }
            Transition::SweepCompleted { amount } => {
                b.fees = sub("fees", b.fees, amount as u128)?;
            }
            Transition::OutflowUnknown { amount } => {
                b.unknown_outflow = add("unknown_outflow", b.unknown_outflow, amount as u128)?;
            }
            Transition::OutflowResolved { amount } => {
                b.unknown_outflow = sub("unknown_outflow", b.unknown_outflow, amount as u128)?;
            }
        }
        Ok(b)
    }

    /// Escrow change caused by a transition, if any. Used by tests and the audit to relate bucket
    /// changes to expected ledger balance changes. `ReturnFinalized` moves no ICP.
    pub fn escrow_delta(t: &Transition) -> i128 {
        match *t {
            Transition::PullSucceeded { amount } => amount as i128,
            Transition::RefundCompleted { amount } => -(amount as i128),
            Transition::ReleaseCompleted { amount, release_fee } => -((amount - release_fee) as i128),
            Transition::SweepCompleted { amount } => -(amount as i128),
            _ => 0,
        }
    }
}

/// Result of checking invariant C1 against an observed escrow balance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageReport {
    pub escrow_balance: u128,
    pub liabilities: u128,
    pub unknown_outflow: u128,
    /// `liabilities - unknown_outflow`, floored at zero.
    pub required: u128,
    /// `escrow_balance - required` when covered.
    pub surplus: Option<u128>,
    /// `required - escrow_balance` when not covered.
    pub deficit: Option<u128>,
    pub ok: bool,
}

pub fn check_coverage(b: &Buckets, escrow_balance: u128) -> Result<CoverageReport, AccountingError> {
    let liabilities = b.liabilities()?;
    let required = liabilities.saturating_sub(b.unknown_outflow);
    let (surplus, deficit, ok) = if escrow_balance >= required {
        (Some(escrow_balance - required), None, true)
    } else {
        (None, Some(required - escrow_balance), false)
    };
    Ok(CoverageReport { escrow_balance, liabilities, unknown_outflow: b.unknown_outflow, required, surplus, deficit, ok })
}

/// Result of checking invariants C2 and C3 against a finalized Gate supply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupplyReport {
    pub supply: u128,
    pub outstanding: u128,
    pub inflight_mint: u128,
    /// `O + M_inflight`.
    pub bound: u128,
    /// C2: `supply <= bound`. A violation halts the destination.
    pub within_bound: bool,
    /// True only when the caller says the observer is caught up AND no mint is in flight.
    pub quiescent: bool,
    /// C3 `supply == O`: `Some` only when quiescent. Sui has no unattributed burn, so any
    /// difference is a real discrepancy, not a gap to be explained.
    pub exact_match: Option<bool>,
}

/// Check C2 and, when quiescent, C3. `idle` is the caller's statement that the observer is
/// caught up and no operation is mid-flight; the function additionally requires
/// `inflight_mint == 0` before it calls the state quiescent.
pub fn check_destination_supply(
    b: &Buckets,
    supply: u128,
    inflight_mint: u128,
    idle: bool,
) -> Result<SupplyReport, AccountingError> {
    if inflight_mint > b.pending_mint {
        return Err(AccountingError::InvalidAmounts(format!(
            "in-flight mint {inflight_mint} exceeds pending mint {}",
            b.pending_mint
        )));
    }
    let bound = add("supply_bound", b.outstanding, inflight_mint)?;
    let quiescent = idle && inflight_mint == 0;
    let exact_match = if quiescent { Some(supply == b.outstanding) } else { None };
    Ok(SupplyReport {
        supply,
        outstanding: b.outstanding,
        inflight_mint,
        bound,
        within_bound: supply <= bound,
        quiescent,
        exact_match,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    const E8: u64 = 100_000_000;

    fn run(start: Buckets, ts: &[Transition]) -> Result<Buckets, AccountingError> {
        ts.iter().try_fold(start, |b, t| b.apply(t))
    }

    /// Track the escrow balance implied by transitions alongside the buckets.
    fn run_with_escrow(ts: &[Transition]) -> (Buckets, i128) {
        let mut b = Buckets::default();
        let mut escrow = 0i128;
        for t in ts {
            b = b.apply(t).unwrap_or_else(|e| panic!("{t:?}: {e}"));
            escrow += Buckets::escrow_delta(t);
            let report = check_coverage(&b, escrow as u128).unwrap();
            assert!(report.ok, "coverage broken after {t:?}: {report:?}");
        }
        (b, escrow)
    }

    fn minted(a: u64) -> Vec<Transition> {
        vec![
            Transition::AdmitDeposit { amount: a },
            Transition::PullSucceeded { amount: a },
            Transition::MintDelivered { amount: a, fee: 0 },
        ]
    }

    /// A full round trip of `a`: mint, atomic-burn return, release.
    fn round_trip(a: u64) -> Vec<Transition> {
        let mut ts = minted(a);
        ts.extend([Transition::ReturnFinalized { amount: a }, Transition::ReleaseCompleted { amount: a, release_fee: 0 }]);
        ts
    }

    #[test]
    fn round_trip_100_icusd_no_fees() {
        let (b, escrow) = run_with_escrow(&round_trip(100 * E8));
        assert_eq!(b, Buckets::default());
        assert_eq!(escrow, 0);
    }

    #[test]
    fn liabilities_are_the_documented_sum() {
        let b = Buckets {
            outstanding: 1,
            pending_mint: 2,
            pending_release: 4,
            pending_refund: 8,
            held: 16,
            fees: 32,
            reserved_pulls: 1_000,
            unknown_outflow: 2_000,
        };
        assert_eq!(b.liabilities().unwrap(), 63, "reserved pulls and unknown outflow are not liabilities");
        assert_eq!(b.destination_exposure().unwrap(), 3);
    }

    #[test]
    fn a_return_moves_o_to_pr_and_nothing_else() {
        let a = 7 * E8;
        let start = run(Buckets::default(), &minted(a)).unwrap();
        let pr = start.apply(&Transition::ReturnFinalized { amount: a }).unwrap();
        assert_eq!((pr.outstanding, pr.pending_release), (0, a as u128));
        assert_eq!(pr.liabilities().unwrap(), start.liabilities().unwrap(), "no ICP moved");
        assert_eq!(Buckets::escrow_delta(&Transition::ReturnFinalized { amount: a }), 0);
        // More than outstanding cannot be returned.
        assert!(matches!(start.apply(&Transition::ReturnFinalized { amount: a + 1 }), Err(AccountingError::Underflow { .. })));
    }

    #[test]
    fn nothing_is_payable_without_a_final_return() {
        for b in [
            Buckets { outstanding: 5, ..Default::default() },
            Buckets { pending_mint: 5, ..Default::default() },
            Buckets { held: 5, ..Default::default() },
            Buckets { pending_refund: 5, ..Default::default() },
            Buckets { fees: 5, ..Default::default() },
        ] {
            for amount in [1u64, 5, 6] {
                assert!(b.apply(&Transition::ReleaseCompleted { amount, release_fee: 0 }).is_err(), "{b:?} released {amount}");
                assert!(b.apply(&Transition::ReleaseHeld { amount }).is_err(), "{b:?} held {amount}");
            }
        }
    }

    #[test]
    fn exactly_one_mint_per_escrowed_deposit() {
        let a = 100 * E8;
        let b = run(Buckets::default(), &minted(a)).unwrap();
        assert_eq!(b.outstanding, a as u128);
        assert!(matches!(b.apply(&Transition::MintDelivered { amount: a, fee: 0 }), Err(AccountingError::Underflow { .. })));
        assert!(b.apply(&Transition::RefundStarted { amount: a }).is_err(), "a delivered mint cannot also be refunded");
    }

    #[test]
    fn exactly_one_release_per_final_return() {
        let a = 100 * E8;
        let b = run(Buckets::default(), &round_trip(a)).unwrap();
        assert!(b.apply(&Transition::ReleaseCompleted { amount: a, release_fee: 0 }).is_err());
        assert!(b.apply(&Transition::ReturnFinalized { amount: 1 }).is_err());
    }

    #[test]
    fn fees_are_separate_from_backing() {
        let a = 100 * E8;
        let dep_fee = E8 / 10;
        let rel_fee = E8 / 20;
        let m = a - dep_fee;
        let (b, escrow) = run_with_escrow(&[
            Transition::AdmitDeposit { amount: a },
            Transition::PullSucceeded { amount: a },
            Transition::MintDelivered { amount: a, fee: dep_fee },
            Transition::ReturnFinalized { amount: m },
            Transition::ReleaseCompleted { amount: m, release_fee: rel_fee },
        ]);
        assert_eq!(b.outstanding, 0);
        assert_eq!(b.fees, (dep_fee + rel_fee) as u128);
        assert_eq!(escrow as u128, b.fees, "after the round trip only the fees remain in escrow");
        assert!(b.apply(&Transition::SweepCompleted { amount: dep_fee + rel_fee + 1 }).is_err());
        let swept = b.apply(&Transition::SweepCompleted { amount: dep_fee + rel_fee }).unwrap();
        assert_eq!(swept, Buckets::default());
    }

    #[test]
    fn refund_path_never_also_delivers() {
        let a = 5 * E8;
        let (b, escrow) = run_with_escrow(&[
            Transition::AdmitDeposit { amount: a },
            Transition::PullSucceeded { amount: a },
            Transition::RefundStarted { amount: a },
            Transition::RefundCompleted { amount: a },
        ]);
        assert_eq!(b, Buckets::default());
        assert_eq!(escrow, 0);
        assert!(b.apply(&Transition::MintDelivered { amount: a, fee: 0 }).is_err());
    }

    #[test]
    fn held_release_remint_restores_outstanding() {
        let a = 3 * E8;
        let mut ts = minted(a);
        ts.extend([Transition::ReturnFinalized { amount: a }, Transition::ReleaseHeld { amount: a }]);
        let (b, _) = run_with_escrow(&ts);
        assert_eq!((b.held, b.outstanding, b.pending_release), (a as u128, 0, 0));
        let b = run(b, &[Transition::RemintStarted { amount: a }, Transition::RemintDelivered { amount: a }]).unwrap();
        assert_eq!((b.outstanding, b.held, b.pending_mint), (a as u128, 0, 0));
    }

    #[test]
    fn failed_pull_releases_capacity_only() {
        let a = E8;
        let b = run(Buckets::default(), &[Transition::AdmitDeposit { amount: a }, Transition::PullFailed { amount: a }]).unwrap();
        assert_eq!(b, Buckets::default());
        assert!(b.apply(&Transition::PullFailed { amount: 1 }).is_err());
        assert!(b.apply(&Transition::PullSucceeded { amount: 1 }).is_err());
    }

    #[test]
    fn unknown_outflow_keeps_coverage_true_until_resolved() {
        let a = 10 * E8;
        let mut ts = round_trip(a);
        ts.pop(); // stop before the release completes
        let b = run(Buckets::default(), &ts).unwrap();
        assert_eq!(b.pending_release, a as u128);
        let b = b.apply(&Transition::OutflowUnknown { amount: a }).unwrap();
        // The release executed on the ledger but the reply was lost: escrow is 0.
        assert!(check_coverage(&b, 0).unwrap().ok);
        let done = run(b, &[Transition::OutflowResolved { amount: a }, Transition::ReleaseCompleted { amount: a, release_fee: 0 }]).unwrap();
        assert!(check_coverage(&done, 0).unwrap().ok);
        let not_done = b.apply(&Transition::OutflowResolved { amount: a }).unwrap();
        assert!(check_coverage(&not_done, a as u128).unwrap().ok);
        assert!(!check_coverage(&not_done, (a - 1) as u128).unwrap().ok);
    }

    #[test]
    fn coverage_reports_surplus_and_deficit() {
        let b = Buckets { outstanding: 100, pending_release: 10, pending_mint: 5, fees: 1, ..Default::default() };
        assert_eq!(b.liabilities().unwrap(), 116);
        let r = check_coverage(&b, 115).unwrap();
        assert!(!r.ok);
        assert_eq!(r.deficit, Some(1));
        let r = check_coverage(&b, 116).unwrap();
        assert!(r.ok);
        assert_eq!(r.surplus, Some(0));
        assert_eq!(check_coverage(&b, 200).unwrap().surplus, Some(84));
    }

    #[test]
    fn fee_larger_than_amount_is_rejected() {
        let b = Buckets { pending_mint: 10, ..Default::default() };
        assert!(matches!(b.apply(&Transition::MintDelivered { amount: 10, fee: 11 }), Err(AccountingError::InvalidAmounts(_))));
        let b = Buckets { pending_release: 10, ..Default::default() };
        assert!(b.apply(&Transition::ReleaseCompleted { amount: 10, release_fee: 11 }).is_err());
    }

    #[test]
    fn eight_decimal_boundaries() {
        let b = run(Buckets::default(), &[Transition::AdmitDeposit { amount: 1 }, Transition::PullSucceeded { amount: 1 }, Transition::MintDelivered { amount: 1, fee: 0 }]).unwrap();
        assert_eq!(b.outstanding, 1);
        let max = u64::MAX;
        let b = run(Buckets::default(), &[Transition::AdmitDeposit { amount: max }, Transition::PullSucceeded { amount: max }, Transition::MintDelivered { amount: max, fee: 1 }]).unwrap();
        assert_eq!(b.outstanding, (max - 1) as u128);
        assert_eq!(b.fees, 1);
        let c = run(b, &[Transition::ReturnFinalized { amount: max - 1 }]).unwrap();
        assert_eq!(c.pending_release, (max - 1) as u128);
    }

    #[test]
    fn overflow_is_rejected_not_wrapped() {
        let b = Buckets { outstanding: u128::MAX, pending_mint: 1, ..Default::default() };
        assert!(matches!(b.apply(&Transition::MintDelivered { amount: 1, fee: 0 }), Err(AccountingError::Overflow { .. })));
        assert!(matches!(b.liabilities(), Err(AccountingError::Overflow { .. })));
        assert!(matches!(b.destination_exposure(), Err(AccountingError::Overflow { .. })));
    }

    #[test]
    fn apply_is_atomic_on_error() {
        let b = Buckets { reserved_pulls: 5, ..Default::default() };
        let before = b;
        assert!(b.apply(&Transition::PullSucceeded { amount: 6 }).is_err());
        assert_eq!(b, before);
    }

    #[test]
    fn supply_bound_c2_and_exact_c3() {
        let b = Buckets { outstanding: 10, pending_mint: 3, ..Default::default() };
        let r = check_destination_supply(&b, 10, 0, true).unwrap();
        assert!(r.within_bound && r.quiescent);
        assert_eq!(r.exact_match, Some(true));
        // A supply above O + in-flight is a violation of C2.
        let r = check_destination_supply(&b, 11, 0, true).unwrap();
        assert!(!r.within_bound);
        assert_eq!(r.exact_match, Some(false));
        // In-flight mints widen the bound and make the state non-quiescent.
        let r = check_destination_supply(&b, 12, 3, true).unwrap();
        assert!(r.within_bound && !r.quiescent && r.exact_match.is_none());
        // Not idle: no exact check even with nothing in flight.
        assert_eq!(check_destination_supply(&b, 10, 0, false).unwrap().exact_match, None);
        // Sui has no unattributed burn: a supply below O when quiescent is a discrepancy, not a gap.
        let r = check_destination_supply(&b, 9, 0, true).unwrap();
        assert_eq!(r.exact_match, Some(false));
        assert!(check_destination_supply(&b, 10, 4, true).is_err(), "in-flight cannot exceed pending mint");
    }

    /// Every transition that can be applied to a state, over a small amount alphabet.
    fn all_transitions() -> Vec<Transition> {
        let mut v = vec![];
        for a in [1u64, 2] {
            v.extend([
                Transition::AdmitDeposit { amount: a },
                Transition::PullFailed { amount: a },
                Transition::PullSucceeded { amount: a },
                Transition::MintDelivered { amount: a, fee: 0 },
                Transition::MintDelivered { amount: a, fee: 1 },
                Transition::RefundStarted { amount: a },
                Transition::RefundCompleted { amount: a },
                Transition::ReturnFinalized { amount: a },
                Transition::ReleaseHeld { amount: a },
                Transition::ReleaseCompleted { amount: a, release_fee: 0 },
                Transition::ReleaseCompleted { amount: a, release_fee: 1 },
                Transition::RemintStarted { amount: a },
                Transition::RemintDelivered { amount: a },
                Transition::SweepCompleted { amount: a },
                Transition::OutflowUnknown { amount: a },
                Transition::OutflowResolved { amount: a },
            ]);
        }
        v
    }

    /// Exhaustive breadth-first exploration of every state reachable within `depth` transitions:
    /// `pending_release` grows only through `ReturnFinalized` (and `ReleaseHeld` never feeds it),
    /// a release never exceeds `pending_release`, `ReturnFinalized` moves no escrow, and C1 holds
    /// against the escrow implied by the transition deltas.
    #[test]
    fn lifecycle_property_exhaustive_small_state_space() {
        type State = (Buckets, i128);
        let transitions = all_transitions();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut frontier: Vec<State> = vec![(Buckets::default(), 0)];
        seen.insert(format!("{:?}", frontier[0]));
        let mut explored = 0usize;
        for _depth in 0..9 {
            let mut next: Vec<State> = vec![];
            for (b, escrow) in &frontier {
                for t in &transitions {
                    let Ok(nb) = b.apply(t) else { continue };
                    explored += 1;
                    let delta = Buckets::escrow_delta(t);
                    let ne = escrow + delta;
                    if nb.pending_release > b.pending_release {
                        match t {
                            Transition::ReturnFinalized { amount } => {
                                assert_eq!(nb.pending_release - b.pending_release, *amount as u128);
                                assert_eq!(b.outstanding - nb.outstanding, *amount as u128);
                            }
                            other => panic!("PR grew through {other:?} from {b:?}"),
                        }
                    }
                    if let Transition::ReleaseCompleted { amount, .. } | Transition::ReleaseHeld { amount } = t {
                        assert!(*amount as u128 <= b.pending_release, "{t:?} paid more than PR in {b:?}");
                    }
                    if matches!(t, Transition::ReturnFinalized { .. }) {
                        assert_eq!(delta, 0, "{t:?} must not move ICP");
                    }
                    assert!(ne >= 0, "escrow went negative after {t:?}");
                    let cov = check_coverage(&nb, ne as u128).unwrap();
                    assert!(cov.ok, "C1 broken after {t:?}: {cov:?}");
                    if seen.insert(format!("{:?}", (nb, ne))) {
                        next.push((nb, ne));
                    }
                }
            }
            frontier = next;
        }
        assert!(explored > 10_000, "the exploration should be non-trivial, got {explored}");
    }

    #[test]
    fn lifecycle_property_ghost_totals_over_random_walks() {
        // Deterministic pseudo-random walk. Ghost totals: everything ever released was first
        // returned-final, and PR equals returned-final minus released minus held-from-PR.
        let mut seed: u64 = 0x5eed_1234;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut b = Buckets::default();
        let mut escrow: i128 = 0;
        let (mut returned, mut released, mut held_from_pr) = (0u128, 0u128, 0u128);
        for _ in 0..20_000 {
            let amt = (next() % 1_000) + 1;
            let fee = next() % (amt + 1);
            let candidates = [
                Transition::AdmitDeposit { amount: amt },
                Transition::PullFailed { amount: amt },
                Transition::PullSucceeded { amount: amt },
                Transition::MintDelivered { amount: amt, fee },
                Transition::RefundStarted { amount: amt },
                Transition::RefundCompleted { amount: amt },
                Transition::ReturnFinalized { amount: amt },
                Transition::ReturnFinalized { amount: amt },
                Transition::ReleaseHeld { amount: amt },
                Transition::ReleaseCompleted { amount: amt, release_fee: fee },
                Transition::RemintStarted { amount: amt },
                Transition::RemintDelivered { amount: amt },
                Transition::SweepCompleted { amount: amt },
            ];
            let t = &candidates[(next() % candidates.len() as u64) as usize];
            if let Ok(nb) = b.apply(t) {
                match t {
                    Transition::ReturnFinalized { amount } => returned += *amount as u128,
                    Transition::ReleaseCompleted { amount, .. } => released += *amount as u128,
                    Transition::ReleaseHeld { amount } => held_from_pr += *amount as u128,
                    _ => {}
                }
                b = nb;
                escrow += Buckets::escrow_delta(t);
                assert!(escrow >= 0);
                let r = check_coverage(&b, escrow as u128).unwrap();
                assert!(r.ok, "{t:?} broke coverage: {r:?}");
                assert_eq!(r.surplus, Some(0), "{t:?}: with no unknown outflows escrow equals liabilities");
                assert_eq!(b.pending_release, returned - released - held_from_pr, "PR is fed only by ReturnFinalized ({t:?})");
                assert!(released <= returned);
            }
        }
        assert!(returned > 0 && released > 0, "the walk exercised the lifecycle: {returned} {released}");
    }
}
