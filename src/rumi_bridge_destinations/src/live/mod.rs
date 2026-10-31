//! Live client transport for the supply collector (card BB-58, feature
//! `live-transport`, off by default).
//!
//! [`LiveReadings`] implements [`crate::collect::Readings`] over two small
//! transports, so the collector, the boundary checks and the report are exactly
//! the ones the fixture run uses:
//!
//! * [`ic`]: an [`ic::IcQuery`] for the ICP ledger (`icrc1_total_supply`,
//!   `icrc1_balance_of`) and the backend's per-chain recorded supply. The real
//!   implementation is `ic-agent`: anonymous identity, query calls only, response
//!   verification left on.
//! * [`evm`]: a [`evm::JsonRpcTransport`] per destination, with the bridge
//!   canister's head rule driven by the profile (finalized quorum, Monad
//!   verified = finalized minus 3, blocked rows ask nothing) and `totalSupply`
//!   read at a NUMBERED block. `latest` is never sent. Each provider is first
//!   asked `eth_chainId` and bound to the profile's chain id; a provider on
//!   another chain is excluded from every vote.
//!
//! * [`xrpl`]: the XRPL lane's issued outstanding, read from the XRP Ledger
//!   itself (the issuer's icUSD `obligations` at a numbered VALIDATED ledger
//!   agreed by the profile's provider quorum, providers on another network or
//!   with an unvalidated answer excluded, amounts converted exactly). `E_X` is
//!   an ordinary ICRC-1 balance read through the IC transport above.
//!
//! * [`ic::instance_supply_audit`]: each counted EVM destination's bridge
//!   instance is also asked for its own `get_supply_audit` (query, anonymous).
//!   That reply is a self-report, shown next to the independent reads and
//!   checked against them, never used as a balance or a supply.
//!
//! Every report from here is labelled `source: live` and carries the read time
//! and the head height of each EVM reading, so it cannot be mistaken for a
//! fixture run (which prints `source: fixture`).
//!
//! # Safety posture
//!
//! There are NO built-in endpoints, canister ids or token addresses: all of them
//! come from the explicit endpoint config ([`LiveConfig`]) the operator passes.
//! Nothing in this crate runs these clients by itself. The tests use mocked
//! transports and one loopback server only. Executing the real clients against
//! IC mainnet or a public EVM RPC is a separate, held step.
//!
//! Unavailable by design: Solana and Sui (see [`crate::collect`]); a profile
//! whose head is blocked; a backend without the per-chain query or without
//! configured chain ids; an XRPL lane without a configured issuer and providers.
//! Each is a named error, never a zero.

pub mod evm;
pub mod ic;
pub mod xrpl;

use std::collections::BTreeMap;

use candid::Principal;
use serde::Deserialize;

use crate::aggregate::DestinationConfig;
use crate::collect::{EvmSupply, InstanceAudit, ReadError, Readings, XrplIssued};
use crate::escrow::EscrowBinding;
use crate::types::Network;

// ---------------------------------------------------------------------------
// Endpoint config
// ---------------------------------------------------------------------------

