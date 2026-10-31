//! Read-only aggregate icUSD reporting (design section 2.4, card BB-58).
//!
//! A pure function from MEASURED inputs to the escrow-adjusted circulating
//! figures. It makes no network call, reads no clock and holds no state. The
//! module [`crate::collect`] is the executable consumer that gathers the
//! readings through a [`crate::collect::Readings`] source.
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
//! # Index sets and the typed boundary
//!
//! * `D` is the set of destinations flagged `in_d`: bridge-instance networks
//!   whose escrow account is on the production icUSD ledger. A test-ledger
//!   instance is passed with `in_d = false` and contributes nothing.
//! * `X` is the optional XRPL lane (one escrow account, one issued amount). It
//!   is never a member of `D`.
//!
//! A string flag is not enough to keep `D` and `X` apart, so the boundary is
//! checked, not documented: every destination and the lane carry their escrow
//! binding ([`EscrowBinding`]); every network id must exist in the embedded
//! [`Catalog`]; a destination must not be an XRPL-family network and the lane
//! must be one; and no network id and no escrow account (after normalizing the
//! principal text and the zero subaccount) may appear twice across `D` and `X`.
//! [`check_boundary`] runs those rules and [`aggregate`] runs it first.
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

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::Deserialize;

use crate::escrow::{AccountIdentity, EscrowBinding};
use crate::types::{Family, Network, Profile};

// ---------------------------------------------------------------------------
// Catalog
// ---------------------------------------------------------------------------

/// The networks the aggregate may name, taken from the destination profiles.
#[derive(Debug, Clone)]
pub struct Catalog {
    profiles: Vec<Profile>,
}

impl Catalog {
    /// Build a catalog. A network id that appears twice is an error.
    pub fn new(profiles: Vec<Profile>) -> Result<Catalog, String> {
        let mut seen = BTreeSet::new();
        for p in &profiles {
            for n in &p.networks {
                if !seen.insert(n.network_id.as_str()) {
                    return Err(format!("duplicate catalog network id {}", n.network_id));
                }
            }
        }
        Ok(Catalog { profiles })
    }

    /// The nine embedded profiles.
    pub fn embedded() -> Result<Catalog, String> {
        Catalog::new(crate::load_embedded()?)
    }

    /// The profile and network record for an id.
    pub fn lookup(&self, network_id: &str) -> Option<(&Profile, &Network)> {
        self.profiles.iter().find_map(|p| {
            p.networks
                .iter()
                .find(|n| n.network_id == network_id)
                .map(|n| (p, n))
        })
    }
}

// ---------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------

/// Which network, whether it counts, and where its escrow sits. This is the
/// part of a destination known before any reading is taken.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DestinationConfig {
    /// Network id as in the profile catalog (for example `base-mainnet`).
    pub network_id: String,
    /// Member of `D`: the escrow account is on the production icUSD ledger.
    /// `false` for a test-ledger instance, which is reported but never counted.
    pub in_d: bool,
    /// Required when `in_d`. Optional for an excluded instance, whose escrow
    /// is on another ledger.
    #[serde(default)]
    pub escrow: Option<EscrowBinding>,
}

/// The XRPL lane (`X`): a network id (an XRPL-family network) and its escrow.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XrplLaneConfig {
    pub network_id: String,
    pub escrow: EscrowBinding,
}

/// One bridge-instance destination, as measured by the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DestinationReading {
    pub config: DestinationConfig,
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

/// The XRPL lane, as measured. A payment to the issuer redeems, so it has no
/// custody term.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XrplLaneReading {
    pub config: XrplLaneConfig,
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
    /// A network id is empty.
    EmptyNetworkId,
    /// The same network id appears more than once (in `D`, excluded, or `X`).
    DuplicateNetworkId(String),
    /// A network id is not in the catalog.
    UnknownNetworkId(String),
    /// A destination is an XRPL-family network. XRPL is the lane `X`, never a
    /// member of `D`.
    DestinationIsXrpl(String),
    /// The XRPL lane names a network that is not XRPL family.
    LaneIsNotXrpl(String),
    /// A destination in `D` has no escrow binding.
    MissingEscrowBinding(String),
    /// An escrow binding is not a usable ICRC-1 account.
    InvalidEscrowBinding { network_id: String, reason: String },
    /// Two entries of `D` union `X` name the same escrow account.
    DuplicateEscrowBinding { first: String, second: String },
    /// A checked addition overflowed `u128`. The text names the term.
    Overflow(String),
    /// A checked subtraction would go below zero. The text names the term.
    Underflow(String),
}

