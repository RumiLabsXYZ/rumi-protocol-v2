//! Read-only aggregate icUSD reporting (design section 2.4, card BB-58).
//!
//! A pure function from MEASURED inputs to the escrow-adjusted circulating
//! figures. It makes no network call, reads no clock and holds no state.
//!
//! **Inputs must be fresh reads of the ICP ledger and of each destination
//! chain, taken by the caller. They must never be an instance's self-report**
//! (for example the numbers a bridge canister returns from its own audit
//! query): the point of an aggregate is to check the instances, so feeding it
//! their own claims proves nothing. `escrow_balance` is the ledger balance of
//! the destination's escrow account, `supply` is the destination token supply
//! at its final head, and `custody` is the measured balance of the
//! destination's canister-owned return account at that same head.
//!
//! Index sets (no escrow account and no destination supply is counted twice):
//!
//! * `D` is the set of destinations flagged `in_d`: bridge-instance networks
//!   whose escrow account is on the production icUSD ledger. A test-ledger
//!   instance is passed with `in_d = false` and contributes nothing.
//! * `X` is the optional XRPL lane (one escrow account, one issued amount). It
//!   is a separate input and is never a member of `D`.
//!
//! ```text
//! total_icusd         = icp_ledger_total_supply + debt_backed_chain_supplies
//! escrow_locked       = sum(E_d, d in D) + E_X
//! circulating_on_icp  = icp_ledger_total_supply - escrow_locked
//! circulating_dest_d  = supply_d - custody_d             (d in D)
//! circulating_xrpl    = issued_outstanding_X
//! circulating_total   = circulating_on_icp + sum(circulating_dest_d)
//!                       + circulating_xrpl + debt_backed_chain_supplies
//! ```
//!
//! All amounts are e8s as `u128`. Arithmetic is checked: an overflow or an
//! underflow is an error, never a saturated or wrapped number. A custody
//! balance that differs from `rc + bu + stray` is reported as a FAILURE flag
//! (an unexplained excess must halt credits), not as a warning.

use std::collections::BTreeSet;
use std::fmt;

/// One bridge-instance destination, as measured by the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DestinationReading {
    /// Network id as in the profile (for example `base-mainnet`).
    pub network_id: String,
    /// Member of `D`: the escrow account is on the production icUSD ledger.
    /// `false` for a test-ledger instance, which is reported but never counted.
    pub in_d: bool,
    /// `E_d`: ledger balance of this destination's own escrow account.
    pub escrow_balance: u128,
    /// `supply_d`: destination token supply at the final head.
    pub supply: u128,
    /// Measured balance of the canister-owned return account at that same
    /// head. Zero for families whose return burns atomically.
    pub custody: u128,
    /// `RC_d`: finalized valid returns sitting in the return account.
    pub rc: u128,
    /// `BU_d`: custodied returns whose burn was signed but is not final.
    pub bu: u128,
    /// Strays held in the return account (never credited, awaiting refund).
    pub stray: u128,
}

/// The XRPL lane (`X`). A payment to the issuer redeems, so it has no custody.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XrplLaneReading {
    /// `E_X`: ledger balance of the XRPL lane's escrow account.
    pub escrow_balance: u128,
    /// Issued amount held off the issuer.
    pub issued_outstanding: u128,
}

/// Everything the aggregate needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AggregateInput {
    /// `icrc1_total_supply` of the production icUSD ledger.
    pub icp_ledger_total_supply: u128,
    /// Sum of the debt-backed chain supplies (already summed by the caller).
    pub debt_backed_chain_supplies: u128,
    pub destinations: Vec<DestinationReading>,
    pub xrpl: Option<XrplLaneReading>,
}

/// Why no report was produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AggregateError {
    /// A destination has an empty network id.
    EmptyNetworkId,
    /// The same network id appears more than once (in `D` or not).
    DuplicateNetworkId(String),
    /// A checked addition overflowed `u128`. The text names the term.
    Overflow(String),
    /// A checked subtraction would go below zero. The text names the term.
    Underflow(String),
}

impl fmt::Display for AggregateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AggregateError::EmptyNetworkId => write!(f, "destination with an empty network id"),
            AggregateError::DuplicateNetworkId(id) => {
                write!(f, "duplicate network id {id}")
            }
            AggregateError::Overflow(what) => write!(f, "overflow computing {what}"),
            AggregateError::Underflow(what) => write!(f, "underflow computing {what}"),
        }
    }
}

