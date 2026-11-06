//! Escrow accounting: disjoint liability buckets and the only functions allowed to
//! change them.
//!
//! Every amount is in e8s. The buckets are liabilities of the escrow account
//! `(bridge canister, ESCROW_SUBACCOUNT)`; none of them is borrowing debt.
//!
//! | Field              | Symbol | Meaning |
//! |--------------------|--------|---------|
//! | `outstanding_base` | O      | Finalized delivered Base mints minus finalized processed bridge-outs |
//! | `pending_mint`     | PM     | Escrowed deposit amounts (and re-mints) not yet delivered on Base |
//! | `pending_release`  | PR     | Finalized valid bridge-outs not yet released on ICP |
//! | `pending_refund`   | RF     | Deposits being refunded to their ICP source account |
//! | `held`             | H      | Finalized bridge-outs that cannot be released as requested |
//! | `fees`             | F      | Earned bridge fees not yet swept |
//!
//! `reserved_pulls` is admission capacity for pulls whose outcome is not proven yet.
//! It is not a liability (no funds have moved). `unknown_outflow` (U) is the escrow
//! decrease that transfers with an unknown outcome would cause if they executed.
//!
//! Invariant C1 (coverage): `escrow_balance >= liabilities - unknown_outflow`.
//! Invariant C2 (Base bound): `base_total_supply <= outstanding_base + inflight_mint`.
//! Invariant C3 (quiescent match): with nothing in flight and the observer caught up,
//! `base_total_supply == outstanding_base`.

