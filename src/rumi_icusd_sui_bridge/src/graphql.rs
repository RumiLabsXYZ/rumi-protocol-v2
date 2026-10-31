//! The Sui GraphQL mirror: bounded query builders, strict response parsers, per-provider network
//! and identity checks, a deterministic response transform and the quorum rule. Pure functions
//! only: nothing here sends a request. The wasm HTTP transport and the lane that decides what a
//! verdict means belong to BB-53; this module only guarantees that a missing, unparseable,
//! wrong-network or disagreeing reading can never be mistaken for zero or success.
//!
//! # Schema assumptions (honest record)
//!
//! Field names come from `crates/sui-indexer-alt-graphql/schema.graphql` at `mainnet-v1.82.3`
//! (the SDL, read 2026-10-31). The fixtures in the tests are SCHEMA-DERIVED: hand-built to the
//! SDL, never captured from a live provider. Facts used:
//!
//! * `Query.chainIdentifier: String!`;
//! * `Query.checkpoint(sequenceNumber: UInt53, digest: String): Checkpoint` and
//!   `Checkpoint { digest: String, epoch: Epoch, sequenceNumber: UInt53! }`, `Epoch { epochId: UInt53! }`;
//! * `Query.object(address: SuiAddress!, version: UInt53, rootVersion: UInt53, atCheckpoint: UInt53): Object`
//!   and `Object { address: SuiAddress!, version: UInt53, digest: String, owner: Owner,
//!   asMoveObject: MoveObject }`, `MoveObject { contents: MoveValue }`, `MoveValue { bcs: Base64,
//!   type: MoveType }`, `MoveType { repr: String! }`,
//!   `union Owner = AddressOwner | ObjectOwner | Shared | Immutable | ConsensusAddressOwner`,
//!   `Shared { initialSharedVersion: UInt53 }`;
//! * `Query.address(address: SuiAddress, name: String, rootVersion: UInt53, atCheckpoint: UInt53): Address`
//!   and `Address { dynamicField(name: DynamicFieldName!): DynamicField }`,
//!   `DynamicFieldName { bcs: Base64, literal: String, type: String }`,
//!   `DynamicField { name: MoveValue, value: DynamicFieldValue }`,
//!   `union DynamicFieldValue = MoveObject | MoveValue`.
//!
//! Assumed, not verified against a provider: for a *dynamic field* (as opposed to a dynamic object
//! field) `DynamicField.value` is the stored value `V` itself, so its `bcs` is the BCS of
//! `MintRecord` or `ReturnRecord`, not of the framework's `Field<K, V>` wrapper. The decoders and
//! fixtures here follow that reading; if a provider returns the wrapper the entry fails to decode
//! and is excluded (`Malformed`/`Identity`), never accepted.
//!
//! Unmeasured and therefore disabled by default: a provider's keyless availability, its
//! retention, its real response shapes (JSON-number versus string `UInt53`, the `repr` address
//! form), cycles per call, and checkpoint cadence. The parsers accept both number and string
//! `UInt53` and both short and long package address forms in a type `repr`.
//!
//! Every query asks for `chainIdentifier` in the same request, so a response always identifies
//! the network it came from. A provider whose identifier is not the configured network's is
//! excluded before the quorum counts it.

use crate::gate::{decode_gate, decode_mint_record, decode_return_record, ConfiguredGate, GateObject, MintRecord, ReturnRecord, GATE_STRUCT};
use crate::sui::{format_address, parse_address_padded, parse_digest_base58, Bytes32};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use candid::{CandidType, Deserialize};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

pub const MAX_PROVIDERS: usize = 8;
pub const MAX_PROVIDER_ID_LEN: usize = 32;
pub const MAX_URL_LEN: usize = 256;
pub const MIN_RESPONSE_BYTES: u32 = 1_024;
pub const MAX_RESPONSE_BYTES: u32 = 65_536;
/// `UInt53` is exact only below 2^53.
pub const MAX_UINT53: u64 = (1u64 << 53) - 1;

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct ProviderConfig {
    /// A short stable name, `[a-z0-9-]`, unique in the set.
    pub id: String,
    /// `https://host[/path]` with no credentials, query or fragment.
    pub url: String,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct MirrorSettings {
    pub providers: Vec<ProviderConfig>,
    /// Providers that must report the same fact.
    pub min_quorum: u8,
    /// Usable (identity-checked) providers required before any verdict.
    pub min_independent_providers: u8,
    /// `max_response_bytes` for the HTTP outcall: the cap is billed up front.
    pub max_response_bytes: u32,
    /// Off by default. Even when on, this foundation sends nothing: the transport is BB-53's.
    pub enabled: bool,
}

impl Default for MirrorSettings {
    fn default() -> Self {
        MirrorSettings { providers: vec![], min_quorum: 2, min_independent_providers: 2, max_response_bytes: 16_384, enabled: false }
    }
}

fn credential_like(segment: &str) -> bool {
    segment.len() >= 20
        && segment.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        && segment.bytes().any(|b| b.is_ascii_digit())
        && segment.bytes().any(|b| b.is_ascii_alphabetic())
}

/// Keyless-only rule (chain-sui.md section 5): refuse userinfo, query strings, fragments and
/// credential-looking path segments. The last is a heuristic, not a guarantee.
pub fn validate_provider_url(url: &str) -> Result<(), String> {
    if url.len() > MAX_URL_LEN {
        return Err("the URL is too long".into());
    }
    let rest = url.strip_prefix("https://").ok_or("only https URLs are allowed")?;
    if rest.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("the URL contains whitespace or control characters".into());
    }
    if rest.contains('@') {
        return Err("the URL carries credentials".into());
    }
    if rest.contains('?') || rest.contains('#') {
        return Err("the URL carries a query or fragment".into());
    }
    let (host, path) = match rest.split_once('/') {
        Some((h, p)) => (h, p),
        None => (rest, ""),
    };
    if host.is_empty() || !host.contains('.') || host.starts_with('.') || host.ends_with('.') {
        return Err("the URL has no valid host".into());
    }
    if host.bytes().any(|b| !(b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b':')) {
        return Err("the host has unexpected characters".into());
    }
    if path.split('/').any(credential_like) {
        return Err("a path segment looks like an API key; keys are not allowed in configuration".into());
    }
    Ok(())
}

