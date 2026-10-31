//! The GraphQL reads and the one mutation added in 53b: queries, strict parsers and interpreters
//! for the head, a checkpoint with its epoch and reference gas price, the owned objects the mint
//! lane spends (the `MintCap` and the gas coin), the `AdminCap`, the coin metadata, a transaction
//! by digest, and `executeTransaction`. Pure; the quorum and the transport are in `mirror.rs`.
//!
//! # Sources (pinned, read 2026-10-31)
//!
//! `MystenLabs/sui` at `mainnet-v1.82.3`, `crates/sui-indexer-alt-graphql/schema.graphql`
//! (the SDL; the fixtures in the tests are schema-derived, never captured from a provider):
//!
//! * `Query.checkpoint(sequenceNumber: UInt53, digest: String): Checkpoint`; with no argument it is
//!   "the latest checkpoint" (the head query below, which the transform rounds down);
//! * `Checkpoint { sequenceNumber: UInt53!, digest: String, epoch: Epoch }`,
//!   `Epoch { epochId: UInt53!, referenceGasPrice: BigInt }` (`BigInt` is a decimal string);
//! * `Query.object(address, version, rootVersion, atCheckpoint)`, `union Owner = AddressOwner |
//!   ObjectOwner | Shared | Immutable | ConsensusAddressOwner`, `AddressOwner { address: Address }`,
//!   `Address { address: SuiAddress! }`;
//! * `Query.coinMetadata(coinType: String!): CoinMetadata` (null when no metadata object exists)
//!   with `CoinMetadata { decimals: Int, symbol: String }`;
//! * `Query.transaction(digest: String!): Transaction` with `Transaction { digest: String!, effects:
//!   TransactionEffects }` and `TransactionEffects { status: ExecutionStatus, executionError:
//!   ExecutionError, checkpoint: Checkpoint, epoch: Epoch }`, `enum ExecutionStatus { SUCCESS,
//!   FAILURE }`, `ExecutionError { abortCode: BigInt, message: String! }`;
//! * `Mutation.executeTransaction(transactionDataBcs: Base64!, signatures: [Base64!]!):
//!   ExecutionResult!` ("waits until the transaction has reached finality ... or returns the error
//!   that prevented finality") with `ExecutionResult { effects: TransactionEffects }`.
//!
//! # Ownership and version semantics (pinned source, `mainnet-v1.82.3`)
//!
//! `sui-execution/latest/sui-adapter/src/temporary_store.rs` `ensure_active_inputs_mutated`: "For
//! every object from active_inputs (i.e. all mutable objects), if they are not mutated during the
//! transaction execution, force mutating them by incrementing the sequence number. This is
//! required to achieve safety." `InputObjects::mutables_with_input_kinds`
//! (`crates/sui-types/src/transaction.rs`) keeps every `ImmOrOwnedMoveObject` input that is not
//! `object.is_immutable()`, and `exclusive_mutable_inputs` includes them. So an OWNED input such as
//! the `MintCap` or the gas coin is a mutable input of the transaction that uses it even when the
//! Move function takes it by `&` (`mint(.., cap: &MintCap, ..)`), and a transaction that executes
//! advances its version. **Not read from source here:** whether a transaction that ABORTS (the
//! Move call fails) also advances those versions (`GasCharger::handle_error` was not read). The
//! lane does not depend on it: every attempt reads the current references from a quorum before it
//! builds, treats a changed version without a receipt as an unexplained consumer (it halts the
//! lane, it does not replace), and only a finalized receipt or a finalized consumed mint record
//! settles an attempt.

use crate::gate::ConfiguredGate;
use crate::graphql::{
    check_chain_identifier, identity, malformed, parse_object, str_field, uint53, Exclusion, GraphqlRequest, ObjectFact, OwnerFact,
};
use crate::sui::{format_address, parse_address_padded, parse_digest_base58, Bytes32};
use serde_json::{json, Value};

/// The head the transform returns is rounded down to a multiple of this many checkpoints, so every
/// replica of an outcall (and every provider) that is within a window of the same checkpoint
/// reports the same number. The checkpoint cadence is unmeasured; this is a starting value.
pub const HEAD_ROUNDING: u64 = 100;

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

/// The latest checkpoint number. ONLY valid through `transform_sui_graphql_head`, which rounds the
/// number down and drops everything that depends on the exact head.
pub const QUERY_HEAD: &str = "query{chainIdentifier checkpoint{sequenceNumber}}";
pub const QUERY_CHECKPOINT_GAS: &str = "query($s:UInt53){chainIdentifier checkpoint(sequenceNumber:$s){sequenceNumber digest epoch{epochId referenceGasPrice}}}";
pub const QUERY_OWNED_OBJECT: &str = "query($a:SuiAddress!,$c:UInt53){chainIdentifier object(address:$a,atCheckpoint:$c){address version digest owner{__typename ... on AddressOwner{address{address}}} asMoveObject{contents{type{repr} bcs}}}}";
pub const QUERY_COIN_METADATA: &str = "query($t:String!){chainIdentifier coinMetadata(coinType:$t){decimals symbol}}";
pub const QUERY_TRANSACTION: &str =
    "query($d:String!){chainIdentifier transaction(digest:$d){digest effects{status executionError{abortCode message} checkpoint{sequenceNumber} epoch{epochId}}}}";
