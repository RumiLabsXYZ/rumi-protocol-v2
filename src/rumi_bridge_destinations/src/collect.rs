//! Read-only collector for the aggregate icUSD report (card BB-58, design
//! section 2.4).
//!
//! [`collect`] gathers every number the aggregate uses through a [`Readings`]
//! source and hands them to [`crate::aggregate::aggregate`]. It is the
//! executable consumer that design 2.4 asks for: the escrow balance and the
//! destination supply are READ here, never taken from an instance's audit
//! query. `Readings` deliberately has no method that returns an instance's own
//! buckets or audit figures, so a self-report cannot be passed in by accident.
//!
//! The typed D/X boundary ([`crate::aggregate::check_boundary`]) runs before
//! the first reading, so a rejected configuration costs no query.
//!
//! # Instance audits
//!
//! For every counted EVM destination the collector also reads the bridge
//! instance's own `get_supply_audit` ([`Readings::instance_audit`]) and prints
//! its buckets NEXT TO the independent reads. The audit is never used as
//! `E_d` or `supply_d`. From the independent reads the collector recomputes
//!
//! * C1: `E_d >= liabilities_d - unknown_outflow_d`, and
//! * the supply bound: `supply_d <= outstanding_d + inflight_mint_d`,
//!
//! and either failing is a Failure flag. A difference between the instance's
//! self-reported escrow balance or supply and the independent read is an Info
//! flag naming both values, and an audit whose escrow account is not the
//! configured binding is a Failure (the coverage check would compare the wrong
//! account). Liabilities are summed across D in the report.
//!
//! # Adapters by family
//!
//! | Family | Source | Status |
//! | --- | --- | --- |
//! | EVM (Base and the other EVM rows) | token `totalSupply` at the profile's settlement head, custody zero (the return burns atomically) | wired through [`Readings::evm_total_supply`] |
//! | Solana | supply plus the measured return-account balance | `Unavailable`, delivered by card BB-49 |
//! | Sui | wrapped-currency supply | `Unavailable`, delivered by card BB-53 |
//! | XRPL lane | `E_X` (ICRC-1 balance of the lane escrow, the same IC transport) and the issuer's obligations for the icUSD currency at a VALIDATED ledger | wired through [`Readings::xrpl_issued_outstanding`] (card BB-57); a source without it fails with `Unavailable` |
//!
//! An unavailable source fails the whole collection with an explicit error
//! naming the card. No value is ever invented, defaulted or carried over.
//!
//! # The XRPL lane (`X`)
//!
//! `E_X` is read exactly like a destination escrow (ICRC-1 balance on the
//! production icUSD ledger) and the issued outstanding is read from the XRP
//! Ledger itself, never from the backend's own report: the issuer's
//! obligations for the icUSD currency, at a numbered validated ledger agreed by
//! a provider quorum. The lane is read BEFORE the ICP-side numbers, so a
//! source that has no XRPL reader fails before any other query is made. The
//! lane escrow is counted once, in `escrow_locked`, and the boundary check has
//! already refused a lane whose escrow or network id is also in `D`.
//!
//! # Sources
//!
//! * [`FixtureReadings`] replays numbers from a local JSON file. Its
//!   [`Readings::source`] is `fixture`, and [`render`] prints that as the first
//!   line, so a fixture run can never read as a live one.
//! * `LiveReadings` (module `live`, cargo feature `live-transport`, off by
//!   default) implements `Readings` over an IC query client (`ic-agent`,
//!   anonymous, response verification on) and an EVM JSON-RPC client (`reqwest`),
//!   with the bridge canister's head rule driven by the profile. Its
//!   `Readings::source` is `live`, and the report carries the read time and the
//!   head height of each EVM reading. It is verified here with mocked
//!   transports only and is never run against public networks under the current
//!   hold; the `collect_live` example needs an explicit endpoint config and an
//!   explicit `--live` argument. A default build of this crate makes no network
//!   call and pulls none of those dependencies.

use std::collections::BTreeMap;
use std::fmt;

use serde::Deserialize;

use crate::aggregate::{
    aggregate, check_boundary, AggregateError, AggregateInput, AggregateReport, Catalog,
    DestinationConfig, DestinationReading, Flag, FlagKind, Severity, XrplLaneConfig,
    XrplLaneReading,
};
use crate::escrow::{account_label, AccountIdentity, EscrowBinding};
use crate::types::{Family, FinalityRule, Network};

/// Card that delivers the Solana supply and custody source.
pub const SOLANA_SOURCE_CARD: &str = "BB-49";
/// Card that delivers the Sui supply source.
pub const SUI_SOURCE_CARD: &str = "BB-53";
/// Cards that deliver the XRPL issued-outstanding source.
pub const XRPL_SOURCE_CARDS: &str = "BB-56/BB-57";

const E8: u128 = 100_000_000;

// ---------------------------------------------------------------------------
// Readings
// ---------------------------------------------------------------------------

/// An EVM `totalSupply` reading and the head it was taken at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmSupply {
    /// Token `totalSupply`, in e8s.
    pub supply: u128,
    /// The finality rule of the head the read was pinned to. The collector
    /// refuses a reading whose rule differs from the profile's.
    pub finality: FinalityRule,
    /// Block number of that head.
    pub block: u64,
}

/// The XRPL lane's issued outstanding and the validated ledger it was read at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XrplIssued {
    /// The issuer's obligations for the icUSD currency, in e8s (exact: an
    /// amount beyond 8 decimals or 15 significant digits is a failed read).
    pub issued_outstanding: u128,
    /// The finality rule of the ledger the read was pinned to. The collector
    /// refuses a reading whose rule differs from the profile's.
    pub finality: FinalityRule,
    /// Index of that validated ledger.
    pub ledger_index: u64,
    /// Its hash (hex), agreed by the provider quorum.
    pub ledger_hash: String,
}

/// The parts of a bridge instance's `get_supply_audit` the collector uses, in
/// neutral names (the instance's record is Base-named; the live decoder maps
/// it). This is the instance's SELF-REPORT: it is displayed and checked against
/// the independent reads, never used as one.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceAudit {
    /// The escrow account the instance says it uses.
    pub escrow_account: EscrowBinding,
    /// The instance's last reconciled escrow balance, if any.
    #[serde(default, with = "opt_u128_string")]
    pub escrow_balance_e8s: Option<u128>,
    #[serde(with = "u128_string")]
    pub outstanding_e8s: u128,
    #[serde(with = "u128_string")]
    pub pending_mint_e8s: u128,
    #[serde(with = "u128_string")]
    pub inflight_mint_e8s: u128,
    #[serde(with = "u128_string")]
    pub pending_release_e8s: u128,
    #[serde(with = "u128_string")]
    pub pending_refund_e8s: u128,
    #[serde(with = "u128_string")]
    pub held_e8s: u128,
    #[serde(with = "u128_string")]
    pub fees_e8s: u128,
    #[serde(with = "u128_string")]
    pub reserved_pulls_e8s: u128,
    #[serde(with = "u128_string")]
    pub unknown_outflow_e8s: u128,
    #[serde(with = "u128_string")]
    pub liabilities_e8s: u128,
    #[serde(with = "u128_string")]
    pub required_escrow_e8s: u128,
    /// The instance's own coverage verdict.
    #[serde(default)]
    pub coverage_ok: Option<bool>,
    /// The destination supply the instance last recorded, and its block.
    #[serde(default, with = "opt_u128_string")]
    pub reported_supply_e8s: Option<u128>,
    #[serde(default)]
    pub reported_supply_block: Option<u64>,
}