impl std::error::Error for AggregateError {}

/// How seriously a flag must be taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Worth showing; changes no number and halts nothing.
    Info,
    /// The destination's accounting does not close. Credits must halt.
    Failure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlagKind {
    /// The destination was passed with `in_d = false`; none of its numbers
    /// were used.
    ExcludedFromD,
    /// `custody > rc + bu + stray`: tokens sit in the return account that no
    /// open claim explains.
    CustodyUnexplainedExcess { custody: u128, explained: u128 },
    /// `custody < rc + bu + stray`: claimed custodied returns are missing.
    CustodyShortfall { custody: u128, explained: u128 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flag {
    pub network_id: String,
    pub kind: FlagKind,
}

impl Flag {
    pub fn severity(&self) -> Severity {
        match self.kind {
            FlagKind::ExcludedFromD => Severity::Info,
            FlagKind::CustodyUnexplainedExcess { .. } | FlagKind::CustodyShortfall { .. } => {
                Severity::Failure
            }
        }
    }
}

/// Circulating figure of one destination in `D`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DestinationCirculation {
    pub network_id: String,
    /// `supply_d - custody_d`.
    pub circulating: u128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AggregateReport {
    pub total_icusd: u128,
    pub escrow_locked: u128,
    pub circulating_on_icp: u128,
    /// One entry per destination in `D`, in input order.
    pub circulating_dest: Vec<DestinationCirculation>,
    /// Zero when no XRPL lane was supplied.
    pub circulating_xrpl: u128,
    pub circulating_total: u128,
    pub flags: Vec<Flag>,
}

impl AggregateReport {
    /// The flags that are failures (credits must halt while any exists).
    pub fn failures(&self) -> impl Iterator<Item = &Flag> {
        self.flags
            .iter()
            .filter(|f| f.severity() == Severity::Failure)
    }

    pub fn has_failure(&self) -> bool {
        self.failures().next().is_some()
    }
}

fn add(a: u128, b: u128, what: &str) -> Result<u128, AggregateError> {
    a.checked_add(b)
        .ok_or_else(|| AggregateError::Overflow(what.to_string()))
}

fn sub(a: u128, b: u128, what: &str) -> Result<u128, AggregateError> {
    a.checked_sub(b)
        .ok_or_else(|| AggregateError::Underflow(format!("{what} ({a} - {b})")))
}

/// Compute the aggregate. See the module documentation for the formulas and
/// for the requirement that inputs are fresh independent reads.
pub fn aggregate(input: &AggregateInput) -> Result<AggregateReport, AggregateError> {
    // Identity first: an ambiguous id could hide a double count.
    let mut seen = BTreeSet::new();
    for d in &input.destinations {
        if d.network_id.is_empty() {
            return Err(AggregateError::EmptyNetworkId);
        }
        if !seen.insert(d.network_id.as_str()) {
            return Err(AggregateError::DuplicateNetworkId(d.network_id.clone()));
        }
    }

    let mut flags = Vec::new();
    let mut escrow_locked: u128 = 0;
    let mut circulating_dest = Vec::new();
    let mut dest_circulation_sum: u128 = 0;

    for d in &input.destinations {
        if !d.in_d {
            flags.push(Flag {
                network_id: d.network_id.clone(),
                kind: FlagKind::ExcludedFromD,
            });
            continue;
        }
        escrow_locked = add(
            escrow_locked,
            d.escrow_balance,
            &format!("escrow_locked at {}", d.network_id),
        )?;

        let explained = add(
            add(d.rc, d.bu, &format!("rc + bu at {}", d.network_id))?,
            d.stray,
            &format!("rc + bu + stray at {}", d.network_id),
        )?;
        if d.custody > explained {
            flags.push(Flag {
                network_id: d.network_id.clone(),
                kind: FlagKind::CustodyUnexplainedExcess {
                    custody: d.custody,
                    explained,
                },
            });
        } else if d.custody < explained {
            flags.push(Flag {
                network_id: d.network_id.clone(),
                kind: FlagKind::CustodyShortfall {
                    custody: d.custody,
                    explained,
                },
            });
        }

        let circulating = sub(
            d.supply,
            d.custody,
            &format!("circulating_dest at {} (supply - custody)", d.network_id),
        )?;
        dest_circulation_sum = add(
            dest_circulation_sum,
            circulating,
            &format!("sum of circulating_dest at {}", d.network_id),
        )?;
        circulating_dest.push(DestinationCirculation {
            network_id: d.network_id.clone(),
            circulating,
        });
    }

    let circulating_xrpl = match input.xrpl {
        Some(x) => {
            escrow_locked = add(escrow_locked, x.escrow_balance, "escrow_locked with E_X")?;
            x.issued_outstanding
        }
        None => 0,
    };

    let total_icusd = add(
        input.icp_ledger_total_supply,
        input.debt_backed_chain_supplies,
        "total_icusd",
    )?;
    let circulating_on_icp = sub(
        input.icp_ledger_total_supply,
        escrow_locked,
        "circulating_on_icp (icp_ledger_total_supply - escrow_locked)",
    )?;
    let mut circulating_total = add(
        circulating_on_icp,
        dest_circulation_sum,
        "circulating_total (on icp + destinations)",
    )?;
    circulating_total = add(
        circulating_total,
        circulating_xrpl,
        "circulating_total (+ xrpl)",
    )?;
    circulating_total = add(
        circulating_total,
        input.debt_backed_chain_supplies,
        "circulating_total (+ debt-backed chains)",
    )?;

    Ok(AggregateReport {
        total_icusd,
        escrow_locked,
        circulating_on_icp,
        circulating_dest,
        circulating_xrpl,
        circulating_total,
        flags,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const E8: u128 = 100_000_000;

    /// A destination whose return burns atomically (EVM, Sui gate, XRPL-like):
    /// no custody, supply equal to escrow.
    fn burn_on_return(id: &str, escrow: u128) -> DestinationReading {
        DestinationReading {
            network_id: id.to_string(),
            in_d: true,
            escrow_balance: escrow,
            supply: escrow,
            custody: 0,
            rc: 0,
            bu: 0,
            stray: 0,
        }
    }

    fn input(
        icp: u128,
        debt_backed: u128,
        destinations: Vec<DestinationReading>,
    ) -> AggregateInput {
        AggregateInput {
            icp_ledger_total_supply: icp,
            debt_backed_chain_supplies: debt_backed,
            destinations,
            xrpl: None,
        }
    }

    #[test]
    fn base_only_golden() {
        let report = aggregate(&input(
            1_000 * E8,
            0,
            vec![burn_on_return("base-mainnet", 200 * E8 + 12_345_678)],
        ))
        .unwrap();

        assert_eq!(
            report,
            AggregateReport {
                total_icusd: 1_000 * E8,
                escrow_locked: 200 * E8 + 12_345_678,
                circulating_on_icp: 800 * E8 - 12_345_678,
                circulating_dest: vec![DestinationCirculation {
                    network_id: "base-mainnet".into(),
                    circulating: 200 * E8 + 12_345_678,
                }],
                circulating_xrpl: 0,
                // Fully backed and nothing in custody: every icUSD is
                // circulating exactly once.
                circulating_total: 1_000 * E8,
                flags: vec![],
            }
        );
        assert!(!report.has_failure());
    }

    #[test]
    fn base_and_solana_with_custody_and_stray_golden() {
        let solana = DestinationReading {
            network_id: "solana-mainnet".into(),
            in_d: true,
            escrow_balance: 150 * E8,
            supply: 150 * E8,
            custody: 40 * E8,
            rc: 25 * E8,
            bu: 10 * E8,
            stray: 5 * E8,
        };
        let report = aggregate(&input(
            10_000 * E8,
            500 * E8,
            vec![burn_on_return("base-mainnet", 300 * E8), solana],
        ))
        .unwrap();

        assert_eq!(report.total_icusd, 10_500 * E8);
        assert_eq!(report.escrow_locked, 450 * E8);
        assert_eq!(report.circulating_on_icp, 9_550 * E8);
        assert_eq!(
            report.circulating_dest,
            vec![
                DestinationCirculation {
                    network_id: "base-mainnet".into(),
                    circulating: 300 * E8,
                },
                DestinationCirculation {
                    network_id: "solana-mainnet".into(),
                    circulating: 110 * E8,
                },
            ]
        );
        assert_eq!(report.circulating_xrpl, 0);
        // 9550 + 300 + 110 + 0 + 500.
        assert_eq!(report.circulating_total, 10_460 * E8);
        // Custody is excluded exactly once: total minus the 40 held in custody.
        assert_eq!(report.circulating_total, report.total_icusd - 40 * E8);
        assert!(report.flags.is_empty(), "custody is fully explained");
        assert!(!report.has_failure());
    }

    #[test]
    fn xrpl_lane_is_counted_once_and_outside_d() {
        let mut inp = input(
            5_000 * E8,
            0,
            vec![burn_on_return("base-mainnet", 100 * E8)],
        );
        inp.xrpl = Some(XrplLaneReading {
            escrow_balance: 120 * E8,
            issued_outstanding: 120 * E8,
        });
        let report = aggregate(&inp).unwrap();

        // E_X joins escrow_locked once; Base's escrow is a separate term.
        assert_eq!(report.escrow_locked, 220 * E8);
        assert_eq!(report.circulating_on_icp, 4_780 * E8);
        assert_eq!(report.circulating_dest.len(), 1);
        assert_eq!(report.circulating_xrpl, 120 * E8);
        assert_eq!(report.circulating_total, 5_000 * E8);
        assert_eq!(report.total_icusd, 5_000 * E8);

        // The lane alone (no bridge instance at all).
        let mut only_x = input(5_000 * E8, 0, vec![]);
        only_x.xrpl = Some(XrplLaneReading {
            escrow_balance: 120 * E8,
            issued_outstanding: 120 * E8,
        });
        let r = aggregate(&only_x).unwrap();
        assert_eq!(r.escrow_locked, 120 * E8);
        assert_eq!(r.circulating_total, 5_000 * E8);
        assert!(r.circulating_dest.is_empty());
    }

    #[test]
    fn test_ledger_instance_is_excluded_and_untouched() {
        // A test instance escrows on a test ledger. Its numbers are not even
        // read: values that would overflow every sum prove that.
        let test_instance = DestinationReading {
            network_id: "base-sepolia".into(),
            in_d: false,
            escrow_balance: u128::MAX,
            supply: u128::MAX,
            custody: 1,
            rc: 0,
            bu: 0,
            stray: 0,
        };
        let report = aggregate(&input(
            1_000 * E8,
            0,
            vec![burn_on_return("base-mainnet", 200 * E8), test_instance],
        ))
        .unwrap();

        assert_eq!(report.escrow_locked, 200 * E8);
        assert_eq!(report.circulating_on_icp, 800 * E8);
        assert_eq!(report.circulating_dest.len(), 1);
        assert_eq!(report.circulating_dest[0].network_id, "base-mainnet");
        assert_eq!(report.circulating_total, 1_000 * E8);
        assert_eq!(
            report.flags,
            vec![Flag {
                network_id: "base-sepolia".into(),
                kind: FlagKind::ExcludedFromD,
            }]
        );
        assert_eq!(report.flags[0].severity(), Severity::Info);
        assert!(
            !report.has_failure(),
            "an excluded instance's custody mismatch must not halt production credits"
        );
    }

    #[test]
    fn duplicate_network_id_is_refused() {
        let err = aggregate(&input(
            1_000 * E8,
            0,
            vec![
                burn_on_return("base-mainnet", 100 * E8),
                burn_on_return("base-mainnet", 100 * E8),
            ],
        ))
        .unwrap_err();
        assert_eq!(
            err,
            AggregateError::DuplicateNetworkId("base-mainnet".into())
        );

        // The same id as a test instance and a production instance is still
        // ambiguous and refused.
        let mut shadow = burn_on_return("base-mainnet", 1);
        shadow.in_d = false;
        let err = aggregate(&input(
            1_000 * E8,
            0,
            vec![burn_on_return("base-mainnet", 100 * E8), shadow],
        ))
        .unwrap_err();
        assert_eq!(
            err,
            AggregateError::DuplicateNetworkId("base-mainnet".into())
        );
    }

    #[test]
    fn empty_network_id_is_refused() {
        let err = aggregate(&input(1_000 * E8, 0, vec![burn_on_return("", 1)])).unwrap_err();
        assert_eq!(err, AggregateError::EmptyNetworkId);
    }

    #[test]
    fn custody_excess_is_a_failure_not_a_warning() {
        let mut solana = DestinationReading {
            network_id: "solana-mainnet".into(),
            in_d: true,
            escrow_balance: 150 * E8,
            supply: 150 * E8,
            custody: 50 * E8,
            rc: 25 * E8,
            bu: 10 * E8,
            stray: 5 * E8,
        };
        let report = aggregate(&input(10_000 * E8, 0, vec![solana.clone()])).unwrap();

        assert_eq!(
            report.flags,
            vec![Flag {
                network_id: "solana-mainnet".into(),
                kind: FlagKind::CustodyUnexplainedExcess {
                    custody: 50 * E8,
                    explained: 40 * E8,
                },
            }]
        );
        assert_eq!(report.flags[0].severity(), Severity::Failure);
        assert!(report.has_failure());
        assert_eq!(report.failures().count(), 1);
        // The numbers are still reported, so an operator can see the gap.
        assert_eq!(report.circulating_dest[0].circulating, 100 * E8);

        // A shortfall (claimed custody missing) is a failure as well.
        solana.custody = 30 * E8;
        let report = aggregate(&input(10_000 * E8, 0, vec![solana])).unwrap();
        assert_eq!(
            report.flags[0].kind,
            FlagKind::CustodyShortfall {
                custody: 30 * E8,
                explained: 40 * E8,
            }
        );
        assert!(report.has_failure());
    }

    #[test]
    fn underflows_are_refused_never_saturated() {
        // Escrow exceeds the ledger total: the reads are inconsistent.
        let err = aggregate(&input(
            100 * E8,
            0,
            vec![burn_on_return("base-mainnet", 101 * E8)],
        ))
        .unwrap_err();
        assert!(
            matches!(&err, AggregateError::Underflow(m) if m.contains("circulating_on_icp")),
            "got {err:?}"
        );

        // XRPL escrow pushes the same sum over the ledger total.
        let mut inp = input(100 * E8, 0, vec![burn_on_return("base-mainnet", 60 * E8)]);
        inp.xrpl = Some(XrplLaneReading {
            escrow_balance: 60 * E8,
            issued_outstanding: 60 * E8,
        });
        assert!(matches!(
            aggregate(&inp).unwrap_err(),
            AggregateError::Underflow(_)
        ));

        // Custody larger than the destination supply.
        let bad = DestinationReading {
            network_id: "solana-mainnet".into(),
            in_d: true,
            escrow_balance: 10 * E8,
            supply: 10 * E8,
            custody: 11 * E8,
            rc: 11 * E8,
            bu: 0,
            stray: 0,
        };
        let err = aggregate(&input(1_000 * E8, 0, vec![bad])).unwrap_err();
        assert!(
            matches!(&err, AggregateError::Underflow(m) if m.contains("solana-mainnet")),
            "got {err:?}"
        );
    }

    #[test]
    fn overflows_are_refused_never_wrapped() {
        // total_icusd.
        let err = aggregate(&input(u128::MAX, 1, vec![])).unwrap_err();
        assert!(matches!(err, AggregateError::Overflow(_)));

        // Two escrow balances that sum past u128::MAX.
        let err = aggregate(&input(
            u128::MAX,
            0,
            vec![burn_on_return("a", u128::MAX), burn_on_return("b", 1)],
        ))
        .unwrap_err();
        assert!(matches!(&err, AggregateError::Overflow(m) if m.contains("escrow_locked")));

        // rc + bu + stray.
        let big = DestinationReading {
            network_id: "solana-mainnet".into(),
            in_d: true,
            escrow_balance: 0,
            supply: 0,
            custody: 0,
            rc: u128::MAX,
            bu: 1,
            stray: 0,
        };
        let err = aggregate(&input(1, 0, vec![big])).unwrap_err();
        assert!(matches!(&err, AggregateError::Overflow(m) if m.contains("rc + bu")));

        // circulating_total: on icp (all of u128::MAX) plus a debt-backed
        // supply that total_icusd alone would not reject.
        let ok = aggregate(&input(u128::MAX, 0, vec![])).map(|r| r.circulating_total);
        assert_eq!(ok, Ok(u128::MAX));
        let mut inp = input(u128::MAX - 1, 0, vec![]);
        inp.xrpl = Some(XrplLaneReading {
            escrow_balance: 0,
            issued_outstanding: 2,
        });
        assert!(matches!(
            aggregate(&inp).unwrap_err(),
            AggregateError::Overflow(_)
        ));
    }

    #[test]
    fn empty_input_reports_the_ledger_total() {
        let report = aggregate(&input(1_000 * E8, 7 * E8, vec![])).unwrap();
        assert_eq!(report.total_icusd, 1_007 * E8);
        assert_eq!(report.escrow_locked, 0);
        assert_eq!(report.circulating_on_icp, 1_000 * E8);
        assert_eq!(report.circulating_total, 1_007 * E8);
        assert!(report.flags.is_empty());
    }
}