pub fn validate_settings(s: &MirrorSettings) -> Result<(), String> {
    if s.providers.len() > MAX_PROVIDERS {
        return Err(format!("at most {MAX_PROVIDERS} providers"));
    }
    let mut ids = BTreeSet::new();
    for p in &s.providers {
        if p.id.is_empty() || p.id.len() > MAX_PROVIDER_ID_LEN || !p.id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
            return Err(format!("provider id {:?} must be 1 to {MAX_PROVIDER_ID_LEN} characters of [a-z0-9-]", p.id));
        }
        if !ids.insert(p.id.clone()) {
            return Err(format!("duplicate provider id {}", p.id));
        }
        validate_provider_url(&p.url).map_err(|e| format!("provider {}: {e}", p.id))?;
    }
    if s.min_quorum < 2 || s.min_independent_providers < 2 {
        return Err("min_quorum and min_independent_providers must be at least 2".into());
    }
    if !(MIN_RESPONSE_BYTES..=MAX_RESPONSE_BYTES).contains(&s.max_response_bytes) {
        return Err(format!("max_response_bytes must be {MIN_RESPONSE_BYTES} to {MAX_RESPONSE_BYTES}"));
    }
    if s.enabled && s.providers.len() < usize::from(s.min_quorum.max(s.min_independent_providers)) {
        return Err("fewer providers than the quorum needs".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

macro_rules! object_fields {
    () => {
        "address version digest owner{__typename ... on Shared{initialSharedVersion}} asMoveObject{contents{type{repr} bcs}}"
    };
}

pub const QUERY_CHAIN: &str = "query{chainIdentifier}";
pub const QUERY_CHECKPOINT: &str = "query($s:UInt53){chainIdentifier checkpoint(sequenceNumber:$s){sequenceNumber digest epoch{epochId}}}";
pub const QUERY_OBJECT_AT_CHECKPOINT: &str =
    concat!("query($a:SuiAddress!,$c:UInt53){chainIdentifier object(address:$a,atCheckpoint:$c){", object_fields!(), "}}");
pub const QUERY_OBJECT_AT_VERSION: &str =
    concat!("query($a:SuiAddress!,$v:UInt53){chainIdentifier object(address:$a,version:$v){", object_fields!(), "}}");
pub const QUERY_TABLE_ENTRY: &str = "query($t:SuiAddress!,$c:UInt53,$n:DynamicFieldName!){chainIdentifier address(address:$t,atCheckpoint:$c){dynamicField(name:$n){name{bcs} value{__typename ... on MoveValue{type{repr} bcs}}}}}";

/// One GraphQL request: the fixed query text and its variables.
#[derive(Clone, Debug, PartialEq)]
pub struct GraphqlRequest {
    pub query: &'static str,
    pub variables: Value,
}

impl GraphqlRequest {
    /// The POST body: `{"query":..,"variables":..}`, compact and with sorted keys.
    pub fn body(&self) -> Vec<u8> {
        let mut obj = Map::new();
        obj.insert("query".into(), Value::String(self.query.to_string()));
        obj.insert("variables".into(), self.variables.clone());
        let mut out = Vec::new();
        canonical_json(&Value::Object(obj), &mut out);
        out
    }
}

fn checked_uint53(n: u64) -> u64 {
    assert!(n <= MAX_UINT53, "UInt53 values are exact only below 2^53");
    n
}

pub fn request_chain_identifier() -> GraphqlRequest {
    GraphqlRequest { query: QUERY_CHAIN, variables: json!({}) }
}

/// A checkpoint by sequence number. Never "latest": every read names a number so all replicas
/// of an outcall ask the same question.
pub fn request_checkpoint(sequence: u64) -> GraphqlRequest {
    GraphqlRequest { query: QUERY_CHECKPOINT, variables: json!({ "s": checked_uint53(sequence) }) }
}

/// An object as of a finalized checkpoint.
pub fn request_object_at_checkpoint(address: &Bytes32, checkpoint: u64) -> GraphqlRequest {
    GraphqlRequest { query: QUERY_OBJECT_AT_CHECKPOINT, variables: json!({ "a": format_address(address), "c": checked_uint53(checkpoint) }) }
}

/// An object at an exact version (the fallback for old checkpoints, chain-sui.md section 4).
pub fn request_object_at_version(address: &Bytes32, version: u64) -> GraphqlRequest {
    GraphqlRequest { query: QUERY_OBJECT_AT_VERSION, variables: json!({ "a": format_address(address), "v": checked_uint53(version) }) }
}

/// One entry of a gate table (`mints` or `returns`) by its 32-byte key, as of a checkpoint.
pub fn request_table_entry(table: &Bytes32, checkpoint: u64, key: &Bytes32) -> GraphqlRequest {
    let name = json!({ "type": crate::gate::TABLE_KEY_TYPE, "bcs": STANDARD.encode(crate::gate::table_key_bcs(key)) });
    GraphqlRequest { query: QUERY_TABLE_ENTRY, variables: json!({ "t": format_address(table), "c": checked_uint53(checkpoint), "n": name }) }
}

// ---------------------------------------------------------------------------
// Provider outcomes
// ---------------------------------------------------------------------------

/// Why a provider's answer was set aside before the quorum counted it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Exclusion {
    /// The provider reports another network's chain identifier.
    WrongNetwork(String),
    /// The answer is about another object, key or checkpoint than was asked, or the object is
    /// not the configured gate or table entry.
    Identity(String),
    /// The body is not the expected shape.
    Malformed(String),
    /// The request failed, the provider returned GraphQL errors, or there was no body.
    Transport(String),
}

/// One provider's reading.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome<T> {
    /// A fact the provider asserts.
    Fact(T),
    /// The provider says the thing does not exist (a null object or dynamic field). This is a
    /// reading, not zero: it only means something when the quorum agrees on it, and for a table
    /// entry it is never proof of non-delivery on its own.
    Missing,
    Excluded(Exclusion),
}