pub const MUTATION_EXECUTE: &str = "mutation($t:Base64!,$s:[Base64!]!){executeTransaction(transactionDataBcs:$t,signatures:$s){effects{digest status}}}";

pub fn request_head() -> GraphqlRequest {
    GraphqlRequest { query: QUERY_HEAD, variables: json!({}) }
}

pub fn request_checkpoint_gas(sequence: u64) -> GraphqlRequest {
    assert!(sequence <= crate::graphql::MAX_UINT53, "UInt53 values are exact only below 2^53");
    GraphqlRequest { query: QUERY_CHECKPOINT_GAS, variables: json!({ "s": sequence }) }
}

/// An owned (or any) object as of a finalized checkpoint, asking for the owner's address.
pub fn request_owned_object(address: &Bytes32, checkpoint: u64) -> GraphqlRequest {
    assert!(checkpoint <= crate::graphql::MAX_UINT53, "UInt53 values are exact only below 2^53");
    GraphqlRequest { query: QUERY_OWNED_OBJECT, variables: json!({ "a": format_address(address), "c": checkpoint }) }
}

pub fn request_coin_metadata(coin_type: &str) -> GraphqlRequest {
    GraphqlRequest { query: QUERY_COIN_METADATA, variables: json!({ "t": coin_type }) }
}

pub fn request_transaction(digest: &Bytes32) -> GraphqlRequest {
    GraphqlRequest { query: QUERY_TRANSACTION, variables: json!({ "d": crate::sui::format_digest_base58(digest) }) }
}

/// `executeTransaction` for one signed transaction: the base64 BCS and the base64 wire signatures.
pub fn request_execute(tx_bytes_base64: &str, signatures_base64: &[String]) -> GraphqlRequest {
    GraphqlRequest { query: MUTATION_EXECUTE, variables: json!({ "t": tx_bytes_base64, "s": signatures_base64 }) }
}

// ---------------------------------------------------------------------------
// Transforms (the deterministic part of the head and send outcalls)
// ---------------------------------------------------------------------------

/// Round `n` down to a multiple of [`HEAD_ROUNDING`].
pub fn round_head(n: u64) -> u64 {
    n - n % HEAD_ROUNDING
}

/// The head response, reduced to `{chainIdentifier, checkpoint:{sequenceNumber: <rounded>}}`.
/// Anything else a head response carries depends on the exact head and is dropped.
pub fn canonicalize_head(body: &[u8], max_bytes: usize) -> Result<Vec<u8>, String> {
    if body.len() > max_bytes {
        return Err("the response is larger than the configured cap".into());
    }
    let v: Value = serde_json::from_slice(body).map_err(|e| format!("not JSON: {e}"))?;
    let data = v.get("data").and_then(Value::as_object).ok_or("the head response has no data object")?;
    let chain = data.get("chainIdentifier").and_then(Value::as_str).ok_or("no chainIdentifier")?;
    let checkpoint = data.get("checkpoint").ok_or("no checkpoint")?;
    let seq = match checkpoint.get("sequenceNumber") {
        Some(Value::Number(n)) => n.as_u64(),
        Some(Value::String(s)) => s.parse::<u64>().ok(),
        _ => None,
    }
    .ok_or("no sequenceNumber")?;
    let out = json!({ "data": { "chainIdentifier": chain, "checkpoint": { "sequenceNumber": round_head(seq) } } });
    let mut bytes = Vec::new();
    crate::graphql::canonical_json(&out, &mut bytes);
    Ok(bytes)
}

/// The body every successful `executeTransaction` POST is reduced to: whether the provider
/// accepted the request is the only thing a replica contributes, because a duplicate submission
/// of the same signed bytes may answer differently from the first. The canister confirms by
/// transaction lookup, never from this body.
pub const SEND_ACCEPTED_BODY: &[u8] = br#"{"sent":true}"#;

// ---------------------------------------------------------------------------
// Parsers and facts
// ---------------------------------------------------------------------------

