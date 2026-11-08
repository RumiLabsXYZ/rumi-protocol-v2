//! Escrow accounting for one Solana destination: disjoint liability buckets and the only
//! functions allowed to change them. Copied from `rumi_icusd_bridge/src/accounting.rs`
//! (the Base instance) and extended for the **custodied return lifecycle** of the memo plus
//! durable-nonce option (`chain-solana.md` section 3; design section 2.3).
//!
//! Every amount is in e8s. The buckets are liabilities of the escrow account
//! `(bridge canister, ESCROW_SUBACCOUNT)`; none of them is borrowing debt.
//!
//! | Field              | Symbol | Meaning |
//! |--------------------|--------|---------|
//! | `outstanding`      | O      | Finalized delivered mints minus finalized valid returns |
//! | `pending_mint`     | PM     | Escrowed deposit amounts (and re-mints) not yet delivered on Solana |
//! | `return_custodied` | RC     | Finalized valid returns whose tokens sit in the return account, not yet burned |
//! | `burn_unknown`     | BU     | Custodied returns whose burn was signed but is not yet final or reconciled |
//! | `pending_release`  | PR     | Returns whose tokens are destroyed (burn final), not yet released on ICP |
//! | `pending_refund`   | RF     | Deposits being refunded to their ICP source account |
//! | `held`             | H      | Burned returns that cannot be released as requested |
//! | `fees`             | F      | Earned bridge fees not yet swept |
//!
//! `stray` is **not** a liability and is not in `liabilities()`: it is the part of `O` that
//! sits in the return account as tokens that failed the exact-shape check (never credited,
//! awaiting refund to the verified sender, section 2.4). It exists so the custody audit
//! `custody_measured == RC + BU + stray` can be stated, and is bounded by `O` (a stray is
//! circulating supply that landed in custody). `reserved_pulls` is admission capacity for
//! pulls whose outcome is not proven yet (no funds moved, not a liability).
//! `unknown_outflow` (U) is the escrow decrease that transfers with an unknown outcome would
//! cause if they executed.
//!
//! # Lifecycle (the only way into `PR`)
//!
//! ```text
//! PM --MintDelivered--> O --ReturnCustodied--> RC --BurnSigned--> BU --BurnFinalized--> PR --ReleaseCompleted--> (ICP)
//!                                               ^                  |
//!                                               +---BurnNotLanded--+
//! ```
//!
//! There is deliberately **no** `O -> PR` transition (the Base `BridgeOutAccepted`): on
//! Solana a return does not destroy tokens, so a payout straight from `O` would pay ICP
//! while the tokens still circulate. `PR` is credited only by `BurnFinalized`, which
//! consumes `BU`, so **no ICP release is reachable while a unit is in `RC` or `BU`**
//! (`lifecycle_property_*` tests).
//!
//! # Invariants
//!
//! * C1 (coverage): `escrow_balance >= L - U`, `L = O + PM + RC + BU + PR + RF + H + F`.
//! * C2 (supply bound): `supply(final head) <= O + M_inflight + RC + BU`, `M_inflight <= PM`.
//! * C3 (quiescent match): with nothing in flight and the observer caught up,
//!   `supply + UB == O`, where `UB` is cumulative unattributed holder burn. **The `UB`
//!   seam is implementation missing** (blocker `solana-unattributed-burn-policy`, note
//!   section 12): until a burn observer exists only `supply <= O` is enforced and the gap
//!   `O - supply` is reported as unattributed. A legitimate supply decrease never halts,
//!   earns no ICP credit without a recipient-bound claim, and causes no automatic escrow
//!   sweep or refund.
//! * C4 (cumulative mints) does not exist on Solana (no on-chain mint counter under D3).

use candid::CandidType;
use serde::{Deserialize, Serialize};