impl fmt::Display for AggregateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AggregateError::EmptyNetworkId => write!(f, "empty network id"),
            AggregateError::DuplicateNetworkId(id) => write!(f, "duplicate network id {id}"),
            AggregateError::UnknownNetworkId(id) => {
                write!(f, "network id {id} is not in the profile catalog")
            }
            AggregateError::DestinationIsXrpl(id) => write!(
                f,
                "destination {id} is an XRPL network: XRPL is the lane X and never a member of D"
            ),
            AggregateError::LaneIsNotXrpl(id) => {
                write!(f, "the XRPL lane names {id}, which is not an XRPL network")
            }
            AggregateError::MissingEscrowBinding(id) => {
                write!(f, "destination {id} is in D but has no escrow binding")
            }
            AggregateError::InvalidEscrowBinding { network_id, reason } => {
                write!(f, "escrow binding of {network_id}: {reason}")
            }
            AggregateError::DuplicateEscrowBinding { first, second } => write!(
                f,
                "{first} and {second} name the same escrow account; it would be counted twice"
            ),
            AggregateError::Overflow(what) => write!(f, "overflow computing {what}"),
            AggregateError::Underflow(what) => write!(f, "underflow computing {what}"),
        }
    }
}

impl std::error::Error for AggregateError {}