/// The head: a checkpoint number that is a multiple of [`HEAD_ROUNDING`], so a response the
/// transform did not round is refused rather than trusted.
pub fn parse_head(data: &Value, expected_genesis_b58: &str) -> Result<Option<u64>, Exclusion> {
    check_chain_identifier(data, expected_genesis_b58)?;
    let c = match data.get("checkpoint") {
        Some(Value::Null) => return Ok(None),
        Some(c) if c.is_object() => c,
        _ => return malformed("checkpoint is missing"),
    };
    let seq = uint53(c.get("sequenceNumber").unwrap_or(&Value::Null), "sequenceNumber")?;
    if seq % HEAD_ROUNDING != 0 {
        return malformed("the head was not rounded by the transform");
    }
    Ok(Some(seq))
}

/// A checkpoint with its epoch and the epoch's reference gas price.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckpointGasFact {
    pub sequence: u64,
    pub digest: Bytes32,
    pub epoch: u64,
    pub reference_gas_price: u64,
}

/// A decimal `BigInt` (a JSON string, or a number) that fits a `u64`.
fn bigint_u64(v: &Value, what: &str) -> Result<u64, Exclusion> {
    let n = match v {
        Value::String(s) => s.parse::<u64>().ok(),
        Value::Number(n) => n.as_u64(),
        _ => None,
    };
    n.ok_or_else(|| Exclusion::Malformed(format!("{what} is not a u64 BigInt")))
}

pub fn parse_checkpoint_gas(data: &Value, requested: u64, expected_genesis_b58: &str) -> Result<Option<CheckpointGasFact>, Exclusion> {
    check_chain_identifier(data, expected_genesis_b58)?;
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
    let epoch_v = c.get("epoch").filter(|e| e.is_object()).ok_or_else(|| Exclusion::Malformed("epoch is missing".into()))?;
    let epoch = uint53(epoch_v.get("epochId").unwrap_or(&Value::Null), "epochId")?;
    let reference_gas_price = bigint_u64(epoch_v.get("referenceGasPrice").unwrap_or(&Value::Null), "referenceGasPrice")?;
    if reference_gas_price == 0 {
        return malformed("the reference gas price is zero");
    }
    Ok(Some(CheckpointGasFact { sequence, digest, epoch, reference_gas_price }))
}

/// The object-parse for [`request_owned_object`]: the same strict parse as the gate's.
pub fn parse_owned_object(data: &Value, requested: &Bytes32, expected_genesis_b58: &str) -> Result<Option<ObjectFact>, Exclusion> {
    parse_object(data, requested, expected_genesis_b58)
}

/// A reference the lane can put in a transaction: the exact id, version and digest, observed as of a
/// finalized checkpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObservedRef {
    pub id: Bytes32,
    pub version: u64,
    pub digest: Bytes32,
}

impl From<ObservedRef> for crate::sui::ObjectRef {
    fn from(r: ObservedRef) -> Self {
        crate::sui::ObjectRef { id: r.id, version: r.version, digest: r.digest }
    }
}

fn expect_owner(obj: &ObjectFact, owner: &Bytes32, what: &str) -> Result<(), Exclusion> {
    match &obj.owner {
        OwnerFact::Address(a) if a == owner => Ok(()),
        OwnerFact::Address(a) => identity(format!("the {what} is owned by {}, not by the expected address", format_address(a))),
        OwnerFact::Shared { .. } => identity(format!("the {what} is a shared object")),
        OwnerFact::Other(kind) => identity(format!("the {what} owner is {kind}, not an address owner")),
    }
}

/// `MintCap { id: UID, gate_id: ID }`: 64 bytes, the object's own id then the gate's.
fn decode_cap(bytes: &[u8], what: &str) -> Result<(Bytes32, Bytes32), Exclusion> {
    if bytes.len() != 64 {
        return identity(format!("the {what} contents are {} bytes, not 64", bytes.len()));
    }
    let mut id = [0u8; 32];
    let mut gate = [0u8; 32];
    id.copy_from_slice(&bytes[..32]);
    gate.copy_from_slice(&bytes[32..]);
    Ok((id, gate))
}

/// The `MintCap`: the exact configured id, the exact type, bound to the configured gate, owned by the
/// treasury role address.
pub fn interpret_mint_cap(obj: &ObjectFact, cfg: &ConfiguredGate, treasury: &Bytes32) -> Result<ObservedRef, Exclusion> {
    if obj.address != cfg.mint_cap {
        return identity("the object is not the configured mint cap");
    }
    if !cfg.type_repr_matches(&obj.type_repr, "MintCap") {
        return identity(format!("the object type {:?} is not the configured MintCap type", obj.type_repr));
    }
    expect_owner(obj, treasury, "mint cap")?;
    let (id, gate) = decode_cap(&obj.contents_bcs, "mint cap")?;
    if id != cfg.mint_cap || gate != cfg.gate {
        return identity("the mint cap is bound to another gate or carries another id");
    }
    Ok(ObservedRef { id: obj.address, version: obj.version, digest: obj.digest })
}