/// Why a reading is missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadError {
    /// No adapter exists yet; `card` delivers it.
    Unavailable { card: &'static str },
    /// The source cannot be read for a named reason (a blocked head, a missing
    /// query, a missing config entry). Never a zero.
    UnavailableGap(String),
    /// The read failed (transport error, or a fixture without the entry).
    Failed(String),
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReadError::Unavailable { card } => {
                write!(f, "unavailable: the source is delivered by card {card}")
            }
            ReadError::UnavailableGap(gap) => write!(f, "unavailable: {gap}"),
            ReadError::Failed(why) => write!(f, "{why}"),
        }
    }
}

/// Every number the collector uses comes through this trait.
pub trait Readings {
    /// Label printed on every report: `fixture`, or `live` for a live source.
    fn source(&self) -> &'static str;
    /// `icrc1_total_supply` of the production icUSD ledger.
    fn icp_ledger_total_supply(&self) -> Result<u128, ReadError>;
    /// `icrc1_balance_of` of an escrow account on the production icUSD ledger.
    fn icrc1_balance_of(&self, binding: &EscrowBinding) -> Result<u128, ReadError>;
    /// Sum of the debt-backed chain supplies (backend chain supplies).
    fn debt_backed_chain_supplies(&self) -> Result<u128, ReadError>;
    /// Token `totalSupply` of an EVM destination at its settlement head.
    fn evm_total_supply(&self, network: &Network) -> Result<EvmSupply, ReadError>;
    /// The XRPL lane's issued outstanding: the issuer's obligations for the icUSD
    /// currency at a validated ledger. A source with no XRPL reader answers
    /// `Unavailable`, which fails the collection before any other read.
    fn xrpl_issued_outstanding(&self, _network: &Network) -> Result<XrplIssued, ReadError> {
        Err(ReadError::Unavailable {
            card: XRPL_SOURCE_CARDS,
        })
    }
    /// The bridge instance's own `get_supply_audit` for a counted EVM
    /// destination (its `instance` is a canonical principal). A query call; the
    /// reply is a self-report and is only compared with the independent reads.
    fn instance_audit(&self, destination: &DestinationConfig) -> Result<InstanceAudit, ReadError>;
    /// Extra label lines printed under `source:` (for example the read time of
    /// a live source). A fixture has none.
    fn notes(&self) -> Vec<String> {
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// Collector
// ---------------------------------------------------------------------------

/// What to collect: the destinations and the optional XRPL lane.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectConfig {
    pub destinations: Vec<DestinationConfig>,
    #[serde(default)]
    pub xrpl: Option<XrplLaneConfig>,
}

/// Head an EVM supply was read at, for the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadNote {
    pub network_id: String,
    pub finality: FinalityRule,
    pub block: u64,
}

/// One destination's instance audit beside the independent reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DestinationAudit {
    pub network_id: String,
    /// The bridge instance canister (principal text).
    pub instance: String,
    pub audit: InstanceAudit,
    /// `E_d` read from the ledger (not from the audit).
    pub escrow_independent: u128,
    /// `supply_d` read at the settlement head (not from the audit).
    pub supply_independent: u128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collected {
    /// The [`Readings::source`] label.
    pub source: &'static str,
    /// The [`Readings::notes`] lines (read time of a live source).
    pub notes: Vec<String>,
    /// The measured inputs the aggregate used.
    pub input: AggregateInput,
    /// Per counted EVM destination: the instance's audit next to the
    /// independent reads.
    pub audits: Vec<DestinationAudit>,
    /// Sum of the instances' liabilities across D (checked).
    pub liabilities_total: u128,
    pub report: AggregateReport,
    pub heads: Vec<HeadNote>,
    /// The XRPL lane's reading (validated ledger and hash), when a lane was
    /// collected.
    pub xrpl_read: Option<XrplIssued>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectError {
    /// The boundary or the arithmetic refused the input.
    Rejected(AggregateError),
    /// A reading is missing or unavailable.
    Read { what: String, error: ReadError },
    /// An EVM supply was read at a different head rule than the profile's.
    WrongHead {
        network_id: String,
        expected: FinalityRule,
        got: FinalityRule,
    },
}

impl fmt::Display for CollectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CollectError::Rejected(e) => write!(f, "rejected: {e}"),
            CollectError::Read { what, error } => write!(f, "cannot read {what}: {error}"),
            CollectError::WrongHead {
                network_id,
                expected,
                got,
            } => write!(
                f,
                "{network_id}: supply was read at head rule {got:?}, the profile requires {expected:?}"
            ),
        }
    }
}

impl std::error::Error for CollectError {}

fn read<T>(what: impl Into<String>, result: Result<T, ReadError>) -> Result<T, CollectError> {
    result.map_err(|error| CollectError::Read {
        what: what.into(),
        error,
    })
}

/// One destination's plan, decided before any reading is taken.
enum Plan<'a> {
    Excluded(&'a DestinationConfig),
    Evm {
        config: &'a DestinationConfig,
        binding: &'a EscrowBinding,
        network: &'a Network,
        instance: &'a str,
    },
}

