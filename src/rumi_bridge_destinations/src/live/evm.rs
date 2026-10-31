//! EVM side of the live transport (feature `live-transport`).
//!
//! A small trait ([`JsonRpcTransport`]: post one JSON-RPC body to provider `i`,
//! get the response body back) and the supply read built on it. The head rule
//! mirrors the bridge canister's (`rumi_icusd_bridge`, `chains.rs` and
//! `evm/rpc.rs`), but is driven by the destination PROFILE:
//!
//! * `TagFinalizedQuorum` (`evm_tag` rule): ask every provider for its
//!   `finalized` block, take `n` as the `quorum`-th highest reported height
//!   (one far-ahead outlier cannot raise it), then require a quorum on the
//!   block hash at exactly `n`. A provider that reported `n` is reused; one that
//!   reported more is asked for block `n`; one below `n` does not vote.
//! * `MonadVerified` (`monad_verified` rule): the same `n`, then head `n - 3`
//!   (the profile's `execution_delay_blocks`), with the hash at the head agreed
//!   by a quorum.
//! * `Blocked`: a profile whose head cannot be read safely returns
//!   [`ReadError::UnavailableGap`] before any request is sent.
//!
//! `totalSupply()` is then called at the NUMBERED head on every provider and a
//! quorum of identical answers is required. No request ever uses `latest`,
//! `pending`, `safe` or `earliest`: the only tag sent is `finalized`, for head
//! discovery. Providers are named by position in errors, never by URL.
//!
//! [`ReqwestJsonRpc`] is the real transport (blocking `reqwest`, rustls, no
//! redirects, bounded body). It is never constructed by anything but an
//! explicit endpoint config.

use serde_json::{json, Value};

use crate::collect::{EvmSupply, ReadError};
use crate::types::{FinalityRule, Network};

/// `totalSupply()` selector.
const TOTAL_SUPPLY_SELECTOR: &str = "0x18160ddd";

/// Post JSON-RPC bodies to a fixed, ordered set of providers.
pub trait JsonRpcTransport {
    fn provider_count(&self) -> usize;
    /// Post one request body to provider `index` and return the response body.
    /// An error must not contain the provider URL.
    fn post(&self, index: usize, body: &str) -> Result<String, String>;
}

// ---------------------------------------------------------------------------
// Head rule, driven by the profile
// ---------------------------------------------------------------------------

/// Profile blockers that make a head unsafe to read (the bridge's `Blocked`
/// rows: Robinhood's finalized tag is unverified, HyperEVM lacks historical
/// state and its finality semantics are unverified).
const HEAD_BLOCKERS: [&str; 3] = [
    "robinhood-finalized-tag-unverified",
    "hyperevm-historical-state",
    "hyperevm-finality-semantics-unverified",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadRule {
    TagFinalizedQuorum,
    MonadVerified { delay_blocks: u64 },
    Blocked(String),
}

/// The head rule of a profile network.
pub fn head_rule(network: &Network) -> HeadRule {
    if let Some(b) = network
        .activation_blockers
        .iter()
        .find(|b| HEAD_BLOCKERS.contains(&b.id.as_str()))
    {
        return HeadRule::Blocked(b.id.clone());
    }
    let params = &network.finality.parameters.value;
    match network.finality.rule {
        FinalityRule::EvmTag => {
            if params.tag.as_deref() == Some("finalized") {
                HeadRule::TagFinalizedQuorum
            } else {
                HeadRule::Blocked("the profile's head tag is not `finalized`".into())
            }
        }
        FinalityRule::MonadVerified => match params.execution_delay_blocks {
            Some(d) => HeadRule::MonadVerified {
                delay_blocks: u64::from(d),
            },
            None => HeadRule::Blocked("the Monad profile has no execution_delay_blocks".into()),
        },
        other => HeadRule::Blocked(format!("{other:?} is not an EVM head rule")),
    }
}

// ---------------------------------------------------------------------------
// JSON-RPC helpers
// ---------------------------------------------------------------------------

/// A block as `(number, hash)`.
type BlockRef = (u64, [u8; 32]);

fn quantity(n: u64) -> String {
    format!("0x{n:x}")
}

fn parse_quantity(v: &Value, what: &str) -> Result<u64, String> {
    let s = v
        .as_str()
        .ok_or_else(|| format!("{what} is not a string"))?;
    let digits = s
        .strip_prefix("0x")
        .ok_or_else(|| format!("{what} {s:?} is not 0x-prefixed"))?;
    if digits.is_empty()
        || digits.len() > 16
        || !digits.bytes().all(|b| b.is_ascii_hexdigit())
        || (digits.len() > 1 && digits.starts_with('0'))
    {
        return Err(format!("{what} {s:?} is not a canonical quantity"));
    }
    u64::from_str_radix(digits, 16).map_err(|e| format!("{what} {s:?}: {e}"))
}

fn parse_hex_bytes(s: &str, what: &str) -> Result<Vec<u8>, String> {
    let digits = s
        .strip_prefix("0x")
        .ok_or_else(|| format!("{what} is not 0x-prefixed"))?;
    if digits.len() % 2 != 0 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("{what} is not valid hex"));
    }
    (0..digits.len() / 2)
        .map(|i| {
            u8::from_str_radix(&digits[2 * i..2 * i + 2], 16).map_err(|e| format!("{what}: {e}"))
        })
        .collect()
}