/// Blocker id carried by the reports that expose the missing unattributed-burn seam.
pub const UNATTRIBUTED_BURN_BLOCKER: &str = "solana-unattributed-burn-policy";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Buckets {
    #[serde(default)]
    pub outstanding: u128,
    #[serde(default)]
    pub pending_mint: u128,
    #[serde(default)]
    pub return_custodied: u128,
    #[serde(default)]
    pub burn_unknown: u128,
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
    /// Memo bucket, a subset of `outstanding`; excluded from `liabilities()`.
    #[serde(default)]
    pub stray: u128,
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
    /// A finalized, verified Solana mint delivered `amount - fee` for a deposit of `amount`.
    MintDelivered { amount: u64, fee: u64 },
    /// Non-delivery was proven and a refund of the full deposit started.
    RefundStarted { amount: u64 },
    /// The refund transfer was proven; the escrow decreased by `amount`.
    RefundCompleted { amount: u64 },
    /// A finalized valid return (note 2.2) of `amount`: `O -> RC`. Supply is unchanged and the
    /// tokens sit in the return account.
    ReturnCustodied { amount: u64 },
    /// The burn message was persisted and then signed: `RC -> BU`.
    BurnSigned { amount: u64 },
    /// The burn is final, success, and its message bytes equal the stored message: `BU -> PR`.
    BurnFinalized { amount: u64 },
    /// The burn was proven not to have landed (the nonce was consumed by our own cancel or
    /// by our own failed burn): `BU -> RC`. The tokens are still in custody.
    BurnNotLanded { amount: u64 },
    /// An accepted release became unreleasable (for example the ledger fee rose above it)
    /// and moves to the held bucket for re-mint.
    ReleaseHeld { amount: u64 },
    /// The release transfer was proven; the escrow decreased by `amount - release_fee`.
    ReleaseCompleted { amount: u64, release_fee: u64 },
    /// A held claim is being re-minted to its Solana sender.
    RemintStarted { amount: u64 },
    /// The re-mint was finalized and verified on Solana.
    RemintDelivered { amount: u64 },
    /// A fee sweep was proven; the escrow decreased by `amount`.
    SweepCompleted { amount: u64 },
    /// An escrow outflow of `amount` (amount sent plus ledger fee) has an unknown outcome.
    OutflowUnknown { amount: u64 },
    /// A previously unknown escrow outflow of `amount` was resolved (either way).
    OutflowResolved { amount: u64 },
    /// A transfer of `amount` landed in the return account without passing the exact-shape
    /// check (note 2.2 table): it stays in `O`, tracked as `stray`.
    StrayObserved { amount: u64 },
    /// A stray of `amount` left the return account (refund to the verified sender, final).
    StrayRefunded { amount: u64 },
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
    /// `O + PM + RC + BU + PR + RF + H + F`. `stray` is a subset of `O` and is not added.
    pub fn liabilities(&self) -> Result<u128, AccountingError> {
        let mut total = 0u128;
        for (name, v) in [
            ("outstanding", self.outstanding),
            ("pending_mint", self.pending_mint),
            ("return_custodied", self.return_custodied),
            ("burn_unknown", self.burn_unknown),
            ("pending_release", self.pending_release),
            ("pending_refund", self.pending_refund),
            ("held", self.held),
            ("fees", self.fees),
        ] {
            total = add(name, total, v)?;
        }
        Ok(total)
    }

    /// Value that exists on Solana, or will, for capacity checks: `O + PM + RC + BU`.
    /// Custodied and burn-unknown returns are still circulating supply until the burn is final.
    pub fn destination_exposure(&self) -> Result<u128, AccountingError> {
        let mut total = add("destination_exposure", self.outstanding, self.pending_mint)?;
        total = add("destination_exposure", total, self.return_custodied)?;
        add("destination_exposure", total, self.burn_unknown)
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
            Transition::ReturnCustodied { amount } => {
                b.outstanding = sub("outstanding", b.outstanding, amount as u128)?;
                b.return_custodied = add("return_custodied", b.return_custodied, amount as u128)?;
            }
            Transition::BurnSigned { amount } => {
                b.return_custodied = sub("return_custodied", b.return_custodied, amount as u128)?;
                b.burn_unknown = add("burn_unknown", b.burn_unknown, amount as u128)?;
            }
            Transition::BurnFinalized { amount } => {
                b.burn_unknown = sub("burn_unknown", b.burn_unknown, amount as u128)?;
                b.pending_release = add("pending_release", b.pending_release, amount as u128)?;
            }
            Transition::BurnNotLanded { amount } => {
                b.burn_unknown = sub("burn_unknown", b.burn_unknown, amount as u128)?;
                b.return_custodied = add("return_custodied", b.return_custodied, amount as u128)?;
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
            Transition::StrayObserved { amount } => {
                b.stray = add("stray", b.stray, amount as u128)?;
            }
            Transition::StrayRefunded { amount } => {
                b.stray = sub("stray", b.stray, amount as u128)?;
            }
        }
        // A stray is circulating supply that landed in custody, so it is part of O and can
        // never exceed it. Checked after every transition so no path can break it.
        if b.stray > b.outstanding {
            return Err(AccountingError::InvalidAmounts(format!(
                "stray {} would exceed outstanding {}",
                b.stray, b.outstanding
            )));
        }
        Ok(b)
    }

    /// Escrow change caused by a transition, if any. Used by tests and the audit to
    /// relate bucket changes to expected ledger balance changes. Every return-lifecycle
    /// transition (`ReturnCustodied`, `BurnSigned`, `BurnFinalized`, `BurnNotLanded`) and
    /// every stray transition moves no ICP.
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

/// Result of checking invariants C2 and C3 against a finalized mint supply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupplyReport {
    pub supply: u128,
    pub outstanding: u128,
    pub inflight_mint: u128,
    pub return_custodied: u128,
    pub burn_unknown: u128,
    /// `O + M_inflight + RC + BU`.
    pub bound: u128,
    /// C2: `supply <= bound`. A violation halts the destination.
    pub within_bound: bool,
    /// True only when the caller says the observer is caught up AND no mint is in flight AND
    /// no return is custodied or burn-unknown.
    pub quiescent: bool,
    /// C3 `supply + UB == O`: `Some` only when quiescent and `UB` is known.
    pub exact_match: Option<bool>,
    /// Quiescent and `supply < O`: the gap `O - supply`, reported as unattributed holder burn
    /// (or as a not yet observed one). It is information only: it never halts, credits,
    /// sweeps or refunds (blocker [`UNATTRIBUTED_BURN_BLOCKER`]).
    pub unattributed_gap: Option<u128>,
}

