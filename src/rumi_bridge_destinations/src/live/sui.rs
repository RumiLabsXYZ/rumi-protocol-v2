//! Sui side of the live transport (feature `live-transport`, card BB-53, step 53d).
//!
//! Reads a Sui destination's supply from the chain itself: the configured Gate's wrapped
//! `TreasuryCap` `Supply.value`, with the Gate's own counters, at ONE numbered finalized checkpoint.
//! Nothing is taken from the bridge instance's own report, and the instance's audit (a self-report) is
//! only displayed beside this read. The escrow `E_d` is an ordinary ICRC-1 balance read through the
//! existing IC transport.
//!
//! The provider rule mirrors the Solana, EVM and XRPL readers and reuses their
//! [`super::evm::JsonRpcTransport`] (the body posted here is a GraphQL document, not JSON-RPC):
//!
//! 1. every provider is asked for its `chainIdentifier` and its latest finalized checkpoint number. The
//!    identifier must be exactly the profile network's genesis digest (Base58); a provider on another
//!    network is excluded from every vote. GraphQL `errors` (valid or malformed) exclude a provider too;
//! 2. the read is pinned to a number `n`: the operator's explicit `checkpoint` when configured (every
//!    provider must serve it), otherwise the `quorum`-th highest latest checkpoint (one far-ahead provider
//!    cannot raise it). Providers below `n` do not vote;
//! 3. every voting provider is asked, in ONE request, for checkpoint `n` (digest, epoch), the Gate object
//!    `atCheckpoint: n` (type, owner, contents) and the currency's registered decimals. The Gate must be
//!    exactly the configured id and `<package>::rumi_icusd_gate::Gate`, a shared object, and its BCS
//!    contents must decode with the accepted layout and pass the checks in [`decode_gate`];
//! 4. a quorum of identical answers (checkpoint digest and epoch included) wins; a disagreement or too
//!    few identical answers fails the read, it is never averaged or guessed.
//!
//! # What this does and does not prove
//!
//! The decimals check uses `Query.coinMetadata(coinType)` (schema `mainnet-v1.82.3`), which takes no
//! checkpoint: it answers from the provider's current state, which is sound only because a registered
//! currency's decimals never change. The Gate creates its currency through the coin registry and
//! registers it in a separate `finalize_registration` transaction, so until that has happened (or when a
//! provider does not resolve a registry currency this way, which is unmeasured) the answer is null and
//! the read FAILS with a named error: it does not assume 8 decimals.
//!
//! A numbered checkpoint is final (Sui checkpoints do not fork), and the Gate is read at that exact
//! checkpoint, so unlike the Solana reader this one IS pinned to a number and the reported checkpoint is
//! the actual one. It proves what a quorum of providers reports about the Gate at that checkpoint. It
//! does not prove who controls the package or its `UpgradeCap` (D6 stays a disclosed root of trust), that
//! the published module is the accepted source (no module digest is compared), or any provider's
//! availability or retention, none of which is measured. Custody is not read: the Gate burns a returned
//! coin in the same transaction that records the return, so custody is zero by design and the collector
//! says so.
//!
//! Providers are named by position in errors, never by URL. The configured package and Gate are operator
//! input: each must be `0x` plus 64 lowercase hex digits and they differ.

use serde_json::{json, Value};

use super::evm::{tally, JsonRpcTransport};
use super::solana::{base58_to_32, base64_decode};
use crate::collect::{ReadError, SuiGate, SuiSupply};
use crate::types::{FinalityRule, IdentityKind, Network};

/// The module the Gate lives in.
pub const MODULE: &str = "rumi_icusd_gate";
/// The code version this reader understands (`Gate.version`).
pub const GATE_VERSION: u64 = 1;
/// The decimals of the bridged icUSD (the profile's representation).
pub const ICUSD_DECIMALS: u64 = 8;
/// Largest Gate object body accepted from a provider.
const MAX_GATE_BYTES: usize = 1024;
/// `UInt53` is exact only below 2^53.
const MAX_UINT53: u64 = (1u64 << 53) - 1;

/// What to read: the published package, the shared Gate and, optionally, an explicit checkpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub package: [u8; 32],
    pub gate: [u8; 32],
    /// Read at exactly this checkpoint. `None`: the quorum-th highest finalized checkpoint.
    pub checkpoint: Option<u64>,
}

fn parse_address_strict(text: &str) -> Result<[u8; 32], String> {
    let digits = text
        .strip_prefix("0x")
        .ok_or("must be 0x followed by 64 lowercase hex digits")?;
    if digits.len() != 64 || !digits.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return Err("must be 0x followed by 64 lowercase hex digits".into());
    }
    let mut out = [0u8; 32];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&digits[2 * i..2 * i + 2], 16).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// An address as a provider prints it: `0x` and 1 to 64 hex digits, left-padded with zeros.
fn parse_address_padded(text: &str) -> Result<[u8; 32], String> {
    let digits = text.strip_prefix("0x").ok_or("an address must start with 0x")?;
    if digits.is_empty() || digits.len() > 64 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("an address must be 0x and 1 to 64 hex digits".into());
    }
    let padded = format!("{digits:0>64}");
    let mut out = [0u8; 32];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&padded[2 * i..2 * i + 2], 16).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

