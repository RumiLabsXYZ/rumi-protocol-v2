//! Profile validator, rules V1 to V11 (design section 9.4).
//!
//! `validate` takes `today` as a parameter and never reads a clock. It returns
//! every violation it finds, each with its rule id.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde_json::Value;

use crate::label;
use crate::types::*;

/// The nine profile ids that must be present exactly once (V11).
pub const REQUIRED_PROFILE_IDS: [&str; 9] = [
    "ethereum",
    "base",
    "arbitrum",
    "robinhood",
    "hyperevm",
    "monad",
    "solana",
    "sui",
    "xrpl",
];

/// Live Base signing and identity strings (design section 5.1). Only the
/// `base` profile may carry them; any other profile reusing one is rejected (V6).
const LIVE_BASE_STRINGS: [&str; 6] = [
    "rumi.icusd.base.mint.v1",
    "rumi.icusd.base.remint.v1",
    "rumi.icusd.base.bridge-out.v1",
    "rumi-icusd-base-bridge:escrow:v1",
    "rumi-icusd-base-bridge",
    "rumi-base-icusd-bridge-v1",
];

/// The template element every derivation domain must contain (V6).
const IDENTITY_ELEMENT: &str = "{network_identity}";
/// ... or, for XRPL only, the network name that PR #586 puts in its path.
const NAME_ELEMENT: &str = "{network_name}";
/// Elements that a signing role fills.
const ROLE_ELEMENTS: [&str; 2] = ["{lane}", "{role}"];

/// Blockers that explain a provider shortage (V9).
const PROVIDER_SHORTAGE_BLOCKERS: [&str; 2] =
    ["single-keyless-endpoint", "sui-rpc-provider-contract"];

/// One rule failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub rule: &'static str,
    pub message: String,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.rule, self.message)
    }
}

type Out = Vec<Violation>;
type Date = (u32, u32, u32);

fn add(out: &mut Out, rule: &'static str, message: String) {
    out.push(Violation { rule, message });
}

/// Check all profiles against rules V1 to V11. `today` is `YYYY-MM-DD`.
pub fn validate(profiles: &[Profile], today: &str) -> Result<(), Vec<Violation>> {
    let mut out = Vec::new();
    let today_date = parse_date(today);
    if today_date.is_none() {
        add(
            &mut out,
            "V4",
            format!("today is not a valid YYYY-MM-DD date: {today:?}"),
        );
    }
    v1_identity(profiles, &mut out);
    v2_disabled(profiles, &mut out);
    v3_representation(profiles, &mut out);
    v4_facts(profiles, today_date, &mut out);
    v5_family(profiles, &mut out);
    v6_signing(profiles, &mut out);
    v7_blockers(profiles, &mut out);
    v8_xrpl(profiles, &mut out);
    v9_rpc(profiles, &mut out);
    v10_hygiene(profiles, &mut out);
    v11_nine(profiles, &mut out);
    if out.is_empty() {
        Ok(())
    } else {
        Err(out)
    }
}

// ---------------------------------------------------------------------------
// V1: schema version, ids, identity uniqueness
// ---------------------------------------------------------------------------