use candid::CandidType;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Buckets {
    #[serde(default)]
    pub outstanding_base: u128,
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

/// The complete set of bucket changes. Each variant is one row of the design's
/// transition table (design section 7.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, CandidType)]
pub enum Transition {
    /// A deposit was admitted; reserve capacity for its pull.
    AdmitDeposit { amount: u64 },
    /// The pull was proven not to have happened.
    PullFailed { amount: u64 },
    /// The pull was proven (`Ok` or `Duplicate`); the escrow now holds `amount` more.
    PullSucceeded { amount: u64 },
    /// A finalized, verified Base mint delivered `amount - fee` for a deposit of `amount`.
    MintDelivered { amount: u64, fee: u64 },
    /// Non-delivery was proven and a refund of the full deposit started.
    RefundStarted { amount: u64 },
    /// The refund transfer was proven; the escrow decreased by `amount`.
    RefundCompleted { amount: u64 },
    /// A finalized bridge-out of `amount` will be released on ICP.
    BridgeOutAccepted { amount: u64 },
    /// A finalized bridge-out of `amount` cannot be released as requested.
    BridgeOutHeld { amount: u64 },
    /// An accepted release became unreleasable (for example the ledger fee rose above
    /// it) and moves to the held bucket for re-mint.
    ReleaseHeld { amount: u64 },
    /// The release transfer was proven; the escrow decreased by `amount - release_fee`.
    ReleaseCompleted { amount: u64, release_fee: u64 },
    /// A held claim is being re-minted to its Base sender.
    RemintStarted { amount: u64 },
    /// The re-mint was finalized and verified on Base.
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
            ("outstanding_base", self.outstanding_base),
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

    /// Value that will exist on Base, or already does, for capacity checks: `O + PM`.
    pub fn base_exposure(&self) -> Result<u128, AccountingError> {
        add("base_exposure", self.outstanding_base, self.pending_mint)
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
                b.outstanding_base = add("outstanding_base", b.outstanding_base, (amount - fee) as u128)?;
                b.fees = add("fees", b.fees, fee as u128)?;
            }
            Transition::RefundStarted { amount } => {
                b.pending_mint = sub("pending_mint", b.pending_mint, amount as u128)?;
                b.pending_refund = add("pending_refund", b.pending_refund, amount as u128)?;
            }
            Transition::RefundCompleted { amount } => {
                b.pending_refund = sub("pending_refund", b.pending_refund, amount as u128)?;
            }
            Transition::BridgeOutAccepted { amount } => {
                b.outstanding_base = sub("outstanding_base", b.outstanding_base, amount as u128)?;
                b.pending_release = add("pending_release", b.pending_release, amount as u128)?;
            }
            Transition::BridgeOutHeld { amount } => {
                b.outstanding_base = sub("outstanding_base", b.outstanding_base, amount as u128)?;
                b.held = add("held", b.held, amount as u128)?;
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
                b.outstanding_base = add("outstanding_base", b.outstanding_base, amount as u128)?;
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

    /// Escrow decrease caused by a transition, if any. Used by tests and the audit to
    /// relate bucket changes to expected ledger balance changes.
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
    Ok(CoverageReport {
        escrow_balance,
        liabilities,
        unknown_outflow: b.unknown_outflow,
        required,
        surplus,
        deficit,
        ok,
    })
}

/// Result of checking invariants C2 and C3 against a finalized Base `totalSupply`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaseSupplyReport {
    pub base_total_supply: u128,
    pub outstanding_base: u128,
    pub inflight_mint: u128,
    /// C2: `base_total_supply <= outstanding_base + inflight_mint`.
    pub within_bound: bool,
    /// C3, only meaningful when `quiescent` is true.
    pub exact_match: Option<bool>,
}

pub fn check_base_supply(
    b: &Buckets,
    base_total_supply: u128,
    inflight_mint: u128,
    quiescent: bool,
) -> BaseSupplyReport {
    let bound = b.outstanding_base.saturating_add(inflight_mint);
    BaseSupplyReport {
        base_total_supply,
        outstanding_base: b.outstanding_base,
        inflight_mint,
        within_bound: base_total_supply <= bound,
        exact_match: if quiescent { Some(base_total_supply == b.outstanding_base) } else { None },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn round_trip_100_icusd_no_fees() {
        let a = 100 * E8;
        let (b, escrow) = run_with_escrow(&[
            Transition::AdmitDeposit { amount: a },
            Transition::PullSucceeded { amount: a },
            Transition::MintDelivered { amount: a, fee: 0 },
            Transition::BridgeOutAccepted { amount: a },
            Transition::ReleaseCompleted { amount: a, release_fee: 0 },
        ]);
        assert_eq!(b, Buckets::default());
        assert_eq!(escrow, 0);
    }

    #[test]
    fn exactly_one_mint_per_escrowed_deposit() {
        let a = 100 * E8;
        let b = run(
            Buckets::default(),
            &[
                Transition::AdmitDeposit { amount: a },
                Transition::PullSucceeded { amount: a },
                Transition::MintDelivered { amount: a, fee: 0 },
            ],
        )
        .unwrap();
        assert_eq!(b.outstanding_base, a as u128);
        // A second delivery for the same deposit has no pending mint to consume.
        assert!(matches!(
            b.apply(&Transition::MintDelivered { amount: a, fee: 0 }),
            Err(AccountingError::Underflow { .. })
        ));
        // Nor can it then be refunded.
        assert!(b.apply(&Transition::RefundStarted { amount: a }).is_err());
    }

    #[test]
    fn exactly_one_release_per_finalized_burn() {
        let a = 100 * E8;
        let b = run(
            Buckets::default(),
            &[
                Transition::AdmitDeposit { amount: a },
                Transition::PullSucceeded { amount: a },
                Transition::MintDelivered { amount: a, fee: 0 },
                Transition::BridgeOutAccepted { amount: a },
                Transition::ReleaseCompleted { amount: a, release_fee: 0 },
            ],
        )
        .unwrap();
        assert!(b.apply(&Transition::ReleaseCompleted { amount: a, release_fee: 0 }).is_err());
        // A bridge-out larger than outstanding Base supply cannot be accepted.
        assert!(b.apply(&Transition::BridgeOutAccepted { amount: 1 }).is_err());
    }

    #[test]
    fn fees_are_separate_from_backing() {
        let a = 100 * E8;
        let dep_fee = E8 / 10; // 0.1 icUSD
        let rel_fee = E8 / 20; // 0.05 icUSD
        let m = a - dep_fee;
        let (b, escrow) = run_with_escrow(&[
            Transition::AdmitDeposit { amount: a },
            Transition::PullSucceeded { amount: a },
            Transition::MintDelivered { amount: a, fee: dep_fee },
            Transition::BridgeOutAccepted { amount: m },
            Transition::ReleaseCompleted { amount: m, release_fee: rel_fee },
        ]);
        assert_eq!(b.outstanding_base, 0);
        assert_eq!(b.fees, (dep_fee + rel_fee) as u128);
        assert_eq!(escrow as u128, b.fees);
        // Sweeping more than earned fees is impossible.
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
    fn held_claim_remint_restores_outstanding() {
        let a = 3 * E8;
        let (b, _) = run_with_escrow(&[
            Transition::AdmitDeposit { amount: a },
            Transition::PullSucceeded { amount: a },
            Transition::MintDelivered { amount: a, fee: 0 },
            Transition::BridgeOutHeld { amount: a },
        ]);
        assert_eq!(b.held, a as u128);
        assert_eq!(b.outstanding_base, 0);
        let b = run(b, &[Transition::RemintStarted { amount: a }, Transition::RemintDelivered { amount: a }]).unwrap();
        assert_eq!(b.outstanding_base, a as u128);
        assert_eq!(b.held, 0);
        assert_eq!(b.pending_mint, 0);
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
        let b = run(
            Buckets::default(),
            &[
                Transition::AdmitDeposit { amount: a },
                Transition::PullSucceeded { amount: a },
                Transition::MintDelivered { amount: a, fee: 0 },
                Transition::BridgeOutAccepted { amount: a },
                Transition::OutflowUnknown { amount: a },
            ],
        )
        .unwrap();
        // The release executed on the ledger but the reply was lost: escrow is 0.
        let r = check_coverage(&b, 0).unwrap();
        assert!(r.ok, "{r:?}");
        // Resolved as executed.
        let done = run(b, &[Transition::OutflowResolved { amount: a }, Transition::ReleaseCompleted { amount: a, release_fee: 0 }]).unwrap();
        assert!(check_coverage(&done, 0).unwrap().ok);
        // Resolved as not executed: escrow still holds a.
        let not_done = b.apply(&Transition::OutflowResolved { amount: a }).unwrap();
        assert!(check_coverage(&not_done, a as u128).unwrap().ok);
        assert!(!check_coverage(&not_done, (a - 1) as u128).unwrap().ok);
    }

    #[test]
    fn coverage_reports_deficit_and_surplus_exactly() {
        let b = Buckets { outstanding_base: 100, pending_mint: 5, fees: 1, ..Default::default() };
        let r = check_coverage(&b, 105).unwrap();
        assert!(!r.ok);
        assert_eq!(r.deficit, Some(1));
        let r = check_coverage(&b, 106).unwrap();
        assert!(r.ok);
        assert_eq!(r.surplus, Some(0));
        let r = check_coverage(&b, 200).unwrap();
        assert_eq!(r.surplus, Some(94));
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
        // One e8s works, as does u64::MAX.
        let b = run(Buckets::default(), &[Transition::AdmitDeposit { amount: 1 }, Transition::PullSucceeded { amount: 1 }, Transition::MintDelivered { amount: 1, fee: 0 }]).unwrap();
        assert_eq!(b.outstanding_base, 1);
        let max = u64::MAX;
        let b = run(Buckets::default(), &[Transition::AdmitDeposit { amount: max }, Transition::PullSucceeded { amount: max }, Transition::MintDelivered { amount: max, fee: 1 }]).unwrap();
        assert_eq!(b.outstanding_base, (max - 1) as u128);
        assert_eq!(b.fees, 1);
    }

    #[test]
    fn overflow_is_rejected_not_wrapped() {
        let b = Buckets { outstanding_base: u128::MAX, ..Default::default() };
        let b2 = Buckets { pending_mint: 1, ..b };
        assert!(matches!(b2.apply(&Transition::MintDelivered { amount: 1, fee: 0 }), Err(AccountingError::Overflow { .. })));
        assert!(matches!(b2.liabilities(), Err(AccountingError::Overflow { .. })));
    }

    #[test]
    fn apply_is_atomic_on_error() {
        let b = Buckets { reserved_pulls: 5, ..Default::default() };
        let before = b;
        assert!(b.apply(&Transition::PullSucceeded { amount: 6 }).is_err());
        assert_eq!(b, before);
    }

    #[test]
    fn base_supply_bound_and_exact_match() {
        let b = Buckets { outstanding_base: 100, ..Default::default() };
        assert!(check_base_supply(&b, 100, 0, true).within_bound);
        assert_eq!(check_base_supply(&b, 100, 0, true).exact_match, Some(true));
        assert_eq!(check_base_supply(&b, 99, 0, true).exact_match, Some(false));
        assert!(!check_base_supply(&b, 101, 0, false).within_bound);
        // An unconfirmed mint may already be on Base.
        assert!(check_base_supply(&b, 150, 50, false).within_bound);
        assert!(!check_base_supply(&b, 151, 50, false).within_bound);
        assert_eq!(check_base_supply(&b, 150, 50, false).exact_match, None);
    }

    #[test]
    fn transition_sequences_preserve_coverage() {
        // Deterministic pseudo-random walk over valid transitions.
        let mut seed: u64 = 0x5eed;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut b = Buckets::default();
        let mut escrow: i128 = 0;
        for _ in 0..5_000 {
            let amt = (next() % 1_000) + 1;
            let fee = next() % (amt + 1);
            let candidates = [
                Transition::AdmitDeposit { amount: amt },
                Transition::PullFailed { amount: amt },
                Transition::PullSucceeded { amount: amt },
                Transition::MintDelivered { amount: amt, fee },
                Transition::RefundStarted { amount: amt },
                Transition::RefundCompleted { amount: amt },
                Transition::BridgeOutAccepted { amount: amt },
                Transition::BridgeOutHeld { amount: amt },
                Transition::ReleaseHeld { amount: amt },
                Transition::ReleaseCompleted { amount: amt, release_fee: fee },
                Transition::RemintStarted { amount: amt },
                Transition::RemintDelivered { amount: amt },
                Transition::SweepCompleted { amount: amt },
            ];
            let t = &candidates[(next() % candidates.len() as u64) as usize];
            if let Ok(nb) = b.apply(t) {
                b = nb;
                escrow += Buckets::escrow_delta(t);
                assert!(escrow >= 0);
                let r = check_coverage(&b, escrow as u128).unwrap();
                assert!(r.ok, "{t:?} broke coverage: {r:?}");
                // With no unknown outflows the escrow equals liabilities exactly.
                assert_eq!(r.surplus, Some(0), "{t:?}");
            }
        }
    }
}