impl Target {
    /// Validate the operator's input.
    pub fn new(package: &str, gate: &str, checkpoint: Option<u64>) -> Result<Target, String> {
        let package_key = parse_address_strict(package).map_err(|e| format!("the package {e}"))?;
        let gate_key = parse_address_strict(gate).map_err(|e| format!("the gate {e}"))?;
        if package_key == [0u8; 32] || gate_key == [0u8; 32] {
            return Err("the package and the gate must not be the zero address".into());
        }
        if package_key == gate_key {
            return Err("the package and the gate are the same address".into());
        }
        if let Some(c) = checkpoint {
            if c == 0 || c > MAX_UINT53 {
                return Err("the checkpoint must be between 1 and 2^53 - 1".into());
            }
        }
        Ok(Target { package: package_key, gate: gate_key, checkpoint })
    }

    fn gate_text(&self) -> String {
        format!("0x{}", hex_of(&self.gate))
    }

    fn coin_type(&self) -> String {
        format!("0x{}::{MODULE}::RUMI_ICUSD_GATE", hex_of(&self.package))
    }
}

fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ---------------------------------------------------------------------------
// The Gate object (BCS), decoded with the accepted layout
// ---------------------------------------------------------------------------

/// The decoded shared `Gate`, in the Move struct order:
///
/// ```text
/// id:UID(32) version:u64 treasury:TreasuryCap{id:UID(32) total_supply:Supply{value:u64}}
/// chain_identifier:vector<u8> supply_cap:u64 min_bridge_out:u64 minted_total:u128 burned_total:u128
/// mint_count:u64 bridge_out_count:u64 mints:Table{id:UID(32) size:u64} returns:Table{id:UID(32) size:u64}
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateObject {
    pub id: [u8; 32],
    pub version: u64,
    pub total_supply: u64,
    pub chain_identifier: Vec<u8>,
    pub supply_cap: u64,
    pub min_bridge_out: u64,
    pub minted_total: u128,
    pub burned_total: u128,
    pub mint_count: u64,
    pub bridge_out_count: u64,
    pub mints_table: [u8; 32],
    pub mints_size: u64,
    pub returns_table: [u8; 32],
    pub returns_size: u64,
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).filter(|e| *e <= self.bytes.len()).ok_or("the Gate contents end early")?;
        let out = &self.bytes[self.at..end];
        self.at = end;
        Ok(out)
    }

    fn array32(&mut self) -> Result<[u8; 32], String> {
        let mut out = [0u8; 32];
        out.copy_from_slice(self.take(32)?);
        Ok(out)
    }

    fn u64(&mut self) -> Result<u64, String> {
        let mut b = [0u8; 8];
        b.copy_from_slice(self.take(8)?);
        Ok(u64::from_le_bytes(b))
    }

    fn u128(&mut self) -> Result<u128, String> {
        let mut b = [0u8; 16];
        b.copy_from_slice(self.take(16)?);
        Ok(u128::from_le_bytes(b))
    }

    /// A BCS length (ULEB128, at most 32 bits, canonical).
    fn uleb(&mut self) -> Result<usize, String> {
        let mut value: u64 = 0;
        for shift in 0..5 {
            let byte = self.take(1)?[0];
            value |= u64::from(byte & 0x7f) << (7 * shift);
            if byte & 0x80 == 0 {
                if shift > 0 && byte == 0 {
                    return Err("a non-canonical length in the Gate contents".into());
                }
                return usize::try_from(value).map_err(|_| "a length that does not fit".to_string());
            }
        }
        Err("a length over 32 bits in the Gate contents".into())
    }
}

/// Decode the BCS `contents` of a `Gate`, refusing trailing bytes, and apply the checks that do not
/// depend on the configuration: the supported version, the stored chain identifier is 32 bytes, the
/// supply identity `minted_total - burned_total == total_supply`, `min_bridge_out >= 1`, `supply_cap`
/// within the Move maximum, the table sizes equal to the counters, and distinct object ids.
pub fn decode_gate(bytes: &[u8]) -> Result<GateObject, String> {
    if bytes.len() > MAX_GATE_BYTES {
        return Err("the Gate contents are too large".into());
    }
    let mut r = Reader { bytes, at: 0 };
    let id = r.array32()?;
    let version = r.u64()?;
    let treasury_id = r.array32()?;
    let total_supply = r.u64()?;
    let len = r.uleb()?;
    let chain_identifier = r.take(len)?.to_vec();
    let supply_cap = r.u64()?;
    let min_bridge_out = r.u64()?;
    let minted_total = r.u128()?;
    let burned_total = r.u128()?;
    let mint_count = r.u64()?;
    let bridge_out_count = r.u64()?;
    let mints_table = r.array32()?;
    let mints_size = r.u64()?;
    let returns_table = r.array32()?;
    let returns_size = r.u64()?;
    if r.at != bytes.len() {
        return Err("the Gate contents have trailing bytes".into());
    }
    if version != GATE_VERSION {
        return Err(format!("the Gate is at code version {version}, this reader understands {GATE_VERSION}"));
    }
    if chain_identifier.len() != 32 {
        return Err("the Gate stored a chain identifier that is not 32 bytes".into());
    }
    if supply_cap > u64::MAX - 1 {
        return Err("the Gate's supply cap is above the Move maximum".into());
    }
    if min_bridge_out == 0 {
        return Err("the Gate's minimum return is zero".into());
    }
    if minted_total.checked_sub(burned_total) != Some(u128::from(total_supply)) {
        return Err("the Gate's minted_total less burned_total is not its treasury supply".into());
    }
    if mints_size != mint_count || returns_size != bridge_out_count {
        return Err("a Gate table size differs from its counter".into());
    }
    let ids = [id, treasury_id, mints_table, returns_table];
    for i in 0..ids.len() {
        for j in i + 1..ids.len() {
            if ids[i] == ids[j] {
                return Err("two of the Gate's object ids are the same".into());
            }
        }
    }
    Ok(GateObject {
        id,
        version,
        total_supply,
        chain_identifier,
        supply_cap,
        min_bridge_out,
        minted_total,
        burned_total,
        mint_count,
        bridge_out_count,
        mints_table,
        mints_size,
        returns_table,
        returns_size,
    })
}