/// Collect independent readings and aggregate them.
///
/// Order: boundary check, then adapter availability for every counted
/// destination and the lane, then the readings. Nothing is read for an excluded
/// (test-ledger) instance.
pub fn collect(
    config: &CollectConfig,
    catalog: &Catalog,
    readings: &dyn Readings,
) -> Result<Collected, CollectError> {
    check_boundary(config.destinations.iter(), config.xrpl.as_ref(), catalog)
        .map_err(CollectError::Rejected)?;

    let mut plan = Vec::new();
    for d in &config.destinations {
        if !d.in_d {
            plan.push(Plan::Excluded(d));
            continue;
        }
        let (Some((profile, network)), Some(binding)) =
            (catalog.lookup(&d.network_id), d.escrow.as_ref())
        else {
            // check_boundary has already refused both; keep the error typed.
            return Err(CollectError::Rejected(AggregateError::UnknownNetworkId(
                d.network_id.clone(),
            )));
        };
        let card = match profile.family {
            Family::Evm => {
                // A counted EVM destination needs its instance, before any read.
                let Some(instance) = d.instance.as_deref() else {
                    return Err(CollectError::Rejected(
                        AggregateError::MissingInstanceBinding(d.network_id.clone()),
                    ));
                };
                plan.push(Plan::Evm {
                    config: d,
                    binding,
                    network,
                    instance,
                });
                continue;
            }
            Family::Solana => SOLANA_SOURCE_CARD,
            Family::Sui => SUI_SOURCE_CARD,
            Family::Xrpl => XRPL_SOURCE_CARDS,
        };
        return Err(CollectError::Read {
            what: format!("destination supply of {}", d.network_id),
            error: ReadError::Unavailable { card },
        });
    }
    // The lane's issued outstanding is read first: a source without an XRPL
    // reader (or a lane that cannot be read) fails before any other query.
    let mut xrpl_read = None;
    if let Some(x) = &config.xrpl {
        let Some((_, network)) = catalog.lookup(&x.network_id) else {
            return Err(CollectError::Rejected(AggregateError::UnknownNetworkId(
                x.network_id.clone(),
            )));
        };
        let issued = read(
            format!("issued outstanding of the XRPL lane {}", x.network_id),
            readings.xrpl_issued_outstanding(network),
        )?;
        if issued.finality != network.finality.rule {
            return Err(CollectError::WrongHead {
                network_id: x.network_id.clone(),
                expected: network.finality.rule,
                got: issued.finality,
            });
        }
        xrpl_read = Some(issued);
    }

    let icp_ledger_total_supply = read(
        "ICP ledger total supply",
        readings.icp_ledger_total_supply(),
    )?;
    let debt_backed_chain_supplies = read(
        "debt-backed chain supplies",
        readings.debt_backed_chain_supplies(),
    )?;

    let mut destinations = Vec::new();
    let mut heads = Vec::new();
    let mut audits = Vec::new();
    for step in plan {
        match step {
            Plan::Excluded(d) => destinations.push(DestinationReading {
                config: d.clone(),
                escrow_balance: 0,
                supply: 0,
                custody: 0,
                rc: 0,
                bu: 0,
                stray: 0,
            }),
            Plan::Evm {
                config: d,
                binding,
                network,
                instance,
            } => {
                let id = &d.network_id;
                let escrow_balance = read(
                    format!("escrow balance of {id}"),
                    readings.icrc1_balance_of(binding),
                )?;
                let evm = read(
                    format!("token totalSupply of {id}"),
                    readings.evm_total_supply(network),
                )?;
                if evm.finality != network.finality.rule {
                    return Err(CollectError::WrongHead {
                        network_id: id.clone(),
                        expected: network.finality.rule,
                        got: evm.finality,
                    });
                }
                let audit = read(
                    format!("instance audit of {id}"),
                    readings.instance_audit(d),
                )?;
                audits.push(DestinationAudit {
                    network_id: id.clone(),
                    instance: instance.to_string(),
                    audit,
                    escrow_independent: escrow_balance,
                    supply_independent: evm.supply,
                });
                heads.push(HeadNote {
                    network_id: id.clone(),
                    finality: evm.finality,
                    block: evm.block,
                });
                // The EVM return burns atomically: no custody, no buckets.
                destinations.push(DestinationReading {
                    config: d.clone(),
                    escrow_balance,
                    supply: evm.supply,
                    custody: 0,
                    rc: 0,
                    bu: 0,
                    stray: 0,
                });
            }
        }
    }

    // E_X: the lane escrow's ICRC-1 balance, read through the same transport as
    // every destination escrow. Counted once, in `escrow_locked`.
    let xrpl = match (&config.xrpl, &xrpl_read) {
        (Some(x), Some(issued)) => {
            let escrow_balance = read(
                format!("escrow balance of the XRPL lane {}", x.network_id),
                readings.icrc1_balance_of(&x.escrow),
            )?;
            heads.push(HeadNote {
                network_id: x.network_id.clone(),
                finality: issued.finality,
                block: issued.ledger_index,
            });
            Some(XrplLaneReading {
                config: x.clone(),
                escrow_balance,
                issued_outstanding: issued.issued_outstanding,
            })
        }
        _ => None,
    };

    let input = AggregateInput {
        icp_ledger_total_supply,
        debt_backed_chain_supplies,
        destinations,
        xrpl,
    };
    let mut report = aggregate(&input, catalog).map_err(CollectError::Rejected)?;

    // Instance audits against the independent reads. Configured bindings are
    // matched by network id (check_boundary has already made the ids unique).
    let mut liabilities_total: u128 = 0;
    for a in &audits {
        let Some(configured) = config
            .destinations
            .iter()
            .find(|d| d.network_id == a.network_id)
            .and_then(|d| d.escrow.as_ref())
        else {
            continue;
        };
        report
            .flags
            .extend(audit_flags(a, configured).map_err(CollectError::Rejected)?);
        liabilities_total = liabilities_total
            .checked_add(a.audit.liabilities_e8s)
            .ok_or_else(|| {
                CollectError::Rejected(AggregateError::Overflow(
                    "sum of instance liabilities".into(),
                ))
            })?;
    }

    Ok(Collected {
        source: readings.source(),
        notes: readings.notes(),
        input,
        audits,
        liabilities_total,
        report,
        heads,
        xrpl_read,
    })
}

/// Recompute the instance's invariants from the INDEPENDENT reads and compare
/// its self-report to them. The self-report is never used as `E_d` or `supply_d`.
fn audit_flags(
    a: &DestinationAudit,
    configured: &EscrowBinding,
) -> Result<Vec<Flag>, AggregateError> {
    let id = &a.network_id;
    let audit = &a.audit;
    let flag = |kind: FlagKind| Flag {
        network_id: id.clone(),
        kind,
    };
    let mut flags = Vec::new();

    // The audit must describe the account the independent balance came from.
    let label = |b: &EscrowBinding| match b.identity() {
        Ok(identity) => account_label(&identity),
        Err(_) => b.owner.clone(),
    };
    let same_account = matches!(
        (audit.escrow_account.identity(), configured.identity()),
        (Ok(x), Ok(y)) if x == y
    );
    if !same_account {
        flags.push(flag(FlagKind::AuditEscrowAccountMismatch {
            audit: label(&audit.escrow_account),
            configured: label(configured),
        }));
    }

    // C1: E_d(independent) >= liabilities_d - unknown_outflow_d, the required
    // escrow being floored at zero as the instance defines it.
    let required = audit
        .liabilities_e8s
        .saturating_sub(audit.unknown_outflow_e8s);
    if a.escrow_independent < required {
        flags.push(flag(FlagKind::CoverageBroken {
            escrow_independent: a.escrow_independent,
            liabilities: audit.liabilities_e8s,
            unknown_outflow: audit.unknown_outflow_e8s,
        }));
    }

    // Supply bound: supply_d(independent) <= outstanding_d + inflight_mint_d.
    let bound = audit
        .outstanding_e8s
        .checked_add(audit.inflight_mint_e8s)
        .ok_or_else(|| AggregateError::Overflow(format!("outstanding + inflight mint at {id}")))?;
    if a.supply_independent > bound {
        flags.push(flag(FlagKind::SupplyBoundBroken {
            supply_independent: a.supply_independent,
            outstanding: audit.outstanding_e8s,
            inflight_mint: audit.inflight_mint_e8s,
        }));
    }

    // Self-report versus independent read: named, Info on its own.
    if let Some(reported) = audit.escrow_balance_e8s {
        if reported != a.escrow_independent {
            flags.push(flag(FlagKind::SelfReportedEscrowDiffers {
                reported,
                independent: a.escrow_independent,
            }));
        }
    }
    if let Some(reported) = audit.reported_supply_e8s {
        if reported != a.supply_independent {
            flags.push(flag(FlagKind::SelfReportedSupplyDiffers {
                reported,
                independent: a.supply_independent,
            }));
        }
    }
    Ok(flags)
}

/// e8s as `whole.00000000`.
pub fn format_e8s(v: u128) -> String {
    format!("{}.{:08}", v / E8, v % E8)
}