impl<T> Outcome<T> {
    fn excluded(e: Exclusion) -> Self {
        Outcome::Excluded(e)
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

fn malformed<T>(m: impl Into<String>) -> Result<T, Exclusion> {
    Err(Exclusion::Malformed(m.into()))
}

fn identity<T>(m: impl Into<String>) -> Result<T, Exclusion> {
    Err(Exclusion::Identity(m.into()))
}

/// The `data` object of a response, or the reason the response cannot be used.
pub fn parse_envelope(body: &[u8]) -> Result<Value, Exclusion> {
    if body.is_empty() {
        return Err(Exclusion::Transport("empty response body".into()));
    }
    let v: Value = serde_json::from_slice(body).map_err(|e| Exclusion::Malformed(format!("not JSON: {e}")))?;
    let obj = v.as_object().ok_or_else(|| Exclusion::Malformed("the response is not a JSON object".into()))?;
    if let Some(errors) = obj.get("errors") {
        if errors.as_array().map(|a| !a.is_empty()).unwrap_or(true) {
            let first = errors
                .as_array()
                .and_then(|a| a.first())
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("GraphQL errors");
            return Err(Exclusion::Transport(first.chars().take(200).collect()));
        }
    }
    match obj.get("data") {
        Some(d) if d.is_object() => Ok(d.clone()),
        _ => malformed("the response has no data object"),
    }
}

/// A `UInt53` as a JSON number or a decimal string.
fn uint53(v: &Value, what: &str) -> Result<u64, Exclusion> {
    let n = match v {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.parse::<u64>().ok(),
        _ => None,
    };
    match n {
        Some(n) if n <= MAX_UINT53 => Ok(n),
        _ => malformed(format!("{what} is not a UInt53")),
    }
}

fn str_field<'a>(v: &'a Value, key: &str) -> Result<&'a str, Exclusion> {
    v.get(key).and_then(Value::as_str).ok_or_else(|| Exclusion::Malformed(format!("missing string field {key}")))
}