// ---------------------------------------------------------------------------
// GraphQL
// ---------------------------------------------------------------------------

/// The latest finalized checkpoint (head discovery; the collector is not an outcall consensus, so it may
/// ask each provider for its own head and then pin every later read to a number).
pub const QUERY_HEAD: &str = "query{chainIdentifier checkpoint{sequenceNumber}}";
/// The checkpoint `$c` (digest, epoch), the Gate `$g` at that checkpoint and the registered decimals of the
/// coin type `$t`, in one request.
pub const QUERY_GATE: &str = "query($c:UInt53,$g:SuiAddress!,$t:String!){chainIdentifier checkpoint(sequenceNumber:$c){sequenceNumber digest epoch{epochId}} object(address:$g,atCheckpoint:$c){address version digest owner{__typename ... on Shared{initialSharedVersion}} asMoveObject{contents{type{repr} bcs}}} coinMetadata(coinType:$t){decimals}}";

/// One request to one provider; returns the `data` object. GraphQL `errors`, valid or malformed, make the
/// provider's answer unusable.
fn graphql(t: &dyn JsonRpcTransport, i: usize, query: &str, variables: Value) -> Result<Value, String> {
    let body = json!({"query": query, "variables": variables}).to_string();
    let text = t.post(i, &body)?;
    let envelope: Value = serde_json::from_str(&text).map_err(|e| format!("response is not JSON: {e}"))?;
    let o = envelope.as_object().ok_or("response is not an object")?;
    if let Some(errors) = o.get("errors") {
        let first = errors
            .as_array()
            .and_then(|a| a.first())
            .and_then(|e| e.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("GraphQL errors");
        if errors.as_array().is_none_or(|a| !a.is_empty()) {
            let first: String = first.chars().take(120).collect();
            return Err(format!("graphql error: {first}"));
        }
    }
    match o.get("data") {
        Some(d) if d.is_object() => Ok(d.clone()),
        _ => Err("response has no data object".into()),
    }
}

fn uint53(v: &Value, what: &str) -> Result<u64, String> {
    let n = match v {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.parse::<u64>().ok(),
        _ => None,
    };
    n.filter(|n| *n <= MAX_UINT53).ok_or_else(|| format!("{what} is not a UInt53"))
}

/// The profile network's genesis digest (Base58 text and its 32 bytes).
fn expected_chain(network: &Network) -> Result<(String, [u8; 32]), String> {
    let id = &network.identity.value;
    if id.kind != IdentityKind::SuiChainIdentifier {
        return Err("the profile identity is not a Sui chain identifier".into());
    }
    let bytes = base58_to_32(&id.value).map_err(|e| format!("the profile's chain identifier {e}"))?;
    Ok((id.value.clone(), bytes))
}

/// The provider's `chainIdentifier`, bound to the profile's.
fn check_chain(data: &Value, expected: &str) -> Result<(), String> {
    let reported = data.get("chainIdentifier").and_then(Value::as_str).ok_or("the answer has no chainIdentifier")?;
    if reported != expected {
        return Err("reports another network than the profile's chain identifier".into());
    }
    Ok(())
}

/// One provider's account of the Gate at the pinned checkpoint. Every field takes part in the vote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateReading {
    pub checkpoint_digest: String,
    pub epoch: u64,
    pub object_version: u64,
    pub gate: GateObject,
}

fn type_is_gate(repr: &str, package: &[u8; 32]) -> bool {
    if repr.contains('<') {
        return false;
    }
    let parts: Vec<&str> = repr.split("::").collect();
    parts.len() == 3 && parts[1] == MODULE && parts[2] == "Gate" && parse_address_padded(parts[0]).is_ok_and(|p| p == *package)
}