/// A plain-text report. The first line is the source label.
pub fn render(c: &Collected) -> String {
    let mut out = String::new();
    out.push_str(&format!("source: {}\n", c.source));
    for note in &c.notes {
        out.push_str(note);
        out.push('\n');
    }
    if c.source == "fixture" {
        out.push_str(
            "recorded fixture readings from a local file; no network was queried; not live data\n",
        );
    }
    let line = |label: &str, v: u128| format!("{label}: {} ({v} e8s)\n", format_e8s(v));
    out.push_str(&line(
        "icp_ledger_total_supply",
        c.input.icp_ledger_total_supply,
    ));
    out.push_str(&line(
        "debt_backed_chain_supplies",
        c.input.debt_backed_chain_supplies,
    ));
    for d in &c.input.destinations {
        if !d.config.in_d {
            out.push_str(&format!(
                "destination {}: excluded from D (test ledger), nothing read\n",
                d.config.network_id
            ));
            continue;
        }
        out.push_str(&format!(
            "destination {}: escrow {}, supply {}, custody {}\n",
            d.config.network_id,
            format_e8s(d.escrow_balance),
            format_e8s(d.supply),
            format_e8s(d.custody)
        ));
    }
    if let Some(x) = &c.input.xrpl {
        out.push_str(&format!(
            "xrpl lane {}: escrow {}, issued outstanding {}\n",
            x.config.network_id,
            format_e8s(x.escrow_balance),
            format_e8s(x.issued_outstanding)
        ));
    }
    for h in &c.heads {
        out.push_str(&format!(
            "head {}: {:?} block {}\n",
            h.network_id, h.finality, h.block
        ));
    }
    if let Some(r) = &c.xrpl_read {
        out.push_str(&format!(
            "xrpl issued outstanding read at validated ledger {} (hash {})\n",
            r.ledger_index, r.ledger_hash
        ));
    }
    for a in &c.audits {
        let au = &a.audit;
        let opt = |v: Option<u128>| v.map_or("n/a".to_string(), format_e8s);
        out.push_str(&format!(
            "instance audit {} (instance {}, self-report):\n  outstanding {}, pending_mint {}, inflight_mint {}, pending_release {}, pending_refund {}, held {}, fees {}, reserved_pulls {}\n  unknown_outflow {}, liabilities {}, required_escrow {}, coverage_ok {:?}\n  self-reported escrow {}, self-reported supply {} (block {})\n  independent reads: escrow {}, supply {}\n",
            a.network_id,
            a.instance,
            format_e8s(au.outstanding_e8s),
            format_e8s(au.pending_mint_e8s),
            format_e8s(au.inflight_mint_e8s),
            format_e8s(au.pending_release_e8s),
            format_e8s(au.pending_refund_e8s),
            format_e8s(au.held_e8s),
            format_e8s(au.fees_e8s),
            format_e8s(au.reserved_pulls_e8s),
            format_e8s(au.unknown_outflow_e8s),
            format_e8s(au.liabilities_e8s),
            format_e8s(au.required_escrow_e8s),
            au.coverage_ok,
            opt(au.escrow_balance_e8s),
            opt(au.reported_supply_e8s),
            au.reported_supply_block
                .map_or("n/a".to_string(), |b| b.to_string()),
            format_e8s(a.escrow_independent),
            format_e8s(a.supply_independent),
        ));
    }
    if !c.audits.is_empty() {
        out.push_str(&line(
            "liabilities_total (instances, D)",
            c.liabilities_total,
        ));
    }
    let r = &c.report;
    out.push_str(&line("total_icusd", r.total_icusd));
    out.push_str(&line("escrow_locked", r.escrow_locked));
    out.push_str(&line("circulating_on_icp", r.circulating_on_icp));
    for d in &r.circulating_dest {
        out.push_str(&line(
            &format!("circulating_dest {}", d.network_id),
            d.circulating,
        ));
    }
    out.push_str(&line("circulating_xrpl", r.circulating_xrpl));
    out.push_str(&line("circulating_total", r.circulating_total));
    if r.flags.is_empty() {
        out.push_str("flags: none\n");
    }
    for f in &r.flags {
        let sev = match f.severity() {
            Severity::Info => "info",
            Severity::Failure => "FAILURE",
        };
        out.push_str(&format!("flag {sev}: {} {:?}\n", f.network_id, f.kind));
    }
    out.push_str(if r.has_failure() {
        "status: FAILURE flags present, credits must halt\n"
    } else {
        "status: ok\n"
    });
    out
}

// ---------------------------------------------------------------------------
// Fixture source
// ---------------------------------------------------------------------------

/// A strict canonical decimal `u128` (digits only, no sign, no leading zero).
pub fn parse_u128_decimal(s: &str) -> Result<u128, String> {
    if s.is_empty() {
        return Err("empty amount string".to_string());
    }
    if !s.bytes().all(|c| c.is_ascii_digit()) {
        return Err(format!("amount {s:?} is not plain ASCII digits"));
    }
    if s.len() > 1 && s.starts_with('0') {
        return Err(format!("amount {s:?} has a leading zero"));
    }
    s.parse::<u128>()
        .map_err(|_| format!("amount {s:?} is above u128::MAX"))
}

mod u128_string {
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u128, D::Error> {
        let s = String::deserialize(d)?;
        super::parse_u128_decimal(&s).map_err(serde::de::Error::custom)
    }
}