fn parse_block(v: &Value, expect: Option<u64>) -> Result<BlockRef, String> {
    if v.is_null() {
        return Err("block not found".into());
    }
    let o = v.as_object().ok_or("block is not an object")?;
    let number = parse_quantity(
        o.get("number").ok_or("block has no number")?,
        "block number",
    )?;
    if let Some(e) = expect {
        if number != e {
            return Err(format!("asked for block {e}, got block {number}"));
        }
    }
    let hash_text = o
        .get("hash")
        .and_then(Value::as_str)
        .ok_or("block has no hash")?;
    let bytes = parse_hex_bytes(hash_text, "block hash")?;
    let hash: [u8; 32] = bytes
        .try_into()
        .map_err(|_| "block hash is not 32 bytes".to_string())?;
    Ok((number, hash))
}

/// Refuse to send a request that names a non-numbered, non-`finalized` block
/// tag. Applied to every outgoing request, so `latest` cannot slip in.
fn check_no_forbidden_tag(params: &Value) -> Result<(), String> {
    fn walk(v: &Value) -> bool {
        match v {
            Value::String(s) => ["latest", "pending", "safe", "earliest"].contains(&s.as_str()),
            Value::Array(a) => a.iter().any(walk),
            Value::Object(o) => o.values().any(walk),
            _ => false,
        }
    }
    if walk(params) {
        return Err("refusing to send a request with a non-numbered block tag".into());
    }
    Ok(())
}

/// One request to one provider; returns the `result` value.
fn call(t: &dyn JsonRpcTransport, i: usize, method: &str, params: Value) -> Result<Value, String> {
    check_no_forbidden_tag(&params)?;
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string();
    let text = t.post(i, &body)?;
    let envelope: Value =
        serde_json::from_str(&text).map_err(|e| format!("response is not JSON: {e}"))?;
    let o = envelope.as_object().ok_or("response is not an object")?;
    if o.get("id").and_then(Value::as_u64) != Some(1) {
        return Err("response id does not match the request".into());
    }
    if let Some(err) = o.get("error") {
        let code = err.get("code").and_then(Value::as_i64).unwrap_or(0);
        let message = err.get("message").and_then(Value::as_str).unwrap_or("");
        let message: String = message.chars().take(120).collect();
        return Err(format!("rpc error {code}: {message}"));
    }
    o.get("result")
        .cloned()
        .ok_or_else(|| "response has neither result nor error".to_string())
}

fn providers_label(idx: &[usize]) -> String {
    idx.iter()
        .map(|i| format!("p{}", i + 1))
        .collect::<Vec<_>>()
        .join(",")
}