/// Parse one provider's answer to [`QUERY_GATE`] for checkpoint `n`.
pub fn parse_gate(data: &Value, target: &Target, chain: &(String, [u8; 32]), n: u64) -> Result<GateReading, String> {
    check_chain(data, &chain.0)?;
    // The checkpoint: the number asked for, a digest of 32 bytes, an epoch.
    let cp = data.get("checkpoint").filter(|c| c.is_object()).ok_or("the provider does not serve the checkpoint")?;
    if uint53(cp.get("sequenceNumber").unwrap_or(&Value::Null), "the checkpoint number")? != n {
        return Err("answered for another checkpoint than the one asked".into());
    }
    let digest = cp.get("digest").and_then(Value::as_str).ok_or("the checkpoint has no digest")?;
    base58_to_32(digest).map_err(|e| format!("the checkpoint digest {e}"))?;
    let epoch = uint53(cp.get("epoch").and_then(|e| e.get("epochId")).unwrap_or(&Value::Null), "the epoch")?;
    // The registered decimals: the supply is in e8s only if the coin has 8 decimals.
    let decimals = data
        .get("coinMetadata")
        .filter(|m| m.is_object())
        .and_then(|m| m.get("decimals"))
        .and_then(Value::as_u64)
        .ok_or("the currency's decimals are not registered (no coin metadata)")?;
    if decimals != ICUSD_DECIMALS {
        return Err(format!("the coin has {decimals} decimals, the bridged icUSD has {ICUSD_DECIMALS}"));
    }
    // The Gate object.
    let o = data.get("object").filter(|o| o.is_object()).ok_or("the Gate does not exist at that checkpoint")?;
    let address = parse_address_padded(o.get("address").and_then(Value::as_str).ok_or("the object has no address")?)?;
    if address != target.gate {
        return Err("answered for another object than the configured Gate".into());
    }
    let owner = o.get("owner").filter(|v| v.is_object()).ok_or("the object has no owner")?;
    let shared = owner.get("__typename").and_then(Value::as_str) == Some("Shared")
        && uint53(owner.get("initialSharedVersion").unwrap_or(&Value::Null), "the initial shared version")? > 0;
    if !shared {
        return Err("the Gate is not a shared object".into());
    }
    let object_version = uint53(o.get("version").unwrap_or(&Value::Null), "the object version")?;
    let contents = o
        .get("asMoveObject")
        .and_then(|m| m.get("contents"))
        .filter(|c| c.is_object())
        .ok_or("the object has no Move contents")?;
    let repr = contents.get("type").and_then(|t| t.get("repr")).and_then(Value::as_str).ok_or("the contents have no type")?;
    if !type_is_gate(repr, &target.package) {
        return Err(format!("the object type {repr:?} is not the configured package's Gate"));
    }
    let text = contents.get("bcs").and_then(Value::as_str).ok_or("the contents have no bcs")?;
    if text.len() > MAX_GATE_BYTES * 2 {
        return Err("the Gate contents are too large".into());
    }
    let bytes = base64_decode(text).map_err(|e| format!("the Gate contents: {e}"))?;
    let gate = decode_gate(&bytes)?;
    if gate.id != target.gate {
        return Err("the Gate contents carry another object id than the configured Gate".into());
    }
    if gate.chain_identifier != chain.1 {
        return Err("the Gate was created for another chain identifier than the profile's".into());
    }
    Ok(GateReading { checkpoint_digest: digest.to_string(), epoch, object_version, gate })
}