/// What was observed about the `AdminCap`: it is the configured one and bound to the gate; its
/// owner is recorded and disclosed, never invented.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdminCapFact {
    pub reference: ObservedRef,
    pub owner: Bytes32,
}

pub fn interpret_admin_cap(obj: &ObjectFact, cfg: &ConfiguredGate) -> Result<AdminCapFact, Exclusion> {
    if obj.address != cfg.admin_cap {
        return identity("the object is not the configured admin cap");
    }
    if !cfg.type_repr_matches(&obj.type_repr, "AdminCap") {
        return identity(format!("the object type {:?} is not the configured AdminCap type", obj.type_repr));
    }
    let owner = match &obj.owner {
        OwnerFact::Address(a) => *a,
        _ => return identity("the admin cap is not owned by an address"),
    };
    let (id, gate) = decode_cap(&obj.contents_bcs, "admin cap")?;
    if id != cfg.admin_cap || gate != cfg.gate {
        return identity("the admin cap is bound to another gate or carries another id");
    }
    Ok(AdminCapFact { reference: ObservedRef { id: obj.address, version: obj.version, digest: obj.digest }, owner })
}

/// Whether `repr` is the SUI coin type `0x2::coin::Coin<0x2::sui::SUI>`, in the short or the padded
/// address form.
pub fn is_sui_coin_type(repr: &str) -> bool {
    let Some(rest) = repr.split_once("::coin::Coin<") else { return false };
    let Some(inner) = rest.1.strip_suffix('>') else { return false };
    let Some(sui) = inner.split_once("::sui::SUI") else { return false };
    if !sui.1.is_empty() {
        return false;
    }
    let is_two = |a: &str| parse_address_padded(a).map(|b| b == two()).unwrap_or(false);
    is_two(rest.0) && is_two(sui.0)
}

fn two() -> Bytes32 {
    let mut a = [0u8; 32];
    a[31] = 2;
    a
}

/// A gas coin the lane may spend: the configured object, a SUI coin, owned by the gas role address,
/// holding at least `min_balance` MIST. `Coin<SUI> { id: UID, balance: Balance<SUI> }` is 40 bytes.
pub fn interpret_gas_coin(obj: &ObjectFact, configured: &Bytes32, gas_owner: &Bytes32, min_balance: u64) -> Result<(ObservedRef, u64), Exclusion> {
    if &obj.address != configured {
        return identity("the object is not the configured gas coin");
    }
    if !is_sui_coin_type(&obj.type_repr) {
        return identity(format!("the object type {:?} is not Coin<SUI>", obj.type_repr));
    }
    expect_owner(obj, gas_owner, "gas coin")?;
    if obj.contents_bcs.len() != 40 || obj.contents_bcs[..32] != configured[..] {
        return identity("the gas coin contents are not a 40-byte Coin<SUI> with its own id");
    }
    let mut b = [0u8; 8];
    b.copy_from_slice(&obj.contents_bcs[32..]);
    let balance = u64::from_le_bytes(b);
    if balance < min_balance {
        return identity(format!("the gas coin holds {balance} MIST, below the {min_balance} the mint may spend"));
    }
    Ok((ObservedRef { id: obj.address, version: obj.version, digest: obj.digest }, balance))
}

/// The currency's registered decimals and symbol.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoinMetadataFact {
    pub decimals: u8,
    pub symbol: String,
}

pub fn parse_coin_metadata(data: &Value, expected_genesis_b58: &str) -> Result<Option<CoinMetadataFact>, Exclusion> {
    check_chain_identifier(data, expected_genesis_b58)?;
    let m = match data.get("coinMetadata") {
        Some(Value::Null) => return Ok(None),
        Some(m) if m.is_object() => m,
        _ => return malformed("coinMetadata is missing"),
    };
    let decimals = match m.get("decimals") {
        Some(Value::Number(n)) => n.as_u64().and_then(|d| u8::try_from(d).ok()),
        _ => None,
    }
    .ok_or_else(|| Exclusion::Malformed("decimals is not a small integer".into()))?;
    let symbol = str_field(m, "symbol")?.to_string();
    Ok(Some(CoinMetadataFact { decimals, symbol }))
}

/// How a transaction ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecStatus {
    Success,
    Failure { abort_code: Option<String>, message: String },
}

/// A transaction as the chain recorded it: executed, in a checkpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransactionFact {
    pub digest: Bytes32,
    pub status: ExecStatus,
    pub checkpoint: u64,
    pub epoch: u64,
}