/// A value wins when exactly one distinct value was returned by at least
/// `quorum` providers. Providers are named by position only.
fn tally<T: PartialEq + Clone + std::fmt::Debug>(
    parsed: Vec<Result<T, String>>,
    quorum: usize,
    what: &str,
) -> Result<T, String> {
    let total = parsed.len();
    let mut groups: Vec<(T, Vec<usize>)> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for (i, p) in parsed.into_iter().enumerate() {
        match p {
            Ok(v) => match groups.iter_mut().find(|(g, _)| *g == v) {
                Some((_, idx)) => idx.push(i),
                None => groups.push((v, vec![i])),
            },
            Err(e) => errors.push(format!("p{} failed: {}", i + 1, e)),
        }
    }
    if groups.is_empty() {
        return Err(format!(
            "{what}: all {total} providers failed: {}",
            errors.join("; ")
        ));
    }
    let winners: Vec<usize> = groups
        .iter()
        .enumerate()
        .filter(|(_, (_, idx))| idx.len() >= quorum)
        .map(|(g, _)| g)
        .collect();
    if winners.len() == 1 {
        return Ok(groups.swap_remove(winners[0]).0);
    }
    let mut parts: Vec<String> = groups
        .iter()
        .map(|(v, idx)| format!("[{}] answered {v:?}", providers_label(idx)))
        .collect();
    parts.extend(errors);
    let why = if winners.is_empty() {
        "no quorum"
    } else {
        "conflicting answers each reached quorum"
    };
    Err(format!(
        "{what}: {why} (need {quorum} identical of {total}): {}",
        parts.join("; ")
    ))
}

/// The `quorum`-th highest reported height.
fn agreed_height(heads: &[Result<BlockRef, String>], quorum: usize) -> Result<u64, String> {
    let mut numbers: Vec<u64> = heads
        .iter()
        .filter_map(|h| h.as_ref().ok())
        .map(|b| b.0)
        .collect();
    if numbers.len() < quorum {
        return Err(format!(
            "finalized head: only {} of {} providers answered, need {quorum}",
            numbers.len(),
            heads.len()
        ));
    }
    numbers.sort_unstable_by(|a, b| b.cmp(a));
    Ok(numbers[quorum - 1])
}

fn block_params_by_number(n: u64) -> Value {
    json!([quantity(n), false])
}

/// The quorum-agreed finalized block `(n, hash)`.
fn tag_finalized_quorum(t: &dyn JsonRpcTransport, quorum: usize) -> Result<BlockRef, String> {
    let count = t.provider_count();
    let heads: Vec<Result<BlockRef, String>> = (0..count)
        .map(|i| {
            call(t, i, "eth_getBlockByNumber", json!(["finalized", false]))
                .and_then(|v| parse_block(&v, None))
        })
        .collect();
    let n = agreed_height(&heads, quorum)?;
    let votes: Vec<Result<BlockRef, String>> = heads
        .into_iter()
        .enumerate()
        .map(|(i, h)| match h {
            Ok(b) if b.0 == n => Ok(b),
            Ok(b) if b.0 > n => call(t, i, "eth_getBlockByNumber", block_params_by_number(n))
                .and_then(|v| parse_block(&v, Some(n))),
            Ok(b) => Err(format!(
                "finalized height {} is below the agreed height {n}",
                b.0
            )),
            Err(e) => Err(e),
        })
        .collect();
    tally(votes, quorum, &format!("finalized block {n}"))
}

/// Block `n` as `(number, hash)`, by quorum.
fn block_by_number(t: &dyn JsonRpcTransport, quorum: usize, n: u64) -> Result<BlockRef, String> {
    let votes: Vec<Result<BlockRef, String>> = (0..t.provider_count())
        .map(|i| {
            call(t, i, "eth_getBlockByNumber", block_params_by_number(n))
                .and_then(|v| parse_block(&v, Some(n)))
        })
        .collect();
    tally(votes, quorum, &format!("block {n}"))
}