/// The provider must report exactly the configured network's four-byte chain identifier.
pub fn check_chain_identifier(data: &Value, expected_short: &str) -> Result<(), Exclusion> {
    let reported = str_field(data, "chainIdentifier")?;
    let well_formed = reported.len() == 8 && reported.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if !well_formed {
        return malformed("chainIdentifier is not 8 lowercase hex digits");
    }
    if reported != expected_short {
        return Err(Exclusion::WrongNetwork(reported.to_string()));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckpointFact {
    pub sequence: u64,
    pub digest: Bytes32,
    pub epoch: u64,
}

/// A checkpoint reading. `None` means the provider has no such checkpoint (yet).
pub fn parse_checkpoint(data: &Value, requested: u64, expected_short: &str) -> Result<Option<CheckpointFact>, Exclusion> {
    check_chain_identifier(data, expected_short)?;
    let c = match data.get("checkpoint") {
        Some(Value::Null) => return Ok(None),
        Some(c) if c.is_object() => c,
        _ => return malformed("checkpoint is missing"),
    };
    let sequence = uint53(c.get("sequenceNumber").unwrap_or(&Value::Null), "sequenceNumber")?;
    if sequence != requested {
        return identity("the checkpoint is not the one requested");
    }
    let digest = parse_digest_base58(str_field(c, "digest")?).map_err(|e| Exclusion::Malformed(format!("checkpoint digest: {e}")))?;
    let epoch = uint53(c.get("epoch").and_then(|e| e.get("epochId")).unwrap_or(&Value::Null), "epochId")?;
    Ok(Some(CheckpointFact { sequence, digest, epoch }))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OwnerFact {
    Shared { initial_shared_version: u64 },
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectFact {
    pub address: Bytes32,
    pub version: u64,
    pub digest: Bytes32,
    pub owner: OwnerFact,
    pub type_repr: String,
    pub contents_bcs: Vec<u8>,
}

/// An object reading. `None` means the provider says the object does not exist there.
pub fn parse_object(data: &Value, requested: &Bytes32, expected_short: &str) -> Result<Option<ObjectFact>, Exclusion> {
    check_chain_identifier(data, expected_short)?;
    let o = match data.get("object") {
        Some(Value::Null) => return Ok(None),
        Some(o) if o.is_object() => o,
        _ => return malformed("object is missing"),
    };
    let address = parse_address_padded(str_field(o, "address")?).map_err(|e| Exclusion::Malformed(format!("object address: {e}")))?;
    if &address != requested {
        return identity("the object is not the one requested");
    }
    let version = uint53(o.get("version").unwrap_or(&Value::Null), "version")?;
    let digest = parse_digest_base58(str_field(o, "digest")?).map_err(|e| Exclusion::Malformed(format!("object digest: {e}")))?;
    let owner_v = o.get("owner").filter(|v| v.is_object()).ok_or_else(|| Exclusion::Malformed("owner is missing".into()))?;
    let owner = match str_field(owner_v, "__typename")? {
        "Shared" => OwnerFact::Shared { initial_shared_version: uint53(owner_v.get("initialSharedVersion").unwrap_or(&Value::Null), "initialSharedVersion")? },
        other => OwnerFact::Other(other.to_string()),
    };
    let contents = o
        .get("asMoveObject")
        .and_then(|m| m.get("contents"))
        .filter(|c| c.is_object())
        .ok_or_else(|| Exclusion::Malformed("the object has no Move contents".into()))?;
    let type_repr = contents.get("type").and_then(|t| t.get("repr")).and_then(Value::as_str).ok_or_else(|| Exclusion::Malformed("missing type repr".into()))?.to_string();
    let contents_bcs = STANDARD.decode(str_field(contents, "bcs")?).map_err(|_| Exclusion::Malformed("contents bcs is not base64".into()))?;
    Ok(Some(ObjectFact { address, version, digest, owner, type_repr, contents_bcs }))
}

/// The gate as observed through one provider, after every check against the configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GateFact {
    pub gate: GateObject,
    pub initial_shared_version: u64,
    pub object_version: u64,
    pub object_digest: Bytes32,
}

/// Turn an object reading into a gate reading: the exact configured address, the exact
/// `<package>::rumi_icusd_gate::Gate` type, a shared owner, contents that decode, and every Move
/// invariant (`GateObject::check_against`). A failure is an `Identity` exclusion.
pub fn interpret_gate(obj: &ObjectFact, cfg: &ConfiguredGate, chain_identifier: &Bytes32) -> Result<GateFact, Exclusion> {
    if obj.address != cfg.gate {
        return identity("the object is not the configured gate");
    }
    if !cfg.type_repr_matches(&obj.type_repr, GATE_STRUCT) {
        return identity(format!("the object type {:?} is not the configured Gate type", obj.type_repr));
    }
    let initial_shared_version = match obj.owner {
        OwnerFact::Shared { initial_shared_version } if initial_shared_version > 0 => initial_shared_version,
        _ => return identity("the gate is not a shared object"),
    };
    let gate = decode_gate(&obj.contents_bcs).map_err(|e| Exclusion::Malformed(format!("gate contents: {e}")))?;
    gate.check_against(cfg, chain_identifier).map_err(|e| Exclusion::Identity(format!("{e}")))?;
    Ok(GateFact { gate, initial_shared_version, object_version: obj.version, object_digest: obj.digest })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableEntryFact {
    pub value_type: String,
    pub value_bcs: Vec<u8>,
}

/// A table entry reading for `key`. `None` means the provider has no such dynamic field. It
/// never means "not delivered": absence is only meaningful once the attempt is proven dead.
pub fn parse_table_entry(data: &Value, key: &Bytes32, expected_short: &str) -> Result<Option<TableEntryFact>, Exclusion> {
    check_chain_identifier(data, expected_short)?;
    let address = match data.get("address") {
        Some(a) if a.is_object() => a,
        // The Address object always exists in the schema; null means the provider does not
        // serve this table at the requested checkpoint.
        _ => return malformed("address is missing"),
    };
    let field = match address.get("dynamicField") {
        Some(Value::Null) => return Ok(None),
        Some(f) if f.is_object() => f,
        _ => return malformed("dynamicField is missing"),
    };
    let name_bcs = STANDARD
        .decode(field.get("name").and_then(|n| n.get("bcs")).and_then(Value::as_str).ok_or_else(|| Exclusion::Malformed("missing name bcs".into()))?)
        .map_err(|_| Exclusion::Malformed("name bcs is not base64".into()))?;
    if name_bcs != crate::gate::table_key_bcs(key) {
        return identity("the dynamic field is not the requested key");
    }
    let value = field.get("value").filter(|v| v.is_object()).ok_or_else(|| Exclusion::Malformed("missing value".into()))?;
    if str_field(value, "__typename")? != "MoveValue" {
        return malformed("the table value is not a MoveValue");
    }
    let value_type = value.get("type").and_then(|t| t.get("repr")).and_then(Value::as_str).ok_or_else(|| Exclusion::Malformed("missing value type".into()))?.to_string();
    let value_bcs = STANDARD.decode(str_field(value, "bcs")?).map_err(|_| Exclusion::Malformed("value bcs is not base64".into()))?;
    Ok(Some(TableEntryFact { value_type, value_bcs }))
}

/// `mints[mint_id]` as a record: the value type must be exactly the configured `MintRecord`.
pub fn interpret_mint_entry(e: &TableEntryFact, cfg: &ConfiguredGate) -> Result<MintRecord, Exclusion> {
    if !cfg.type_repr_matches(&e.value_type, "MintRecord") {
        return identity(format!("the value type {:?} is not the configured MintRecord", e.value_type));
    }
    decode_mint_record(&e.value_bcs).map_err(|e| Exclusion::Identity(format!("{e}")))
}

/// `returns[transfer_id]` as a record.
pub fn interpret_return_entry(e: &TableEntryFact, cfg: &ConfiguredGate) -> Result<ReturnRecord, Exclusion> {
    if !cfg.type_repr_matches(&e.value_type, "ReturnRecord") {
        return identity(format!("the value type {:?} is not the configured ReturnRecord", e.value_type));
    }
    decode_return_record(&e.value_bcs).map_err(|e| Exclusion::Identity(format!("{e}")))
}

/// Run a parser on a raw body. Any failure becomes an exclusion; a successful `None` is `Missing`.
pub fn outcome_from_body<T>(body: &[u8], parse: impl FnOnce(&Value) -> Result<Option<T>, Exclusion>) -> Outcome<T> {
    let data = match parse_envelope(body) {
        Ok(d) => d,
        Err(e) => return Outcome::excluded(e),
    };
    match parse(&data) {
        Ok(Some(v)) => Outcome::Fact(v),
        Ok(None) => Outcome::Missing,
        Err(e) => Outcome::excluded(e),
    }
}

// ---------------------------------------------------------------------------
// Quorum
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuorumRule {
    pub min_quorum: usize,
    pub min_independent: usize,
}

impl From<&MirrorSettings> for QuorumRule {
    fn from(s: &MirrorSettings) -> Self {
        QuorumRule { min_quorum: usize::from(s.min_quorum), min_independent: usize::from(s.min_independent_providers) }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HoldReason {
    /// Two usable providers gave different readings (or one found it and one did not).
    Disagreement,
    /// The same provider id appears twice in one tally.
    DuplicateProvider(String),
}

/// What the quorum concluded. Nothing here is a default: `Held` and `Unavailable` carry a
/// reason and must never be read as zero, absent or success.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict<T> {
    Agreed(T),
    /// At least `min_quorum` usable providers say it does not exist, and none says otherwise.
    AgreedMissing,
    Held(HoldReason),
    Unavailable(String),
}

/// The strict rule: exclusions are set aside first (wrong network, wrong identity, malformed,
/// failed); fewer than `min_independent` usable providers is `Unavailable`; ANY difference
/// among usable providers is `Held`; one agreeing group below `min_quorum` is `Unavailable`.
pub fn tally<T: Clone + Eq>(readings: &[(String, Outcome<T>)], rule: QuorumRule) -> Verdict<T> {
    let mut ids = BTreeSet::new();
    for (id, _) in readings {
        if !ids.insert(id.clone()) {
            return Verdict::Held(HoldReason::DuplicateProvider(id.clone()));
        }
    }
    let usable: Vec<&Outcome<T>> = readings.iter().map(|(_, o)| o).filter(|o| !matches!(o, Outcome::Excluded(_))).collect();
    if usable.len() < rule.min_independent {
        return Verdict::Unavailable(format!("{} usable providers, {} required", usable.len(), rule.min_independent));
    }
    let mut facts: Vec<(&T, usize)> = vec![];
    let mut missing = 0usize;
    for o in usable {
        match o {
            Outcome::Fact(v) => match facts.iter_mut().find(|(x, _)| **x == *v) {
                Some(group) => group.1 += 1,
                None => facts.push((v, 1)),
            },
            Outcome::Missing => missing += 1,
            Outcome::Excluded(_) => {}
        }
    }
    if facts.len() + usize::from(missing > 0) > 1 {
        return Verdict::Held(HoldReason::Disagreement);
    }
    match facts.first() {
        Some((v, n)) if *n >= rule.min_quorum => Verdict::Agreed((*v).clone()),
        Some((_, n)) => Verdict::Unavailable(format!("{n} agreeing providers, {} required", rule.min_quorum)),
        None if missing >= rule.min_quorum => Verdict::AgreedMissing,
        None => Verdict::Unavailable(format!("{missing} providers report it missing, {} required", rule.min_quorum)),
    }
}

/// The providers a tally set aside and why, for the audit trail.
pub fn exclusions<T>(readings: &[(String, Outcome<T>)]) -> Vec<(String, Exclusion)> {
    readings
        .iter()
        .filter_map(|(id, o)| match o {
            Outcome::Excluded(e) => Some((id.clone(), e.clone())),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Deterministic transform
// ---------------------------------------------------------------------------

/// Compact JSON with object keys sorted, recursively. The same logical value always yields the
/// same bytes, whatever order a provider (or `serde_json`'s map) used.
pub fn canonical_json(v: &Value, out: &mut Vec<u8>) {
    match v {
        Value::Object(map) => {
            out.push(b'{');
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                out.extend_from_slice(serde_json::to_string(k.as_str()).expect("string").as_bytes());
                out.push(b':');
                canonical_json(&map[k.as_str()], out);
            }
            out.push(b'}');
        }
        Value::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                canonical_json(item, out);
            }
            out.push(b']');
        }
        other => out.extend_from_slice(other.to_string().as_bytes()),
    }
}

/// The transform every outcall response goes through before consensus: keep only `data` and
/// the (truncated) error messages, drop `extensions` and every other member, and re-serialize
/// canonically. An oversized or non-object body is refused.
pub fn canonicalize_response(body: &[u8], max_bytes: usize) -> Result<Vec<u8>, String> {
    if body.len() > max_bytes {
        return Err("the response is larger than the configured cap".into());
    }
    let v: Value = serde_json::from_slice(body).map_err(|e| format!("not JSON: {e}"))?;
    let obj = v.as_object().ok_or("the response is not a JSON object")?;
    let mut keep = Map::new();
    if let Some(data) = obj.get("data") {
        keep.insert("data".into(), data.clone());
    }
    if let Some(errors) = obj.get("errors").and_then(Value::as_array) {
        let messages: Vec<Value> = errors
            .iter()
            .take(4)
            .map(|e| json!({ "message": e.get("message").and_then(Value::as_str).unwrap_or("").chars().take(200).collect::<String>() }))
            .collect();
        keep.insert("errors".into(), Value::Array(messages));
    }
    let mut out = Vec::new();
    canonical_json(&Value::Object(keep), &mut out);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{SuiNetwork, TESTNET_CHAIN_IDENTIFIER};
    use crate::sui::format_digest_base58;
    use crate::vectors::*;

    const SHORT: &str = "4c78adac";

    fn gate_object_json(chain: &str, address: &str, owner: Value, type_repr: &str, bcs_b64: &str) -> Value {
        json!({ "data": {
            "chainIdentifier": chain,
            "object": {
                "address": address,
                "version": 105,
                "digest": format_digest_base58(&[0x77; 32]),
                "owner": owner,
                "asMoveObject": { "contents": { "type": { "repr": type_repr }, "bcs": bcs_b64 } }
            }
        }})
    }

    fn good_gate_json() -> Value {
        gate_object_json(
            SHORT,
            &format_address(&GATE_ID),
            json!({ "__typename": "Shared", "initialSharedVersion": 42 }),
            &configured().gate_type(),
            GATE_BCS_B64,
        )
    }

    fn data_of(v: &Value) -> Value {
        parse_envelope(&serde_json::to_vec(v).unwrap()).unwrap()
    }

    #[test]
    fn requests_are_fixed_queries_with_pinned_variables() {
        assert_eq!(String::from_utf8(request_chain_identifier().body()).unwrap(), r#"{"query":"query{chainIdentifier}","variables":{}}"#);
        let cp = request_checkpoint(1_234);
        assert_eq!(cp.variables, json!({ "s": 1234 }));
        assert!(cp.query.contains("chainIdentifier"), "every query carries the chain identifier");
        assert!(cp.query.contains("checkpoint(sequenceNumber:$s)"));
        let obj = request_object_at_checkpoint(&GATE_ID, 99);
        assert_eq!(obj.variables, json!({ "a": format!("0x{}", "6a".repeat(32)), "c": 99 }));
        assert!(obj.query.contains("object(address:$a,atCheckpoint:$c)") && obj.query.contains("chainIdentifier"));
        let ver = request_object_at_version(&GATE_ID, 105);
        assert_eq!(ver.variables, json!({ "a": format!("0x{}", "6a".repeat(32)), "v": 105 }));
        assert!(ver.query.contains("object(address:$a,version:$v)"));
        let t = request_table_entry(&[0xe1; 32], 99, &mint_id_bytes());
        assert_eq!(t.variables["n"], json!({ "type": "vector<u8>", "bcs": MINT_ID_NAME_B64 }));
        assert!(t.query.contains("address(address:$t,atCheckpoint:$c)") && t.query.contains("dynamicField(name:$n)"));
        // No query ever asks for "latest": every one takes a number, address or version.
        for q in [QUERY_CHAIN, QUERY_CHECKPOINT, QUERY_OBJECT_AT_CHECKPOINT, QUERY_OBJECT_AT_VERSION, QUERY_TABLE_ENTRY] {
            assert!(!q.to_lowercase().contains("latest"), "{q}");
            assert!(q.len() < 600, "queries stay small");
        }
        // Bodies are deterministic.
        assert_eq!(request_checkpoint(5).body(), request_checkpoint(5).body());
    }

    #[test]
    #[should_panic(expected = "UInt53")]
    fn a_checkpoint_number_above_2_pow_53_is_refused() {
        let _ = request_checkpoint(MAX_UINT53 + 1);
    }

    #[test]
    fn the_envelope_separates_data_from_errors() {
        assert!(matches!(parse_envelope(b""), Err(Exclusion::Transport(_))));
        assert!(matches!(parse_envelope(b"nope"), Err(Exclusion::Malformed(_))));
        assert!(matches!(parse_envelope(b"[]"), Err(Exclusion::Malformed(_))));
        assert!(matches!(parse_envelope(br#"{"errors":[{"message":"rate limited"}]}"#), Err(Exclusion::Transport(m)) if m == "rate limited"));
        assert!(matches!(parse_envelope(br#"{"data":null}"#), Err(Exclusion::Malformed(_))));
        assert!(matches!(parse_envelope(br#"{"data":{"x":1},"errors":[{"message":"partial"}]}"#), Err(Exclusion::Transport(_))), "partial data with errors is not used");
        assert!(parse_envelope(br#"{"data":{"x":1},"extensions":{"a":1}}"#).is_ok());
        assert!(parse_envelope(br#"{"data":{"x":1},"errors":[]}"#).is_ok());
    }

    #[test]
    fn the_chain_identifier_must_be_the_configured_networks() {
        let ok = json!({ "chainIdentifier": SHORT });
        assert!(check_chain_identifier(&ok, SHORT).is_ok());
        assert_eq!(check_chain_identifier(&json!({ "chainIdentifier": "35834a8a" }), SHORT), Err(Exclusion::WrongNetwork("35834a8a".into())));
        assert!(matches!(check_chain_identifier(&json!({ "chainIdentifier": "4C78ADAC" }), SHORT), Err(Exclusion::Malformed(_))));
        assert!(matches!(check_chain_identifier(&json!({ "chainIdentifier": "4c78ad" }), SHORT), Err(Exclusion::Malformed(_))));
        assert!(matches!(check_chain_identifier(&json!({}), SHORT), Err(Exclusion::Malformed(_))));
        assert_eq!(SuiNetwork::Testnet.short_chain_id(), SHORT);
    }

    #[test]
    fn checkpoints_are_checked_against_the_request() {
        let d = |cp: Value| json!({ "chainIdentifier": SHORT, "checkpoint": cp });
        let good = json!({ "sequenceNumber": 500, "digest": format_digest_base58(&[3; 32]), "epoch": { "epochId": 77 } });
        let fact = parse_checkpoint(&d(good.clone()), 500, SHORT).unwrap().unwrap();
        assert_eq!(fact, CheckpointFact { sequence: 500, digest: [3; 32], epoch: 77 });
        // UInt53 as a string works too.
        let as_string = json!({ "sequenceNumber": "500", "digest": format_digest_base58(&[3; 32]), "epoch": { "epochId": "77" } });
        assert_eq!(parse_checkpoint(&d(as_string), 500, SHORT).unwrap().unwrap().epoch, 77);
        assert_eq!(parse_checkpoint(&d(Value::Null), 500, SHORT), Ok(None), "no such checkpoint yet is a missing reading");
        assert!(matches!(parse_checkpoint(&d(good.clone()), 501, SHORT), Err(Exclusion::Identity(_))), "another checkpoint");
        let bad_digest = json!({ "sequenceNumber": 500, "digest": "abc", "epoch": { "epochId": 77 } });
        assert!(matches!(parse_checkpoint(&d(bad_digest), 500, SHORT), Err(Exclusion::Malformed(_))));
        let no_epoch = json!({ "sequenceNumber": 500, "digest": format_digest_base58(&[3; 32]) });
        assert!(parse_checkpoint(&d(no_epoch), 500, SHORT).is_err());
        let wrong_net = json!({ "chainIdentifier": "35834a8a", "checkpoint": good });
        assert!(matches!(parse_checkpoint(&wrong_net, 500, SHORT), Err(Exclusion::WrongNetwork(_))));
    }

    #[test]
    fn the_gate_reading_is_accepted_only_when_every_identity_matches() {
        let cfg = configured();
        let data = data_of(&good_gate_json());
        let obj = parse_object(&data, &GATE_ID, SHORT).unwrap().unwrap();
        assert_eq!(obj.version, 105);
        assert_eq!(obj.owner, OwnerFact::Shared { initial_shared_version: 42 });
        let fact = interpret_gate(&obj, &cfg, &TESTNET_CHAIN_IDENTIFIER).unwrap();
        assert_eq!(fact.initial_shared_version, 42);
        assert_eq!(fact.object_version, 105);
        assert_eq!(fact.gate.supply_cap, 1000 * 100_000_000);

        // Another object than requested.
        assert!(matches!(parse_object(&data, &[0x6b; 32], SHORT), Err(Exclusion::Identity(_))));
        // Short and long address forms of the request normalize.
        let mut short_form = good_gate_json();
        short_form["data"]["object"]["address"] = json!(format!("0x{}", "6a".repeat(32)).to_uppercase().replace("0X", "0x"));
        assert!(parse_object(&data_of(&short_form), &GATE_ID, SHORT).unwrap().is_some());
        // A null object is a missing reading, not an error and not zero.
        let missing = json!({ "chainIdentifier": SHORT, "object": null });
        assert_eq!(parse_object(&missing, &GATE_ID, SHORT), Ok(None));
        // Wrong network.
        let mut wn = good_gate_json();
        wn["data"]["chainIdentifier"] = json!("35834a8a");
        assert!(matches!(parse_object(&data_of(&wn), &GATE_ID, SHORT), Err(Exclusion::WrongNetwork(_))));

        let reject = |mutate: &dyn Fn(&mut Value)| {
            let mut v = good_gate_json();
            mutate(&mut v);
            match parse_object(&data_of(&v), &GATE_ID, SHORT) {
                Ok(Some(o)) => interpret_gate(&o, &cfg, &TESTNET_CHAIN_IDENTIFIER).is_err(),
                Ok(None) => false,
                Err(_) => true,
            }
        };
        assert!(reject(&|v| v["data"]["object"]["asMoveObject"]["contents"]["type"]["repr"] = json!(format!("0x{}::rumi_icusd_gate::Gate", "71".repeat(32)))), "another package");
        assert!(reject(&|v| v["data"]["object"]["asMoveObject"]["contents"]["type"]["repr"] = json!(configured().coin_type())), "the coin type is not the gate type");
        assert!(reject(&|v| v["data"]["object"]["owner"] = json!({ "__typename": "AddressOwner" })), "an owned gate");
        assert!(reject(&|v| v["data"]["object"]["owner"] = json!({ "__typename": "Shared", "initialSharedVersion": 0 })));
        assert!(reject(&|v| v["data"]["object"]["asMoveObject"]["contents"]["bcs"] = json!(base64::engine::general_purpose::STANDARD.encode(hex::decode(GATE_BCS_VERSION_2).unwrap()))), "an unsupported gate version");
        assert!(reject(&|v| v["data"]["object"]["asMoveObject"]["contents"]["bcs"] = json!("AAAA")), "contents that do not decode");
        assert!(reject(&|v| v["data"]["object"]["asMoveObject"]["contents"]["bcs"] = json!("not base64!")));
        assert!(reject(&|v| v["data"]["object"]["digest"] = json!("0OIl")), "a malformed digest");
        assert!(reject(&|v| v["data"]["object"]["asMoveObject"] = Value::Null), "no Move contents");
        // Same bytes, but the configured network is mainnet: the gate's chain identifier differs.
        let o = parse_object(&data, &GATE_ID, SHORT).unwrap().unwrap();
        assert!(interpret_gate(&o, &cfg, &crate::ids::MAINNET_CHAIN_IDENTIFIER).is_err());
        // A different configured gate id.
        let other = ConfiguredGate { gate: [0x6b; 32], ..cfg };
        assert!(interpret_gate(&o, &other, &TESTNET_CHAIN_IDENTIFIER).is_err());
    }

    fn entry_json(name_b64: &str, typename: &str, repr: &str, bcs_b64: &str) -> Value {
        json!({ "chainIdentifier": SHORT, "address": { "dynamicField": {
            "name": { "bcs": name_b64 },
            "value": { "__typename": typename, "type": { "repr": repr }, "bcs": bcs_b64 }
        }}})
    }

    #[test]
    fn table_entries_bind_to_the_requested_key_and_value_type() {
        let cfg = configured();
        let record_type = format!("0x{}::rumi_icusd_gate::MintRecord", hex::encode(PACKAGE));
        let bcs_b64 = STANDARD.encode(hex::decode(MINT_RECORD_BCS).unwrap());
        let good = entry_json(MINT_ID_NAME_B64, "MoveValue", &record_type, &bcs_b64);
        let key = mint_id_bytes();
        let entry = parse_table_entry(&good, &key, SHORT).unwrap().unwrap();
        let rec = interpret_mint_entry(&entry, &cfg).unwrap();
        assert_eq!((rec.recipient, rec.amount, rec.sequence), (RECIPIENT, AMOUNT, 4));

        // A null dynamic field is Missing, not "not delivered".
        let none = json!({ "chainIdentifier": SHORT, "address": { "dynamicField": null } });
        assert_eq!(parse_table_entry(&none, &key, SHORT), Ok(None));
        // The field returned is for another key.
        let other_key = STANDARD.encode(crate::gate::table_key_bcs(&[0x99; 32]));
        let wrong_key = entry_json(&other_key, "MoveValue", &record_type, &bcs_b64);
        assert!(matches!(parse_table_entry(&wrong_key, &key, SHORT), Err(Exclusion::Identity(_))));
        // A dynamic OBJECT field (value is an object) is not a table value.
        let object_value = entry_json(MINT_ID_NAME_B64, "MoveObject", &record_type, &bcs_b64);
        assert!(matches!(parse_table_entry(&object_value, &key, SHORT), Err(Exclusion::Malformed(_))));
        // Wrong value type: a ReturnRecord where a MintRecord is required, and another package.
        let ret_type = format!("0x{}::rumi_icusd_gate::ReturnRecord", hex::encode(PACKAGE));
        let wrong_type = parse_table_entry(&entry_json(MINT_ID_NAME_B64, "MoveValue", &ret_type, &bcs_b64), &key, SHORT).unwrap().unwrap();
        assert!(interpret_mint_entry(&wrong_type, &cfg).is_err());
        let foreign = format!("0x{}::rumi_icusd_gate::MintRecord", "71".repeat(32));
        let foreign_entry = parse_table_entry(&entry_json(MINT_ID_NAME_B64, "MoveValue", &foreign, &bcs_b64), &key, SHORT).unwrap().unwrap();
        assert!(interpret_mint_entry(&foreign_entry, &cfg).is_err());
        // Truncated record bytes.
        let short = STANDARD.encode(&hex::decode(MINT_RECORD_BCS).unwrap()[..40]);
        let truncated = parse_table_entry(&entry_json(MINT_ID_NAME_B64, "MoveValue", &record_type, &short), &key, SHORT).unwrap().unwrap();
        assert!(interpret_mint_entry(&truncated, &cfg).is_err());
        // Return records go through the same path.
        let ret_b64 = STANDARD.encode(hex::decode(RETURN_RECORD_BCS).unwrap());
        let ret = parse_table_entry(&entry_json(MINT_ID_NAME_B64, "MoveValue", &ret_type, &ret_b64), &key, SHORT).unwrap().unwrap();
        assert_eq!(interpret_return_entry(&ret, &cfg).unwrap().client_nonce, 42);
        assert!(interpret_return_entry(&entry, &cfg).is_err());
    }

    fn id(s: &str) -> String {
        s.to_string()
    }

    const RULE: QuorumRule = QuorumRule { min_quorum: 2, min_independent: 2 };

    #[test]
    fn the_quorum_is_strict() {
        use Outcome::*;
        let fact = |n: u32| Fact(n);
        // Agreement.
        assert_eq!(tally(&[(id("a"), fact(7)), (id("b"), fact(7))], RULE), Verdict::Agreed(7));
        assert_eq!(tally(&[(id("a"), fact(7)), (id("b"), fact(7)), (id("c"), fact(7))], RULE), Verdict::Agreed(7));
        // Any dissent holds, even with a majority.
        assert_eq!(tally(&[(id("a"), fact(7)), (id("b"), fact(7)), (id("c"), fact(8))], RULE), Verdict::Held(HoldReason::Disagreement));
        // A provider that finds it and one that does not: held, never "missing wins".
        assert_eq!(tally(&[(id("a"), fact(7)), (id("b"), Missing)], RULE), Verdict::Held(HoldReason::Disagreement));
        assert_eq!(tally::<u32>(&[(id("a"), Missing), (id("b"), Missing)], RULE), Verdict::AgreedMissing);
        // Too few usable providers is unavailable, never zero.
        assert!(matches!(tally(&[(id("a"), fact(7))], RULE), Verdict::Unavailable(_)));
        assert!(matches!(tally::<u32>(&[], RULE), Verdict::Unavailable(_)));
        assert!(matches!(tally::<u32>(&[(id("a"), Missing), (id("b"), Excluded(Exclusion::Transport("x".into())))], RULE), Verdict::Unavailable(_)));
        // Excluded providers are set aside before counting.
        let wrong = Excluded(Exclusion::WrongNetwork("35834a8a".into()));
        assert_eq!(tally(&[(id("a"), fact(7)), (id("b"), fact(7)), (id("c"), wrong.clone())], RULE), Verdict::Agreed(7));
        assert!(matches!(tally(&[(id("a"), fact(7)), (id("b"), wrong.clone())], RULE), Verdict::Unavailable(_)), "a wrong-network provider never makes up a quorum");
        // A larger quorum needs more agreement.
        let rule3 = QuorumRule { min_quorum: 3, min_independent: 3 };
        assert!(matches!(tally(&[(id("a"), fact(7)), (id("b"), fact(7))], rule3), Verdict::Unavailable(_)));
        // Quorum smaller than the independence floor: three usable, two agree, one differs -> held.
        assert_eq!(tally(&[(id("a"), fact(1)), (id("b"), fact(1)), (id("c"), fact(2))], RULE), Verdict::Held(HoldReason::Disagreement));
        // The same provider cannot vote twice.
        assert_eq!(tally(&[(id("a"), fact(7)), (id("a"), fact(7))], RULE), Verdict::Held(HoldReason::DuplicateProvider("a".into())));
        // The audit trail names what was set aside.
        let readings = [(id("a"), fact(7)), (id("b"), wrong)];
        assert_eq!(exclusions(&readings), vec![(id("b"), Exclusion::WrongNetwork("35834a8a".into()))]);
    }

    #[test]
    fn bodies_become_outcomes_without_ever_defaulting() {
        let cfg = configured();
        let body = serde_json::to_vec(&good_gate_json()).unwrap();
        let parse = |d: &Value| {
            parse_object(d, &GATE_ID, SHORT).and_then(|o| match o {
                Some(o) => interpret_gate(&o, &cfg, &TESTNET_CHAIN_IDENTIFIER).map(Some),
                None => Ok(None),
            })
        };
        assert!(matches!(outcome_from_body(&body, parse), Outcome::Fact(f) if f.initial_shared_version == 42));
        assert!(matches!(outcome_from_body(br#"{"data":{"chainIdentifier":"4c78adac","object":null}}"#, parse), Outcome::Missing));
        assert!(matches!(outcome_from_body(b"", parse), Outcome::Excluded(Exclusion::Transport(_))));
        assert!(matches!(outcome_from_body(br#"{"errors":[{"message":"boom"}]}"#, parse), Outcome::Excluded(Exclusion::Transport(_))));
        assert!(matches!(outcome_from_body(br#"{"data":{"chainIdentifier":"35834a8a","object":null}}"#, parse), Outcome::Excluded(Exclusion::WrongNetwork(_))));
    }

    #[test]
    fn the_transform_is_deterministic_and_drops_noise() {
        let a = br#"{"extensions":{"t":"1"},"data":{"b":2,"a":[{"z":1,"y":2}]}}"#;
        let b = br#"{"data":{"a":[{"y":2,"z":1}],"b":2},"extensions":{"t":"999","x":[1,2,3]}}"#;
        let ca = canonicalize_response(a, 4096).unwrap();
        assert_eq!(ca, canonicalize_response(b, 4096).unwrap(), "key order and extensions do not matter");
        assert_eq!(String::from_utf8(ca).unwrap(), r#"{"data":{"a":[{"y":2,"z":1}],"b":2}}"#);
        let with_errors = br#"{"errors":[{"message":"x","locations":[1]}],"data":null}"#;
        assert_eq!(String::from_utf8(canonicalize_response(with_errors, 4096).unwrap()).unwrap(), r#"{"data":null,"errors":[{"message":"x"}]}"#);
        assert!(canonicalize_response(&vec![b' '; 5000], 4096).is_err(), "oversized");
        assert!(canonicalize_response(b"[1]", 4096).is_err());
        assert!(canonicalize_response(b"junk", 4096).is_err());
        // Strings with quotes survive and stay stable.
        let q = canonicalize_response(br#"{"data":{"s":"a\"b"}}"#, 4096).unwrap();
        assert_eq!(String::from_utf8(q).unwrap(), r#"{"data":{"s":"a\"b"}}"#);
    }

    #[test]
    fn settings_are_validated() {
        let p = |id: &str, url: &str| ProviderConfig { id: id.into(), url: url.into() };
        let ok = MirrorSettings {
            providers: vec![p("zan", "https://api.zan.top/node/v1/sui/testnet/graphql"), p("found", "https://graphql.testnet.sui.io/graphql")],
            enabled: true,
            ..MirrorSettings::default()
        };
        assert_eq!(validate_settings(&ok), Ok(()));
        assert_eq!(validate_settings(&MirrorSettings::default()), Ok(()), "disabled and empty is the default");
        assert!(!MirrorSettings::default().enabled);

        let with = |f: &dyn Fn(&mut MirrorSettings)| {
            let mut s = ok.clone();
            f(&mut s);
            validate_settings(&s)
        };
        assert!(with(&|s| s.providers[1].id = "zan".into()).is_err(), "duplicate id");
        assert!(with(&|s| s.providers[0].id = "Zan".into()).is_err());
        assert!(with(&|s| s.providers[0].id = String::new()).is_err());
        assert!(with(&|s| s.providers.truncate(1)).is_err(), "enabled with fewer providers than the quorum");
        assert!(with(&|s| s.min_quorum = 1).is_err());
        assert!(with(&|s| s.min_independent_providers = 1).is_err());
        assert!(with(&|s| s.max_response_bytes = 100).is_err());
        assert!(with(&|s| s.max_response_bytes = MAX_RESPONSE_BYTES + 1).is_err());
        assert!(with(&|s| s.providers = (0..9).map(|i| p(&format!("p{i}"), "https://a.example.com")).collect()).is_err());
        for bad in [
            "http://a.example.com",
            "https://user:pw@a.example.com",
            "https://a.example.com/graphql?key=abc",
            "https://a.example.com/graphql#x",
            "https://a.example.com/path with space",
            "https://localhost/graphql",
            "https://.example.com",
            "https://a.example.com/v2/Ab3dEf6hIj9lMn2pQr5tUv8x",
            "ftp://a.example.com",
            "",
        ] {
            assert!(validate_provider_url(bad).is_err(), "{bad}");
        }
        assert!(validate_provider_url("https://graphql.mainnet.sui.io/graphql").is_ok());
        assert!(validate_provider_url("https://a.example.com:8443/graphql").is_ok());
    }
}