/// Check C2 and, when quiescent, C3.
///
/// `unattributed_burn` is the cumulative `UB`; `None` means no burn observer exists yet
/// (the seam is implementation missing), in which case C3 is not evaluated and only the
/// reported gap remains. `idle` is the caller's statement that the observer is caught up and
/// no operation is mid-flight; the function additionally requires `inflight_mint == 0` and
/// `RC == BU == 0` before it calls the state quiescent.
pub fn check_destination_supply(
    b: &Buckets,
    supply: u128,
    inflight_mint: u128,
    idle: bool,
    unattributed_burn: Option<u128>,
) -> Result<SupplyReport, AccountingError> {
    if inflight_mint > b.pending_mint {
        return Err(AccountingError::InvalidAmounts(format!(
            "in-flight mint {inflight_mint} exceeds pending mint {}",
            b.pending_mint
        )));
    }
    let mut bound = add("supply_bound", b.outstanding, inflight_mint)?;
    bound = add("supply_bound", bound, b.return_custodied)?;
    bound = add("supply_bound", bound, b.burn_unknown)?;
    let quiescent = idle && inflight_mint == 0 && b.return_custodied == 0 && b.burn_unknown == 0;
    let exact_match = match (quiescent, unattributed_burn) {
        (true, Some(ub)) => Some(add("supply_plus_ub", supply, ub)? == b.outstanding),
        _ => None,
    };
    let unattributed_gap = if quiescent && supply < b.outstanding { Some(b.outstanding - supply) } else { None };
    Ok(SupplyReport {
        supply,
        outstanding: b.outstanding,
        inflight_mint,
        return_custodied: b.return_custodied,
        burn_unknown: b.burn_unknown,
        bound,
        within_bound: supply <= bound,
        quiescent,
        exact_match,
        unattributed_gap,
    })
}