/// The settlement head height for a rule, by quorum.
fn head_height(t: &dyn JsonRpcTransport, quorum: usize, rule: &HeadRule) -> Result<u64, String> {
    match rule {
        HeadRule::TagFinalizedQuorum => Ok(tag_finalized_quorum(t, quorum)?.0),
        HeadRule::MonadVerified { delay_blocks } => {
            let finalized = tag_finalized_quorum(t, quorum)?.0;
            let head = finalized.checked_sub(*delay_blocks).ok_or_else(|| {
                format!(
                    "verified head: finalized height {finalized} is below the execution delay of {delay_blocks} blocks"
                )
            })?;
            Ok(block_by_number(t, quorum, head)?.0)
        }
        HeadRule::Blocked(reason) => Err(format!("blocked: {reason}")),
    }
}

/// `totalSupply()` of `token` at numbered block `head`, by quorum on the bytes.
fn total_supply_at(
    t: &dyn JsonRpcTransport,
    quorum: usize,
    token: &[u8; 20],
    head: u64,
) -> Result<u128, String> {
    let to = format!(
        "0x{}",
        token.iter().map(|b| format!("{b:02x}")).collect::<String>()
    );
    let votes: Vec<Result<Vec<u8>, String>> = (0..t.provider_count())
        .map(|i| {
            let params = json!([{"to": to, "data": TOTAL_SUPPLY_SELECTOR}, quantity(head)]);
            call(t, i, "eth_call", params).and_then(|v| {
                let s = v.as_str().ok_or("eth_call result is not a string")?;
                parse_hex_bytes(s, "eth_call result")
            })
        })
        .collect();
    let bytes = tally(votes, quorum, &format!("totalSupply at block {head}"))?;
    if bytes.len() != 32 {
        return Err(format!(
            "totalSupply returned {} bytes, expected 32",
            bytes.len()
        ));
    }
    if bytes[..16].iter().any(|b| *b != 0) {
        return Err("totalSupply does not fit in u128".into());
    }
    let mut low = [0u8; 16];
    low.copy_from_slice(&bytes[16..]);
    Ok(u128::from_be_bytes(low))
}

/// Read a destination token's `totalSupply` at the profile's settlement head.
///
/// `quorum` and the head rule come from the profile. A blocked rule returns
/// [`ReadError::UnavailableGap`] before any request is sent. The reading carries
/// the head height the supply was read at.
pub fn read_total_supply(
    t: &dyn JsonRpcTransport,
    token: &[u8; 20],
    network: &Network,
) -> Result<EvmSupply, ReadError> {
    let rule = head_rule(network);
    if let HeadRule::Blocked(reason) = &rule {
        return Err(ReadError::UnavailableGap(format!(
            "{} head is blocked ({reason}); no request was sent",
            network.network_id
        )));
    }
    let quorum = usize::from(network.rpc.min_quorum.value);
    let count = t.provider_count();
    if quorum == 0 || quorum > count {
        return Err(ReadError::Failed(format!(
            "{}: quorum {quorum} is not in 1..={count} configured providers",
            network.network_id
        )));
    }
    let fail = |e: String| ReadError::Failed(format!("{}: {e}", network.network_id));
    let head = head_height(t, quorum, &rule).map_err(fail)?;
    let supply = total_supply_at(t, quorum, token, head).map_err(fail)?;
    Ok(EvmSupply {
        supply,
        finality: network.finality.rule,
        block: head,
    })
}

// ---------------------------------------------------------------------------
// Real transport
// ---------------------------------------------------------------------------

/// Largest JSON-RPC response body accepted.
const MAX_BODY_BYTES: u64 = 1 << 20;

/// Blocking JSON-RPC over `reqwest` with rustls. No redirects, a 30 second
/// timeout and a bounded body. URLs are operator input (they may carry a key),
/// so no error repeats them.
pub struct ReqwestJsonRpc {
    urls: Vec<String>,
    client: reqwest::blocking::Client,
}

