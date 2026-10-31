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
//! # Adapters by family
//!
//! | Family | Source | Status |
//! | --- | --- | --- |
//! | EVM (Base and the other EVM rows) | token `totalSupply` at the profile's settlement head, custody zero (the return burns atomically) | wired through [`Readings::evm_total_supply`] |
//! | Solana | supply plus the measured return-account balance | `Unavailable`, delivered by card BB-49 |
//! | Sui | wrapped-currency supply | `Unavailable`, delivered by card BB-53 |
//! | XRPL lane | issued outstanding | `Unavailable`, delivered by cards BB-56 and BB-57 |
//!
//! An unavailable source fails the whole collection with an explicit error
//! naming the card. No value is ever invented, defaulted or carried over.
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
    DestinationConfig, DestinationReading, Severity, XrplLaneConfig,
};
use crate::escrow::{AccountIdentity, EscrowBinding};
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collected {
    /// The [`Readings::source`] label.
    pub source: &'static str,
    /// The [`Readings::notes`] lines (read time of a live source).
    pub notes: Vec<String>,
    /// The measured inputs the aggregate used.
    pub input: AggregateInput,
    pub report: AggregateReport,
    pub heads: Vec<HeadNote>,
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
                plan.push(Plan::Evm {
                    config: d,
                    binding,
                    network,
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
    if let Some(x) = &config.xrpl {
        return Err(CollectError::Read {
            what: format!("issued outstanding of the XRPL lane {}", x.network_id),
            error: ReadError::Unavailable {
                card: XRPL_SOURCE_CARDS,
            },
        });
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

    let input = AggregateInput {
        icp_ledger_total_supply,
        debt_backed_chain_supplies,
        destinations,
        // Unreachable until the XRPL source exists: a configured lane failed above.
        xrpl: None,
    };
    let report = aggregate(&input, catalog).map_err(CollectError::Rejected)?;
    Ok(Collected {
        source: readings.source(),
        notes: readings.notes(),
        input,
        report,
        heads,
    })
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
    for h in &c.heads {
        out.push_str(&format!(
            "head {}: {:?} block {}\n",
            h.network_id, h.finality, h.block
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
        Ok(LoadedFixture {
            description: file.description,
            config: file.config,
            readings: FixtureReadings {
                icp_ledger_total_supply: file.readings.icp_ledger_total_supply,
                debt_backed_chain_supplies: file.readings.debt_backed_chain_supplies,
                balances,
                evm,
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

    fn evm_total_supply(&self, network: &Network) -> Result<EvmSupply, ReadError> {
        self.evm.get(&network.network_id).cloned().ok_or_else(|| {
            ReadError::Failed(format!("fixture has no supply for {}", network.network_id))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::escrow::principal_text_to_bytes;

    const BASE_ONLY: &str = include_str!("../fixtures/collect_base_only.json");
    const OVERLAP: &str = include_str!("../fixtures/collect_rejected_xrpl_overlap.json");

    fn catalog() -> Catalog {
        Catalog::embedded().expect("embedded catalog")
    }

    fn binding_for(id: &str) -> EscrowBinding {
        EscrowBinding::from_principal_bytes(format!("syn-{id}").as_bytes(), None)
    }

    fn dest(id: &str) -> DestinationConfig {
        DestinationConfig {
            network_id: id.into(),
            in_d: true,
            escrow: Some(binding_for(id)),
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
        fn evm_total_supply(&self, _: &Network) -> Result<EvmSupply, ReadError> {
            Ok(EvmSupply {
                supply: self.supply,
                finality: self.finality,
                block: 7,
            })
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
        for json in [BASE_ONLY, OVERLAP] {
            let fx = FixtureReadings::parse(json).unwrap();
            assert!(
                fx.description.to_lowercase().contains("synthetic"),
                "fixture must say it is synthetic"
            );
            let mut owners: Vec<&str> = fx
                .config
                .destinations
                .iter()
                .filter_map(|d| d.escrow.as_ref())
                .map(|b| b.owner.as_str())
                .collect();
            if let Some(x) = &fx.config.xrpl {
                owners.push(&x.escrow.owner);
            }
            assert!(!owners.is_empty());
            for owner in owners {
                let bytes = principal_text_to_bytes(owner).unwrap();
                assert!(
                    bytes.starts_with(b"synthetic-"),
                    "{owner} must decode to bytes spelling synthetic-..."
                );
            }
        }
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
        };
        assert!(matches!(
            collect(&cfg, &catalog(), &flat).unwrap_err(),
            CollectError::Rejected(AggregateError::Underflow(_))
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