/// A transaction reading for `requested`. `None` means the provider has no such transaction, or
/// has it without a checkpoint (not final): never proof that it did not execute.
pub fn parse_transaction(data: &Value, requested: &Bytes32, expected_genesis_b58: &str) -> Result<Option<TransactionFact>, Exclusion> {
    check_chain_identifier(data, expected_genesis_b58)?;
    let t = match data.get("transaction") {
        Some(Value::Null) => return Ok(None),
        Some(t) if t.is_object() => t,
        _ => return malformed("transaction is missing"),
    };
    let digest = parse_digest_base58(str_field(t, "digest")?).map_err(|e| Exclusion::Malformed(format!("transaction digest: {e}")))?;
    if &digest != requested {
        return identity("the transaction is not the one requested");
    }
    let effects = match t.get("effects") {
        Some(Value::Null) => return Ok(None),
        Some(e) if e.is_object() => e,
        _ => return malformed("a transaction without effects"),
    };
    let checkpoint = match effects.get("checkpoint") {
        Some(Value::Null) => return Ok(None),
        Some(c) if c.is_object() => uint53(c.get("sequenceNumber").unwrap_or(&Value::Null), "checkpoint sequenceNumber")?,
        _ => return malformed("effects.checkpoint is missing"),
    };
    let epoch = uint53(effects.get("epoch").and_then(|e| e.get("epochId")).unwrap_or(&Value::Null), "epochId")?;
    let status = match str_field(effects, "status")? {
        "SUCCESS" => ExecStatus::Success,
        "FAILURE" => {
            let err = effects.get("executionError").filter(|e| e.is_object());
            let message = err.and_then(|e| e.get("message")).and_then(Value::as_str).unwrap_or("").chars().take(200).collect();
            let abort_code = err.and_then(|e| e.get("abortCode")).and_then(|c| match c {
                Value::String(s) => Some(s.clone()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            });
            ExecStatus::Failure { abort_code, message }
        }
        other => return malformed(format!("unknown execution status {other}")),
    };
    Ok(Some(TransactionFact { digest, status, checkpoint, epoch }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::SuiNetwork;
    use crate::sui::format_digest_base58;

    const TESTNET_B58: &str = "69WiPg3DAQiwdxfncX6wYQ2siKwAe6L9BZthQea3JNMD";

    fn cfg() -> ConfiguredGate {
        ConfiguredGate { package: [0x70; 32], gate: [0x6a; 32], mint_cap: [0x6d; 32], admin_cap: [0x6e; 32] }
    }

    fn obj(address: Bytes32, owner: OwnerFact, type_repr: &str, contents: Vec<u8>) -> ObjectFact {
        ObjectFact { address, version: 7, digest: [0x99; 32], owner, type_repr: type_repr.into(), contents_bcs: contents }
    }

    fn cap_bytes(id: Bytes32, gate: Bytes32) -> Vec<u8> {
        [id.as_slice(), gate.as_slice()].concat()
    }

    fn data(v: Value) -> Value {
        crate::graphql::parse_envelope(&serde_json::to_vec(&json!({ "data": v })).unwrap()).unwrap()
    }

    #[test]
    fn requests_are_fixed_queries_with_pinned_variables() {
        assert_eq!(String::from_utf8(request_head().body()).unwrap(), r#"{"query":"query{chainIdentifier checkpoint{sequenceNumber}}","variables":{}}"#);
        assert_eq!(request_checkpoint_gas(7).variables, json!({ "s": 7 }));
        assert_eq!(request_owned_object(&[0x6d; 32], 9).variables, json!({ "a": format!("0x{}", "6d".repeat(32)), "c": 9 }));
        assert_eq!(request_coin_metadata("0x1::m::C").variables, json!({ "t": "0x1::m::C" }));
        assert_eq!(request_transaction(&[1; 32]).variables, json!({ "d": format_digest_base58(&[1; 32]) }));
        let exec = request_execute("AAA=", &["sig1".into(), "sig2".into()]);
        assert_eq!(exec.variables, json!({ "t": "AAA=", "s": ["sig1", "sig2"] }));
        assert!(exec.query.starts_with("mutation"));
        for q in [QUERY_HEAD, QUERY_CHECKPOINT_GAS, QUERY_OWNED_OBJECT, QUERY_COIN_METADATA, QUERY_TRANSACTION] {
            assert!(q.contains("chainIdentifier"), "every read carries the chain identifier: {q}");
        }
        // Every query is far below the 5 KB read limit and carries no inline literal.
        for q in [QUERY_HEAD, QUERY_CHECKPOINT_GAS, QUERY_OWNED_OBJECT, QUERY_COIN_METADATA, QUERY_TRANSACTION, MUTATION_EXECUTE] {
            assert!(q.len() < 400 && !q.contains('"'), "{q}");
        }
    }

    #[test]
    fn the_head_is_rounded_down_and_everything_else_is_dropped() {
        let body = serde_json::to_vec(&json!({
            "data": { "chainIdentifier": TESTNET_B58, "checkpoint": { "sequenceNumber": 12_345, "digest": "x", "epoch": { "epochId": 3 } } },
            "extensions": { "usage": 5 }
        }))
        .unwrap();
        let out = canonicalize_head(&body, 1_024).unwrap();
        assert_eq!(String::from_utf8(out.clone()).unwrap(), format!(r#"{{"data":{{"chainIdentifier":"{TESTNET_B58}","checkpoint":{{"sequenceNumber":12300}}}}}}"#));
        // Two providers that are within one window of each other agree after the transform.
        let other = serde_json::to_vec(&json!({ "data": { "chainIdentifier": TESTNET_B58, "checkpoint": { "sequenceNumber": "12399" } } })).unwrap();
        assert_eq!(canonicalize_head(&other, 1_024).unwrap(), out);
        assert_eq!(round_head(99), 0);
        assert_eq!(round_head(100), 100);
        assert!(canonicalize_head(&body, 10).is_err(), "oversized");
        assert!(canonicalize_head(b"not json", 1_024).is_err());
        assert!(canonicalize_head(br#"{"data":{"chainIdentifier":"a"}}"#, 1_024).is_err(), "no checkpoint");
        // The parser accepts only what the transform produced.
        let rounded = data(json!({ "chainIdentifier": TESTNET_B58, "checkpoint": { "sequenceNumber": 12_300 } }));
        assert_eq!(parse_head(&rounded, TESTNET_B58), Ok(Some(12_300)));
        let raw = data(json!({ "chainIdentifier": TESTNET_B58, "checkpoint": { "sequenceNumber": 12_345 } }));
        assert!(matches!(parse_head(&raw, TESTNET_B58), Err(Exclusion::Malformed(_))), "an unrounded head means the transform did not run");
        let null = data(json!({ "chainIdentifier": TESTNET_B58, "checkpoint": null }));
        assert_eq!(parse_head(&null, TESTNET_B58), Ok(None));
        let wrong = data(json!({ "chainIdentifier": "4btiuiMPvEENsttpZC7CZ53DruC3MAgfznDbASZ7DR6S", "checkpoint": { "sequenceNumber": 100 } }));
        assert!(matches!(parse_head(&wrong, TESTNET_B58), Err(Exclusion::WrongNetwork(_))));
    }

    #[test]
    fn a_checkpoint_carries_its_epoch_and_a_nonzero_reference_gas_price() {
        let good = |price: Value, seq: u64| {
            data(json!({ "chainIdentifier": TESTNET_B58, "checkpoint": {
                "sequenceNumber": seq, "digest": format_digest_base58(&[5; 32]), "epoch": { "epochId": "12", "referenceGasPrice": price } } }))
        };
        assert_eq!(
            parse_checkpoint_gas(&good(json!("750"), 500), 500, TESTNET_B58),
            Ok(Some(CheckpointGasFact { sequence: 500, digest: [5; 32], epoch: 12, reference_gas_price: 750 }))
        );
        assert_eq!(parse_checkpoint_gas(&good(json!(750), 500), 500, TESTNET_B58).unwrap().unwrap().reference_gas_price, 750, "a number is tolerated");
        assert!(matches!(parse_checkpoint_gas(&good(json!("0"), 500), 500, TESTNET_B58), Err(Exclusion::Malformed(_))), "zero price");
        assert!(matches!(parse_checkpoint_gas(&good(json!("abc"), 500), 500, TESTNET_B58), Err(Exclusion::Malformed(_))));
        assert!(matches!(parse_checkpoint_gas(&good(json!(null), 500), 500, TESTNET_B58), Err(Exclusion::Malformed(_))));
        assert!(matches!(parse_checkpoint_gas(&good(json!("750"), 501), 500, TESTNET_B58), Err(Exclusion::Identity(_))), "another checkpoint");
        let null = data(json!({ "chainIdentifier": TESTNET_B58, "checkpoint": null }));
        assert_eq!(parse_checkpoint_gas(&null, 500, TESTNET_B58), Ok(None));
        let _ = SuiNetwork::Testnet;
    }

    #[test]
    fn the_mint_cap_must_be_the_configured_one_of_this_gate_owned_by_the_treasury() {
        let c = cfg();
        let treasury = [0x01; 32];
        let repr = format!("0x{}::rumi_icusd_gate::MintCap", hex::encode(c.package));
        let good = obj(c.mint_cap, OwnerFact::Address(treasury), &repr, cap_bytes(c.mint_cap, c.gate));
        assert_eq!(interpret_mint_cap(&good, &c, &treasury), Ok(ObservedRef { id: c.mint_cap, version: 7, digest: [0x99; 32] }));
        let bad: Vec<(&str, ObjectFact)> = vec![
            ("another object", ObjectFact { address: [1; 32], ..good.clone() }),
            ("another owner", ObjectFact { owner: OwnerFact::Address([0x02; 32]), ..good.clone() }),
            ("shared", ObjectFact { owner: OwnerFact::Shared { initial_shared_version: 3 }, ..good.clone() }),
            ("object owned", ObjectFact { owner: OwnerFact::Other("ObjectOwner".into()), ..good.clone() }),
            ("another type", ObjectFact { type_repr: repr.replace("MintCap", "AdminCap"), ..good.clone() }),
            ("another package", ObjectFact { type_repr: format!("0x{}::rumi_icusd_gate::MintCap", "71".repeat(32)), ..good.clone() }),
            ("another gate", ObjectFact { contents_bcs: cap_bytes(c.mint_cap, [0x6b; 32]), ..good.clone() }),
            ("another id inside", ObjectFact { contents_bcs: cap_bytes([1; 32], c.gate), ..good.clone() }),
            ("short contents", ObjectFact { contents_bcs: vec![0; 63], ..good.clone() }),
            ("long contents", ObjectFact { contents_bcs: vec![0; 65], ..good.clone() }),
        ];
        for (what, o) in bad {
            assert!(matches!(interpret_mint_cap(&o, &c, &treasury), Err(Exclusion::Identity(_))), "{what}");
        }
    }

    #[test]
    fn the_admin_cap_is_recorded_with_its_owner_and_never_assumed() {
        let c = cfg();
        let repr = format!("0x{}::rumi_icusd_gate::AdminCap", hex::encode(c.package));
        let good = obj(c.admin_cap, OwnerFact::Address([0x03; 32]), &repr, cap_bytes(c.admin_cap, c.gate));
        let f = interpret_admin_cap(&good, &c).unwrap();
        assert_eq!((f.owner, f.reference.id, f.reference.version), ([0x03; 32], c.admin_cap, 7));
        for (what, o) in [
            ("shared", ObjectFact { owner: OwnerFact::Shared { initial_shared_version: 1 }, ..good.clone() }),
            ("other owner kind", ObjectFact { owner: OwnerFact::Other("Immutable".into()), ..good.clone() }),
            ("wrong type", ObjectFact { type_repr: repr.replace("AdminCap", "MintCap"), ..good.clone() }),
            ("wrong gate", ObjectFact { contents_bcs: cap_bytes(c.admin_cap, [9; 32]), ..good.clone() }),
            ("wrong object", ObjectFact { address: c.mint_cap, ..good.clone() }),
        ] {
            assert!(matches!(interpret_admin_cap(&o, &c), Err(Exclusion::Identity(_))), "{what}");
        }
    }

    #[test]
    fn a_gas_coin_is_the_configured_sui_coin_owned_by_the_gas_role_with_enough_balance() {
        let coin = [0x42; 32];
        let gas = [0x04; 32];
        let contents = |id: Bytes32, balance: u64| [id.as_slice(), balance.to_le_bytes().as_slice()].concat();
        let good = obj(coin, OwnerFact::Address(gas), "0x2::coin::Coin<0x2::sui::SUI>", contents(coin, 5_000));
        assert_eq!(interpret_gas_coin(&good, &coin, &gas, 1_000), Ok((ObservedRef { id: coin, version: 7, digest: [0x99; 32] }, 5_000)));
        let padded = format!("0x{:0>64}::coin::Coin<0x{:0>64}::sui::SUI>", "2", "2");
        assert!(interpret_gas_coin(&ObjectFact { type_repr: padded, ..good.clone() }, &coin, &gas, 1_000).is_ok(), "padded address form");
        for (what, o) in [
            ("another coin type", ObjectFact { type_repr: "0x2::coin::Coin<0x3::x::Y>".into(), ..good.clone() }),
            ("not a coin", ObjectFact { type_repr: "0x2::sui::SUI".into(), ..good.clone() }),
            ("trailing type text", ObjectFact { type_repr: "0x2::coin::Coin<0x2::sui::SUI>x".into(), ..good.clone() }),
            ("another owner", ObjectFact { owner: OwnerFact::Address([5; 32]), ..good.clone() }),
            ("shared", ObjectFact { owner: OwnerFact::Shared { initial_shared_version: 1 }, ..good.clone() }),
            ("another id", ObjectFact { address: [1; 32], ..good.clone() }),
            ("short contents", ObjectFact { contents_bcs: vec![0; 39], ..good.clone() }),
            ("foreign id inside", ObjectFact { contents_bcs: contents([1; 32], 5_000), ..good.clone() }),
            ("too little gas", ObjectFact { contents_bcs: contents(coin, 999), ..good.clone() }),
        ] {
            assert!(matches!(interpret_gas_coin(&o, &coin, &gas, 1_000), Err(Exclusion::Identity(_))), "{what}");
        }
        assert!(is_sui_coin_type("0x2::coin::Coin<0x2::sui::SUI>"));
        assert!(!is_sui_coin_type("0x2::coin::Coin<0x2::sui::SUI><"));
        assert!(!is_sui_coin_type("0x2::coin::Coin<0x2::sui::SUIx>"));
    }

    #[test]
    fn coin_metadata_carries_decimals_and_a_missing_one_is_missing() {
        let good = data(json!({ "chainIdentifier": TESTNET_B58, "coinMetadata": { "decimals": 8, "symbol": "icUSD" } }));
        assert_eq!(parse_coin_metadata(&good, TESTNET_B58), Ok(Some(CoinMetadataFact { decimals: 8, symbol: "icUSD".into() })));
        let null = data(json!({ "chainIdentifier": TESTNET_B58, "coinMetadata": null }));
        assert_eq!(parse_coin_metadata(&null, TESTNET_B58), Ok(None));
        for bad in [json!({ "decimals": null, "symbol": "x" }), json!({ "decimals": 300, "symbol": "x" }), json!({ "decimals": "8", "symbol": "x" }), json!({ "decimals": 8 })] {
            let d = data(json!({ "chainIdentifier": TESTNET_B58, "coinMetadata": bad }));
            assert!(matches!(parse_coin_metadata(&d, TESTNET_B58), Err(Exclusion::Malformed(_))));
        }
    }

    fn tx_json(digest: &Bytes32, effects: Value) -> Value {
        data(json!({ "chainIdentifier": TESTNET_B58, "transaction": { "digest": format_digest_base58(digest), "effects": effects } }))
    }

    #[test]
    fn a_transaction_is_final_only_with_a_status_and_a_checkpoint() {
        let d = [0x31; 32];
        let ok = tx_json(&d, json!({ "status": "SUCCESS", "executionError": null, "checkpoint": { "sequenceNumber": 900 }, "epoch": { "epochId": 4 } }));
        assert_eq!(parse_transaction(&ok, &d, TESTNET_B58), Ok(Some(TransactionFact { digest: d, status: ExecStatus::Success, checkpoint: 900, epoch: 4 })));
        let failed = tx_json(
            &d,
            json!({ "status": "FAILURE", "executionError": { "abortCode": "5", "message": "MoveAbort in mint" }, "checkpoint": { "sequenceNumber": 901 }, "epoch": { "epochId": 4 } }),
        );
        assert_eq!(
            parse_transaction(&failed, &d, TESTNET_B58),
            Ok(Some(TransactionFact {
                digest: d,
                status: ExecStatus::Failure { abort_code: Some("5".into()), message: "MoveAbort in mint".into() },
                checkpoint: 901,
                epoch: 4
            }))
        );
        // A failure with no error detail is still a failure.
        let bare = tx_json(&d, json!({ "status": "FAILURE", "executionError": null, "checkpoint": { "sequenceNumber": 901 }, "epoch": { "epochId": 4 } }));
        assert!(matches!(parse_transaction(&bare, &d, TESTNET_B58), Ok(Some(TransactionFact { status: ExecStatus::Failure { .. }, .. }))));
        // Null transaction, null effects and null checkpoint are all "not found (final)", never success or failure.
        let null = data(json!({ "chainIdentifier": TESTNET_B58, "transaction": null }));
        assert_eq!(parse_transaction(&null, &d, TESTNET_B58), Ok(None));
        let no_effects = data(json!({ "chainIdentifier": TESTNET_B58, "transaction": { "digest": format_digest_base58(&d), "effects": null } }));
        assert_eq!(parse_transaction(&no_effects, &d, TESTNET_B58), Ok(None));
        let no_cp = tx_json(&d, json!({ "status": "SUCCESS", "checkpoint": null, "epoch": { "epochId": 4 } }));
        assert_eq!(parse_transaction(&no_cp, &d, TESTNET_B58), Ok(None));
        // Another digest, an unknown status and a missing status are exclusions.
        let other = tx_json(&[0x32; 32], json!({ "status": "SUCCESS", "checkpoint": { "sequenceNumber": 1 }, "epoch": { "epochId": 4 } }));
        assert!(matches!(parse_transaction(&other, &d, TESTNET_B58), Err(Exclusion::Identity(_))));
        let weird = tx_json(&d, json!({ "status": "PENDING", "checkpoint": { "sequenceNumber": 1 }, "epoch": { "epochId": 4 } }));
        assert!(matches!(parse_transaction(&weird, &d, TESTNET_B58), Err(Exclusion::Malformed(_))));
        let missing = tx_json(&d, json!({ "checkpoint": { "sequenceNumber": 1 }, "epoch": { "epochId": 4 } }));
        assert!(matches!(parse_transaction(&missing, &d, TESTNET_B58), Err(Exclusion::Malformed(_))));
    }
}