/// `^[a-z][a-z0-9-]{1,23}$`
fn valid_slug(s: &str) -> bool {
    let b = s.as_bytes();
    (2..=24).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// `D1` to `D9`, a lower-case sub-decision such as `D1a`, or a coupling such as `D5xD8`.
fn valid_decision_id(id: &str) -> bool {
    let b = id.as_bytes();
    let digit = |c: u8| (b'1'..=b'9').contains(&c);
    match b.len() {
        2 => b[0] == b'D' && digit(b[1]),
        3 => b[0] == b'D' && digit(b[1]) && b[2].is_ascii_lowercase(),
        5 => b[0] == b'D' && digit(b[1]) && b[2] == b'x' && b[3] == b'D' && digit(b[4]),
        _ => false,
    }
}

fn v1_identity(profiles: &[Profile], out: &mut Out) {
    let mut profile_ids = BTreeSet::new();
    let mut network_ids = BTreeSet::new();
    let mut identities = BTreeSet::new();
    for p in profiles {
        if p.schema_version != SCHEMA_VERSION {
            add(
                out,
                "V1",
                format!("{}: unknown schema_version {}", p.id, p.schema_version),
            );
        }
        if !valid_slug(&p.id) {
            add(
                out,
                "V1",
                format!(
                    "profile id {:?} does not match ^[a-z][a-z0-9-]{{1,23}}$",
                    p.id
                ),
            );
        }
        if !profile_ids.insert(p.id.clone()) {
            add(out, "V1", format!("duplicate profile id {}", p.id));
        }
        if !p.networks.iter().any(|n| n.role == Role::Mainnet) {
            add(out, "V1", format!("{}: no mainnet network record", p.id));
        }
        let mut decision_ids = BTreeSet::new();
        for d in &p.open_decisions {
            if d.id == "D6" && d.choice.status == DecisionStatus::PlanningDecision {
                add(out, "V1", format!("{}: D6 (controllers and custody) stays open and cannot be a planning decision", p.id));
            }
            if !valid_decision_id(&d.id) {
                add(out, "V1", format!("{}: open decision id {:?} is not D1 to D9, a D1a style sub-decision or D5xD8 style coupling", p.id, d.id));
            }
            if !decision_ids.insert(d.id.clone()) {
                add(
                    out,
                    "V1",
                    format!("{}: duplicate open decision {}", p.id, d.id),
                );
            }
        }
        for n in &p.networks {
            if !valid_slug(&n.network_id) {
                add(
                    out,
                    "V1",
                    format!(
                        "network id {:?} does not match the slug pattern",
                        n.network_id
                    ),
                );
            }
            if !n.network_id.starts_with(&format!("{}-", p.id)) {
                add(
                    out,
                    "V1",
                    format!("network id {} does not start with {}-", n.network_id, p.id),
                );
            }
            if !network_ids.insert(n.network_id.clone()) {
                add(out, "V1", format!("duplicate network id {}", n.network_id));
            }
            // PR #586's XrplNetwork has only Mainnet and Testnet, so no XRPL devnet record can be served.
            if n.role == Role::Devnet && !matches!(p.family, Family::Solana | Family::Sui) {
                add(
                    out,
                    "V1",
                    format!(
                        "{}: devnet is only allowed for solana and sui",
                        n.network_id
                    ),
                );
            }
            let key = (
                label(&n.identity.value.kind),
                n.identity.value.value.clone(),
            );
            if !identities.insert(key.clone()) {
                add(
                    out,
                    "V1",
                    format!(
                        "{}: identity ({}, {}) is not unique across profiles",
                        n.network_id, key.0, key.1
                    ),
                );
            }
            let mut blocker_ids = BTreeSet::new();
            for b in &n.activation_blockers {
                if !blocker_ids.insert(b.id.clone()) {
                    add(
                        out,
                        "V1",
                        format!("{}: duplicate blocker id {}", n.network_id, b.id),
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// V2: everything stays disabled
// ---------------------------------------------------------------------------

fn v2_disabled(profiles: &[Profile], out: &mut Out) {
    for p in profiles {
        for n in &p.networks {
            if n.status != Status::Disabled {
                add(
                    out,
                    "V2",
                    format!(
                        "{}: status is {}, only disabled is accepted",
                        n.network_id,
                        label(&n.status)
                    ),
                );
            }
            if n.instance_ref.is_some() {
                add(
                    out,
                    "V2",
                    format!("{}: instance_ref must be null", n.network_id),
                );
            }
            let c = &n.claims;
            if c.deployment != Deployment::None
                || c.mint_authority != MintAuthority::None
                || c.mint_enabled
                || c.finality_checked
                || c.settlement_proven
            {
                add(
                    out,
                    "V2",
                    format!("{}: claims are not at groundwork defaults", n.network_id),
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// V3: representation and amount bounds
// ---------------------------------------------------------------------------

const XRPL_DECIMAL_15_MAX: u64 = 999_999_999_999_999;

fn v3_representation(profiles: &[Profile], out: &mut Out) {
    for p in profiles {
        let r = &p.representation;
        if r.decimals != 8 {
            add(
                out,
                "V3",
                format!("{}: decimals is {}, must be 8", p.id, r.decimals),
            );
        }
        if r.debt_creating {
            add(out, "V3", format!("{}: debt_creating must be false", p.id));
        }
        if r.backing != Backing::EscrowLockedIcusd {
            add(
                out,
                "V3",
                format!("{}: backing must be escrow_locked_icusd", p.id),
            );
        }
        let b = &r.amount_bounds.value;
        if b.min_e8s < 1 {
            add(
                out,
                "V3",
                format!("{}: amount_bounds.min_e8s must be at least 1", p.id),
            );
        }
        if b.max_e8s < b.min_e8s {
            add(
                out,
                "V3",
                format!("{}: amount_bounds.max_e8s is below min_e8s", p.id),
            );
        }
        let (want_encoding, ceiling) = match r.standard.value {
            Standard::Erc20 | Standard::SplToken2022 | Standard::SplToken => {
                (Encoding::U64, u64::MAX)
            }
            // Sui `mint_balance` aborts when supply would reach U64_MAX.
            Standard::SuiCurrency => (Encoding::U64, u64::MAX - 1),
            Standard::XrplMpt => (Encoding::Int63, i64::MAX as u64),
            Standard::XrplIou => (Encoding::XrplDecimal15, XRPL_DECIMAL_15_MAX),
        };
        if b.encoding != want_encoding {
            add(
                out,
                "V3",
                format!(
                    "{}: amount encoding {} does not match standard {}",
                    p.id,
                    label(&b.encoding),
                    label(&r.standard.value)
                ),
            );
        }
        match (b.encoding, b.max_outstanding_e8s) {
            (Encoding::XrplDecimal15, None) => {
                add(out, "V3", format!("{}: xrpl_decimal_15 needs max_outstanding_e8s (15 significant digits on total outstanding)", p.id));
            }
            (_, Some(total)) => {
                if total > ceiling {
                    add(out, "V3", format!("{}: amount_bounds.max_outstanding_e8s {total} exceeds the {} ceiling {ceiling}", p.id, label(&b.encoding)));
                }
                if total < b.min_e8s {
                    add(
                        out,
                        "V3",
                        format!(
                            "{}: amount_bounds.max_outstanding_e8s is below min_e8s",
                            p.id
                        ),
                    );
                }
            }
            (_, None) => {}
        }
        if b.max_e8s > ceiling {
            add(
                out,
                "V3",
                format!(
                    "{}: amount_bounds.max_e8s {} exceeds the {} ceiling {}",
                    p.id,
                    b.max_e8s,
                    label(&b.encoding),
                    ceiling
                ),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// V4: facts carry a source and a date
// ---------------------------------------------------------------------------

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
                29
            } else {
                28
            }
        }
    }
}

/// Strict `YYYY-MM-DD` with a real calendar day.
fn parse_date(s: &str) -> Option<Date> {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    if !b
        .iter()
        .enumerate()
        .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
    {
        return None;
    }
    let y: u32 = s[0..4].parse().ok()?;
    let m: u32 = s[5..7].parse().ok()?;
    let d: u32 = s[8..10].parse().ok()?;
    if y < 1970 || !(1..=12).contains(&m) || d < 1 || d > days_in_month(y, m) {
        return None;
    }
    Some((y, m, d))
}

fn is_https_url(u: &str) -> bool {
    match u.strip_prefix("https://") {
        Some(rest) => {
            let host = rest.split(['/', '?', '#']).next().unwrap_or("");
            !host.is_empty() && !host.contains('@') && !u.chars().any(char::is_whitespace)
        }
        None => false,
    }
}

fn is_repo_path(u: &str) -> bool {
    !u.is_empty()
        && !u.starts_with('/')
        && !u.starts_with('~')
        && !u.contains("://")
        && !u.contains("..")
        && !u.contains('\\')
        && !u.chars().any(char::is_whitespace)
}

struct FactMeta<'a> {
    url: &'a str,
    verified: &'a str,
    kind: SourceKind,
}

fn meta<T>(f: &Fact<T>) -> FactMeta<'_> {
    FactMeta {
        url: &f.source_url,
        verified: &f.verified,
        kind: f.source_kind,
    }
}

/// Every fact in a profile, with its path and whether it is an identity field.
fn collect_facts(p: &Profile) -> Vec<(String, FactMeta<'_>, bool)> {
    let mut v = Vec::new();
    if let Some(f) = &p.topology.existing_work {
        v.push((format!("{}.topology.existing_work", p.id), meta(f), false));
    }
    v.push((
        format!("{}.representation.amount_bounds", p.id),
        meta(&p.representation.amount_bounds),
        false,
    ));
    v.push((
        format!("{}.signing.key_name", p.id),
        meta(&p.signing.key_name),
        false,
    ));
    v.push((
        format!("{}.signing.signed_payload", p.id),
        meta(&p.signing.signed_payload),
        false,
    ));
    v.push((
        format!("{}.wallet.named", p.id),
        meta(&p.wallet.named),
        false,
    ));
    for (i, f) in p.prerequisites.iter().enumerate() {
        v.push((format!("{}.prerequisites[{i}]", p.id), meta(f), false));
    }
    for n in &p.networks {
        let id = &n.network_id;
        v.push((format!("{id}.identity"), meta(&n.identity), true));
        v.push((
            format!("{id}.finality.parameters"),
            meta(&n.finality.parameters),
            false,
        ));
        if let Some(f) = &n.rpc.native_variant {
            v.push((format!("{id}.rpc.native_variant"), meta(f), false));
        }
        v.push((format!("{id}.rpc.endpoints"), meta(&n.rpc.endpoints), false));
        v.push((
            format!("{id}.rpc.outcall_notes"),
            meta(&n.rpc.outcall_notes),
            false,
        ));
    }
    v
}

fn v4_facts(profiles: &[Profile], today: Option<Date>, out: &mut Out) {
    for p in profiles {
        for (path, m, is_identity) in collect_facts(p) {
            let url_ok = if m.kind == SourceKind::Repo {
                is_repo_path(m.url)
            } else {
                is_https_url(m.url)
            };
            if !url_ok {
                let want = if m.kind == SourceKind::Repo {
                    "a repo-relative path"
                } else {
                    "an https URL"
                };
                add(out, "V4", format!("{path}: source_url is not {want}"));
            }
            match parse_date(m.verified) {
                None => add(
                    out,
                    "V4",
                    format!(
                        "{path}: verified date {:?} is not a valid YYYY-MM-DD",
                        m.verified
                    ),
                ),
                Some(d) => {
                    if let Some(t) = today {
                        if d > t {
                            add(
                                out,
                                "V4",
                                format!("{path}: verified date {} is after today", m.verified),
                            );
                        }
                    }
                }
            }
            if is_identity && m.kind != SourceKind::Primary {
                add(
                    out,
                    "V4",
                    format!(
                        "{path}: identity fields must be primary, found {}",
                        label(&m.kind)
                    ),
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// V5: family consistency
// ---------------------------------------------------------------------------

fn is_base58(s: &str) -> bool {
    const B58: &str = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    !s.is_empty() && s.chars().all(|c| B58.contains(c))
}

/// The execution delay is unresolved only when a blocker says so.
const MONAD_DELAY_BLOCKER: &str = "monad-execution-delay-unknown";

/// Read the parameter values, not just the rule name: each rule has exactly
/// one valid shape and every other family's fields must stay empty.
fn v5_finality_parameters(n: &Network, out: &mut Out) {
    let id = &n.network_id;
    let fact = &n.finality.parameters;
    let f = &fact.value;
    let rule = n.finality.rule;
    let finalized = Some("finalized");
    let (tag, commitment, checkpoint, ledger_index, required_result) = match rule {
        FinalityRule::EvmTag | FinalityRule::MonadVerified => (finalized, None, None, None, None),
        FinalityRule::SolanaCommitment => (None, finalized, None, None, None),
        FinalityRule::SuiCheckpoint => (None, None, finalized, None, None),
        FinalityRule::XrplValidated => (None, None, None, Some("validated"), Some("tesSUCCESS")),
    };
    let fields = [
        ("tag", &f.tag, tag),
        ("commitment", &f.commitment, commitment),
        ("checkpoint", &f.checkpoint, checkpoint),
        ("ledger_index", &f.ledger_index, ledger_index),
        ("required_result", &f.required_result, required_result),
    ];
    for (name, got, want) in fields {
        if got.as_deref() != want {
            add(
                out,
                "V5",
                format!(
                    "{id}: finality parameter {name} is {got:?}, rule {} requires {want:?}",
                    label(&rule)
                ),
            );
        }
    }
    match (rule, f.execution_delay_blocks) {
        (FinalityRule::MonadVerified, Some(0)) => {
            add(
                out,
                "V5",
                format!("{id}: monad execution_delay_blocks must be positive, found 0"),
            );
        }
        (FinalityRule::MonadVerified, Some(_)) => {
            if fact.source_kind != SourceKind::Primary {
                add(
                    out,
                    "V5",
                    format!("{id}: monad execution_delay_blocks must come from a primary source"),
                );
            }
        }
        (FinalityRule::MonadVerified, None) => {
            if !n
                .activation_blockers
                .iter()
                .any(|b| b.id == MONAD_DELAY_BLOCKER)
            {
                add(
                    out,
                    "V5",
                    format!("{id}: monad execution_delay_blocks is missing and no {MONAD_DELAY_BLOCKER} blocker is present"),
                );
            }
        }
        (_, Some(_)) => {
            add(
                out,
                "V5",
                format!("{id}: execution_delay_blocks only applies to monad_verified"),
            );
        }
        (_, None) => {}
    }
}

fn v5_family(profiles: &[Profile], out: &mut Out) {
    for p in profiles {
        let (scheme, standards, identity_kind, rule, path): (_, &[Standard], _, _, _) =
            match p.family {
                Family::Evm => (
                    Scheme::EcdsaSecp256k1,
                    &[Standard::Erc20],
                    IdentityKind::EvmChainId,
                    FinalityRule::EvmTag,
                    RpcPath::EvmRpcCanister,
                ),
                Family::Solana => (
                    Scheme::SchnorrEd25519,
                    &[Standard::SplToken2022, Standard::SplToken],
                    IdentityKind::SolanaGenesisHash,
                    FinalityRule::SolanaCommitment,
                    RpcPath::SolRpcCanister,
                ),
                Family::Sui => (
                    Scheme::SchnorrEd25519,
                    &[Standard::SuiCurrency],
                    IdentityKind::SuiChainIdentifier,
                    FinalityRule::SuiCheckpoint,
                    RpcPath::HttpsOutcallGraphql,
                ),
                Family::Xrpl => (
                    Scheme::SchnorrEd25519,
                    &[Standard::XrplIou, Standard::XrplMpt],
                    IdentityKind::XrplNetworkId,
                    FinalityRule::XrplValidated,
                    RpcPath::HttpsOutcallJsonrpc,
                ),
            };
        if p.signing.scheme != scheme {
            add(
                out,
                "V5",
                format!(
                    "{}: signing scheme {} does not match family {}",
                    p.id,
                    label(&p.signing.scheme),
                    label(&p.family)
                ),
            );
        }
        if !standards.contains(&p.representation.standard.value) {
            add(
                out,
                "V5",
                format!(
                    "{}: token standard {} does not match family {}",
                    p.id,
                    label(&p.representation.standard.value),
                    label(&p.family)
                ),
            );
        }
        for n in &p.networks {
            let id = &n.network_id;
            if n.identity.value.kind != identity_kind {
                add(
                    out,
                    "V5",
                    format!(
                        "{id}: identity kind {} does not match family {}",
                        label(&n.identity.value.kind),
                        label(&p.family)
                    ),
                );
            }
            let value = &n.identity.value.value;
            let value_ok = match n.identity.value.kind {
                IdentityKind::EvmChainId => {
                    value.parse::<u64>().map(|v| v > 0).unwrap_or(false) && !value.starts_with('0')
                }
                IdentityKind::XrplNetworkId => value.parse::<u32>().is_ok(),
                IdentityKind::SolanaGenesisHash | IdentityKind::SuiChainIdentifier => {
                    is_base58(value)
                }
            };
            if !value_ok {
                add(
                    out,
                    "V5",
                    format!(
                        "{id}: identity value is not well formed for {}",
                        label(&n.identity.value.kind)
                    ),
                );
            }
            if p.family == Family::Xrpl {
                let want_id = match n.role {
                    Role::Mainnet => "0",
                    Role::Testnet => "1",
                    Role::Devnet => "2",
                };
                if value != want_id {
                    add(
                        out,
                        "V5",
                        format!(
                            "{id}: xrpl network id {value} does not match role {}",
                            label(&n.role)
                        ),
                    );
                }
            }
            // Monad must use the verified head; no other chain may.
            let want_rule = if p.id == "monad" {
                FinalityRule::MonadVerified
            } else {
                rule
            };
            if n.finality.rule != want_rule {
                add(
                    out,
                    "V5",
                    format!(
                        "{id}: finality rule {} must be {}",
                        label(&n.finality.rule),
                        label(&want_rule)
                    ),
                );
            }
            v5_finality_parameters(n, out);
            if n.rpc.path != path {
                add(
                    out,
                    "V5",
                    format!(
                        "{id}: rpc path {} does not match family {}",
                        label(&n.rpc.path),
                        label(&p.family)
                    ),
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// V6: signing keys and derivation domains
// ---------------------------------------------------------------------------

fn v6_signing(profiles: &[Profile], out: &mut Out) {
    let live: BTreeSet<&str> = LIVE_BASE_STRINGS.into_iter().collect();
    let mut expanded_paths: BTreeMap<String, String> = BTreeMap::new();
    let mut domain_owners: BTreeMap<String, String> = BTreeMap::new();
    for p in profiles {
        let s = &p.signing;
        if s.key_name.value.mainnet != "key_1" {
            add(
                out,
                "V6",
                format!(
                    "{}: mainnet key must be key_1, found {:?}",
                    p.id, s.key_name.value.mainnet
                ),
            );
        }
        if s.key_name.value.testnet != "test_key_1" {
            add(
                out,
                "V6",
                format!(
                    "{}: testnet and devnet key must be test_key_1, found {:?}",
                    p.id, s.key_name.value.testnet
                ),
            );
        }
        let (want, forbidden) = if p.family == Family::Xrpl {
            (NAME_ELEMENT, IDENTITY_ELEMENT)
        } else {
            (IDENTITY_ELEMENT, NAME_ELEMENT)
        };
        if s.derivation_domain.iter().filter(|e| *e == want).count() != 1 {
            add(
                out,
                "V6",
                format!(
                    "{}: derivation_domain must contain {want} exactly once",
                    p.id
                ),
            );
        }
        if s.derivation_domain.iter().any(|e| e == forbidden) {
            add(
                out,
                "V6",
                format!(
                    "{}: derivation_domain must not contain {forbidden} for family {}",
                    p.id,
                    label(&p.family)
                ),
            );
        }
        let role_elements = s
            .derivation_domain
            .iter()
            .filter(|e| ROLE_ELEMENTS.contains(&e.as_str()))
            .count();
        if role_elements > 1 {
            add(
                out,
                "V6",
                format!("{}: derivation_domain has more than one role element", p.id),
            );
        }
        if role_elements == 1 && s.roles.is_empty() {
            add(
                out,
                "V6",
                format!(
                    "{}: derivation_domain has a role element but no roles are listed",
                    p.id
                ),
            );
        }
        if role_elements == 0 && !s.roles.is_empty() {
            add(
                out,
                "V6",
                format!(
                    "{}: roles are listed but derivation_domain has no role element",
                    p.id
                ),
            );
        }
        let mut seen_roles = BTreeSet::new();
        for r in &s.roles {
            if !valid_slug(r) || !seen_roles.insert(r.clone()) {
                add(
                    out,
                    "V6",
                    format!("{}: role {r:?} is not a unique lower-case slug", p.id),
                );
            }
        }
        let mut domain_strings: Vec<&String> = s.derivation_domain.iter().collect();
        domain_strings.extend(
            [
                &s.domains.mint_id,
                &s.domains.remint_id,
                &s.domains.return_id,
                &s.domains.burn_id,
            ]
            .into_iter()
            .flatten(),
        );
        domain_strings.push(&s.domains.escrow_label);
        if p.id != "base" {
            for d in &domain_strings {
                if live.contains(d.as_str()) {
                    add(
                        out,
                        "V6",
                        format!("{}: reuses a live Base string {d:?}", p.id),
                    );
                }
            }
        }
        for d in s
            .domains
            .mint_id
            .iter()
            .chain(&s.domains.remint_id)
            .chain(&s.domains.return_id)
            .chain(&s.domains.burn_id)
            .chain([&s.domains.escrow_label])
        {
            if let Some(owner) = domain_owners.insert(d.clone(), p.id.clone()) {
                if owner != p.id {
                    add(
                        out,
                        "V6",
                        format!("{}: domain string {d:?} is also used by {owner}", p.id),
                    );
                }
            }
        }
        for n in &p.networks {
            let network_element = if p.family == Family::Xrpl {
                label(&n.role)
            } else {
                format!(
                    "{}:{}",
                    label(&n.identity.value.kind),
                    n.identity.value.value
                )
            };
            let roles: Vec<&str> = if s.roles.is_empty() {
                vec![""]
            } else {
                s.roles.iter().map(String::as_str).collect()
            };
            for role in roles {
                let path = s
                    .derivation_domain
                    .iter()
                    .map(|e| match e.as_str() {
                        IDENTITY_ELEMENT | NAME_ELEMENT => network_element.as_str(),
                        "{lane}" | "{role}" => role,
                        other => other,
                    })
                    .collect::<Vec<_>>()
                    .join("/");
                if let Some(prev) = expanded_paths.insert(path, n.network_id.clone()) {
                    add(
                        out,
                        "V6",
                        format!(
                            "{}: derivation path is not unique (same as {prev})",
                            n.network_id
                        ),
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// V7: required blockers
// ---------------------------------------------------------------------------

fn required_blockers(profile_id: &str) -> &'static [&'static str] {
    match profile_id {
        "hyperevm" => &["hyperevm-historical-state"],
        "monad" => &["monad-verified-head"],
        "robinhood" => &[
            "robinhood-finalized-tag-unverified",
            "single-keyless-endpoint",
        ],
        "solana" => &[
            "solana-guard-missing",
            "sol-rpc-canister-id-conflict",
            "solana-return-custody",
            "solana-unattributed-burn-policy",
        ],
        "sui" => &["sui-gate-module-missing", "sui-rpc-provider-contract"],
        "xrpl" => &["xrpl-pr586-unmerged", "xrpl-return-intent-missing"],
        _ => &[],
    }
}

fn v7_blockers(profiles: &[Profile], out: &mut Out) {
    for p in profiles {
        for n in &p.networks {
            for b in &n.activation_blockers {
                if b.id.trim().is_empty() || b.text.trim().is_empty() || b.source.trim().is_empty()
                {
                    add(
                        out,
                        "V7",
                        format!(
                            "{}: blocker {:?} has an empty id, text or source",
                            n.network_id, b.id
                        ),
                    );
                }
            }
            for required in required_blockers(&p.id) {
                if !n.activation_blockers.iter().any(|b| b.id == *required) {
                    add(
                        out,
                        "V7",
                        format!("{}: required blocker {required} is missing", n.network_id),
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// V8: one XRPL icUSD, the PR #586 backend module
// ---------------------------------------------------------------------------

fn v8_xrpl(profiles: &[Profile], out: &mut Out) {
    let by_family = profiles.iter().filter(|p| p.family == Family::Xrpl).count();
    if by_family > 1 {
        add(
            out,
            "V8",
            format!("{by_family} XRPL profiles found, only one is allowed"),
        );
    }
    let by_standard = profiles
        .iter()
        .filter(|p| {
            matches!(
                p.representation.standard.value,
                Standard::XrplIou | Standard::XrplMpt
            )
        })
        .count();
    if by_standard > 1 {
        add(
            out,
            "V8",
            format!(
                "{by_standard} profiles use an XRPL token standard, only one XRPL icUSD is allowed"
            ),
        );
    }
    for p in profiles.iter().filter(|p| p.family == Family::Xrpl) {
        if p.topology.kind.value != TopologyKind::BackendModule {
            add(
                out,
                "V8",
                format!("{}: topology must be backend_module", p.id),
            );
        }
        if p.topology.existing_work.as_ref().map(|f| f.value) != Some(ExistingWork::Pr586) {
            add(out, "V8", format!("{}: existing_work must be pr586", p.id));
        }
    }
}

// ---------------------------------------------------------------------------
// V9: RPC quorum and providers
// ---------------------------------------------------------------------------

fn v9_rpc(profiles: &[Profile], out: &mut Out) {
    for p in profiles {
        for n in &p.networks {
            let id = &n.network_id;
            let r = &n.rpc;
            let quorum = r.min_quorum.value;
            if quorum == 0 {
                add(out, "V9", format!("{id}: min_quorum must be at least 1"));
            }
            if n.role == Role::Mainnet && quorum < 2 {
                add(
                    out,
                    "V9",
                    format!("{id}: mainnet min_quorum is {quorum}, must be at least 2"),
                );
            }
            if r.min_independent_providers < quorum {
                add(
                    out,
                    "V9",
                    format!(
                        "{id}: min_independent_providers {} is below min_quorum {quorum}",
                        r.min_independent_providers
                    ),
                );
            }
            if p.family == Family::Evm && !r.historical_state_required {
                add(
                    out,
                    "V9",
                    format!("{id}: historical_state_required must be true for EVM"),
                );
            }
            // A canister with built-in providers supplies its own; otherwise the
            // listed endpoints are the providers.
            let built_in = r.native_variant.is_some() || r.path == RpcPath::SolRpcCanister;
            let listed = r.endpoints.value.len();
            if !built_in
                && listed < r.min_independent_providers as usize
                && !n
                    .activation_blockers
                    .iter()
                    .any(|b| PROVIDER_SHORTAGE_BLOCKERS.contains(&b.id.as_str()))
            {
                add(
                    out,
                    "V9",
                    format!(
                        "{id}: {listed} provider(s) listed, {} needed, and no provider-shortage blocker is present",
                        r.min_independent_providers
                    ),
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// V10: public-repository hygiene
// ---------------------------------------------------------------------------

fn valid_host(h: &str) -> bool {
    !h.is_empty()
        && h.contains('.')
        && !h.starts_with(['.', '-'])
        && !h.ends_with(['.', '-'])
        && h.bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'.' || c == b'-')
}

/// Hygiene findings for one string. Never echoes the offending text.
fn scan_string(s: &str) -> Vec<&'static str> {
    const B58: &str = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    let mut found: Vec<&'static str> = Vec::new();
    let mut flag = |what: &'static str| {
        if !found.contains(&what) {
            found.push(what);
        }
    };
    let b = s.as_bytes();

    // 0x-prefixed hex runs of 40 (address) or 64 (hash) digits.
    let mut i = 0;
    while i + 1 < b.len() {
        let boundary = i == 0 || !b[i - 1].is_ascii_alphanumeric();
        if boundary && b[i] == b'0' && (b[i + 1] == b'x' || b[i + 1] == b'X') {
            let mut j = i + 2;
            while j < b.len() && b[j].is_ascii_hexdigit() {
                j += 1;
            }
            match j - (i + 2) {
                40 => flag("20-byte hex address"),
                64 => flag("64-hex hash"),
                _ => {}
            }
            i = j;
        } else {
            i += 1;
        }
    }

    // Bare hex tokens and XRPL classic addresses.
    for tok in s.split(|c: char| !c.is_ascii_alphanumeric()) {
        if tok.bytes().all(|c| c.is_ascii_hexdigit()) {
            match tok.len() {
                40 => flag("20-byte hex address"),
                64 => flag("64-hex hash"),
                _ => {}
            }
        }
        if (25..=35).contains(&tok.len())
            && tok.starts_with('r')
            && tok.chars().all(|c| B58.contains(c))
        {
            flag("XRPL classic address");
        }
    }

    // Absolute local paths and URLs carrying credentials.
    for w in s.split(|c: char| c.is_whitespace() || "\"'(),;`<>[]".contains(c)) {
        if w.contains("://") {
            let lower = w.to_ascii_lowercase();
            let after = lower.split_once("://").map(|x| x.1).unwrap_or("");
            let authority = after.split(['/', '?', '#']).next().unwrap_or("");
            if authority.contains('@') {
                flag("URL with credentials");
            }
            if let Some(pos) = after.find(['?', '#']) {
                let query = &after[pos..];
                if [
                    "key=",
                    "token=",
                    "secret=",
                    "password=",
                    "passwd=",
                    "auth=",
                    "sig=",
                ]
                .iter()
                .any(|k| query.contains(k))
                {
                    flag("URL with a key or token parameter");
                }
            }
            continue;
        }
        let drive = w.len() >= 3
            && w.as_bytes()[0].is_ascii_alphabetic()
            && (&w[1..3] == ":\\" || &w[1..3] == ":/");
        if (w.len() > 1 && w.starts_with('/'))
            || w.starts_with("~/")
            || drive
            || w.contains("/Users/")
            || w.contains("/home/")
            || w.contains("/private/")
        {
            flag("absolute local path");
        }
    }
    found
}

fn walk_strings(v: &Value, path: &str, out: &mut Out) {
    match v {
        Value::String(s) => {
            for what in scan_string(s) {
                add(out, "V10", format!("{path}: contains a {what}"));
            }
        }
        Value::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                walk_strings(x, &format!("{path}[{i}]"), out);
            }
        }
        Value::Object(m) => {
            for (k, x) in m {
                walk_strings(x, &format!("{path}.{k}"), out);
            }
        }
        _ => {}
    }
}

fn v10_hygiene(profiles: &[Profile], out: &mut Out) {
    for p in profiles {
        match serde_json::to_value(p) {
            Ok(v) => walk_strings(&v, &p.id, out),
            Err(e) => add(
                out,
                "V10",
                format!("{}: could not serialize for the hygiene scan: {e}", p.id),
            ),
        }
        for n in &p.networks {
            for h in &n.rpc.endpoints.value {
                if !valid_host(h) {
                    add(
                        out,
                        "V10",
                        format!(
                            "{}: rpc endpoint {h:?} is not a plain host name",
                            n.network_id
                        ),
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// V11: exactly the nine profiles
// ---------------------------------------------------------------------------

fn v11_nine(profiles: &[Profile], out: &mut Out) {
    for want in REQUIRED_PROFILE_IDS {
        let n = profiles.iter().filter(|p| p.id == want).count();
        if n != 1 {
            add(
                out,
                "V11",
                format!("profile {want} is present {n} times, expected exactly once"),
            );
        }
    }
    for p in profiles {
        if !REQUIRED_PROFILE_IDS.contains(&p.id.as_str()) {
            add(out, "V11", format!("unexpected profile id {}", p.id));
        }
    }
    if profiles.len() != REQUIRED_PROFILE_IDS.len() {
        add(
            out,
            "V11",
            format!(
                "{} profiles found, expected {}",
                profiles.len(),
                REQUIRED_PROFILE_IDS.len()
            ),
        );
    }
}