/// The typed boundary between `D` and `X`, checked before any arithmetic and,
/// in the collector, before any reading.
///
/// * every network id is non-empty, unique across `D`, excluded entries and
///   `X`, and present in the catalog;
/// * no destination is an XRPL-family network; the lane is one;
/// * every entry counted in `D` has a usable escrow binding (an excluded entry
///   may omit it, but one it supplies must be well formed);
/// * no escrow account appears twice across the entries counted in `D` and
///   `X`. An excluded entry escrows on another ledger and is not compared.
pub fn check_boundary<'a>(
    destinations: impl IntoIterator<Item = &'a DestinationConfig>,
    xrpl: Option<&XrplLaneConfig>,
    catalog: &Catalog,
) -> Result<(), AggregateError> {
    let mut ids: BTreeSet<&str> = BTreeSet::new();
    let mut owners: BTreeMap<AccountIdentity, &str> = BTreeMap::new();

    let bind = |network_id: &str, binding: &EscrowBinding| {
        binding
            .identity()
            .map_err(|reason| AggregateError::InvalidEscrowBinding {
                network_id: network_id.to_string(),
                reason,
            })
    };

    for d in destinations {
        if d.network_id.is_empty() {
            return Err(AggregateError::EmptyNetworkId);
        }
        if !ids.insert(d.network_id.as_str()) {
            return Err(AggregateError::DuplicateNetworkId(d.network_id.clone()));
        }
        let Some((profile, _)) = catalog.lookup(&d.network_id) else {
            return Err(AggregateError::UnknownNetworkId(d.network_id.clone()));
        };
        if profile.family == Family::Xrpl {
            return Err(AggregateError::DestinationIsXrpl(d.network_id.clone()));
        }
        match (&d.escrow, d.in_d) {
            (None, true) => {
                return Err(AggregateError::MissingEscrowBinding(d.network_id.clone()));
            }
            (None, false) => {}
            (Some(binding), in_d) => {
                let identity = bind(&d.network_id, binding)?;
                if in_d {
                    if let Some(first) = owners.insert(identity, d.network_id.as_str()) {
                        return Err(AggregateError::DuplicateEscrowBinding {
                            first: first.to_string(),
                            second: d.network_id.clone(),
                        });
                    }
                }
            }
        }
    }

    if let Some(x) = xrpl {
        if x.network_id.is_empty() {
            return Err(AggregateError::EmptyNetworkId);
        }
        if !ids.insert(x.network_id.as_str()) {
            return Err(AggregateError::DuplicateNetworkId(x.network_id.clone()));
        }
        let Some((profile, _)) = catalog.lookup(&x.network_id) else {
            return Err(AggregateError::UnknownNetworkId(x.network_id.clone()));
        };
        if profile.family != Family::Xrpl {
            return Err(AggregateError::LaneIsNotXrpl(x.network_id.clone()));
        }
        let identity = bind(&x.network_id, &x.escrow)?;
        if let Some(first) = owners.insert(identity, x.network_id.as_str()) {
            return Err(AggregateError::DuplicateEscrowBinding {
                first: first.to_string(),
                second: x.network_id.clone(),
            });
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

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

/// Compute the aggregate. See the module documentation for the formulas, the
/// boundary rules and the requirement that inputs are fresh independent reads.
pub fn aggregate(
    input: &AggregateInput,
    catalog: &Catalog,
) -> Result<AggregateReport, AggregateError> {
    // Identity first: an ambiguous id or a shared escrow would double count.
    check_boundary(
        input.destinations.iter().map(|d| &d.config),
        input.xrpl.as_ref().map(|x| &x.config),
        catalog,
    )?;

    let mut flags = Vec::new();
    let mut escrow_locked: u128 = 0;
    let mut circulating_dest = Vec::new();
    let mut dest_circulation_sum: u128 = 0;

    for d in &input.destinations {
        let id = &d.config.network_id;
        if !d.config.in_d {
            flags.push(Flag {
                network_id: id.clone(),
                kind: FlagKind::ExcludedFromD,
            });
            continue;
        }
        escrow_locked = add(
            escrow_locked,
            d.escrow_balance,
            &format!("escrow_locked at {id}"),
        )?;

        let explained = add(
            add(d.rc, d.bu, &format!("rc + bu at {id}"))?,
            d.stray,
            &format!("rc + bu + stray at {id}"),
        )?;
        if d.custody > explained {
            flags.push(Flag {
                network_id: id.clone(),
                kind: FlagKind::CustodyUnexplainedExcess {
                    custody: d.custody,
                    explained,
                },
            });
        } else if d.custody < explained {
            flags.push(Flag {
                network_id: id.clone(),
                kind: FlagKind::CustodyShortfall {
                    custody: d.custody,
                    explained,
                },
            });
        }

        let circulating = sub(
            d.supply,
            d.custody,
            &format!("circulating_dest at {id} (supply - custody)"),
        )?;
        dest_circulation_sum = add(
            dest_circulation_sum,
            circulating,
            &format!("sum of circulating_dest at {id}"),
        )?;
        circulating_dest.push(DestinationCirculation {
            network_id: id.clone(),
            circulating,
        });
    }

    let circulating_xrpl = match &input.xrpl {
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

    fn catalog() -> Catalog {
        Catalog::embedded().expect("embedded catalog")
    }

    /// A synthetic escrow for a network id: the principal bytes spell
    /// `syn-<id>`, so every id gets its own distinct, obviously fake account.
    fn binding_for(id: &str) -> EscrowBinding {
        EscrowBinding::from_principal_bytes(format!("syn-{id}").as_bytes(), None)
    }

    fn config(id: &str, in_d: bool) -> DestinationConfig {
        DestinationConfig {
            network_id: id.to_string(),
            in_d,
            escrow: Some(binding_for(id)),
        }
    }

    /// A destination whose return burns atomically (EVM, Sui gate, XRPL-like):
    /// no custody, supply equal to escrow.
    fn burn_on_return(id: &str, escrow: u128) -> DestinationReading {
        DestinationReading {
            config: config(id, true),
            escrow_balance: escrow,
            supply: escrow,
            custody: 0,
            rc: 0,
            bu: 0,
            stray: 0,
        }
    }

    fn lane(id: &str, escrow: u128, issued: u128) -> XrplLaneReading {
        XrplLaneReading {
            config: XrplLaneConfig {
                network_id: id.to_string(),
                escrow: binding_for(id),
            },
            escrow_balance: escrow,
            issued_outstanding: issued,
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

    fn run(inp: &AggregateInput) -> Result<AggregateReport, AggregateError> {
        aggregate(inp, &catalog())
    }

    #[test]
    fn base_only_golden() {
        let report = run(&input(
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
            config: config("solana-mainnet", true),
            escrow_balance: 150 * E8,
            supply: 150 * E8,
            custody: 40 * E8,
            rc: 25 * E8,
            bu: 10 * E8,
            stray: 5 * E8,
        };
        let report = run(&input(
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
        inp.xrpl = Some(lane("xrpl-mainnet", 120 * E8, 120 * E8));
        let report = run(&inp).unwrap();

        // E_X joins escrow_locked once; Base's escrow is a separate term.
        assert_eq!(report.escrow_locked, 220 * E8);
        assert_eq!(report.circulating_on_icp, 4_780 * E8);
        assert_eq!(report.circulating_dest.len(), 1);
        assert_eq!(report.circulating_xrpl, 120 * E8);
        assert_eq!(report.circulating_total, 5_000 * E8);
        assert_eq!(report.total_icusd, 5_000 * E8);

        // The lane alone (no bridge instance at all).
        let mut only_x = input(5_000 * E8, 0, vec![]);
        only_x.xrpl = Some(lane("xrpl-mainnet", 120 * E8, 120 * E8));
        let r = run(&only_x).unwrap();
        assert_eq!(r.escrow_locked, 120 * E8);
        assert_eq!(r.circulating_total, 5_000 * E8);
        assert!(r.circulating_dest.is_empty());
    }

    #[test]
    fn test_ledger_instance_is_excluded_and_untouched() {
        // A test instance escrows on a test ledger. Its numbers are not even
        // read: values that would overflow every sum prove that.
        let test_instance = DestinationReading {
            config: DestinationConfig {
                network_id: "base-sepolia".into(),
                in_d: false,
                escrow: None,
            },
            escrow_balance: u128::MAX,
            supply: u128::MAX,
            custody: 1,
            rc: 0,
            bu: 0,
            stray: 0,
        };
        let report = run(&input(
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
    fn excluded_instance_may_share_an_account_text_with_a_counted_one() {
        // Same principal text, different ledger: only counted entries compete.
        let mut test_instance = burn_on_return("base-sepolia", 5);
        test_instance.config.in_d = false;
        test_instance.config.escrow = Some(binding_for("base-mainnet"));
        run(&input(
            1_000 * E8,
            0,
            vec![burn_on_return("base-mainnet", 200 * E8), test_instance],
        ))
        .unwrap();
    }

    // --- the typed D/X boundary (Miles' counterexample and its siblings) ---

    #[test]
    fn xrpl_network_as_a_d_destination_plus_the_lane_is_rejected() {
        // total 1000; a D entry named xrpl-mainnet with escrow and supply 100,
        // plus the XRPL lane with escrow and outstanding 100. Both name one
        // escrow, so this used to report escrow_locked 200 and
        // circulating_on_icp 800 for a single 100 escrow.
        let mut inp = input(
            1_000 * E8,
            0,
            vec![burn_on_return("xrpl-mainnet", 100 * E8)],
        );
        inp.xrpl = Some(lane("xrpl-mainnet", 100 * E8, 100 * E8));
        assert_eq!(
            run(&inp).unwrap_err(),
            AggregateError::DestinationIsXrpl("xrpl-mainnet".into())
        );

        // A different lane network id does not rescue it: XRPL is never in D.
        inp.xrpl = Some(lane("xrpl-testnet", 100 * E8, 100 * E8));
        assert_eq!(
            run(&inp).unwrap_err(),
            AggregateError::DestinationIsXrpl("xrpl-mainnet".into())
        );

        // Not even as an excluded entry.
        let mut excluded = burn_on_return("xrpl-testnet", 1);
        excluded.config.in_d = false;
        assert_eq!(
            run(&input(1_000 * E8, 0, vec![excluded])).unwrap_err(),
            AggregateError::DestinationIsXrpl("xrpl-testnet".into())
        );
    }

    #[test]
    fn the_lane_must_be_an_xrpl_network() {
        let mut inp = input(1_000 * E8, 0, vec![]);
        inp.xrpl = Some(lane("base-mainnet", 100 * E8, 100 * E8));
        assert_eq!(
            run(&inp).unwrap_err(),
            AggregateError::LaneIsNotXrpl("base-mainnet".into())
        );
    }

    #[test]
    fn one_escrow_bound_to_two_d_entries_is_rejected() {
        let mut second = burn_on_return("ethereum-mainnet", 100 * E8);
        second.config.escrow = Some(binding_for("base-mainnet"));
        let err = run(&input(
            1_000 * E8,
            0,
            vec![burn_on_return("base-mainnet", 100 * E8), second],
        ))
        .unwrap_err();
        assert_eq!(
            err,
            AggregateError::DuplicateEscrowBinding {
                first: "base-mainnet".into(),
                second: "ethereum-mainnet".into(),
            }
        );
    }

    #[test]
    fn the_same_account_spelled_two_ways_is_still_a_duplicate() {
        // Missing subaccount versus the all-zero subaccount.
        let mut second = burn_on_return("ethereum-mainnet", 100 * E8);
        second.config.escrow = Some(EscrowBinding {
            subaccount: Some([0; 32]),
            ..binding_for("base-mainnet")
        });
        assert!(matches!(
            run(&input(
                1_000 * E8,
                0,
                vec![burn_on_return("base-mainnet", 100 * E8), second]
            ))
            .unwrap_err(),
            AggregateError::DuplicateEscrowBinding { .. }
        ));

        // A different subaccount of the same owner is a different account.
        let mut sub = [0u8; 32];
        sub[0] = 1;
        let mut third = burn_on_return("arbitrum-mainnet", 100 * E8);
        third.config.escrow = Some(EscrowBinding {
            subaccount: Some(sub),
            ..binding_for("base-mainnet")
        });
        run(&input(
            1_000 * E8,
            0,
            vec![burn_on_return("base-mainnet", 100 * E8), third],
        ))
        .unwrap();
    }

    #[test]
    fn the_same_escrow_on_a_d_entry_and_the_lane_is_rejected() {
        let mut inp = input(
            1_000 * E8,
            0,
            vec![burn_on_return("base-mainnet", 100 * E8)],
        );
        let mut x = lane("xrpl-mainnet", 100 * E8, 100 * E8);
        x.config.escrow = binding_for("base-mainnet");
        inp.xrpl = Some(x);
        assert_eq!(
            run(&inp).unwrap_err(),
            AggregateError::DuplicateEscrowBinding {
                first: "base-mainnet".into(),
                second: "xrpl-mainnet".into(),
            }
        );
    }

    #[test]
    fn unknown_network_ids_are_rejected() {
        assert_eq!(
            run(&input(
                1_000 * E8,
                0,
                vec![burn_on_return("narnia-mainnet", 1)]
            ))
            .unwrap_err(),
            AggregateError::UnknownNetworkId("narnia-mainnet".into())
        );
        let mut inp = input(1_000 * E8, 0, vec![]);
        inp.xrpl = Some(lane("xrpl-narnia", 1, 1));
        assert_eq!(
            run(&inp).unwrap_err(),
            AggregateError::UnknownNetworkId("xrpl-narnia".into())
        );
    }

    #[test]
    fn a_counted_destination_needs_a_usable_escrow_binding() {
        let mut missing = burn_on_return("base-mainnet", 1);
        missing.config.escrow = None;
        assert_eq!(
            run(&input(1_000 * E8, 0, vec![missing])).unwrap_err(),
            AggregateError::MissingEscrowBinding("base-mainnet".into())
        );

        for owner in ["2vxsx-fae", "aaaaa-aa", "not-a-principal", "AAAAA-AA"] {
            let mut bad = burn_on_return("base-mainnet", 1);
            bad.config.escrow = Some(EscrowBinding {
                owner: owner.into(),
                subaccount: None,
            });
            assert!(
                matches!(
                    run(&input(1_000 * E8, 0, vec![bad])).unwrap_err(),
                    AggregateError::InvalidEscrowBinding { .. }
                ),
                "{owner}"
            );
        }
    }

    #[test]
    fn duplicate_network_id_is_refused() {
        let err = run(&input(
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
        shadow.config.in_d = false;
        let err = run(&input(
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
        let err = run(&input(1_000 * E8, 0, vec![burn_on_return("", 1)])).unwrap_err();
        assert_eq!(err, AggregateError::EmptyNetworkId);
    }

    // --- arithmetic ---

    #[test]
    fn custody_excess_is_a_failure_not_a_warning() {
        let mut solana = DestinationReading {
            config: config("solana-mainnet", true),
            escrow_balance: 150 * E8,
            supply: 150 * E8,
            custody: 50 * E8,
            rc: 25 * E8,
            bu: 10 * E8,
            stray: 5 * E8,
        };
        let report = run(&input(10_000 * E8, 0, vec![solana.clone()])).unwrap();

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
        let report = run(&input(10_000 * E8, 0, vec![solana])).unwrap();
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
        let err = run(&input(
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
        inp.xrpl = Some(lane("xrpl-mainnet", 60 * E8, 60 * E8));
        assert!(matches!(
            run(&inp).unwrap_err(),
            AggregateError::Underflow(_)
        ));

        // Custody larger than the destination supply.
        let bad = DestinationReading {
            config: config("solana-mainnet", true),
            escrow_balance: 10 * E8,
            supply: 10 * E8,
            custody: 11 * E8,
            rc: 11 * E8,
            bu: 0,
            stray: 0,
        };
        let err = run(&input(1_000 * E8, 0, vec![bad])).unwrap_err();
        assert!(
            matches!(&err, AggregateError::Underflow(m) if m.contains("solana-mainnet")),
            "got {err:?}"
        );
    }

    #[test]
    fn overflows_are_refused_never_wrapped() {
        // total_icusd.
        let err = run(&input(u128::MAX, 1, vec![])).unwrap_err();
        assert!(matches!(err, AggregateError::Overflow(_)));

        // Two escrow balances that sum past u128::MAX.
        let err = run(&input(
            u128::MAX,
            0,
            vec![
                burn_on_return("base-mainnet", u128::MAX),
                burn_on_return("ethereum-mainnet", 1),
            ],
        ))
        .unwrap_err();
        assert!(matches!(&err, AggregateError::Overflow(m) if m.contains("escrow_locked")));

        // rc + bu + stray.
        let big = DestinationReading {
            config: config("solana-mainnet", true),
            escrow_balance: 0,
            supply: 0,
            custody: 0,
            rc: u128::MAX,
            bu: 1,
            stray: 0,
        };
        let err = run(&input(1, 0, vec![big])).unwrap_err();
        assert!(matches!(&err, AggregateError::Overflow(m) if m.contains("rc + bu")));

        // circulating_total: on icp (all of u128::MAX) plus a debt-backed
        // supply that total_icusd alone would not reject.
        let ok = run(&input(u128::MAX, 0, vec![])).map(|r| r.circulating_total);
        assert_eq!(ok, Ok(u128::MAX));
        let mut inp = input(u128::MAX - 1, 0, vec![]);
        inp.xrpl = Some(lane("xrpl-mainnet", 0, 2));
        assert!(matches!(
            run(&inp).unwrap_err(),
            AggregateError::Overflow(_)
        ));
    }

    #[test]
    fn empty_input_reports_the_ledger_total() {
        let report = run(&input(1_000 * E8, 7 * E8, vec![])).unwrap();
        assert_eq!(report.total_icusd, 1_007 * E8);
        assert_eq!(report.escrow_locked, 0);
        assert_eq!(report.circulating_on_icp, 1_000 * E8);
        assert_eq!(report.circulating_total, 1_007 * E8);
        assert!(report.flags.is_empty());
    }
}