/// The Gate supply and counters at a numbered finalized checkpoint agreed by a quorum of providers.
pub fn read_supply(t: &dyn JsonRpcTransport, target: &Target, network: &Network) -> Result<SuiSupply, ReadError> {
    let fail = |e: String| ReadError::Failed(format!("{}: {e}", network.network_id));
    if network.finality.rule != FinalityRule::SuiCheckpoint {
        return Err(fail(format!("{:?} is not the Sui checkpoint rule", network.finality.rule)));
    }
    if network.finality.parameters.value.checkpoint.as_deref() != Some("finalized") {
        return Err(fail("the profile's checkpoint is not `finalized`".into()));
    }
    let quorum = usize::from(network.rpc.min_quorum.value);
    let count = t.provider_count();
    if quorum == 0 || quorum > count {
        return Err(fail(format!("quorum {quorum} is not in 1..={count} configured providers")));
    }
    let chain = expected_chain(network).map_err(fail)?;

    // 1. Network binding and the latest finalized checkpoint of every provider.
    let tips: Vec<Result<u64, String>> = (0..count)
        .map(|i| {
            let data = graphql(t, i, QUERY_HEAD, json!({}))?;
            check_chain(&data, &chain.0)?;
            let cp = data.get("checkpoint").filter(|c| c.is_object()).ok_or("the provider has no finalized checkpoint")?;
            uint53(cp.get("sequenceNumber").unwrap_or(&Value::Null), "the checkpoint number")
        })
        .collect();
    let mut heights: Vec<u64> = tips.iter().filter_map(|r| r.as_ref().ok()).copied().collect();
    if heights.len() < quorum {
        let excluded: Vec<String> = tips
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.as_ref().err().map(|e| format!("p{} {e}", i + 1)))
            .collect();
        return Err(fail(format!(
            "only {} of {count} providers are on the profile's network with a finalized checkpoint, quorum is {quorum}; excluded: {}",
            heights.len(),
            excluded.join("; ")
        )));
    }

    // 2. The checkpoint to read at: the operator's, or the quorum-th highest latest one.
    heights.sort_unstable_by(|a, b| b.cmp(a));
    let n = target.checkpoint.unwrap_or(heights[quorum - 1]);

    // 3. The Gate, the checkpoint and the decimals from one response per provider at or above `n`.
    let variables = json!({"c": n, "g": target.gate_text(), "t": target.coin_type()});
    let votes: Vec<Result<GateReading, String>> = tips
        .into_iter()
        .enumerate()
        .map(|(i, tip)| match tip {
            Ok(head) if head >= n => graphql(t, i, QUERY_GATE, variables.clone()).and_then(|d| parse_gate(&d, target, &chain, n)),
            Ok(head) => Err(format!("finalized checkpoint {head} is below the checkpoint {n} to read at")),
            Err(e) => Err(format!("excluded: {e}")),
        })
        .collect();

    // 4. A quorum of identical answers.
    let reading = tally(votes, quorum, &format!("the Gate at finalized checkpoint {n}")).map_err(fail)?;
    let g = reading.gate;
    Ok(SuiSupply {
        supply: u128::from(g.total_supply),
        finality: network.finality.rule,
        checkpoint: n,
        checkpoint_digest: reading.checkpoint_digest,
        epoch: reading.epoch,
        gate: SuiGate {
            version: g.version,
            supply_cap: g.supply_cap,
            minted_total: g.minted_total,
            burned_total: g.burned_total,
            mint_count: g.mint_count,
            bridge_out_count: g.bridge_out_count,
        },
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::live::evm::tests::network;
    use crate::live::solana::tests::{b58_encode, b64_encode};
    use std::cell::RefCell;

    /// What one scripted provider answers for a query text and its variables.
    pub(crate) type Script = Box<dyn Fn(&str, &Value) -> Result<Value, String>>;

    pub(crate) const TESTNET_CHAIN: &str = "69WiPg3DAQiwdxfncX6wYQ2siKwAe6L9BZthQea3JNMD";
    pub(crate) const MAINNET_CHAIN: &str = "4btiuiMPvEENsttpZC7CZ53DruC3MAgfznDbASZ7DR6S";
    pub(crate) const PACKAGE: [u8; 32] = [0x70; 32];
    pub(crate) const GATE: [u8; 32] = [0x6a; 32];

    pub(crate) fn address(a: &[u8; 32]) -> String {
        format!("0x{}", hex_of(a))
    }

    /// The accepted Sui crate's reference Gate (`GATE_BCS` in its golden vectors), byte for byte.
    pub(crate) const REFERENCE_GATE_HEX: &str = "6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a0100000000000000dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd00902f5009000000204c78adacf2a2f5ad80f27ed7d54aa69d3a78f1ca67fdef9ecf5754f5b8bb77b000e87648170000000100000000000000004a4d220f000000000000000000000000ba1dd205000000000000000000000004000000000000000300000000000000e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e10400000000000000e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e20300000000000000";

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()).collect()
    }

    /// The BCS of a Gate with the given numbers on the testnet chain.
    pub(crate) fn gate_bcs(supply: u64, minted: u128, burned: u128, mints: u64, returns: u64) -> Vec<u8> {
        gate_bcs_on(&testnet_bytes(), supply, minted, burned, mints, returns)
    }

    /// The BCS of a Gate created for `chain`, with the given numbers.
    pub(crate) fn gate_bcs_on(chain: &[u8; 32], supply: u64, minted: u128, burned: u128, mints: u64, returns: u64) -> Vec<u8> {
        let mut out = vec![];
        out.extend_from_slice(&GATE);
        out.extend_from_slice(&1u64.to_le_bytes());
        out.extend_from_slice(&[0x63; 32]);
        out.extend_from_slice(&supply.to_le_bytes());
        out.push(32);
        out.extend_from_slice(chain);
        out.extend_from_slice(&1_000_000_000_000u64.to_le_bytes()); // supply_cap
        out.extend_from_slice(&1u64.to_le_bytes()); // min_bridge_out
        out.extend_from_slice(&minted.to_le_bytes());
        out.extend_from_slice(&burned.to_le_bytes());
        out.extend_from_slice(&mints.to_le_bytes());
        out.extend_from_slice(&returns.to_le_bytes());
        out.extend_from_slice(&[0x61; 32]);
        out.extend_from_slice(&mints.to_le_bytes());
        out.extend_from_slice(&[0x62; 32]);
        out.extend_from_slice(&returns.to_le_bytes());
        out
    }

    pub(crate) fn testnet_bytes() -> [u8; 32] {
        base58_to_32(TESTNET_CHAIN).unwrap()
    }

    pub(crate) fn gate_object_json(bcs: &[u8], version: u64) -> Value {
        json!({
            "address": address(&GATE),
            "version": version,
            "digest": b58_encode(&[0x77; 32]),
            "owner": {"__typename": "Shared", "initialSharedVersion": 42},
            "asMoveObject": {"contents": {"type": {"repr": format!("{}::rumi_icusd_gate::Gate", address(&PACKAGE))}, "bcs": b64_encode(bcs)}}
        })
    }

    /// One provider on `chain` whose latest finalized checkpoint is `head`, serving the checkpoints up to it
    /// and answering the Gate query with `bcs`.
    pub(crate) fn provider(chain: &'static str, head: u64, digest_seed: u8, bcs: Vec<u8>) -> Script {
        Box::new(move |query, vars| {
            if query == QUERY_HEAD {
                return Ok(json!({"data": {"chainIdentifier": chain, "checkpoint": {"sequenceNumber": head}}}));
            }
            assert_eq!(query, QUERY_GATE);
            let c = vars["c"].as_u64().unwrap();
            let checkpoint = if c > head {
                Value::Null
            } else {
                json!({"sequenceNumber": c, "digest": b58_encode(&[digest_seed; 32]), "epoch": {"epochId": 9}})
            };
            Ok(json!({"data": {"chainIdentifier": chain, "checkpoint": checkpoint, "object": gate_object_json(&bcs, 105), "coinMetadata": {"decimals": 8}}}))
        })
    }

    pub(crate) struct SuiMock {
        pub scripts: Vec<Script>,
        pub log: RefCell<Vec<(usize, String, Value)>>,
    }

    impl SuiMock {
        pub(crate) fn new(scripts: Vec<Script>) -> SuiMock {
            SuiMock { scripts, log: RefCell::new(vec![]) }
        }
    }

    impl JsonRpcTransport for SuiMock {
        fn provider_count(&self) -> usize {
            self.scripts.len()
        }
        fn post(&self, index: usize, body: &str) -> Result<String, String> {
            let req: Value = serde_json::from_str(body).unwrap();
            let query = req["query"].as_str().unwrap().to_string();
            self.log.borrow_mut().push((index, query.clone(), req["variables"].clone()));
            match (self.scripts[index])(&query, &req["variables"]) {
                Ok(v) => Ok(v.to_string()),
                Err(m) if m == "TRANSPORT" => Err("connection refused".into()),
                Err(m) => Ok(json!({"errors": [{"message": m}]}).to_string()),
            }
        }
    }

    pub(crate) fn target() -> Target {
        Target::new(&address(&PACKAGE), &address(&GATE), None).unwrap()
    }

    pub(crate) fn sui_network() -> Network {
        // The testnet network, whose chain identifier the fixtures use.
        network("sui-testnet")
    }

    #[test]
    fn the_accepted_reference_gate_decodes_with_the_collectors_own_reader() {
        // Cross-implementation evidence: these bytes are the Sui crate's golden `GATE_BCS` (copied
        // verbatim; the value set it decodes to is asserted in that crate's `decodes_the_reference_gate`).
        // This decoder is a separate implementation, so agreement on every field is real evidence that the
        // two sides read the same layout.
        let g = decode_gate(&unhex(REFERENCE_GATE_HEX)).unwrap();
        assert_eq!((g.id, g.version, g.total_supply), (GATE, 1, 400 * 100_000_000));
        assert_eq!(g.chain_identifier, testnet_bytes().to_vec(), "the stored identifier is testnet's genesis digest");
        assert_eq!((g.supply_cap, g.min_bridge_out), (1000 * 100_000_000, 1));
        assert_eq!((g.minted_total, g.burned_total), (650 * 100_000_000, 250 * 100_000_000));
        assert_eq!((g.mint_count, g.bridge_out_count), (4, 3));
        assert_eq!((g.mints_table, g.mints_size, g.returns_table, g.returns_size), ([0xe1; 32], 4, [0xe2; 32], 3));
        // And a whole read over it, through the real parse path.
        let bytes = unhex(REFERENCE_GATE_HEX);
        let mock = SuiMock::new(vec![provider(TESTNET_CHAIN, 800, 5, bytes.clone()), provider(TESTNET_CHAIN, 800, 5, bytes)]);
        let got = read_supply(&mock, &target(), &sui_network()).unwrap();
        assert_eq!((got.supply, got.gate.minted_total, got.gate.burned_total), (40_000_000_000, 65_000_000_000, 25_000_000_000));
    }

    #[test]
    fn the_layout_follows_the_move_struct_order_and_refuses_what_the_gate_could_not_hold() {
        let ok = decode_gate(&gate_bcs(300, 500, 200, 3, 2)).unwrap();
        assert_eq!((ok.id, ok.version, ok.total_supply, ok.supply_cap, ok.min_bridge_out), (GATE, 1, 300, 1_000_000_000_000, 1));
        assert_eq!((ok.minted_total, ok.burned_total, ok.mint_count, ok.bridge_out_count, ok.mints_size, ok.returns_size), (500, 200, 3, 2, 3, 2));
        assert_eq!((ok.chain_identifier, ok.mints_table, ok.returns_table), (testnet_bytes().to_vec(), [0x61; 32], [0x62; 32]));
        // Each of these breaks one rule.
        let mut v = gate_bcs(300, 500, 200, 3, 2);
        v.push(0);
        assert!(decode_gate(&v).unwrap_err().contains("trailing"));
        assert!(decode_gate(&gate_bcs(300, 500, 200, 3, 2)[..100]).unwrap_err().contains("end early"));
        assert!(decode_gate(&gate_bcs(301, 500, 200, 3, 2)).unwrap_err().contains("minted_total less burned_total"), "supply identity");
        assert!(decode_gate(&gate_bcs(300, 200, 500, 3, 2)).unwrap_err().contains("minted_total less burned_total"), "burned above minted");
        let mut table = gate_bcs(300, 500, 200, 3, 2);
        let n = table.len();
        table[n - 8..].copy_from_slice(&5u64.to_le_bytes());
        assert!(decode_gate(&table).unwrap_err().contains("table size"));
        let mut version = gate_bcs(300, 500, 200, 3, 2);
        version[32..40].copy_from_slice(&2u64.to_le_bytes());
        assert!(decode_gate(&version).unwrap_err().contains("code version 2"));
        let mut min = gate_bcs(300, 500, 200, 3, 2);
        let at = 32 + 8 + 32 + 8 + 1 + 32 + 8;
        min[at..at + 8].copy_from_slice(&0u64.to_le_bytes());
        assert!(decode_gate(&min).unwrap_err().contains("minimum return is zero"));
        let mut short_chain = gate_bcs(300, 500, 200, 3, 2);
        short_chain[32 + 8 + 32 + 8] = 31; // the stored chain identifier claims 31 bytes
        assert!(decode_gate(&short_chain).is_err());
        let mut same = gate_bcs(300, 500, 200, 3, 2);
        let tables_at = same.len() - 8 - 32 - 8 - 32;
        same[tables_at..tables_at + 32].copy_from_slice(&GATE);
        assert!(decode_gate(&same).unwrap_err().contains("same"));
    }

    #[test]
    fn the_operators_input_is_strict() {
        let p = address(&PACKAGE);
        let g = address(&GATE);
        assert!(Target::new(&p, &g, Some(5)).is_ok());
        for (package, gate, why) in [
            ("0x70", g.as_str(), "short package"),
            (p.as_str(), "6a", "no prefix"),
            (p.as_str(), g.to_uppercase().as_str(), "upper case"),
            (p.as_str(), p.as_str(), "same address"),
            (&format!("0x{}", "00".repeat(32)), g.as_str(), "zero package"),
        ] {
            assert!(Target::new(package, gate, None).is_err(), "{why}");
        }
        assert!(Target::new(&p, &g, Some(0)).is_err());
        assert!(Target::new(&p, &g, Some(1 << 53)).is_err());
    }

    #[test]
    fn three_agreeing_providers_give_the_gate_at_the_quorum_th_highest_checkpoint() {
        let bcs = gate_bcs(300, 500, 200, 3, 2);
        // Heads 900 (an outlier far ahead), 700 and 650 with quorum 2: n = 700.
        let mock = SuiMock::new(vec![
            provider(TESTNET_CHAIN, 900, 5, bcs.clone()),
            provider(TESTNET_CHAIN, 700, 5, bcs.clone()),
            provider(TESTNET_CHAIN, 650, 5, bcs.clone()),
        ]);
        let got = read_supply(&mock, &target(), &sui_network()).unwrap();
        assert_eq!((got.supply, got.checkpoint, got.epoch, got.finality), (300, 700, 9, FinalityRule::SuiCheckpoint));
        assert_eq!(got.checkpoint_digest, b58_encode(&[5; 32]));
        assert_eq!((got.gate.minted_total, got.gate.burned_total, got.gate.mint_count, got.gate.bridge_out_count, got.gate.version), (500, 200, 3, 2, 1));
        // The Gate query names the numbered checkpoint on every voting provider and never asks "latest" for it.
        let gate_queries: Vec<(usize, u64)> = mock.log.borrow().iter().filter(|(_, q, _)| q == QUERY_GATE).map(|(i, _, v)| (*i, v["c"].as_u64().unwrap())).collect();
        assert_eq!(gate_queries, vec![(0, 700), (1, 700)], "the lagging provider (650) does not vote");
        assert!(QUERY_GATE.contains("atCheckpoint:$c") && QUERY_GATE.contains("checkpoint(sequenceNumber:$c)"));
    }

    #[test]
    fn an_explicit_checkpoint_is_the_one_read_and_every_voter_must_serve_it() {
        let bcs = gate_bcs(300, 500, 200, 3, 2);
        let pinned = Target::new(&address(&PACKAGE), &address(&GATE), Some(600)).unwrap();
        let mock = SuiMock::new(vec![provider(TESTNET_CHAIN, 700, 5, bcs.clone()), provider(TESTNET_CHAIN, 650, 5, bcs.clone()), provider(TESTNET_CHAIN, 500, 5, bcs.clone())]);
        let got = read_supply(&mock, &pinned, &sui_network()).unwrap();
        assert_eq!(got.checkpoint, 600);
        let asked: Vec<usize> = mock.log.borrow().iter().filter(|(_, q, _)| q == QUERY_GATE).map(|(i, _, _)| *i).collect();
        assert_eq!(asked, vec![0, 1]);
        // Only one provider has reached the checkpoint: no quorum.
        let mock = SuiMock::new(vec![provider(TESTNET_CHAIN, 700, 5, bcs.clone()), provider(TESTNET_CHAIN, 599, 5, bcs.clone()), provider(TESTNET_CHAIN, 100, 5, bcs)]);
        let err = read_supply(&mock, &pinned, &sui_network()).unwrap_err();
        assert!(err.to_string().contains("no quorum") || err.to_string().contains("failed"), "{err}");
    }

    #[test]
    fn a_provider_on_another_network_or_in_error_is_excluded_and_too_few_is_a_named_failure() {
        let bcs = gate_bcs(300, 500, 200, 3, 2);
        // One provider reports mainnet: excluded; the other two agree.
        let mock = SuiMock::new(vec![provider(MAINNET_CHAIN, 700, 5, bcs.clone()), provider(TESTNET_CHAIN, 700, 5, bcs.clone()), provider(TESTNET_CHAIN, 700, 5, bcs.clone())]);
        assert_eq!(read_supply(&mock, &target(), &sui_network()).unwrap().supply, 300);
        // A second one down: one usable provider is below the quorum of two.
        let down: Script = Box::new(|_, _| Err("TRANSPORT".into()));
        let mock = SuiMock::new(vec![provider(MAINNET_CHAIN, 700, 5, bcs.clone()), down, provider(TESTNET_CHAIN, 700, 5, bcs.clone())]);
        let err = read_supply(&mock, &target(), &sui_network()).unwrap_err().to_string();
        assert!(err.contains("only 1 of 3 providers") && err.contains("another network") && err.contains("connection refused"), "{err}");
        assert!(!err.contains("https://"), "providers are named by position");
    }

    #[test]
    fn graphql_errors_valid_or_malformed_make_an_answer_unusable() {
        let bcs = gate_bcs(300, 500, 200, 3, 2);
        let with_errors = |errors: Value| -> Script {
            let inner = provider(TESTNET_CHAIN, 700, 5, bcs.clone());
            Box::new(move |q, v| {
                let mut r = inner(q, v)?;
                r["errors"] = errors.clone();
                Ok(r)
            })
        };
        for errors in [json!([{"message": "rate limited"}]), json!({"message": "not an array"}), json!("oops"), Value::Null] {
            let mock = SuiMock::new(vec![with_errors(errors.clone()), provider(TESTNET_CHAIN, 700, 5, bcs.clone())]);
            let err = read_supply(&mock, &target(), &sui_network()).unwrap_err().to_string();
            assert!(err.contains("only 1 of 2"), "{errors}: {err}");
        }
        // An empty errors array is not an error.
        let mock = SuiMock::new(vec![with_errors(json!([])), provider(TESTNET_CHAIN, 700, 5, bcs.clone())]);
        assert_eq!(read_supply(&mock, &target(), &sui_network()).unwrap().supply, 300);
    }

    #[test]
    fn disagreement_is_a_failure_never_an_average() {
        let a = gate_bcs(300, 500, 200, 3, 2);
        let b = gate_bcs(310, 510, 200, 4, 2);
        // Two against one: the two agreeing providers win.
        let mock = SuiMock::new(vec![provider(TESTNET_CHAIN, 700, 5, a.clone()), provider(TESTNET_CHAIN, 700, 5, a.clone()), provider(TESTNET_CHAIN, 700, 5, b.clone())]);
        assert_eq!(read_supply(&mock, &target(), &sui_network()).unwrap().supply, 300);
        // One against one: no quorum.
        let mock = SuiMock::new(vec![provider(TESTNET_CHAIN, 700, 5, a.clone()), provider(TESTNET_CHAIN, 700, 5, b)]);
        let err = read_supply(&mock, &target(), &sui_network()).unwrap_err().to_string();
        assert!(err.contains("no quorum"), "{err}");
        // The same supply but a different checkpoint digest is a different answer too.
        let mock = SuiMock::new(vec![provider(TESTNET_CHAIN, 700, 5, a.clone()), provider(TESTNET_CHAIN, 700, 6, a)]);
        assert!(read_supply(&mock, &target(), &sui_network()).unwrap_err().to_string().contains("no quorum"));
    }

    fn mutated(mutate: impl Fn(&mut Value) + 'static, bcs: Vec<u8>) -> Script {
        let inner = provider(TESTNET_CHAIN, 700, 5, bcs);
        Box::new(move |q, v| {
            let mut r = inner(q, v)?;
            if q == QUERY_GATE {
                mutate(&mut r["data"]);
            }
            Ok(r)
        })
    }

    #[test]
    fn a_wrong_gate_type_owner_decimals_chain_or_object_is_a_named_failure_not_a_zero() {
        let bcs = gate_bcs(300, 500, 200, 3, 2);
        type Mutation = Box<dyn Fn(&mut Value)>;
        let cases: Vec<(&str, Mutation, &str)> = vec![
            ("another object", Box::new(|d| d["object"]["address"] = json!(address(&[0x6b; 32]))), "another object"),
            ("another package", Box::new(|d| d["object"]["asMoveObject"]["contents"]["type"]["repr"] = json!(format!("{}::rumi_icusd_gate::Gate", address(&[0x71; 32])))), "not the configured package's Gate"),
            ("another module", Box::new(|d| d["object"]["asMoveObject"]["contents"]["type"]["repr"] = json!(format!("{}::other::Gate", address(&PACKAGE)))), "not the configured package's Gate"),
            ("another struct", Box::new(|d| d["object"]["asMoveObject"]["contents"]["type"]["repr"] = json!(format!("{}::rumi_icusd_gate::MintCap", address(&PACKAGE)))), "not the configured package's Gate"),
            ("an owned gate", Box::new(|d| d["object"]["owner"] = json!({"__typename": "AddressOwner"})), "not a shared object"),
            ("a missing gate", Box::new(|d| d["object"] = Value::Null), "does not exist"),
            ("wrong decimals", Box::new(|d| d["coinMetadata"]["decimals"] = json!(6)), "6 decimals"),
            ("no metadata", Box::new(|d| d["coinMetadata"] = Value::Null), "not registered"),
            ("another checkpoint", Box::new(|d| d["checkpoint"]["sequenceNumber"] = json!(699)), "another checkpoint"),
            ("no checkpoint", Box::new(|d| d["checkpoint"] = Value::Null), "does not serve the checkpoint"),
            ("a bad digest", Box::new(|d| d["checkpoint"]["digest"] = json!("not base58!")), "digest"),
            ("another network", Box::new(|d| d["chainIdentifier"] = json!(MAINNET_CHAIN)), "another network"),
        ];
        for (what, mutate, expect) in cases {
            let mock = SuiMock::new(vec![mutated(mutate, bcs.clone()), provider(TESTNET_CHAIN, 700, 5, bcs.clone())]);
            let err = read_supply(&mock, &target(), &sui_network()).unwrap_err().to_string();
            // Both providers are on the network and have a head, so the head step passes; the vote then has one
            // usable answer against a quorum of two, and the refused one is named by position and reason.
            assert!(err.contains("no quorum") && err.contains("p1 failed") && err.contains(expect), "{what}: {err}");
        }
        // A Gate created for another chain identifier (its stored one is mainnet's) is refused even when the
        // object itself looks right.
        let mut foreign = gate_bcs(300, 500, 200, 3, 2);
        let at = 32 + 8 + 32 + 8 + 1;
        foreign[at..at + 32].copy_from_slice(&base58_to_32(MAINNET_CHAIN).unwrap());
        let mock = SuiMock::new(vec![provider(TESTNET_CHAIN, 700, 5, foreign.clone()), provider(TESTNET_CHAIN, 700, 5, foreign)]);
        let err = read_supply(&mock, &target(), &sui_network()).unwrap_err().to_string();
        assert!(err.contains("another chain identifier"), "{err}");
    }

    #[test]
    fn the_profile_must_be_a_sui_checkpoint_network_with_a_reachable_quorum() {
        let bcs = gate_bcs(300, 500, 200, 3, 2);
        // A non-Sui profile network is refused before any request.
        let mock = SuiMock::new(vec![provider(TESTNET_CHAIN, 700, 5, bcs.clone()), provider(TESTNET_CHAIN, 700, 5, bcs.clone())]);
        let err = read_supply(&mock, &target(), &network("base-mainnet")).unwrap_err().to_string();
        assert!(err.contains("not the Sui checkpoint rule"), "{err}");
        assert!(mock.log.borrow().is_empty());
        // One provider cannot meet a quorum of two.
        let mock = SuiMock::new(vec![provider(TESTNET_CHAIN, 700, 5, bcs)]);
        assert!(read_supply(&mock, &target(), &sui_network()).unwrap_err().to_string().contains("quorum 2 is not in 1..=1"));
    }
}