impl ReqwestJsonRpc {
    pub fn new(urls: Vec<String>) -> Result<ReqwestJsonRpc, String> {
        if urls.is_empty() {
            return Err("no provider URLs".into());
        }
        for u in &urls {
            super::ic::check_endpoint_url(u)?;
        }
        let client = reqwest::blocking::Client::builder()
            .use_rustls_tls()
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| format!("cannot build the HTTP client: {}", e.without_url()))?;
        Ok(ReqwestJsonRpc { urls, client })
    }
}

impl JsonRpcTransport for ReqwestJsonRpc {
    fn provider_count(&self) -> usize {
        self.urls.len()
    }

    fn post(&self, index: usize, body: &str) -> Result<String, String> {
        use std::io::Read;
        let url = self
            .urls
            .get(index)
            .ok_or_else(|| "provider index out of range".to_string())?;
        let response = self
            .client
            .post(url)
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .map_err(|e| format!("request failed: {}", e.without_url()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("HTTP status {}", status.as_u16()));
        }
        let mut text = String::new();
        response
            .take(MAX_BODY_BYTES + 1)
            .read_to_string(&mut text)
            .map_err(|e| format!("cannot read the response: {e}"))?;
        if text.len() as u64 > MAX_BODY_BYTES {
            return Err("response body is too large".into());
        }
        Ok(text)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::aggregate::Catalog;
    use std::cell::RefCell;

    /// What one scripted provider answers for a method and its params.
    pub(crate) type Script = Box<dyn Fn(&str, &Value) -> Result<Value, String>>;

    /// A mock transport: provider `i` answers through `scripts[i]`; every request
    /// body is recorded as `(provider, parsed body)`.
    pub(crate) struct Mock {
        pub scripts: Vec<Script>,
        pub log: RefCell<Vec<(usize, Value)>>,
    }

    impl Mock {
        pub(crate) fn new(scripts: Vec<Script>) -> Mock {
            Mock {
                scripts,
                log: RefCell::new(Vec::new()),
            }
        }
    }

    impl JsonRpcTransport for Mock {
        fn provider_count(&self) -> usize {
            self.scripts.len()
        }
        fn post(&self, index: usize, body: &str) -> Result<String, String> {
            let req: Value = serde_json::from_str(body).unwrap();
            self.log.borrow_mut().push((index, req.clone()));
            let method = req["method"].as_str().unwrap().to_string();
            match (self.scripts[index])(&method, &req["params"]) {
                Ok(result) => Ok(json!({"jsonrpc": "2.0", "id": 1, "result": result}).to_string()),
                Err(message) if message == "TRANSPORT" => Err("connection refused".into()),
                Err(message) => Ok(
                    json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32000, "message": message}})
                        .to_string(),
                ),
            }
        }
    }

    fn hash(n: u64, salt: u8) -> String {
        let mut h = [0u8; 32];
        h[..8].copy_from_slice(&n.to_be_bytes());
        h[31] = salt;
        format!(
            "0x{}",
            h.iter().map(|b| format!("{b:02x}")).collect::<String>()
        )
    }

    fn block(n: u64, salt: u8) -> Value {
        json!({"number": quantity(n), "hash": hash(n, salt)})
    }

    fn word(v: u128) -> Value {
        json!(format!("0x{:064x}", v))
    }

    /// A provider whose chain is at finalized height `finalized`, with block
    /// hashes salted by `salt`, and `supply` as the token's total supply at any
    /// block it has.
    fn provider(finalized: u64, salt: u8, supply: u128) -> Script {
        Box::new(move |method, params| match method {
            "eth_getBlockByNumber" => {
                let tag = params[0].as_str().unwrap();
                if tag == "finalized" {
                    Ok(block(finalized, salt))
                } else {
                    let n = u64::from_str_radix(tag.trim_start_matches("0x"), 16).unwrap();
                    if n > finalized {
                        Ok(Value::Null)
                    } else {
                        Ok(block(n, salt))
                    }
                }
            }
            "eth_call" => {
                let tag = params[1].as_str().unwrap();
                let n = u64::from_str_radix(tag.trim_start_matches("0x"), 16).unwrap();
                if n > finalized {
                    Err("header not found".into())
                } else {
                    Ok(word(supply))
                }
            }
            other => Err(format!("unexpected method {other}")),
        })
    }

    pub(crate) fn network(id: &str) -> Network {
        let catalog = Catalog::embedded().unwrap();
        let (_, n) = catalog.lookup(id).unwrap();
        n.clone()
    }

    const TOKEN: [u8; 20] = [0x11; 20];

    /// No request may ever carry `latest` (or pending, safe, earliest); the only
    /// tag allowed is `finalized`, for head discovery, and everything else is a
    /// numbered quantity.
    pub(crate) fn assert_numbered_or_finalized(mock: &Mock) {
        assert!(!mock.log.borrow().is_empty());
        for (_, req) in mock.log.borrow().iter() {
            let text = req.to_string();
            for forbidden in ["latest", "pending", "safe", "earliest"] {
                assert!(!text.contains(forbidden), "sent {forbidden}: {text}");
            }
            let tag = match req["method"].as_str().unwrap() {
                "eth_getBlockByNumber" => req["params"][0].as_str().unwrap(),
                "eth_call" => req["params"][1].as_str().unwrap(),
                other => panic!("unexpected method {other}"),
            };
            assert!(
                tag == "finalized" || tag.starts_with("0x"),
                "bad block tag {tag}"
            );
            if req["method"] == "eth_call" {
                assert!(
                    tag.starts_with("0x"),
                    "eth_call must be numbered, got {tag}"
                );
            }
        }
    }

    fn eth_call_blocks(mock: &Mock) -> Vec<String> {
        mock.log
            .borrow()
            .iter()
            .filter(|(_, r)| r["method"] == "eth_call")
            .map(|(_, r)| r["params"][1].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn tag_rule_reads_supply_at_the_agreed_numbered_head() {
        let mock = Mock::new(vec![
            provider(100, 1, 2_000),
            provider(100, 1, 2_000),
            provider(100, 1, 2_000),
        ]);
        let got = read_total_supply(&mock, &TOKEN, &network("base-mainnet")).unwrap();
        assert_eq!(got.supply, 2_000);
        assert_eq!(got.block, 100);
        assert_eq!(got.finality, FinalityRule::EvmTag);
        assert_numbered_or_finalized(&mock);
        // totalSupply was called at 0x64 on every provider, never at a tag.
        assert_eq!(eth_call_blocks(&mock), vec!["0x64"; 3]);
    }

    #[test]
    fn tag_rule_takes_the_quorum_th_highest_height_and_ignores_an_outlier() {
        // Heights 500 (far-ahead outlier), 103, 100 with quorum 2: n = 103.
        let mock = Mock::new(vec![
            provider(500, 1, 7_000),
            provider(103, 1, 7_000),
            provider(100, 1, 7_000),
        ]);
        let got = read_total_supply(&mock, &TOKEN, &network("base-mainnet")).unwrap();
        assert_eq!(got.block, 103);
        assert_eq!(got.supply, 7_000);
        assert_numbered_or_finalized(&mock);
        // The outlier was asked for block 103 itself; the lagging provider was
        // not asked for the hash vote and cannot answer eth_call at 103.
        let asked_103: Vec<usize> = mock
            .log
            .borrow()
            .iter()
            .filter(|(_, r)| r["method"] == "eth_getBlockByNumber" && r["params"][0] == "0x67")
            .map(|(i, _)| *i)
            .collect();
        assert_eq!(asked_103, vec![0]);
        assert_eq!(eth_call_blocks(&mock), vec!["0x67"; 3]);
    }

    #[test]
    fn tag_rule_fails_when_the_block_hash_has_no_quorum() {
        // Same height, three different hashes.
        let mock = Mock::new(vec![
            provider(100, 1, 5),
            provider(100, 2, 5),
            provider(100, 3, 5),
        ]);
        let err = read_total_supply(&mock, &TOKEN, &network("base-mainnet")).unwrap_err();
        assert!(
            matches!(&err, ReadError::Failed(m) if m.contains("no quorum") && m.contains("finalized block 100")),
            "{err:?}"
        );
        // It stopped before asking for any supply.
        assert!(eth_call_blocks(&mock).is_empty());
    }

    #[test]
    fn tag_rule_fails_when_supply_answers_have_no_quorum_and_wins_with_one() {
        let split = Mock::new(vec![
            provider(100, 1, 5),
            provider(100, 1, 6),
            provider(100, 1, 7),
        ]);
        let err = read_total_supply(&split, &TOKEN, &network("base-mainnet")).unwrap_err();
        assert!(
            matches!(&err, ReadError::Failed(m) if m.contains("totalSupply at block 100")),
            "{err:?}"
        );

        // Two agree, one dissents: the quorum wins.
        let majority = Mock::new(vec![
            provider(100, 1, 5),
            provider(100, 1, 6),
            provider(100, 1, 6),
        ]);
        assert_eq!(
            read_total_supply(&majority, &TOKEN, &network("base-mainnet"))
                .unwrap()
                .supply,
            6
        );
    }

    #[test]
    fn a_provider_that_fails_does_not_block_a_quorum_of_the_rest() {
        let down: Script = Box::new(|_, _| Err("TRANSPORT".into()));
        let mock = Mock::new(vec![down, provider(100, 1, 9), provider(100, 1, 9)]);
        let got = read_total_supply(&mock, &TOKEN, &network("base-mainnet")).unwrap();
        assert_eq!((got.supply, got.block), (9, 100));
        // Too few answers: only one provider up with quorum 2.
        let down2: Script = Box::new(|_, _| Err("TRANSPORT".into()));
        let down3: Script = Box::new(|_, _| Err("TRANSPORT".into()));
        let mock = Mock::new(vec![provider(100, 1, 9), down2, down3]);
        assert!(read_total_supply(&mock, &TOKEN, &network("base-mainnet")).is_err());
    }

    #[test]
    fn monad_reads_at_the_finalized_height_minus_three() {
        let mock = Mock::new(vec![
            provider(200, 4, 31_337),
            provider(200, 4, 31_337),
            provider(200, 4, 31_337),
        ]);
        let got = read_total_supply(&mock, &TOKEN, &network("monad-testnet")).unwrap();
        assert_eq!(got.block, 197);
        assert_eq!(got.finality, FinalityRule::MonadVerified);
        assert_eq!(got.supply, 31_337);
        assert_numbered_or_finalized(&mock);
        // 0xc5 = 197: the hash was confirmed there and the call was pinned there.
        assert_eq!(eth_call_blocks(&mock), vec!["0xc5"; 3]);
        assert!(mock
            .log
            .borrow()
            .iter()
            .any(|(_, r)| r["method"] == "eth_getBlockByNumber" && r["params"][0] == "0xc5"));
    }

    #[test]
    fn monad_younger_than_its_delay_has_no_verified_head() {
        let mock = Mock::new(vec![
            provider(2, 4, 1),
            provider(2, 4, 1),
            provider(2, 4, 1),
        ]);
        let err = read_total_supply(&mock, &TOKEN, &network("monad-testnet")).unwrap_err();
        assert!(
            matches!(&err, ReadError::Failed(m) if m.contains("execution delay")),
            "{err:?}"
        );
    }

    #[test]
    fn a_blocked_rule_asks_nothing() {
        for id in [
            "robinhood-mainnet",
            "robinhood-testnet",
            "hyperevm-mainnet",
            "hyperevm-testnet",
        ] {
            let mock = Mock::new(vec![provider(1, 1, 1), provider(1, 1, 1)]);
            let err = read_total_supply(&mock, &TOKEN, &network(id)).unwrap_err();
            assert!(
                matches!(&err, ReadError::UnavailableGap(m) if m.contains("blocked") && m.contains("no request was sent")),
                "{id}: {err:?}"
            );
            assert!(mock.log.borrow().is_empty(), "{id} sent a request");
        }
        assert!(matches!(
            head_rule(&network("robinhood-mainnet")),
            HeadRule::Blocked(_)
        ));
        assert_eq!(
            head_rule(&network("base-mainnet")),
            HeadRule::TagFinalizedQuorum
        );
        assert_eq!(
            head_rule(&network("monad-mainnet")),
            HeadRule::MonadVerified { delay_blocks: 3 }
        );
    }

    #[test]
    fn latest_can_never_be_sent() {
        for tag in ["latest", "pending", "safe", "earliest"] {
            assert!(
                check_no_forbidden_tag(&json!([tag, false])).is_err(),
                "{tag}"
            );
            assert!(
                check_no_forbidden_tag(&json!([{"to": "0x01"}, tag])).is_err(),
                "{tag}"
            );
        }
        assert!(check_no_forbidden_tag(&json!(["finalized", false])).is_ok());
        assert!(
            check_no_forbidden_tag(&json!([{"to": "0x01", "data": "0x18160ddd"}, "0x64"])).is_ok()
        );
        // And the guard sits in `call`, so a transport never even sees it.
        let mock = Mock::new(vec![provider(1, 1, 1)]);
        assert!(call(&mock, 0, "eth_call", json!([{"to": "0x01"}, "latest"])).is_err());
        assert!(mock.log.borrow().is_empty());
    }

    #[test]
    fn quorum_larger_than_the_configured_providers_is_refused_up_front() {
        let mock = Mock::new(vec![provider(100, 1, 5)]);
        let err = read_total_supply(&mock, &TOKEN, &network("base-mainnet")).unwrap_err();
        assert!(
            matches!(&err, ReadError::Failed(m) if m.contains("quorum 2")),
            "{err:?}"
        );
        assert!(mock.log.borrow().is_empty());
    }

    #[test]
    fn malformed_answers_are_rejected_not_coerced() {
        let junk = |result: Value| -> Script { Box::new(move |_, _| Ok(result.clone())) };
        for bad in [
            json!("0x1"),
            json!({"number": "0x01", "hash": hash(1, 1)}),
            json!({"number": "0x1"}),
            json!(7),
        ] {
            let mock = Mock::new(vec![junk(bad.clone()), junk(bad.clone()), junk(bad)]);
            assert!(read_total_supply(&mock, &TOKEN, &network("base-mainnet")).is_err());
        }
        // A supply wider than u128 is refused.
        let wide = || -> Script {
            Box::new(|m, _| match m {
                "eth_getBlockByNumber" => Ok(block(5, 1)),
                _ => Ok(json!(format!("0x{}", "ff".repeat(32)))),
            })
        };
        let mock = Mock::new(vec![wide(), wide()]);
        let err = read_total_supply(&mock, &TOKEN, &network("base-mainnet")).unwrap_err();
        assert!(
            matches!(&err, ReadError::Failed(m) if m.contains("u128")),
            "{err:?}"
        );
    }

    #[test]
    fn real_transport_refuses_non_https_and_serves_a_loopback_mock() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        assert!(ReqwestJsonRpc::new(vec![]).is_err());
        assert!(ReqwestJsonRpc::new(vec!["http://example.invalid".into()]).is_err());
        assert!(ReqwestJsonRpc::new(vec!["https://example.invalid".into()]).is_ok());

        // One request against a loopback mock: the real reqwest path, no public network.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            let body = r#"{"jsonrpc":"2.0","id":1,"result":"0x2a"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
            request
        });
        let t = ReqwestJsonRpc::new(vec![format!("http://127.0.0.1:{port}/rpc")]).unwrap();
        let reply = t
            .post(0, r#"{"id":1,"method":"eth_chainId","params":[]}"#)
            .unwrap();
        assert!(reply.contains("0x2a"));
        let request = server.join().unwrap();
        assert!(request.starts_with("POST /rpc "), "{request}");
        assert!(request.contains("eth_chainId"));
        // A dead port is an error that does not repeat the URL.
        let dead =
            ReqwestJsonRpc::new(vec![format!("http://127.0.0.1:{port}/secret-key")]).unwrap();
        let err = dead.post(0, "{}").unwrap_err();
        assert!(
            !err.contains("secret-key") && !err.contains("127.0.0.1"),
            "{err}"
        );
    }
}