/// Result of the custody audit: `custody_measured == RC + BU + stray`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustodyReport {
    /// Return-account token balance read at `finalized`.
    pub custody_measured: u128,
    /// `RC + BU + stray`.
    pub expected: u128,
    /// Measured above expected: tokens we cannot explain. Halts credits and raises an alarm.
    pub excess: Option<u128>,
    /// Measured below expected. With `BU > 0` a burn that already landed explains up to `BU`
    /// of it (see `deficit_within_burn_unknown`).
    pub deficit: Option<u128>,
    pub deficit_within_burn_unknown: bool,
    pub halt_credits: bool,
    pub ok: bool,
}

pub fn check_custody(b: &Buckets, custody_measured: u128) -> Result<CustodyReport, AccountingError> {
    let mut expected = add("custody_expected", b.return_custodied, b.burn_unknown)?;
    expected = add("custody_expected", expected, b.stray)?;
    let (excess, deficit) = if custody_measured > expected {
        (Some(custody_measured - expected), None)
    } else if custody_measured < expected {
        (None, Some(expected - custody_measured))
    } else {
        (None, None)
    };
    Ok(CustodyReport {
        custody_measured,
        expected,
        excess,
        deficit,
        deficit_within_burn_unknown: deficit.map(|d| d <= b.burn_unknown).unwrap_or(false),
        halt_credits: excess.is_some(),
        ok: excess.is_none() && deficit.is_none(),
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

    /// A full custodied round trip of `a`: mint, return, burn, release.
    fn round_trip(a: u64) -> Vec<Transition> {
        let mut ts = minted(a);
        ts.extend([
            Transition::ReturnCustodied { amount: a },
            Transition::BurnSigned { amount: a },
            Transition::BurnFinalized { amount: a },
            Transition::ReleaseCompleted { amount: a, release_fee: 0 },
        ]);
        ts
    }

    #[test]
    fn round_trip_100_icusd_no_fees() {
        let (b, escrow) = run_with_escrow(&round_trip(100 * E8));
        assert_eq!(b, Buckets::default());
        assert_eq!(escrow, 0);
    }

    #[test]
    fn liabilities_are_the_documented_sum_and_exclude_stray() {
        let b = Buckets {
            outstanding: 1,
            pending_mint: 2,
            return_custodied: 4,
            burn_unknown: 8,
            pending_release: 16,
            pending_refund: 32,
            held: 64,
            fees: 128,
            reserved_pulls: 1_000,
            unknown_outflow: 2_000,
            stray: 1,
        };
        assert_eq!(b.liabilities().unwrap(), 255);
        assert_eq!(b.destination_exposure().unwrap(), 1 + 2 + 4 + 8);
    }

    #[test]
    fn each_lifecycle_step_moves_exactly_one_bucket_to_the_next() {
        let a = 7 * E8;
        let start = run(Buckets::default(), &minted(a)).unwrap();
        assert_eq!(start.outstanding, a as u128);

        let rc = start.apply(&Transition::ReturnCustodied { amount: a }).unwrap();
        assert_eq!((rc.outstanding, rc.return_custodied), (0, a as u128));

        let bu = rc.apply(&Transition::BurnSigned { amount: a }).unwrap();
        assert_eq!((bu.return_custodied, bu.burn_unknown), (0, a as u128));

        let pr = bu.apply(&Transition::BurnFinalized { amount: a }).unwrap();
        assert_eq!((pr.burn_unknown, pr.pending_release), (0, a as u128));

        // A proven non-landing returns the unit to custody, never to PR and never to O.
        let back = bu.apply(&Transition::BurnNotLanded { amount: a }).unwrap();
        assert_eq!((back.burn_unknown, back.return_custodied, back.pending_release, back.outstanding), (0, a as u128, 0, 0));

        // Liabilities never change across the lifecycle (no ICP moved).
        for b in [rc, bu, pr, back] {
            assert_eq!(b.liabilities().unwrap(), a as u128);
        }
    }

    #[test]
    fn lifecycle_steps_cannot_skip_or_over_consume() {
        let a = 10u64;
        let after_mint = run(Buckets::default(), &minted(a)).unwrap();
        // O -> BU / O -> PR / RC -> PR do not exist as transitions: every path goes through the next bucket.
        assert!(matches!(after_mint.apply(&Transition::BurnSigned { amount: a }), Err(AccountingError::Underflow { .. })));
        assert!(matches!(after_mint.apply(&Transition::BurnFinalized { amount: a }), Err(AccountingError::Underflow { .. })));
        assert!(matches!(after_mint.apply(&Transition::BurnNotLanded { amount: a }), Err(AccountingError::Underflow { .. })));
        // More than outstanding cannot be custodied.
        assert!(after_mint.apply(&Transition::ReturnCustodied { amount: a + 1 }).is_err());
        let rc = after_mint.apply(&Transition::ReturnCustodied { amount: a }).unwrap();
        assert!(rc.apply(&Transition::BurnFinalized { amount: a }).is_err(), "RC -> PR must go through BU");
        assert!(rc.apply(&Transition::BurnSigned { amount: a + 1 }).is_err());
    }

    #[test]
    fn exactly_one_mint_per_escrowed_deposit() {
        let a = 100 * E8;
        let b = run(Buckets::default(), &minted(a)).unwrap();
        assert_eq!(b.outstanding, a as u128);
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
        let b = run(Buckets::default(), &round_trip(a)).unwrap();
        assert!(b.apply(&Transition::ReleaseCompleted { amount: a, release_fee: 0 }).is_err());
        // A return larger than outstanding supply cannot be custodied.
        assert!(b.apply(&Transition::ReturnCustodied { amount: 1 }).is_err());
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
            Transition::ReturnCustodied { amount: m },
            Transition::BurnSigned { amount: m },
            Transition::BurnFinalized { amount: m },
            Transition::ReleaseCompleted { amount: m, release_fee: rel_fee },
        ]);
        assert_eq!(b.outstanding, 0);
        assert_eq!(b.fees, (dep_fee + rel_fee) as u128);
        assert_eq!(escrow as u128, b.fees);
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
        ts.extend([
            Transition::ReturnCustodied { amount: a },
            Transition::BurnSigned { amount: a },
            Transition::BurnFinalized { amount: a },
            Transition::ReleaseHeld { amount: a },
        ]);
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
    fn coverage_counts_custodied_and_burn_unknown_as_liabilities() {
        let b = Buckets { outstanding: 100, return_custodied: 10, burn_unknown: 5, pending_mint: 5, fees: 1, ..Default::default() };
        assert_eq!(b.liabilities().unwrap(), 121);
        let r = check_coverage(&b, 120).unwrap();
        assert!(!r.ok);
        assert_eq!(r.deficit, Some(1));
        let r = check_coverage(&b, 121).unwrap();
        assert!(r.ok);
        assert_eq!(r.surplus, Some(0));
        assert_eq!(check_coverage(&b, 200).unwrap().surplus, Some(79));
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
        // The largest single unit survives the whole custodied lifecycle.
        let c = run(b, &[Transition::ReturnCustodied { amount: max - 1 }, Transition::BurnSigned { amount: max - 1 }, Transition::BurnFinalized { amount: max - 1 }]).unwrap();
        assert_eq!(c.pending_release, (max - 1) as u128);
    }

    #[test]
    fn overflow_is_rejected_not_wrapped() {
        let b = Buckets { outstanding: u128::MAX, ..Default::default() };
        let b2 = Buckets { pending_mint: 1, ..b };
        assert!(matches!(b2.apply(&Transition::MintDelivered { amount: 1, fee: 0 }), Err(AccountingError::Overflow { .. })));
        assert!(matches!(b2.liabilities(), Err(AccountingError::Overflow { .. })));
        assert!(matches!(b2.destination_exposure(), Err(AccountingError::Overflow { .. })));
        let d = Buckets { burn_unknown: 1, return_custodied: u128::MAX, ..Default::default() };
        assert!(matches!(d.apply(&Transition::BurnNotLanded { amount: 1 }), Err(AccountingError::Overflow { .. })));
    }

    #[test]
    fn apply_is_atomic_on_error() {
        let b = Buckets { reserved_pulls: 5, ..Default::default() };
        let before = b;
        assert!(b.apply(&Transition::PullSucceeded { amount: 6 }).is_err());
        assert_eq!(b, before);
    }

    // ---- stray ----

    #[test]
    fn stray_is_a_subset_of_outstanding_and_not_a_liability() {
        let a = 10u64;
        let b = run(Buckets::default(), &minted(a)).unwrap();
        let s = b.apply(&Transition::StrayObserved { amount: 4 }).unwrap();
        assert_eq!(s.stray, 4);
        assert_eq!(s.outstanding, 10, "a stray stays in O");
        assert_eq!(s.liabilities().unwrap(), b.liabilities().unwrap(), "no new liability");
        assert_eq!(Buckets::escrow_delta(&Transition::StrayObserved { amount: 4 }), 0);
        // It cannot exceed O.
        assert!(matches!(s.apply(&Transition::StrayObserved { amount: 7 }), Err(AccountingError::InvalidAmounts(_))));
        // A valid return can only be custodied out of the part of O that is not stray.
        assert!(s.apply(&Transition::ReturnCustodied { amount: 7 }).is_err());
        let rc = s.apply(&Transition::ReturnCustodied { amount: 6 }).unwrap();
        assert_eq!((rc.outstanding, rc.stray, rc.return_custodied), (4, 4, 6));
        // Refunding the stray clears it and only it.
        let refunded = rc.apply(&Transition::StrayRefunded { amount: 4 }).unwrap();
        assert_eq!(refunded.stray, 0);
        assert_eq!(refunded.outstanding, 4);
        assert!(matches!(refunded.apply(&Transition::StrayRefunded { amount: 1 }), Err(AccountingError::Underflow { .. })));
    }

    #[test]
    fn stray_never_reaches_pending_release() {
        // A stray is never burned or released: no transition moves stray into RC/BU/PR.
        let a = 10u64;
        let b = run(Buckets::default(), &minted(a)).unwrap().apply(&Transition::StrayObserved { amount: a }).unwrap();
        // All of O is stray: nothing is left to custody, so no burn/release path opens.
        assert!(b.apply(&Transition::ReturnCustodied { amount: 1 }).is_err());
        assert_eq!(b.pending_release, 0);
    }

    // ---- custody audit ----

    #[test]
    fn custody_audit_is_rc_plus_bu_plus_stray() {
        let b = Buckets { outstanding: 50, return_custodied: 10, burn_unknown: 5, stray: 3, ..Default::default() };
        let ok = check_custody(&b, 18).unwrap();
        assert!(ok.ok && ok.excess.is_none() && ok.deficit.is_none() && !ok.halt_credits);
        assert_eq!(ok.expected, 18);

        let over = check_custody(&b, 19).unwrap();
        assert_eq!(over.excess, Some(1));
        assert!(over.halt_credits && !over.ok, "an unexplained excess halts credits");

        let under = check_custody(&b, 15).unwrap();
        assert_eq!(under.deficit, Some(3));
        assert!(!under.ok && !under.halt_credits);
        assert!(under.deficit_within_burn_unknown, "a landed-but-unreconciled burn (<= BU) explains it");
        let way_under = check_custody(&b, 10).unwrap();
        assert_eq!(way_under.deficit, Some(8));
        assert!(!way_under.deficit_within_burn_unknown);
    }

    // ---- supply invariants ----

    #[test]
    fn supply_bound_includes_custodied_and_burn_unknown() {
        let b = Buckets { outstanding: 100, return_custodied: 10, burn_unknown: 5, pending_mint: 50, ..Default::default() };
        // O + M_inflight + RC + BU = 100 + 20 + 10 + 5
        assert!(check_destination_supply(&b, 135, 20, false, None).unwrap().within_bound);
        assert!(!check_destination_supply(&b, 136, 20, false, None).unwrap().within_bound);
        assert_eq!(check_destination_supply(&b, 135, 20, false, None).unwrap().bound, 135);
        // In-flight mints cannot exceed PM.
        assert!(check_destination_supply(&b, 0, 51, false, None).is_err());
    }

    #[test]
    fn quiescent_requires_no_inflight_and_no_custody() {
        let idle = Buckets { outstanding: 100, ..Default::default() };
        assert!(check_destination_supply(&idle, 100, 0, true, None).unwrap().quiescent);
        assert!(!check_destination_supply(&idle, 100, 0, false, None).unwrap().quiescent, "observer not caught up");
        let with_rc = Buckets { outstanding: 90, return_custodied: 10, ..Default::default() };
        assert!(!check_destination_supply(&with_rc, 100, 0, true, None).unwrap().quiescent);
        let with_bu = Buckets { outstanding: 90, burn_unknown: 10, ..Default::default() };
        assert!(!check_destination_supply(&with_bu, 100, 0, true, None).unwrap().quiescent);
        let with_pm = Buckets { outstanding: 90, pending_mint: 10, ..Default::default() };
        assert!(!check_destination_supply(&with_pm, 90, 10, true, None).unwrap().quiescent, "a mint is in flight");
    }

    #[test]
    fn c3_exact_match_uses_unattributed_burn_when_known() {
        let b = Buckets { outstanding: 100, ..Default::default() };
        assert_eq!(check_destination_supply(&b, 100, 0, true, Some(0)).unwrap().exact_match, Some(true));
        assert_eq!(check_destination_supply(&b, 90, 0, true, Some(10)).unwrap().exact_match, Some(true));
        assert_eq!(check_destination_supply(&b, 90, 0, true, Some(9)).unwrap().exact_match, Some(false));
        // Not quiescent: no C3.
        assert_eq!(check_destination_supply(&b, 100, 0, false, Some(0)).unwrap().exact_match, None);
    }

    /// Note section 12, test (b): a holder `BurnChecked` without a return memo lowers supply,
    /// leaves `O` unchanged, credits nothing, halts nothing and is reported as unattributed.
    #[test]
    fn holder_burn_is_reported_not_enforced() {
        let b = run(Buckets::default(), &minted(100)).unwrap();
        let before = b;
        // Supply fell from 100 to 90 outside our transactions; no burn observer exists (UB = None).
        let r = check_destination_supply(&b, 90, 0, true, None).unwrap();
        assert!(r.within_bound, "a legitimate supply decrease never halts");
        assert_eq!(r.exact_match, None, "C3 is not evaluated without a burn observer");
        assert_eq!(r.unattributed_gap, Some(10), "the gap is reported as unattributed");
        // Nothing in the accounting moved: no credit, no release, no sweep.
        assert_eq!(b, before);
        assert_eq!((b.outstanding, b.pending_release, b.fees), (100, 0, 0));
        // Excess supply (the real alarm) still trips C2.
        assert!(!check_destination_supply(&b, 101, 0, true, None).unwrap().within_bound);
    }

    #[test]
    fn unattributed_burn_blocker_id_is_the_documented_one() {
        assert_eq!(UNATTRIBUTED_BURN_BLOCKER, "solana-unattributed-burn-policy");
    }

    // ---- lifecycle property: no ICP release while a unit is in RC or BU ----

    #[test]
    fn lifecycle_property_release_needs_a_burn_final_unit() {
        // With every unit in RC or BU, nothing is payable, whatever the amount.
        for (rc, bu) in [(5u128, 0u128), (0, 5), (3, 4)] {
            let b = Buckets { return_custodied: rc, burn_unknown: bu, ..Default::default() };
            for amount in [1u64, 2, 3, 4, 5, 7, 100] {
                assert!(
                    matches!(b.apply(&Transition::ReleaseCompleted { amount, release_fee: 0 }), Err(AccountingError::Underflow { .. })),
                    "release of {amount} must fail with rc={rc} bu={bu}"
                );
                assert!(b.apply(&Transition::ReleaseHeld { amount }).is_err());
                assert!(b.apply(&Transition::SweepCompleted { amount }).is_err(), "fees are not backed by RC/BU either");
            }
        }
        // The same units become payable only after BurnFinalized.
        let b = Buckets { return_custodied: 0, burn_unknown: 5, ..Default::default() };
        let pr = b.apply(&Transition::BurnFinalized { amount: 5 }).unwrap();
        assert!(pr.apply(&Transition::ReleaseCompleted { amount: 5, release_fee: 0 }).is_ok());
        // And a partial BurnFinalized pays out only that part.
        let two = Buckets { burn_unknown: 5, return_custodied: 5, ..Default::default() }.apply(&Transition::BurnFinalized { amount: 2 }).unwrap();
        assert!(two.apply(&Transition::ReleaseCompleted { amount: 3, release_fee: 0 }).is_err());
        assert!(two.apply(&Transition::ReleaseCompleted { amount: 2, release_fee: 0 }).is_ok());
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
                Transition::ReturnCustodied { amount: a },
                Transition::BurnSigned { amount: a },
                Transition::BurnFinalized { amount: a },
                Transition::BurnNotLanded { amount: a },
                Transition::ReleaseHeld { amount: a },
                Transition::ReleaseCompleted { amount: a, release_fee: 0 },
                Transition::ReleaseCompleted { amount: a, release_fee: 1 },
                Transition::RemintStarted { amount: a },
                Transition::RemintDelivered { amount: a },
                Transition::SweepCompleted { amount: a },
                Transition::OutflowUnknown { amount: a },
                Transition::OutflowResolved { amount: a },
                Transition::StrayObserved { amount: a },
                Transition::StrayRefunded { amount: a },
            ]);
        }
        v
    }

    /// Exhaustive breadth-first exploration of every state reachable within `depth`
    /// transitions. For every (state, transition) pair that applies:
    /// * `pending_release` can grow only through `BurnFinalized`, and then exactly by the
    ///   amount `burn_unknown` shrank (a unit in `RC` or `BU` cannot jump to payable);
    /// * a release (`ReleaseCompleted`/`ReleaseHeld`) never exceeds `pending_release`;
    /// * the return-lifecycle transitions never move escrow;
    /// * C1 holds against the escrow implied by the transition deltas.
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
                            Transition::BurnFinalized { amount } => {
                                assert_eq!(nb.pending_release - b.pending_release, *amount as u128);
                                assert_eq!(b.burn_unknown - nb.burn_unknown, *amount as u128);
                            }
                            other => panic!("PR grew through {other:?} from {b:?}"),
                        }
                    }
                    if let Transition::ReleaseCompleted { amount, .. } | Transition::ReleaseHeld { amount } = t {
                        assert!(*amount as u128 <= b.pending_release, "{t:?} paid more than PR in {b:?}");
                    }
                    if matches!(
                        t,
                        Transition::ReturnCustodied { .. }
                            | Transition::BurnSigned { .. }
                            | Transition::BurnFinalized { .. }
                            | Transition::BurnNotLanded { .. }
                            | Transition::StrayObserved { .. }
                            | Transition::StrayRefunded { .. }
                    ) {
                        assert_eq!(delta, 0, "{t:?} must not move ICP");
                    }
                    assert!(ne >= 0, "escrow went negative after {t:?}");
                    let cov = check_coverage(&nb, ne as u128).unwrap();
                    assert!(cov.ok, "C1 broken after {t:?}: {cov:?}");
                    assert!(nb.stray <= nb.outstanding);

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
        // burn-final, and PR equals burn-final minus released minus held-from-PR at all times.
        let mut seed: u64 = 0x5eed_1234;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut b = Buckets::default();
        let mut escrow: i128 = 0;
        let (mut burn_final, mut released, mut held_from_pr) = (0u128, 0u128, 0u128);
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
                Transition::ReturnCustodied { amount: amt },
                Transition::ReturnCustodied { amount: amt },
                Transition::BurnSigned { amount: amt },
                Transition::BurnSigned { amount: amt },
                Transition::BurnFinalized { amount: amt },
                Transition::BurnNotLanded { amount: amt },
                Transition::ReleaseHeld { amount: amt },
                Transition::ReleaseCompleted { amount: amt, release_fee: fee },
                Transition::RemintStarted { amount: amt },
                Transition::RemintDelivered { amount: amt },
                Transition::SweepCompleted { amount: amt },
                Transition::StrayObserved { amount: amt },
                Transition::StrayRefunded { amount: amt },
            ];
            let t = &candidates[(next() % candidates.len() as u64) as usize];
            if let Ok(nb) = b.apply(t) {
                match t {
                    Transition::BurnFinalized { amount } => burn_final += *amount as u128,
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
                assert_eq!(b.pending_release, burn_final - released - held_from_pr, "PR is fed only by BurnFinalized ({t:?})");
                assert!(released <= burn_final);
                assert!(b.stray <= b.outstanding);
            }
        }
        assert!(burn_final > 0 && released > 0, "the walk exercised the lifecycle: {burn_final} {released}");
    }
}