mod opt_u128_string {
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u128>, D::Error> {
        match Option::<String>::deserialize(d)? {
            Some(s) => super::parse_u128_decimal(&s)
                .map(Some)
                .map_err(serde::de::Error::custom),
            None => Ok(None),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureBalance {
    escrow: EscrowBinding,
    #[serde(with = "u128_string")]
    balance: u128,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureEvmSupply {
    network_id: String,
    #[serde(with = "u128_string")]
    supply: u128,
    finality: FinalityRule,
    block: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureXrplIssued {
    network_id: String,
    #[serde(with = "u128_string")]
    issued_outstanding: u128,
    finality: FinalityRule,
    ledger_index: u64,
    ledger_hash: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureData {
    #[serde(default, with = "opt_u128_string")]
    icp_ledger_total_supply: Option<u128>,
    #[serde(default, with = "opt_u128_string")]
    debt_backed_chain_supplies: Option<u128>,
    #[serde(default)]
    icrc1_balances: Vec<FixtureBalance>,
    #[serde(default)]
    evm_supplies: Vec<FixtureEvmSupply>,
    #[serde(default)]
    xrpl_issued: Vec<FixtureXrplIssued>,
    #[serde(default)]
    instance_audits: Vec<FixtureInstanceAudit>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureInstanceAudit {
    /// The instance canister, as in `DestinationConfig::instance`.
    instance: String,
    audit: InstanceAudit,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureFile {
    description: String,
    config: CollectConfig,
    #[serde(default)]
    readings: FixtureData,
}

/// Readings replayed from a local file. Deterministic; it queries nothing.
/// A number the file does not contain is a failed read, never zero.
#[derive(Debug, Clone)]
pub struct FixtureReadings {
    icp_ledger_total_supply: Option<u128>,
    debt_backed_chain_supplies: Option<u128>,
    balances: BTreeMap<AccountIdentity, u128>,
    evm: BTreeMap<String, EvmSupply>,
    xrpl: BTreeMap<String, XrplIssued>,
    audits: BTreeMap<Vec<u8>, InstanceAudit>,
}

/// A parsed fixture: what to collect and what to replay.
#[derive(Debug, Clone)]
pub struct LoadedFixture {
    pub description: String,
    pub config: CollectConfig,
    pub readings: FixtureReadings,
}

impl FixtureReadings {
    /// Parse a fixture file's JSON.
    pub fn parse(json: &str) -> Result<LoadedFixture, String> {
        let file: FixtureFile =
            serde_json::from_str(json).map_err(|e| format!("fixture does not parse: {e}"))?;
        let mut balances = BTreeMap::new();
        for b in &file.readings.icrc1_balances {
            let identity = b
                .escrow
                .identity()
                .map_err(|e| format!("fixture balance for {}: {e}", b.escrow.owner))?;
            if balances.insert(identity, b.balance).is_some() {
                return Err(format!(
                    "fixture lists the balance of {} twice",
                    b.escrow.owner
                ));
            }
        }
        let mut evm = BTreeMap::new();
        for s in file.readings.evm_supplies {
            let id = s.network_id;
            let supply = EvmSupply {
                supply: s.supply,
                finality: s.finality,
                block: s.block,
            };
            if evm.insert(id.clone(), supply).is_some() {
                return Err(format!("fixture lists the supply of {id} twice"));
            }
        }
        let mut xrpl = BTreeMap::new();
        for x in file.readings.xrpl_issued {
            let id = x.network_id;
            let issued = XrplIssued {
                issued_outstanding: x.issued_outstanding,
                finality: x.finality,
                ledger_index: x.ledger_index,
                ledger_hash: x.ledger_hash,
            };
            if xrpl.insert(id.clone(), issued).is_some() {
                return Err(format!("fixture lists the issued outstanding of {id} twice"));
            }
        }
        let mut audits = BTreeMap::new();
        for a in file.readings.instance_audits {
            let key = crate::escrow::usable_principal(&a.instance)
                .map_err(|e| format!("fixture instance audit {}: {e}", a.instance))?;
            if audits.insert(key, a.audit).is_some() {
                return Err(format!(
                    "fixture lists the audit of instance {} twice",
                    a.instance
                ));
            }
        }
        Ok(LoadedFixture {
            description: file.description,
            config: file.config,
            readings: FixtureReadings {
                icp_ledger_total_supply: file.readings.icp_ledger_total_supply,
                debt_backed_chain_supplies: file.readings.debt_backed_chain_supplies,
                balances,
                evm,
                xrpl,
                audits,
            },
        })
    }
}

impl Readings for FixtureReadings {
    fn source(&self) -> &'static str {
        "fixture"
    }

    fn icp_ledger_total_supply(&self) -> Result<u128, ReadError> {
        self.icp_ledger_total_supply
            .ok_or_else(|| ReadError::Failed("fixture has no icp_ledger_total_supply".into()))
    }

    fn icrc1_balance_of(&self, binding: &EscrowBinding) -> Result<u128, ReadError> {
        let identity = binding.identity().map_err(ReadError::Failed)?;
        self.balances.get(&identity).copied().ok_or_else(|| {
            ReadError::Failed(format!("fixture has no balance for {}", binding.owner))
        })
    }

    fn debt_backed_chain_supplies(&self) -> Result<u128, ReadError> {
        self.debt_backed_chain_supplies
            .ok_or_else(|| ReadError::Failed("fixture has no debt_backed_chain_supplies".into()))
    }

    fn instance_audit(&self, destination: &DestinationConfig) -> Result<InstanceAudit, ReadError> {
        let text = destination.instance.as_deref().ok_or_else(|| {
            ReadError::Failed(format!("{} names no instance", destination.network_id))
        })?;
        let key = crate::escrow::usable_principal(text).map_err(ReadError::Failed)?;
        self.audits.get(&key).cloned().ok_or_else(|| {
            ReadError::Failed(format!(
                "fixture has no instance audit for {}",
                destination.network_id
            ))
        })
    }

    fn evm_total_supply(&self, network: &Network) -> Result<EvmSupply, ReadError> {
        self.evm.get(&network.network_id).cloned().ok_or_else(|| {
            ReadError::Failed(format!("fixture has no supply for {}", network.network_id))
        })
    }

    fn xrpl_issued_outstanding(&self, network: &Network) -> Result<XrplIssued, ReadError> {
        self.xrpl.get(&network.network_id).cloned().ok_or_else(|| {
            ReadError::Failed(format!(
                "fixture has no XRPL issued outstanding for {}",
                network.network_id
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::escrow::principal_text_to_bytes;

    const BASE_ONLY: &str = include_str!("../fixtures/collect_base_only.json");
    const OVERLAP: &str = include_str!("../fixtures/collect_rejected_xrpl_overlap.json");
    const C1_BROKEN: &str = include_str!("../fixtures/collect_c1_broken.json");
    const BASE_PLUS_XRPL: &str = include_str!("../fixtures/collect_base_plus_xrpl.json");

    fn catalog() -> Catalog {
        Catalog::embedded().expect("embedded catalog")
    }

    fn binding_for(id: &str) -> EscrowBinding {
        EscrowBinding::from_principal_bytes(format!("syn-{id}").as_bytes(), None)
    }

    fn instance_for(id: &str) -> String {
        crate::escrow::principal_to_text(format!("inst-{id}").as_bytes())
    }

    fn dest(id: &str) -> DestinationConfig {
        DestinationConfig {
            network_id: id.into(),
            in_d: true,
            escrow: Some(binding_for(id)),
            instance: Some(instance_for(id)),
        }
    }

    /// An audit that agrees with independent reads of `escrow` and `supply`.
    fn consistent_audit(id: &str, escrow: u128, supply: u128) -> InstanceAudit {
        InstanceAudit {
            escrow_account: binding_for(id),
            escrow_balance_e8s: Some(escrow),
            outstanding_e8s: supply,
            pending_mint_e8s: 0,
            inflight_mint_e8s: 0,
            pending_release_e8s: 0,
            pending_refund_e8s: 0,
            held_e8s: 0,
            fees_e8s: 0,
            reserved_pulls_e8s: 0,
            unknown_outflow_e8s: 0,
            liabilities_e8s: supply,
            required_escrow_e8s: supply,
            coverage_ok: Some(true),
            reported_supply_e8s: Some(supply),
            reported_supply_block: Some(7),
        }
    }

    /// A source that must never be touched.
    struct Untouchable;
    impl Readings for Untouchable {
        fn source(&self) -> &'static str {
            "fixture"
        }
        fn icp_ledger_total_supply(&self) -> Result<u128, ReadError> {
            panic!("read after a boundary or availability refusal")
        }
        fn icrc1_balance_of(&self, _: &EscrowBinding) -> Result<u128, ReadError> {
            panic!("read after a boundary or availability refusal")
        }
        fn debt_backed_chain_supplies(&self) -> Result<u128, ReadError> {
            panic!("read after a boundary or availability refusal")
        }
        fn instance_audit(&self, _: &DestinationConfig) -> Result<InstanceAudit, ReadError> {
            panic!("read after a boundary or availability refusal")
        }
        fn evm_total_supply(&self, _: &Network) -> Result<EvmSupply, ReadError> {
            panic!("read after a boundary or availability refusal")
        }
    }

    /// A source that answers every EVM read with the same numbers.
    struct Flat {
        icp: u128,
        escrow: u128,
        supply: u128,
        finality: FinalityRule,
        /// Replaces the default (consistent) audit when set.
        audit: Option<InstanceAudit>,
    }
    impl Readings for Flat {
        fn source(&self) -> &'static str {
            "fixture"
        }
        fn icp_ledger_total_supply(&self) -> Result<u128, ReadError> {
            Ok(self.icp)
        }
        fn icrc1_balance_of(&self, _: &EscrowBinding) -> Result<u128, ReadError> {
            Ok(self.escrow)
        }
        fn debt_backed_chain_supplies(&self) -> Result<u128, ReadError> {
            Ok(0)
        }
        fn instance_audit(&self, d: &DestinationConfig) -> Result<InstanceAudit, ReadError> {
            Ok(self
                .audit
                .clone()
                .unwrap_or_else(|| consistent_audit(&d.network_id, self.escrow, self.supply)))
        }
        fn evm_total_supply(&self, _: &Network) -> Result<EvmSupply, ReadError> {
            Ok(EvmSupply {
                supply: self.supply,
                finality: self.finality,
                block: 7,
            })
        }
    }

    fn flat(icp: u128, escrow: u128, supply: u128) -> Flat {
        Flat {
            icp,
            escrow,
            supply,
            finality: FinalityRule::EvmTag,
            audit: None,
        }
    }

    #[test]
    fn base_only_fixture_collects_and_renders_a_fixture_label() {
        let fx = FixtureReadings::parse(BASE_ONLY).unwrap();
        let got = collect(&fx.config, &catalog(), &fx.readings).unwrap();

        assert_eq!(got.source, "fixture");
        let r = &got.report;
        assert_eq!(r.total_icusd, 1_000 * E8);
        assert_eq!(r.escrow_locked, 200 * E8 + 12_345_678);
        assert_eq!(r.circulating_on_icp, 800 * E8 - 12_345_678);
        assert_eq!(r.circulating_dest.len(), 1);
        assert_eq!(r.circulating_dest[0].network_id, "base-mainnet");
        assert_eq!(r.circulating_dest[0].circulating, 200 * E8 + 12_345_678);
        assert_eq!(r.circulating_xrpl, 0);
        assert_eq!(r.circulating_total, 1_000 * E8);
        assert!(!r.has_failure());
        assert_eq!(got.heads.len(), 1);
        assert_eq!(got.heads[0].finality, FinalityRule::EvmTag);

        let text = render(&got);
        assert!(text.starts_with("source: fixture\n"), "{text}");
        assert!(text.contains("not live data"), "{text}");
        assert!(text.contains("circulating_total: 1000.00000000"), "{text}");
        assert!(text.contains("status: ok"), "{text}");
        // The excluded test-ledger instance is shown and was not read.
        assert!(
            text.contains("base-sepolia: excluded from D (test ledger), nothing read"),
            "{text}"
        );
    }

    #[test]
    fn overlap_fixture_is_rejected_before_any_read() {
        let fx = FixtureReadings::parse(OVERLAP).unwrap();
        // Untouchable panics on any read: the boundary refuses first.
        let err = collect(&fx.config, &catalog(), &Untouchable).unwrap_err();
        assert_eq!(
            err,
            CollectError::Rejected(AggregateError::DestinationIsXrpl("xrpl-mainnet".into()))
        );
        assert!(err.to_string().starts_with("rejected:"));
    }

    #[test]
    fn fixture_principals_are_obviously_synthetic() {
        for json in [BASE_ONLY, OVERLAP, C1_BROKEN, BASE_PLUS_XRPL] {
            let fx = FixtureReadings::parse(json).unwrap();
            assert!(
                fx.description.to_lowercase().contains("synthetic"),
                "fixture must say it is synthetic"
            );
            let mut owners: Vec<String> = fx
                .config
                .destinations
                .iter()
                .flat_map(|d| {
                    d.escrow
                        .iter()
                        .map(|b| b.owner.clone())
                        .chain(d.instance.iter().cloned())
                })
                .collect();
            if let Some(x) = &fx.config.xrpl {
                owners.push(x.escrow.owner.clone());
            }
            owners.extend(
                fx.readings
                    .audits
                    .values()
                    .map(|a| a.escrow_account.owner.clone()),
            );
            assert!(!owners.is_empty());
            for owner in owners {
                let bytes = principal_text_to_bytes(&owner).unwrap();
                assert!(
                    bytes.starts_with(b"synthetic-"),
                    "{owner} must decode to bytes spelling synthetic-..."
                );
            }
        }
    }

    /// A source that answers the lane with a fixed reading and counts its calls.
    struct LaneFlat {
        e_d: u128,
        e_x: u128,
        issued: XrplIssued,
        order: std::cell::RefCell<Vec<&'static str>>,
    }
    impl Readings for LaneFlat {
        fn source(&self) -> &'static str {
            "fixture"
        }
        fn icp_ledger_total_supply(&self) -> Result<u128, ReadError> {
            self.order.borrow_mut().push("icp_total");
            Ok(1_000 * E8)
        }
        fn icrc1_balance_of(&self, b: &EscrowBinding) -> Result<u128, ReadError> {
            self.order.borrow_mut().push("escrow");
            Ok(if b.owner == binding_for("xrpl-mainnet").owner {
                self.e_x
            } else {
                self.e_d
            })
        }
        fn debt_backed_chain_supplies(&self) -> Result<u128, ReadError> {
            Ok(0)
        }
        fn instance_audit(&self, d: &DestinationConfig) -> Result<InstanceAudit, ReadError> {
            Ok(consistent_audit(&d.network_id, self.e_d, self.e_d))
        }
        fn evm_total_supply(&self, _: &Network) -> Result<EvmSupply, ReadError> {
            Ok(EvmSupply {
                supply: self.e_d,
                finality: FinalityRule::EvmTag,
                block: 7,
            })
        }
        fn xrpl_issued_outstanding(&self, _: &Network) -> Result<XrplIssued, ReadError> {
            self.order.borrow_mut().push("xrpl_issued");
            Ok(self.issued.clone())
        }
    }

    fn lane_flat(e_d: u128, e_x: u128, issued: u128) -> LaneFlat {
        LaneFlat {
            e_d,
            e_x,
            issued: XrplIssued {
                issued_outstanding: issued,
                finality: FinalityRule::XrplValidated,
                ledger_index: 91_000_000,
                ledger_hash: "AB".repeat(32),
            },
            order: std::cell::RefCell::new(Vec::new()),
        }
    }

    fn d_plus_x() -> CollectConfig {
        CollectConfig {
            destinations: vec![dest("base-mainnet")],
            xrpl: Some(XrplLaneConfig {
                network_id: "xrpl-mainnet".into(),
                escrow: binding_for("xrpl-mainnet"),
            }),
        }
    }

    #[test]
    fn d_plus_x_counts_each_escrow_once_and_x_stays_outside_d() {
        let readings = lane_flat(200 * E8, 50 * E8, 50 * E8);
        let got = collect(&d_plus_x(), &catalog(), &readings).unwrap();
        let r = &got.report;
        // E_d (200) + E_X (50), each once.
        assert_eq!(r.escrow_locked, 250 * E8);
        assert_eq!(r.circulating_on_icp, 750 * E8);
        assert_eq!(r.circulating_dest.len(), 1);
        assert_eq!(r.circulating_dest[0].circulating, 200 * E8);
        assert_eq!(r.circulating_xrpl, 50 * E8);
        assert_eq!(r.circulating_total, 1_000 * E8);
        assert!(!r.has_failure());
        // The lane head is reported with the validated ledger.
        assert!(got.heads.iter().any(|h| h.network_id == "xrpl-mainnet"
            && h.finality == FinalityRule::XrplValidated
            && h.block == 91_000_000));
        assert_eq!(got.xrpl_read.as_ref().unwrap().issued_outstanding, 50 * E8);
        let text = render(&got);
        assert!(text.contains("xrpl lane xrpl-mainnet: escrow 50.00000000, issued outstanding 50.00000000"), "{text}");
        assert!(text.contains("head xrpl-mainnet: XrplValidated block 91000000"), "{text}");
        assert!(text.contains("xrpl issued outstanding read at validated ledger 91000000"), "{text}");
        // The lane is read BEFORE the ICP-side numbers.
        assert_eq!(readings.order.borrow()[0], "xrpl_issued");
    }

    #[test]
    fn a_lane_issued_amount_above_its_escrow_is_reported_not_corrected() {
        // The collector reports what was read: 60 issued against 50 escrowed
        // makes circulating_total exceed total_icusd by exactly 10.
        let got = collect(&d_plus_x(), &catalog(), &lane_flat(200 * E8, 50 * E8, 60 * E8)).unwrap();
        assert_eq!(got.report.circulating_xrpl, 60 * E8);
        assert_eq!(got.report.circulating_total, 1_010 * E8);
    }

    #[test]
    fn the_lane_ledger_must_be_read_under_the_profiles_finality_rule() {
        let mut readings = lane_flat(200 * E8, 50 * E8, 50 * E8);
        readings.issued.finality = FinalityRule::EvmTag;
        let err = collect(&d_plus_x(), &catalog(), &readings).unwrap_err();
        assert!(
            matches!(
                &err,
                CollectError::WrongHead { network_id, expected: FinalityRule::XrplValidated, got: FinalityRule::EvmTag }
                    if network_id == "xrpl-mainnet"
            ),
            "{err:?}"
        );
    }

    #[test]
    fn the_lane_escrow_cannot_also_be_a_d_escrow() {
        // Counting the same account in D and X would double count it: refused
        // before any read.
        let mut cfg = d_plus_x();
        cfg.xrpl.as_mut().unwrap().escrow = binding_for("base-mainnet");
        let err = collect(&cfg, &catalog(), &Untouchable).unwrap_err();
        assert!(matches!(
            err,
            CollectError::Rejected(AggregateError::DuplicateEscrowBinding { .. })
        ));
    }

    #[test]
    fn the_base_plus_xrpl_fixture_collects_with_a_fixture_label() {
        let fx = FixtureReadings::parse(BASE_PLUS_XRPL).unwrap();
        let got = collect(&fx.config, &catalog(), &fx.readings).unwrap();
        assert_eq!(got.source, "fixture");
        let r = &got.report;
        assert_eq!(r.escrow_locked, 20_012_345_678 + 5_000_000_001);
        assert_eq!(r.circulating_xrpl, 5_000_000_001);
        assert_eq!(r.circulating_total, 100_000_000_000);
        assert!(!r.has_failure());
        let text = render(&got);
        assert!(text.starts_with("source: fixture\n"), "{text}");
        assert!(text.contains("xrpl lane xrpl-mainnet"), "{text}");
    }

    #[test]
    fn a_fixture_without_the_xrpl_reading_is_a_failed_read_not_zero() {
        let mut fx = FixtureReadings::parse(BASE_PLUS_XRPL).unwrap();
        fx.readings.xrpl.clear();
        let err = collect(&fx.config, &catalog(), &fx.readings).unwrap_err();
        assert!(
            matches!(
                &err,
                CollectError::Read { error: ReadError::Failed(m), what }
                    if m.contains("no XRPL issued outstanding") && what.contains("XRPL lane")
            ),
            "{err:?}"
        );
    }

    #[test]
    fn unavailable_families_name_their_card_and_read_nothing() {
        let cases = [("solana-mainnet", "BB-49"), ("sui-mainnet", "BB-53")];
        for (id, card) in cases {
            let cfg = CollectConfig {
                destinations: vec![dest(id)],
                xrpl: None,
            };
            let err = collect(&cfg, &catalog(), &Untouchable).unwrap_err();
            match &err {
                CollectError::Read {
                    error: ReadError::Unavailable { card: got },
                    what,
                } => {
                    assert_eq!(*got, card, "{id}");
                    assert!(what.contains(id), "{what}");
                }
                other => panic!("{id}: unexpected {other:?}"),
            }
            assert!(err.to_string().contains(card));
        }

        // The XRPL lane.
        let cfg = CollectConfig {
            destinations: vec![],
            xrpl: Some(XrplLaneConfig {
                network_id: "xrpl-mainnet".into(),
                escrow: binding_for("xrpl-mainnet"),
            }),
        };
        let err = collect(&cfg, &catalog(), &Untouchable).unwrap_err();
        assert!(
            matches!(
                &err,
                CollectError::Read {
                    error: ReadError::Unavailable {
                        card: "BB-56/BB-57"
                    },
                    ..
                }
            ),
            "{err:?}"
        );
    }

    #[test]
    fn an_excluded_solana_instance_needs_no_adapter() {
        // A test-ledger Solana instance is not in D: nothing is read for it.
        let mut sol = dest("solana-devnet");
        sol.in_d = false;
        let cfg = CollectConfig {
            destinations: vec![dest("base-mainnet"), sol],
            xrpl: None,
        };
        let flat = Flat {
            icp: 1_000 * E8,
            escrow: 100 * E8,
            supply: 100 * E8,
            finality: FinalityRule::EvmTag,
            audit: None,
        };
        let got = collect(&cfg, &catalog(), &flat).unwrap();
        assert_eq!(got.report.circulating_total, 1_000 * E8);
        assert_eq!(got.heads.len(), 1);
    }

    #[test]
    fn a_missing_fixture_number_is_a_failed_read_not_zero() {
        let mut fx = FixtureReadings::parse(BASE_ONLY).unwrap();
        fx.readings.icp_ledger_total_supply = None;
        let err = collect(&fx.config, &catalog(), &fx.readings).unwrap_err();
        assert!(
            matches!(&err, CollectError::Read { error: ReadError::Failed(m), .. } if m.contains("icp_ledger_total_supply")),
            "{err:?}"
        );

        let mut fx = FixtureReadings::parse(BASE_ONLY).unwrap();
        fx.readings.balances.clear();
        let err = collect(&fx.config, &catalog(), &fx.readings).unwrap_err();
        assert!(
            matches!(&err, CollectError::Read { what, .. } if what.contains("escrow balance of base-mainnet")),
            "{err:?}"
        );

        let mut fx = FixtureReadings::parse(BASE_ONLY).unwrap();
        fx.readings.audits.clear();
        let err = collect(&fx.config, &catalog(), &fx.readings).unwrap_err();
        assert!(
            matches!(&err, CollectError::Read { what, .. } if what.contains("instance audit of base-mainnet")),
            "{err:?}"
        );

        let mut fx = FixtureReadings::parse(BASE_ONLY).unwrap();
        fx.readings.evm.clear();
        let err = collect(&fx.config, &catalog(), &fx.readings).unwrap_err();
        assert!(
            matches!(&err, CollectError::Read { what, .. } if what.contains("totalSupply of base-mainnet")),
            "{err:?}"
        );
    }

    #[test]
    fn a_supply_read_at_the_wrong_head_is_refused() {
        let cfg = CollectConfig {
            destinations: vec![dest("base-mainnet")],
            xrpl: None,
        };
        let flat = Flat {
            icp: 1_000 * E8,
            escrow: 100 * E8,
            supply: 100 * E8,
            finality: FinalityRule::SolanaCommitment,
            audit: None,
        };
        assert_eq!(
            collect(&cfg, &catalog(), &flat).unwrap_err(),
            CollectError::WrongHead {
                network_id: "base-mainnet".into(),
                expected: FinalityRule::EvmTag,
                got: FinalityRule::SolanaCommitment,
            }
        );
    }

    #[test]
    fn inconsistent_reads_surface_the_aggregate_refusal() {
        // Escrow larger than the ledger total: the aggregate refuses it.
        let cfg = CollectConfig {
            destinations: vec![dest("base-mainnet")],
            xrpl: None,
        };
        let flat = Flat {
            icp: 50 * E8,
            escrow: 100 * E8,
            supply: 100 * E8,
            finality: FinalityRule::EvmTag,
            audit: None,
        };
        assert!(matches!(
            collect(&cfg, &catalog(), &flat).unwrap_err(),
            CollectError::Rejected(AggregateError::Underflow(_))
        ));
    }

    fn base_fixture() -> LoadedFixture {
        FixtureReadings::parse(BASE_ONLY).unwrap()
    }

    fn flags_of(c: &Collected) -> Vec<FlagKind> {
        c.report.flags.iter().map(|f| f.kind.clone()).collect()
    }

    #[test]
    fn the_instance_audit_is_printed_next_to_the_independent_reads() {
        let fx = base_fixture();
        let got = collect(&fx.config, &catalog(), &fx.readings).unwrap();
        assert_eq!(got.audits.len(), 1);
        assert_eq!(got.audits[0].escrow_independent, 200 * E8 + 12_345_678);
        assert_eq!(got.audits[0].supply_independent, 200 * E8 + 12_345_678);
        assert_eq!(got.liabilities_total, 200 * E8 + 12_345_678);
        // Only the excluded-instance info flag: the audit agrees with the reads.
        assert_eq!(flags_of(&got), vec![FlagKind::ExcludedFromD]);
        assert!(!got.report.has_failure());

        let text = render(&got);
        assert!(
            text.contains("instance audit base-mainnet (instance "),
            "{text}"
        );
        assert!(
            text.contains(
                "outstanding 200.00000000, pending_mint 0.12345678, inflight_mint 0.12345678"
            ),
            "{text}"
        );
        assert!(text.contains("unknown_outflow 0.00000000, liabilities 200.12345678, required_escrow 200.12345678, coverage_ok Some(true)"), "{text}");
        assert!(
            text.contains("independent reads: escrow 200.12345678, supply 200.12345678"),
            "{text}"
        );
        assert!(
            text.contains("liabilities_total (instances, D): 200.12345678"),
            "{text}"
        );
        // The report's own figures come from the independent reads.
        assert_eq!(got.report.escrow_locked, 200 * E8 + 12_345_678);
    }

    #[test]
    fn c1_failing_on_the_independent_read_is_a_failure_flag() {
        let fx = FixtureReadings::parse(C1_BROKEN).unwrap();
        let got = collect(&fx.config, &catalog(), &fx.readings).unwrap();
        let flags = flags_of(&got);
        assert!(
            flags.contains(&FlagKind::CoverageBroken {
                escrow_independent: 190 * E8,
                liabilities: 200 * E8 + 12_345_678,
                unknown_outflow: 0,
            }),
            "{flags:?}"
        );
        // The instance still claims a full balance: named, Info.
        assert!(
            flags.contains(&FlagKind::SelfReportedEscrowDiffers {
                reported: 200 * E8 + 12_345_678,
                independent: 190 * E8,
            }),
            "{flags:?}"
        );
        assert!(got.report.has_failure());
        assert_eq!(got.report.failures().count(), 1);
        // The report uses the independent 190, never the self-reported figure.
        assert_eq!(got.report.escrow_locked, 190 * E8);
        let text = render(&got);
        assert!(
            text.contains("status: FAILURE flags present, credits must halt"),
            "{text}"
        );
        assert!(
            text.contains("independent reads: escrow 190.00000000"),
            "{text}"
        );
    }

    #[test]
    fn the_supply_bound_failing_on_the_independent_read_is_a_failure_flag() {
        let mut fx = base_fixture();
        // One e8 more than outstanding plus in-flight mint (200.12345678).
        fx.readings.evm.get_mut("base-mainnet").unwrap().supply = 200 * E8 + 12_345_679;
        let got = collect(&fx.config, &catalog(), &fx.readings).unwrap();
        let flags = flags_of(&got);
        assert!(
            flags.contains(&FlagKind::SupplyBoundBroken {
                supply_independent: 200 * E8 + 12_345_679,
                outstanding: 200 * E8,
                inflight_mint: 12_345_678,
            }),
            "{flags:?}"
        );
        assert!(
            flags.contains(&FlagKind::SelfReportedSupplyDiffers {
                reported: 200 * E8 + 12_345_678,
                independent: 200 * E8 + 12_345_679,
            }),
            "{flags:?}"
        );
        assert!(got.report.has_failure());
        // Exactly at the bound is fine.
        let mut ok = base_fixture();
        ok.readings.evm.get_mut("base-mainnet").unwrap().supply = 200 * E8 + 12_345_678;
        assert!(!collect(&ok.config, &catalog(), &ok.readings)
            .unwrap()
            .report
            .has_failure());
    }

    #[test]
    fn unknown_outflow_lowers_the_required_escrow_in_c1() {
        let cfg = CollectConfig {
            destinations: vec![dest("base-mainnet")],
            xrpl: None,
        };
        let mut audit = consistent_audit("base-mainnet", 20 * E8, 20 * E8);
        audit.liabilities_e8s = 30 * E8;
        audit.unknown_outflow_e8s = 10 * E8;
        audit.required_escrow_e8s = 20 * E8;
        let mut src = flat(1_000 * E8, 20 * E8, 20 * E8);
        src.audit = Some(audit.clone());
        // 20 >= 30 - 10: covered.
        assert!(!collect(&cfg, &catalog(), &src)
            .unwrap()
            .report
            .has_failure());
        // 19 < 20: broken.
        src.escrow = 19 * E8;
        assert!(collect(&cfg, &catalog(), &src)
            .unwrap()
            .report
            .failures()
            .any(|f| matches!(f.kind, FlagKind::CoverageBroken { .. })));
        // An unknown outflow above the liabilities floors the requirement at 0.
        audit.unknown_outflow_e8s = 40 * E8;
        src.audit = Some(audit);
        src.escrow = 0;
        assert!(!collect(&cfg, &catalog(), &src)
            .unwrap()
            .report
            .has_failure());
    }

    #[test]
    fn self_report_disagreement_is_named_info_and_never_used_as_a_read() {
        let mut fx = base_fixture();
        let audit = fx.readings.audits.values_mut().next().unwrap();
        audit.escrow_balance_e8s = Some(1);
        audit.reported_supply_e8s = Some(5);
        let got = collect(&fx.config, &catalog(), &fx.readings).unwrap();
        let flags = flags_of(&got);
        assert!(flags.contains(&FlagKind::SelfReportedEscrowDiffers {
            reported: 1,
            independent: 200 * E8 + 12_345_678,
        }));
        assert!(flags.contains(&FlagKind::SelfReportedSupplyDiffers {
            reported: 5,
            independent: 200 * E8 + 12_345_678,
        }));
        // Info only: C1 and the bound hold on the independent reads.
        assert!(!got.report.has_failure());
        assert_eq!(got.report.escrow_locked, 200 * E8 + 12_345_678);
        assert_eq!(
            got.report.circulating_dest[0].circulating,
            200 * E8 + 12_345_678
        );
    }

    #[test]
    fn an_audit_for_a_different_escrow_account_is_a_failure() {
        let mut fx = base_fixture();
        let audit = fx.readings.audits.values_mut().next().unwrap();
        audit.escrow_account = EscrowBinding::from_principal_bytes(b"synthetic-other-escrow", None);
        let got = collect(&fx.config, &catalog(), &fx.readings).unwrap();
        assert!(got.report.failures().any(|f| matches!(
            &f.kind,
            FlagKind::AuditEscrowAccountMismatch { audit, configured } if audit != configured
        )));
        // The same account spelled with the zero subaccount is not a mismatch.
        let mut ok = base_fixture();
        let a = ok.readings.audits.values_mut().next().unwrap();
        a.escrow_account.subaccount = Some([0; 32]);
        assert!(!collect(&ok.config, &catalog(), &ok.readings)
            .unwrap()
            .report
            .has_failure());
    }

    #[test]
    fn liabilities_are_summed_across_d() {
        let cfg = CollectConfig {
            destinations: vec![dest("base-mainnet"), dest("arbitrum-mainnet")],
            xrpl: None,
        };
        let got = collect(&cfg, &catalog(), &flat(1_000 * E8, 100 * E8, 100 * E8)).unwrap();
        assert_eq!(got.audits.len(), 2);
        assert_eq!(got.liabilities_total, 200 * E8);
        assert!(render(&got).contains("liabilities_total (instances, D): 200.00000000"));
    }

    #[test]
    fn a_missing_or_duplicate_instance_binding_is_rejected_before_any_read() {
        let mut missing = dest("base-mainnet");
        missing.instance = None;
        let cfg = CollectConfig {
            destinations: vec![missing],
            xrpl: None,
        };
        assert_eq!(
            collect(&cfg, &catalog(), &Untouchable).unwrap_err(),
            CollectError::Rejected(AggregateError::MissingInstanceBinding(
                "base-mainnet".into()
            ))
        );

        let mut second = dest("arbitrum-mainnet");
        second.instance = dest("base-mainnet").instance;
        let cfg = CollectConfig {
            destinations: vec![dest("base-mainnet"), second],
            xrpl: None,
        };
        assert_eq!(
            collect(&cfg, &catalog(), &Untouchable).unwrap_err(),
            CollectError::Rejected(AggregateError::DuplicateInstanceBinding {
                first: "base-mainnet".into(),
                second: "arbitrum-mainnet".into(),
            })
        );

        let mut bad = dest("base-mainnet");
        bad.instance = Some("2vxsx-fae".into());
        let cfg = CollectConfig {
            destinations: vec![bad],
            xrpl: None,
        };
        assert!(matches!(
            collect(&cfg, &catalog(), &Untouchable).unwrap_err(),
            CollectError::Rejected(AggregateError::InvalidInstanceBinding { .. })
        ));
    }

    #[test]
    fn fixture_parse_is_strict() {
        assert!(FixtureReadings::parse("{}").is_err());
        assert!(
            FixtureReadings::parse(&BASE_ONLY.replace("\"description\"", "\"surprise\"")).is_err()
        );
        // Amounts are decimal strings, not numbers.
        assert!(
            FixtureReadings::parse(&BASE_ONLY.replace("\"100000000000\"", "100000000000")).is_err()
        );
        assert!(
            FixtureReadings::parse(&BASE_ONLY.replace("\"100000000000\"", "\"0100\"")).is_err()
        );
    }

    #[test]
    fn e8_formatting() {
        assert_eq!(format_e8s(0), "0.00000000");
        assert_eq!(format_e8s(1), "0.00000001");
        assert_eq!(format_e8s(123_456_789_012), "1234.56789012");
    }
}