/// The explicit endpoint config file. Nothing here has a default.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveConfig {
    pub ic: IcConfig,
    /// EVM destinations by network id (as in the profile catalog).
    #[serde(default)]
    pub evm: BTreeMap<String, EvmConfig>,
    /// The XRPL lane by network id (as in the profile catalog).
    #[serde(default)]
    pub xrpl: BTreeMap<String, XrplConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XrplConfig {
    /// The icUSD issuer account (an XRPL classic address). There is no default.
    pub issuer: String,
    /// rippled JSON-RPC provider URLs (https, or http on loopback). They may
    /// carry keys and are never printed.
    pub providers: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IcConfig {
    /// Replica API URL (https, or http on loopback).
    pub url: String,
    /// The production icUSD ledger canister, as principal text.
    pub ledger: String,
    /// The backend and the debt-backed chains to sum. Omit it and the
    /// debt-backed supply reading is Unavailable.
    #[serde(default)]
    pub backend: Option<BackendConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendConfig {
    pub canister: String,
    pub debt_backed_chain_ids: Vec<u32>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvmConfig {
    /// The bridged icUSD token, `0x` plus 40 hex characters.
    pub token: String,
    /// Provider URLs (https, or http on loopback). They may carry keys and are
    /// never printed.
    pub providers: Vec<String>,
}

fn parse_principal(text: &str, what: &str) -> Result<Principal, String> {
    // Reject non-canonical text so a mistyped id cannot name another canister.
    let bytes = crate::escrow::principal_text_to_bytes(text)
        .map_err(|e| format!("{what} is not a principal: {e}"))?;
    Ok(Principal::from_slice(&bytes))
}

fn parse_token(text: &str) -> Result<[u8; 20], String> {
    let digits = text
        .strip_prefix("0x")
        .ok_or("a token address must start with 0x")?;
    if digits.len() != 40 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("a token address must be 0x plus 40 hex characters".into());
    }
    let mut out = [0u8; 20];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&digits[2 * i..2 * i + 2], 16).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// LiveReadings
// ---------------------------------------------------------------------------

/// One EVM destination's token and transport.
pub struct EvmSource {
    pub token: [u8; 20],
    pub transport: Box<dyn evm::JsonRpcTransport>,
}

/// One XRPL lane's issuer and transport.
pub struct XrplSource {
    pub issuer: String,
    pub transport: Box<dyn evm::JsonRpcTransport>,
}

/// [`Readings`] over the IC query client, the EVM JSON-RPC clients and the XRPL
/// JSON-RPC clients.
pub struct LiveReadings {
    ic: Box<dyn ic::IcQuery>,
    ledger: Principal,
    backend: Option<(Principal, Vec<u32>)>,
    evm: BTreeMap<String, EvmSource>,
    xrpl: BTreeMap<String, XrplSource>,
    read_at_unix: u64,
}

impl LiveReadings {
    /// Assemble from already-built transports (this is what the tests use).
    pub fn new(
        ic: Box<dyn ic::IcQuery>,
        ledger: Principal,
        backend: Option<(Principal, Vec<u32>)>,
        evm: BTreeMap<String, EvmSource>,
        read_at_unix: u64,
    ) -> LiveReadings {
        LiveReadings {
            ic,
            ledger,
            backend,
            evm,
            xrpl: BTreeMap::new(),
            read_at_unix,
        }
    }

    /// Replace the IC transport (the tests give the lane a ledger that answers
    /// per escrow account).
    #[cfg(test)]
    pub(crate) fn with_ic(mut self, ic: Box<dyn ic::IcQuery>) -> LiveReadings {
        self.ic = ic;
        self
    }

    /// Add the XRPL lane source of `network_id` (as in the profile catalog).
    pub fn with_xrpl(mut self, network_id: &str, source: XrplSource) -> LiveReadings {
        self.xrpl.insert(network_id.to_string(), source);
        self
    }

    /// Build the real clients from an endpoint config. This constructs HTTP
    /// clients only; no request is sent until a reading is taken.
    pub fn from_config(config: &LiveConfig, read_at_unix: u64) -> Result<LiveReadings, String> {
        let ledger = parse_principal(&config.ic.ledger, "ic.ledger")?;
        let backend = match &config.ic.backend {
            Some(b) => {
                ic::check_unique_chain_ids(&b.debt_backed_chain_ids)
                    .map_err(|e| format!("ic.backend.debt_backed_chain_ids: {e}"))?;
                Some((
                    parse_principal(&b.canister, "ic.backend.canister")?,
                    b.debt_backed_chain_ids.clone(),
                ))
            }
            None => None,
        };
        let ic = ic::AgentQuery::new(&config.ic.url)?;
        let mut evm_sources = BTreeMap::new();
        for (id, c) in &config.evm {
            let token = parse_token(&c.token).map_err(|e| format!("evm.{id}.token: {e}"))?;
            let transport = evm::ReqwestJsonRpc::new(c.providers.clone())
                .map_err(|e| format!("evm.{id}.providers: {e}"))?;
            evm_sources.insert(
                id.clone(),
                EvmSource {
                    token,
                    transport: Box::new(transport),
                },
            );
        }
        let mut live = LiveReadings::new(Box::new(ic), ledger, backend, evm_sources, read_at_unix);
        for (id, c) in &config.xrpl {
            xrpl::check_issuer(&c.issuer).map_err(|e| format!("xrpl.{id}.issuer: {e}"))?;
            let transport = evm::ReqwestJsonRpc::new(c.providers.clone())
                .map_err(|e| format!("xrpl.{id}.providers: {e}"))?;
            live = live.with_xrpl(
                id,
                XrplSource {
                    issuer: c.issuer.clone(),
                    transport: Box::new(transport),
                },
            );
        }
        Ok(live)
    }
}

impl Readings for LiveReadings {
    fn source(&self) -> &'static str {
        "live"
    }

    fn notes(&self) -> Vec<String> {
        vec![
            format!(
                "read_at: {} (unix {})",
                format_utc(self.read_at_unix),
                self.read_at_unix
            ),
            "ic reads are anonymous query calls with response verification on; evm reads are totalSupply at the numbered head shown per destination; xrpl reads are the issuer's icUSD obligations at the numbered validated ledger shown for the lane".to_string(),
        ]
    }

    fn icp_ledger_total_supply(&self) -> Result<u128, ReadError> {
        ic::ledger_total_supply(self.ic.as_ref(), &self.ledger)
    }

    fn icrc1_balance_of(&self, binding: &EscrowBinding) -> Result<u128, ReadError> {
        ic::ledger_balance_of(self.ic.as_ref(), &self.ledger, binding)
    }

    fn debt_backed_chain_supplies(&self) -> Result<u128, ReadError> {
        match &self.backend {
            Some((backend, chains)) => ic::backend_chain_supplies(self.ic.as_ref(), backend, chains),
            None => Err(ReadError::UnavailableGap(
                "the endpoint config names no backend canister, so the debt-backed chain supplies cannot be read".into(),
            )),
        }
    }

    fn instance_audit(&self, destination: &DestinationConfig) -> Result<InstanceAudit, ReadError> {
        let text = destination.instance.as_deref().ok_or_else(|| {
            ReadError::Failed(format!(
                "{} names no bridge instance canister",
                destination.network_id
            ))
        })?;
        let instance =
            parse_principal(text, "the bridge instance canister").map_err(ReadError::Failed)?;
        ic::instance_supply_audit(self.ic.as_ref(), &instance)
    }

    fn evm_total_supply(&self, network: &Network) -> Result<EvmSupply, ReadError> {
        match self.evm.get(&network.network_id) {
            Some(src) => evm::read_total_supply(src.transport.as_ref(), &src.token, network),
            None => Err(ReadError::UnavailableGap(format!(
                "the endpoint config has no token and providers for {}",
                network.network_id
            ))),
        }
    }

    fn xrpl_issued_outstanding(&self, network: &Network) -> Result<XrplIssued, ReadError> {
        match self.xrpl.get(&network.network_id) {
            Some(src) => xrpl::read_issued_outstanding(src.transport.as_ref(), &src.issuer, network),
            None => Err(ReadError::UnavailableGap(format!(
                "the endpoint config has no issuer and providers for {}",
                network.network_id
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// Time
// ---------------------------------------------------------------------------

/// `YYYY-MM-DDTHH:MM:SSZ` for a unix time in seconds (proleptic Gregorian,
/// civil-from-days).
pub fn format_utc(unix: u64) -> String {
    let days = (unix / 86_400) as i64;
    let secs = unix % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs / 3_600,
        (secs % 3_600) / 60,
        secs % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::{Catalog, DestinationConfig};
    use crate::collect::{collect, render, CollectConfig};
    use crate::live::evm::tests::{assert_numbered_or_finalized, network, Mock, Script};
    use crate::live::ic::IcQueryError;
    use candid::{CandidType, Deserialize as CandidDeserialize, Nat};
    use serde_json::{json, Value};
    use std::cell::RefCell;

    fn p(b: u8) -> Principal {
        Principal::from_slice(&[b, 0xAA])
    }

    fn binding_for(id: &str) -> EscrowBinding {
        EscrowBinding::from_principal_bytes(format!("syn-{id}").as_bytes(), None)
    }

    /// The synthetic bridge instance canister of the Base destination.
    fn instance_text() -> String {
        crate::escrow::principal_to_text(instance_principal().as_slice())
    }

    fn instance_principal() -> Principal {
        Principal::from_slice(b"synthetic-instance-base")
    }

    #[derive(CandidType, CandidDeserialize)]
    enum St {
        Registered,
    }
    #[derive(CandidType, CandidDeserialize)]
    struct Full {
        status: Option<St>,
        chain_supply_e8s: Nat,
        extra: bool,
    }

    /// A SupplyAudit-shaped reply (a superset of the fields the collector uses).
    #[derive(CandidType, CandidDeserialize)]
    struct AuditReply {
        escrow_account: ic::Icrc1Account,
        escrow_balance_e8s: Option<Nat>,
        outstanding_base_e8s: Nat,
        pending_mint_e8s: Nat,
        inflight_mint_e8s: Nat,
        pending_release_e8s: Nat,
        pending_refund_e8s: Nat,
        held_e8s: Nat,
        fees_e8s: Nat,
        reserved_pulls_e8s: Nat,
        unknown_outflow_e8s: Nat,
        liabilities_e8s: Nat,
        required_escrow_e8s: Nat,
        coverage_ok: Option<bool>,
        base_total_supply_e8s: Option<Nat>,
        base_supply_block: Option<u64>,
        reporting_note: String,
    }

    fn audit_reply(escrow: u128, supply: u128, outstanding: u128, liabilities: u128) -> Vec<u8> {
        let n = |v: u128| Nat::from(v);
        candid::encode_one(AuditReply {
            escrow_account: ic::Icrc1Account {
                owner: Principal::from_slice(b"syn-base-mainnet"),
                subaccount: None,
            },
            escrow_balance_e8s: Some(n(escrow)),
            outstanding_base_e8s: n(outstanding),
            pending_mint_e8s: n(0),
            inflight_mint_e8s: n(0),
            pending_release_e8s: n(0),
            pending_refund_e8s: n(0),
            held_e8s: n(0),
            fees_e8s: n(0),
            reserved_pulls_e8s: n(0),
            unknown_outflow_e8s: n(0),
            liabilities_e8s: n(liabilities),
            required_escrow_e8s: n(liabilities),
            coverage_ok: Some(true),
            base_total_supply_e8s: Some(n(supply)),
            base_supply_block: Some(999),
            reporting_note: "synthetic".into(),
        })
        .unwrap()
    }

    /// An IC that answers like a small ledger and backend.
    struct FakeIc {
        total: u128,
        balance: u128,
        chain_supply: u128,
        backend_has_query: bool,
        /// Liabilities the instance's audit reports.
        liabilities: u128,
        calls: RefCell<Vec<String>>,
    }
    impl ic::IcQuery for FakeIc {
        fn query(&self, c: &Principal, method: &str, _a: &[u8]) -> Result<Vec<u8>, IcQueryError> {
            self.calls
                .borrow_mut()
                .push(format!("{method}@{}", c.to_text()));
            match method {
                "get_supply_audit" => {
                    assert_eq!(
                        *c,
                        instance_principal(),
                        "audit asked of the wrong canister"
                    );
                    Ok(audit_reply(
                        self.balance,
                        self.balance,
                        self.balance,
                        self.liabilities,
                    ))
                }
                "icrc1_total_supply" => Ok(candid::encode_one(Nat::from(self.total)).unwrap()),
                "icrc1_balance_of" => Ok(candid::encode_one(Nat::from(self.balance)).unwrap()),
                "get_chain_public_launch_status" if self.backend_has_query => {
                    Ok(candid::encode_one(Full {
                        status: Some(St::Registered),
                        chain_supply_e8s: Nat::from(self.chain_supply),
                        extra: true,
                    })
                    .unwrap())
                }
                "get_chain_public_launch_status" => {
                    Err(IcQueryError::MethodNotFound("has no query method".into()))
                }
                other => Err(IcQueryError::Failed(format!("unexpected {other}"))),
            }
        }
    }

    fn provider(finalized: u64, supply: u128) -> Script {
        Box::new(move |method, params: &Value| {
            let hash = |n: u64| format!("0x{:064x}", n);
            match method {
                "eth_chainId" => Ok(json!("0x2105")),
                "eth_getBlockByNumber" => {
                    let tag = params[0].as_str().unwrap();
                    let n = if tag == "finalized" {
                        finalized
                    } else {
                        u64::from_str_radix(tag.trim_start_matches("0x"), 16).unwrap()
                    };
                    Ok(json!({"number": format!("0x{n:x}"), "hash": hash(n)}))
                }
                "eth_call" => Ok(json!(format!("0x{:064x}", supply))),
                other => Err(format!("unexpected {other}")),
            }
        })
    }

    fn live(ic: FakeIc, with_backend: bool) -> LiveReadings {
        let mut evm_map = BTreeMap::new();
        evm_map.insert(
            "base-mainnet".to_string(),
            EvmSource {
                token: [0x22; 20],
                transport: Box::new(Mock::new(vec![
                    provider(1000, 20_000_000_000),
                    provider(1000, 20_000_000_000),
                    provider(1000, 20_000_000_000),
                ])),
            },
        );
        let backend = with_backend.then(|| (p(2), vec![1030u32]));
        LiveReadings::new(Box::new(ic), p(1), backend, evm_map, 1_700_000_000)
    }

    fn base_config() -> CollectConfig {
        CollectConfig {
            destinations: vec![DestinationConfig {
                network_id: "base-mainnet".into(),
                in_d: true,
                escrow: Some(binding_for("base-mainnet")),
                instance: Some(instance_text()),
            }],
            xrpl: None,
        }
    }

    fn ic_ok() -> FakeIc {
        FakeIc {
            total: 100_000_000_000,
            balance: 20_000_000_000,
            chain_supply: 500_000_000,
            backend_has_query: true,
            liabilities: 20_000_000_000,
            calls: RefCell::new(Vec::new()),
        }
    }

    #[test]
    fn live_readings_drive_the_collector_and_label_the_report_live() {
        let readings = live(ic_ok(), true);
        let catalog = Catalog::embedded().unwrap();
        let got = collect(&base_config(), &catalog, &readings).unwrap();

        assert_eq!(got.source, "live");
        // total_icusd = ledger + debt-backed; escrow 200 locked, 200 on Base.
        assert_eq!(got.report.total_icusd, 100_500_000_000);
        assert_eq!(got.report.escrow_locked, 20_000_000_000);
        assert_eq!(got.report.circulating_on_icp, 80_000_000_000);
        assert_eq!(got.report.circulating_total, 100_500_000_000);
        assert_eq!(got.heads.len(), 1);
        assert_eq!(got.heads[0].block, 1000);

        let text = render(&got);
        assert!(text.starts_with("source: live\n"), "{text}");
        assert!(
            text.contains("read_at: 2023-11-14T22:13:20Z (unix 1700000000)"),
            "{text}"
        );
        assert!(
            text.contains("head base-mainnet: EvmTag block 1000"),
            "{text}"
        );
        assert!(!text.contains("source: fixture"), "{text}");
        assert!(!text.contains("not live data"), "{text}");
    }

    #[test]
    fn live_collection_reads_the_instance_audit_and_checks_c1_on_the_independent_read() {
        let readings = live(ic_ok(), true);
        let catalog = Catalog::embedded().unwrap();
        let got = collect(&base_config(), &catalog, &readings).unwrap();
        assert_eq!(got.audits.len(), 1);
        assert_eq!(got.audits[0].audit.liabilities_e8s, 20_000_000_000);
        assert_eq!(got.liabilities_total, 20_000_000_000);
        // The independent escrow read (200) covers liabilities (200).
        assert!(!got.report.has_failure());
        let text = render(&got);
        assert!(
            text.contains("instance audit base-mainnet (instance "),
            "{text}"
        );

        // Liabilities above the independent escrow break C1, whatever the
        // instance says about its own balance.
        let mut broken = ic_ok();
        broken.liabilities = 20_000_000_001;
        let readings = live(broken, true);
        let got = collect(&base_config(), &catalog, &readings).unwrap();
        assert!(got
            .report
            .failures()
            .any(|f| matches!(f.kind, crate::aggregate::FlagKind::CoverageBroken { .. })));
    }

    #[test]
    fn a_missing_instance_binding_makes_no_query_at_all() {
        let ic = ic_ok();
        let mut cfg = base_config();
        cfg.destinations[0].instance = None;
        let mut evm_map = BTreeMap::new();
        evm_map.insert(
            "base-mainnet".to_string(),
            EvmSource {
                token: [0x22; 20],
                transport: Box::new(Mock::new(vec![provider(1000, 1), provider(1000, 1)])),
            },
        );
        let readings = LiveReadings::new(Box::new(ic), p(1), None, evm_map, 0);
        let catalog = Catalog::embedded().unwrap();
        let err = collect(&cfg, &catalog, &readings).unwrap_err();
        assert!(matches!(
            err,
            crate::collect::CollectError::Rejected(
                crate::aggregate::AggregateError::MissingInstanceBinding(_)
            )
        ));
    }

    #[test]
    fn a_fixture_report_never_looks_live() {
        let fx = crate::collect::FixtureReadings::parse(include_str!(
            "../../fixtures/collect_base_only.json"
        ))
        .unwrap();
        let catalog = Catalog::embedded().unwrap();
        let got = collect(&fx.config, &catalog, &fx.readings).unwrap();
        let text = render(&got);
        assert!(text.starts_with("source: fixture\n"));
        assert!(!text.contains("source: live"));
        assert!(!text.contains("read_at"));
    }

    #[test]
    fn evm_requests_through_live_readings_are_numbered_or_finalized_only() {
        let mock = Mock::new(vec![
            provider(1000, 1),
            provider(1000, 1),
            provider(1000, 1),
        ]);
        let net = network("base-mainnet");
        let t: &dyn evm::JsonRpcTransport = &mock;
        evm::read_total_supply(t, &[0x22; 20], &net).unwrap();
        assert_numbered_or_finalized(&mock);
    }

    #[test]
    fn a_backend_without_the_query_makes_the_debt_backed_reading_unavailable() {
        let mut ic = ic_ok();
        ic.backend_has_query = false;
        let readings = live(ic, true);
        let catalog = Catalog::embedded().unwrap();
        let err = collect(&base_config(), &catalog, &readings).unwrap_err();
        match err {
            crate::collect::CollectError::Read {
                error: ReadError::UnavailableGap(g),
                what,
            } => {
                assert!(g.contains("get_chain_public_launch_status"), "{g}");
                assert!(what.contains("debt-backed"), "{what}");
            }
            other => panic!("expected an unavailable gap, got {other:?}"),
        }
    }

    #[test]
    fn no_backend_in_the_config_is_unavailable_not_zero() {
        let readings = live(ic_ok(), false);
        let catalog = Catalog::embedded().unwrap();
        let err = collect(&base_config(), &catalog, &readings).unwrap_err();
        assert!(
            matches!(
                &err,
                crate::collect::CollectError::Read { error: ReadError::UnavailableGap(g), .. }
                    if g.contains("no backend canister")
            ),
            "{err:?}"
        );
    }

    #[test]
    fn an_evm_destination_without_a_configured_source_is_unavailable() {
        let readings = live(ic_ok(), true);
        let catalog = Catalog::embedded().unwrap();
        let net = catalog.lookup("ethereum-mainnet").unwrap().1.clone();
        assert!(matches!(
            readings.evm_total_supply(&net),
            Err(ReadError::UnavailableGap(g)) if g.contains("ethereum-mainnet")
        ));
    }

    #[test]
    fn collection_still_refuses_solana_and_the_lane_under_live_readings() {
        let readings = live(ic_ok(), true);
        let catalog = Catalog::embedded().unwrap();
        let cfg = CollectConfig {
            destinations: vec![DestinationConfig {
                network_id: "solana-mainnet".into(),
                in_d: true,
                escrow: Some(binding_for("solana-mainnet")),
                instance: Some(instance_text()),
            }],
            xrpl: None,
        };
        let err = collect(&cfg, &catalog, &readings).unwrap_err();
        assert!(err.to_string().contains("BB-49"), "{err}");
        // Nothing was read on the IC side before the refusal.
    }

    /// An IC that answers `icrc1_balance_of` per escrow owner, so the D escrow
    /// and the XRPL lane escrow can differ.
    struct TwoEscrowIc {
        total: u128,
        d_balance: u128,
        x_balance: u128,
        calls: RefCell<Vec<String>>,
    }
    impl ic::IcQuery for TwoEscrowIc {
        fn query(&self, c: &Principal, method: &str, a: &[u8]) -> Result<Vec<u8>, IcQueryError> {
            self.calls.borrow_mut().push(format!("{method}@{}", c.to_text()));
            match method {
                "icrc1_balance_of" => {
                    let account: ic::Icrc1Account = candid::decode_one(a).unwrap();
                    let balance = if account.owner.as_slice() == b"syn-xrpl-mainnet" {
                        self.x_balance
                    } else {
                        self.d_balance
                    };
                    Ok(candid::encode_one(Nat::from(balance)).unwrap())
                }
                "icrc1_total_supply" => Ok(candid::encode_one(Nat::from(self.total)).unwrap()),
                "get_supply_audit" => Ok(audit_reply(
                    self.d_balance,
                    self.d_balance,
                    self.d_balance,
                    self.d_balance,
                )),
                "get_chain_public_launch_status" => Ok(candid::encode_one(Full {
                    status: Some(St::Registered),
                    chain_supply_e8s: Nat::from(0u8),
                    extra: true,
                })
                .unwrap()),
                other => Err(IcQueryError::Failed(format!("unexpected {other}"))),
            }
        }
    }

    fn xrpl_source(providers: Vec<xrpl::tests::P>) -> XrplSource {
        XrplSource {
            issuer: xrpl::tests::ISSUER.to_string(),
            transport: Box::new(xrpl::tests::Mock::new(
                providers.into_iter().map(xrpl::tests::P::script).collect(),
            )),
        }
    }

    fn lane_config() -> CollectConfig {
        let mut cfg = base_config();
        cfg.xrpl = Some(crate::aggregate::XrplLaneConfig {
            network_id: "xrpl-mainnet".into(),
            escrow: binding_for("xrpl-mainnet"),
        });
        cfg
    }

    fn live_with_lane(ic: TwoEscrowIc, providers: Vec<xrpl::tests::P>) -> LiveReadings {
        live(
            FakeIc {
                total: ic.total,
                balance: ic.d_balance,
                chain_supply: 0,
                backend_has_query: true,
                liabilities: ic.d_balance,
                calls: RefCell::new(Vec::new()),
            },
            true,
        )
        .with_ic(Box::new(ic))
        .with_xrpl("xrpl-mainnet", xrpl_source(providers))
    }

    #[test]
    fn an_end_to_end_live_collection_counts_d_and_x_escrow_once_each() {
        let ic = TwoEscrowIc {
            total: 100_000_000_000,
            d_balance: 20_000_000_000,
            x_balance: 5_000_000_000,
            calls: RefCell::new(Vec::new()),
        };
        let readings = live_with_lane(
            ic,
            vec![
                xrpl::tests::P::mainnet(91_000_000, "50"),
                xrpl::tests::P::mainnet(91_000_000, "50"),
                xrpl::tests::P::mainnet(91_000_005, "50"),
            ],
        );
        let catalog = Catalog::embedded().unwrap();
        let got = collect(&lane_config(), &catalog, &readings).unwrap();
        let r = &got.report;
        assert_eq!(got.source, "live");
        // E_d = 200 and E_X = 50, each counted once.
        assert_eq!(r.escrow_locked, 25_000_000_000);
        assert_eq!(r.circulating_on_icp, 75_000_000_000);
        assert_eq!(r.circulating_dest[0].circulating, 20_000_000_000);
        assert_eq!(r.circulating_xrpl, 5_000_000_000);
        assert_eq!(r.circulating_total, 100_000_000_000);
        assert!(!r.has_failure(), "{:?}", r.flags);
        // The lane was read at the quorum-agreed validated ledger.
        let lane = got.xrpl_read.as_ref().unwrap();
        assert_eq!((lane.ledger_index, lane.finality), (91_000_000, crate::types::FinalityRule::XrplValidated));

        let text = render(&got);
        assert!(text.starts_with("source: live\n"), "{text}");
        assert!(text.contains("head xrpl-mainnet: XrplValidated block 91000000"), "{text}");
        assert!(text.contains("xrpl lane xrpl-mainnet: escrow 50.00000000, issued outstanding 50.00000000"), "{text}");
        assert!(!text.contains("source: fixture"), "{text}");
    }

    #[test]
    fn a_lane_without_a_configured_xrpl_source_is_unavailable_not_zero() {
        let readings = live(ic_ok(), true);
        let catalog = Catalog::embedded().unwrap();
        let err = collect(&lane_config(), &catalog, &readings).unwrap_err();
        assert!(
            matches!(
                &err,
                crate::collect::CollectError::Read { error: ReadError::UnavailableGap(g), .. }
                    if g.contains("xrpl-mainnet")
            ),
            "{err:?}"
        );
    }

    #[test]
    fn a_failed_xrpl_read_stops_the_collection_before_any_ic_query() {
        let ic = TwoEscrowIc {
            total: 1,
            d_balance: 1,
            x_balance: 1,
            calls: RefCell::new(Vec::new()),
        };
        // Two providers disagree: no quorum.
        let readings = live_with_lane(
            ic,
            vec![
                xrpl::tests::P::mainnet(500, "1"),
                xrpl::tests::P::mainnet(500, "2"),
            ],
        );
        let catalog = Catalog::embedded().unwrap();
        let err = collect(&lane_config(), &catalog, &readings).unwrap_err();
        assert!(err.to_string().contains("no quorum"), "{err}");
    }

    #[test]
    fn xrpl_endpoint_config_is_strict_and_builds_without_sending_anything() {
        let good = r#"{"ic": {"url": "https://ic.example.invalid", "ledger": "ryjl3-tyaaa-aaaaa-aaaba-cai"},
                      "xrpl": {"xrpl-mainnet": {"issuer": "rFixtureTestAccountzzzzzzzzzzzzz",
                                                "providers": ["https://a.example.invalid",
                                                              "https://b.example.invalid"]}}}"#;
        let c: LiveConfig = serde_json::from_str(good).unwrap();
        assert_eq!(c.xrpl.len(), 1);
        let built = LiveReadings::from_config(&c, 0).unwrap();
        assert_eq!(built.source(), "live");
        // Unknown fields and a missing issuer are refused; nothing defaults.
        for bad in [
            r#"{"ic":{"url":"u","ledger":"l"},"xrpl":{"xrpl-mainnet":{"issuer":"r","providers":[],"x":1}}}"#,
            r#"{"ic":{"url":"u","ledger":"l"},"xrpl":{"xrpl-mainnet":{"providers":[]}}}"#,
        ] {
            assert!(serde_json::from_str::<LiveConfig>(bad).is_err(), "{bad}");
        }
        // A malformed issuer, a plain-http provider and no provider all fail to build.
        for (issuer, providers) in [
            ("not-an-address", r#"["https://a.example.invalid"]"#),
            ("rFixtureTestAccountzzzzzzzzzzzzz", r#"["http://example.invalid"]"#),
            ("rFixtureTestAccountzzzzzzzzzzzzz", "[]"),
        ] {
            let text = format!(
                r#"{{"ic": {{"url": "https://ic.example.invalid", "ledger": "ryjl3-tyaaa-aaaaa-aaaba-cai"}},
                    "xrpl": {{"xrpl-mainnet": {{"issuer": "{issuer}", "providers": {providers}}}}}}}"#
            );
            let c: LiveConfig = serde_json::from_str(&text).unwrap();
            assert!(LiveReadings::from_config(&c, 0).is_err(), "{issuer} {providers}");
        }
    }

    #[test]
    fn utc_formatting_matches_known_instants() {
        assert_eq!(format_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_utc(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(format_utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(format_utc(4_102_444_799), "2099-12-31T23:59:59Z");
    }

    #[test]
    fn endpoint_config_is_strict_and_has_no_defaults() {
        let ok = r#"{
            "ic": {"url": "https://ic.example.invalid", "ledger": "aaaaa-aa"},
            "evm": {"base-mainnet": {"token": "0x1111111111111111111111111111111111111111",
                                       "providers": ["https://a.example.invalid"]}}
        }"#;
        let c: LiveConfig = serde_json::from_str(ok).unwrap();
        assert!(c.ic.backend.is_none());
        assert_eq!(c.evm.len(), 1);
        // Unknown fields are refused, and ic is required (no built-in endpoint).
        assert!(
            serde_json::from_str::<LiveConfig>(r#"{"ic":{"url":"u","ledger":"l","x":1}}"#).is_err()
        );
        assert!(serde_json::from_str::<LiveConfig>("{}").is_err());

        assert!(parse_token("0x1111111111111111111111111111111111111111").is_ok());
        assert!(parse_token("1111111111111111111111111111111111111111").is_err());
        assert!(parse_token("0x11").is_err());
        assert!(parse_principal("not a principal", "x").is_err());
        assert!(parse_principal("ryjl3-tyaaa-aaaaa-aaaba-cai", "x").is_ok());
        // Building the real clients sends nothing, but still validates URLs.
        let bad: LiveConfig = serde_json::from_str(
            r#"{"ic": {"url": "http://example.invalid", "ledger": "ryjl3-tyaaa-aaaaa-aaaba-cai"}}"#,
        )
        .unwrap();
        assert!(LiveReadings::from_config(&bad, 0).is_err());
        // A well-formed config builds the real clients without sending anything.
        let good: LiveConfig = serde_json::from_str(
            r#"{"ic": {"url": "https://ic.example.invalid",
                        "ledger": "ryjl3-tyaaa-aaaaa-aaaba-cai",
                        "backend": {"canister": "ryjl3-tyaaa-aaaaa-aaaba-cai",
                                    "debt_backed_chain_ids": [1]}},
                "evm": {"base-mainnet": {"token": "0x1111111111111111111111111111111111111111",
                                           "providers": ["https://a.example.invalid",
                                                         "https://b.example.invalid"]}}}"#,
        )
        .unwrap();
        let built = LiveReadings::from_config(&good, 1_700_000_000).unwrap();
        assert_eq!(built.source(), "live");
    }
}
