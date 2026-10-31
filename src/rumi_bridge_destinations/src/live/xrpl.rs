//! XRPL lane side of the live transport (feature `live-transport`, card BB-57).
//!
//! Reads the lane's issued outstanding from the XRP Ledger itself, never from
//! the backend's own report: the issuer's `obligations` for the icUSD currency
//! (`gateway_balances`) at a NUMBERED VALIDATED ledger agreed by a provider
//! quorum. The escrow `E_X` is read separately, by the existing IC transport,
//! exactly like a destination escrow.
//!
//! The provider rule mirrors the EVM reader ([`super::evm`]) and reuses its
//! [`super::evm::JsonRpcTransport`] (rippled's JSON-RPC differs only in the
//! envelope, which is handled here):
//!
//! 1. every provider is asked `server_info`. Its network id is bound to the
//!    profile network's XRPL network id (`network.identity`): a provider that
//!    reports another id is excluded, and a provider that reports none is
//!    excluded unless the profile network is the mainnet (id 0), the only
//!    network whose servers may omit it. A provider without a validated ledger
//!    is excluded. The profile's quorum must be met by the remaining providers;
//! 2. `n` is the `quorum`-th highest validated ledger index reported (one
//!    far-ahead provider cannot raise it);
//! 3. every provider at or above `n` is asked `gateway_balances` for the issuer
//!    at exactly ledger `n` (`ledger_index` is the number, never `validated`,
//!    `closed` or `current`). The answer must say `validated: true`, name the
//!    configured issuer account and the same ledger index, and carry a ledger
//!    hash. A provider below `n` does not vote;
//! 4. a quorum of identical `(ledger hash, amount)` answers wins.
//!
//! # Amounts
//!
//! `obligations` maps currency codes to decimal strings. Only the icUSD
//! currency code ([`ICUSD_CURRENCY_HEX`]) is accepted; an obligation in any
//! other currency is a failed read (the issuer is dedicated to icUSD, so it
//! means a wrong issuer or currency). An absent or empty `obligations` is the
//! documented encoding of "nothing issued" and reads as exactly zero. The
//! decimal text is converted EXACTLY: more than 8 decimals, more than 15
//! significant digits, a negative number, or a value above the lane ceiling
//! (999,999,999,999,999 e8s) is a failure, never a rounded number.
//!
//! Providers are named by position in errors, never by URL.

use serde_json::{json, Value};

use super::evm::{tally, JsonRpcTransport};
use crate::collect::{ReadError, XrplIssued};
use crate::types::{FinalityRule, IdentityKind, Network};

/// The icUSD currency code on the XRPL: ASCII `icUSD` right-padded with zeros
/// to 160 bits, upper-case hex (the backend's `ICUSD_CURRENCY_HEX`). It is a
/// protocol constant, not an address or an endpoint.
pub const ICUSD_CURRENCY_HEX: &str = "6963555344000000000000000000000000000000";

/// The D7 lane ceiling: 15 significant digits of e8s.
pub const MAX_ISSUED_E8S: u128 = 999_999_999_999_999;

/// XRPL base58 alphabet (no `0`, `O`, `I`, `l`).
const BASE58: &str = "rpshnaf39wBUDNEGHJKLM4PQRST7VWXYZ2bcdeCg65jkm8oFqi1tuvAxyz";

// ---------------------------------------------------------------------------
// Exact decimal handling
// ---------------------------------------------------------------------------

/// An XRPL issued-currency amount (decimal text, possibly with an exponent) as
/// e8s, exactly. Anything that is not exactly an integer number of e8s with at
/// most 15 significant digits, or that is negative or above the lane ceiling,
/// is an error.
pub fn exact_e8s(text: &str) -> Result<u128, String> {
    let bad = |why: &str| format!("amount {text:?} {why}");
    if text.is_empty() {
        return Err(bad("is empty"));
    }
    let (mantissa, exponent) = match text.split_once(['e', 'E']) {
        Some((m, e)) => (m, Some(e)),
        None => (text, None),
    };
    let exp: i32 = match exponent {
        None => 0,
        Some(e) => {
            let (negative, digits) = match e.as_bytes().first() {
                Some(b'-') => (true, &e[1..]),
                Some(b'+') => (false, &e[1..]),
                _ => (false, e),
            };
            if digits.is_empty() || digits.len() > 3 || !digits.bytes().all(|b| b.is_ascii_digit())
            {
                return Err(bad("has a malformed exponent"));
            }
            let v: i32 = digits.parse().map_err(|_| bad("has a malformed exponent"))?;
            if negative {
                -v
            } else {
                v
            }
        }
    };
    if mantissa.starts_with('-') {
        return Err(bad("is negative"));
    }
    let (int_part, frac_part) = match mantissa.split_once('.') {
        Some((i, f)) => {
            if f.is_empty() {
                return Err(bad("ends with a decimal point"));
            }
            (i, f)
        }
        None => (mantissa, ""),
    };
    if int_part.is_empty()
        || !int_part.bytes().all(|b| b.is_ascii_digit())
        || !frac_part.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(bad("is not a plain decimal number"));
    }
    let mut digits = format!("{int_part}{frac_part}");
    let mut e10 = exp - frac_part.len() as i32;
    let trimmed = digits.trim_start_matches('0').to_string();
    digits = trimmed;
    if digits.is_empty() {
        return Ok(0);
    }
    while digits.ends_with('0') {
        digits.pop();
        e10 += 1;
    }
    if digits.len() > 15 {
        return Err(bad("has more than 15 significant digits"));
    }
    let shift = e10 + 8;
    if shift < 0 {
        return Err(bad("has digits beyond 8 decimals"));
    }
    if shift > 38 {
        return Err(bad("is too large"));
    }
    let mantissa: u128 = digits.parse().map_err(|_| bad("is not a number"))?;
    let e8s = mantissa
        .checked_mul(10u128.pow(shift as u32))
        .ok_or_else(|| bad("is too large"))?;
    if e8s > MAX_ISSUED_E8S {
        return Err(bad("is above the 15-digit lane ceiling"));
    }
    Ok(e8s)
}

// ---------------------------------------------------------------------------
// JSON-RPC helpers (rippled envelope)
// ---------------------------------------------------------------------------

/// One request to one provider; returns the `result` object of a success.
fn call(t: &dyn JsonRpcTransport, i: usize, method: &str, params: Value) -> Result<Value, String> {
    let body = json!({"method": method, "params": [params]}).to_string();
    let text = t.post(i, &body)?;
    let envelope: Value =
        serde_json::from_str(&text).map_err(|e| format!("response is not JSON: {e}"))?;
    let result = envelope
        .get("result")
        .filter(|r| r.is_object())
        .ok_or("response has no result object")?;
    if result.get("status").and_then(Value::as_str) != Some("success") {
        let error = result.get("error").and_then(Value::as_str).unwrap_or("unknown");
        let error: String = error.chars().take(120).collect();
        return Err(format!("rpc error: {error}"));
    }
    Ok(result.clone())
}

fn hex_hash(v: Option<&Value>, what: &str) -> Result<String, String> {
    let s = v
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{what} is missing"))?;
    if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("{what} is not 64 hex characters"));
    }
    Ok(s.to_ascii_uppercase())
}

/// The XRPL network id the profile names for this network.
fn expected_network_id(network: &Network) -> Result<u64, String> {
    let id = &network.identity.value;
    if id.kind != IdentityKind::XrplNetworkId {
        return Err("the profile identity is not an XRPL network id".into());
    }
    crate::types::parse_u64_decimal(&id.value)
        .map_err(|e| format!("the profile's XRPL network id is unusable: {e}"))
}

/// `(validated ledger index, hash)` from a `server_info` result, bound to the
/// expected network.
fn validated_tip(result: &Value, expected: u64) -> Result<(u64, String), String> {
    let info = result
        .get("info")
        .and_then(Value::as_object)
        .ok_or("server_info has no info object")?;
    match info.get("network_id") {
        Some(v) => {
            let id = v.as_u64().ok_or("the reported network id is not a number")?;
            if id != expected {
                return Err(format!("reports network {id}, the profile requires {expected}"));
            }
        }
        // Only a mainnet server may leave the network id out.
        None if expected != 0 => {
            return Err(format!(
                "reports no network id, which cannot prove network {expected}"
            ));
        }
        None => {}
    }
    let validated = info
        .get("validated_ledger")
        .and_then(Value::as_object)
        .ok_or("the server has no validated ledger")?;
    let seq = validated
        .get("seq")
        .and_then(Value::as_u64)
        .ok_or("the validated ledger has no sequence")?;
    let hash = hex_hash(validated.get("hash"), "the validated ledger hash")?;
    Ok((seq, hash))
}

/// `(ledger hash, icUSD obligations in e8s)` from a `gateway_balances` result
/// at the numbered ledger `n`.
fn issued_at(result: &Value, issuer: &str, n: u64) -> Result<(String, u128), String> {
    if result.get("validated").and_then(Value::as_bool) != Some(true) {
        return Err("the answer is not from a validated ledger".into());
    }
    if result.get("account").and_then(Value::as_str) != Some(issuer) {
        return Err("the answer is for a different account than the configured issuer".into());
    }
    if result.get("ledger_index").and_then(Value::as_u64) != Some(n) {
        return Err(format!("the answer is not for ledger {n}"));
    }
    let hash = hex_hash(result.get("ledger_hash"), "the ledger hash")?;
    let Some(obligations) = result.get("obligations") else {
        // rippled omits `obligations` when nothing is issued.
        return Ok((hash, 0));
    };
    let map = obligations
        .as_object()
        .ok_or("obligations is not an object")?;
    let mut icusd = None;
    let mut others = Vec::new();
    for (code, value) in map {
        if code.eq_ignore_ascii_case(ICUSD_CURRENCY_HEX) {
            let text = value.as_str().ok_or("the icUSD obligation is not a string")?;
            icusd = Some(exact_e8s(text)?);
        } else {
            others.push(code.chars().take(40).collect::<String>());
        }
    }
    if !others.is_empty() {
        return Err(format!(
            "the issuer has obligations in other currencies ({}): a wrong issuer or currency",
            others.join(",")
        ));
    }
    Ok((hash, icusd.unwrap_or(0)))
}

/// A shape check for an XRPL classic address (no checksum, which needs a hash
/// this crate does not carry; a server refuses a malformed account anyway).
pub fn check_issuer(issuer: &str) -> Result<(), String> {
    if !issuer.starts_with('r')
        || !(25..=35).contains(&issuer.len())
        || !issuer.bytes().all(|b| BASE58.as_bytes().contains(&b))
    {
        return Err("the issuer is not an XRPL classic address".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The reading
// ---------------------------------------------------------------------------

/// The issuer's icUSD obligations at the quorum-agreed validated ledger.
pub fn read_issued_outstanding(
    t: &dyn JsonRpcTransport,
    issuer: &str,
    network: &Network,
) -> Result<XrplIssued, ReadError> {
    let fail = |e: String| ReadError::Failed(format!("{}: {e}", network.network_id));
    if network.finality.rule != FinalityRule::XrplValidated {
        return Err(fail(format!(
            "{:?} is not the XRPL validated-ledger rule",
            network.finality.rule
        )));
    }
    check_issuer(issuer).map_err(fail)?;
    let quorum = usize::from(network.rpc.min_quorum.value);
    let count = t.provider_count();
    if quorum == 0 || quorum > count {
        return Err(fail(format!(
            "quorum {quorum} is not in 1..={count} configured providers"
        )));
    }
    let expected = expected_network_id(network).map_err(fail)?;

    // 1. Network binding and the validated tip of every provider.
    let tips: Vec<Result<(u64, String), String>> = (0..count)
        .map(|i| call(t, i, "server_info", json!({})).and_then(|r| validated_tip(&r, expected)))
        .collect();
    let mut heights: Vec<u64> = tips
        .iter()
        .filter_map(|r| r.as_ref().ok())
        .map(|(seq, _)| *seq)
        .collect();
    if heights.len() < quorum {
        let excluded: Vec<String> = tips
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.as_ref().err().map(|e| format!("p{} {e}", i + 1)))
            .collect();
        return Err(fail(format!(
            "only {} of {count} providers are on network {expected} with a validated ledger, quorum is {quorum}; excluded: {}",
            heights.len(),
            excluded.join("; ")
        )));
    }

    // 2. The agreed height: the quorum-th highest.
    heights.sort_unstable_by(|a, b| b.cmp(a));
    let n = heights[quorum - 1];

    // 3. The obligations at exactly ledger `n`.
    let votes: Vec<Result<(String, u128), String>> = tips
        .into_iter()
        .enumerate()
        .map(|(i, tip)| match tip {
            Ok((seq, _)) if seq >= n => call(
                t,
                i,
                "gateway_balances",
                json!({"account": issuer, "ledger_index": n, "strict": true}),
            )
            .and_then(|r| issued_at(&r, issuer, n)),
            Ok((seq, _)) => Err(format!(
                "validated ledger {seq} is below the agreed ledger {n}"
            )),
            Err(e) => Err(format!("excluded: {e}")),
        })
        .collect();

    // 4. A quorum of identical (ledger hash, amount) answers.
    let (ledger_hash, issued_outstanding) = tally(
        votes,
        quorum,
        &format!("issued outstanding at validated ledger {n}"),
    )
    .map_err(fail)?;
    Ok(XrplIssued {
        issued_outstanding,
        finality: network.finality.rule,
        ledger_index: n,
        ledger_hash,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::aggregate::Catalog;
    use std::cell::RefCell;

    /// A synthetic classic-address SHAPE (it is not a valid account; no real
    /// issuer address appears anywhere in this crate).
    pub(crate) const ISSUER: &str = "rFixtureTestAccountzzzzzzzzzzzzz";
    const OTHER_ISSUER: &str = "rFixtureAnotherAccountzzzzzzzzzzz";

    /// What one scripted provider answers for a method and its first param.
    pub(crate) type Script = Box<dyn Fn(&str, &Value) -> Result<Value, String>>;

    /// A mock transport: provider `i` answers through `scripts[i]`, every request
    /// is recorded as `(provider, parsed body)`. An `Err("TRANSPORT")` is a
    /// connection failure; any other `Err` is an rpc error object.
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
            match (self.scripts[index])(&method, &req["params"][0]) {
                Ok(mut result) => {
                    result["status"] = json!("success");
                    Ok(json!({ "result": result }).to_string())
                }
                Err(m) if m == "TRANSPORT" => Err("connection refused".into()),
                Err(m) => Ok(json!({"result": {"status": "error", "error": m}}).to_string()),
            }
        }
    }

    pub(crate) fn hash(salt: u8) -> String {
        format!("{:064X}", u128::from(salt) * 0x1_0001)
    }

    /// Knobs of one synthetic provider.
    #[derive(Clone)]
    pub(crate) struct P {
        pub network_id: Option<u64>,
        pub seq: u64,
        pub salt: u8,
        pub validated: bool,
        pub account: String,
        pub obligations: Option<Value>,
    }

    impl P {
        pub(crate) fn mainnet(seq: u64, amount: &str) -> P {
            P {
                network_id: None,
                seq,
                salt: 1,
                validated: true,
                account: ISSUER.to_string(),
                obligations: Some(json!({ ICUSD_CURRENCY_HEX: amount })),
            }
        }
        pub(crate) fn script(self) -> Script {
            Box::new(move |method, params| match method {
                "server_info" => {
                    let mut info = json!({
                        "validated_ledger": {"seq": self.seq, "hash": hash(self.salt)}
                    });
                    if let Some(id) = self.network_id {
                        info["network_id"] = json!(id);
                    }
                    Ok(json!({ "info": info }))
                }
                "gateway_balances" => {
                    let n = params["ledger_index"].as_u64().unwrap();
                    if n > self.seq {
                        return Err("lgrNotFound".into());
                    }
                    let mut r = json!({
                        "account": self.account,
                        "ledger_index": n,
                        "ledger_hash": hash(self.salt),
                        "validated": self.validated,
                    });
                    if let Some(o) = &self.obligations {
                        r["obligations"] = o.clone();
                    }
                    Ok(r)
                }
                other => Err(format!("unexpected method {other}")),
            })
        }
    }

    fn network(id: &str) -> Network {
        let catalog = Catalog::embedded().unwrap();
        let (_, n) = catalog.lookup(id).unwrap();
        n.clone()
    }

    fn mock(ps: Vec<P>) -> Mock {
        Mock::new(ps.into_iter().map(P::script).collect())
    }

    fn read(m: &Mock, net: &str) -> Result<XrplIssued, ReadError> {
        read_issued_outstanding(m, ISSUER, &network(net))
    }

    fn failure(r: Result<XrplIssued, ReadError>) -> String {
        match r {
            Err(ReadError::Failed(m)) => m,
            other => panic!("expected a failed read, got {other:?}"),
        }
    }

    const E8: u128 = 100_000_000;

    #[test]
    fn exact_amounts_are_converted_without_rounding() {
        assert_eq!(exact_e8s("0"), Ok(0));
        assert_eq!(exact_e8s("0.0"), Ok(0));
        assert_eq!(exact_e8s("1"), Ok(E8));
        assert_eq!(exact_e8s("123.45678901"), Ok(12_345_678_901));
        assert_eq!(exact_e8s("0.00000001"), Ok(1));
        assert_eq!(exact_e8s("1e-8"), Ok(1));
        assert_eq!(exact_e8s("1.5e-7"), Ok(15));
        assert_eq!(exact_e8s("2E+3"), Ok(2_000 * E8));
        assert_eq!(exact_e8s("00012.50"), Ok(1_250_000_000));
        assert_eq!(exact_e8s("9999999.99999999"), Ok(MAX_ISSUED_E8S));
        for (text, needle) in [
            ("1.123456789", "beyond 8 decimals"),
            ("1e-9", "beyond 8 decimals"),
            ("0.000000001", "beyond 8 decimals"),
            ("12345678.12345678", "15 significant digits"),
            ("1234567890123456", "15 significant digits"),
            ("10000000", "lane ceiling"),
            ("-1", "negative"),
            ("-0.5", "negative"),
            ("+1", "plain decimal"),
            ("", "empty"),
            ("1.", "decimal point"),
            (".5", "plain decimal"),
            ("abc", "plain decimal"),
            ("1e", "exponent"),
            ("1e99999", "exponent"),
            ("1 ", "plain decimal"),
            ("0x10", "plain decimal"),
        ] {
            let e = exact_e8s(text).unwrap_err();
            assert!(e.contains(needle), "{text:?}: {e}");
        }
    }

    #[test]
    fn three_agreeing_providers_give_the_amount_at_the_validated_ledger() {
        let m = mock(vec![
            P::mainnet(500, "120.5"),
            P::mainnet(500, "120.5"),
            P::mainnet(500, "120.5"),
        ]);
        let got = read(&m, "xrpl-mainnet").unwrap();
        assert_eq!(got.issued_outstanding, 12_050_000_000);
        assert_eq!(got.finality, FinalityRule::XrplValidated);
        assert_eq!(got.ledger_index, 500);
        assert_eq!(got.ledger_hash, hash(1));
        // Every gateway_balances request names the NUMBERED ledger, never a tag.
        let log = m.log.borrow();
        let gateway: Vec<&Value> = log
            .iter()
            .filter(|(_, r)| r["method"] == "gateway_balances")
            .map(|(_, r)| r)
            .collect();
        assert_eq!(gateway.len(), 3);
        for r in gateway {
            assert_eq!(r["params"][0]["ledger_index"], json!(500));
            assert_eq!(r["params"][0]["account"], json!(ISSUER));
        }
        assert!(!log.iter().any(|(_, r)| {
            let t = r.to_string();
            t.contains("\"current\"") || t.contains("\"closed\"") || t.contains("\"validated\"")
        }));
    }

    #[test]
    fn a_provider_ahead_is_asked_at_the_agreed_ledger_and_one_below_does_not_vote() {
        // Heights 510, 500, 495 with quorum 2: the agreed ledger is the 2nd
        // highest (500). The provider at 510 is asked for 500; the one at 495
        // does not vote.
        let m = mock(vec![
            P::mainnet(510, "7"),
            P::mainnet(500, "7"),
            P::mainnet(495, "9999"),
        ]);
        let got = read(&m, "xrpl-mainnet").unwrap();
        assert_eq!(got.ledger_index, 500);
        assert_eq!(got.issued_outstanding, 7 * E8);
        let log = m.log.borrow();
        let asked: Vec<usize> = log
            .iter()
            .filter(|(_, r)| r["method"] == "gateway_balances")
            .map(|(i, _)| *i)
            .collect();
        assert_eq!(asked, vec![0, 1], "the provider below the agreed ledger is not asked");
    }

    #[test]
    fn disagreement_without_a_quorum_is_a_failure_that_names_the_providers_by_position() {
        let m = mock(vec![
            P::mainnet(500, "1"),
            P::mainnet(500, "2"),
            P::mainnet(500, "3"),
        ]);
        let e = failure(read(&m, "xrpl-mainnet"));
        assert!(e.contains("no quorum"), "{e}");
        assert!(e.contains("p1") && e.contains("p2") && e.contains("p3"), "{e}");
        assert!(e.contains("need 2 identical of 3"), "{e}");
    }

    #[test]
    fn a_single_dissenter_is_outvoted_by_a_quorum() {
        let m = mock(vec![
            P::mainnet(500, "5"),
            P::mainnet(500, "6"),
            P::mainnet(500, "5"),
        ]);
        assert_eq!(read(&m, "xrpl-mainnet").unwrap().issued_outstanding, 5 * E8);
    }

    #[test]
    fn a_different_ledger_hash_at_the_same_ledger_is_not_agreement() {
        let mut other = P::mainnet(500, "5");
        other.salt = 2;
        let m = mock(vec![P::mainnet(500, "5"), other]);
        let e = failure(read(&m, "xrpl-mainnet"));
        assert!(e.contains("no quorum"), "{e}");
    }

    #[test]
    fn an_unvalidated_answer_is_excluded_from_the_vote() {
        let mut unvalidated = P::mainnet(500, "999");
        unvalidated.validated = false;
        // The two validated providers still agree.
        let m = mock(vec![unvalidated.clone(), P::mainnet(500, "5"), P::mainnet(500, "5")]);
        assert_eq!(read(&m, "xrpl-mainnet").unwrap().issued_outstanding, 5 * E8);
        // Two unvalidated answers leave one voter: no quorum, named reason.
        let m = mock(vec![unvalidated.clone(), unvalidated, P::mainnet(500, "5")]);
        let e = failure(read(&m, "xrpl-mainnet"));
        assert!(e.contains("not from a validated ledger"), "{e}");
    }

    #[test]
    fn a_provider_without_a_validated_ledger_does_not_count_toward_the_quorum() {
        let none: Script = Box::new(|method, _| match method {
            "server_info" => Ok(json!({"info": {}})),
            other => Err(format!("unexpected {other}")),
        });
        let m = Mock::new(vec![none, P::mainnet(500, "5").script()]);
        let e = failure(read(&m, "xrpl-mainnet"));
        assert!(e.contains("only 1 of 2 providers"), "{e}");
        assert!(e.contains("no validated ledger"), "{e}");
    }

    #[test]
    fn a_provider_on_the_wrong_network_is_excluded() {
        // Mainnet profile: a server reporting network 1 is excluded...
        let mut testnet = P::mainnet(500, "999");
        testnet.network_id = Some(1);
        let m = mock(vec![testnet.clone(), P::mainnet(500, "5"), P::mainnet(500, "5")]);
        assert_eq!(read(&m, "xrpl-mainnet").unwrap().issued_outstanding, 5 * E8);
        // ...and without it the quorum fails with the reason named.
        let m = mock(vec![testnet, P::mainnet(500, "5")]);
        let e = failure(read(&m, "xrpl-mainnet"));
        assert!(e.contains("reports network 1, the profile requires 0"), "{e}");

        // Testnet profile (network 1): a server that reports no id cannot prove
        // the network, and one that reports the mainnet id is wrong.
        let mut ok = P::mainnet(700, "3");
        ok.network_id = Some(1);
        let mut silent = P::mainnet(700, "3");
        silent.network_id = None;
        let mut wrong = P::mainnet(700, "3");
        wrong.network_id = Some(0);
        let m = mock(vec![ok.clone(), silent, wrong]);
        let e = failure(read(&m, "xrpl-testnet"));
        assert!(e.contains("only 1 of 3 providers"), "{e}");
        assert!(e.contains("reports no network id"), "{e}");
        assert!(e.contains("reports network 0"), "{e}");
        let m = mock(vec![ok.clone(), ok]);
        assert_eq!(read(&m, "xrpl-testnet").unwrap().issued_outstanding, 3 * E8);
    }

    #[test]
    fn an_answer_for_another_issuer_is_excluded() {
        let mut other = P::mainnet(500, "999");
        other.account = OTHER_ISSUER.to_string();
        let m = mock(vec![other.clone(), P::mainnet(500, "5"), P::mainnet(500, "5")]);
        assert_eq!(read(&m, "xrpl-mainnet").unwrap().issued_outstanding, 5 * E8);
        let m = mock(vec![other.clone(), other]);
        let e = failure(read(&m, "xrpl-mainnet"));
        assert!(e.contains("different account than the configured issuer"), "{e}");
    }

    #[test]
    fn another_currency_in_the_obligations_is_a_failure_not_a_zero() {
        // Only a different currency: not "nothing issued".
        let mut usd = P::mainnet(500, "0");
        usd.obligations = Some(json!({"USD": "50"}));
        let e = failure(read(&mock(vec![usd.clone(), usd.clone()]), "xrpl-mainnet"));
        assert!(e.contains("other currencies (USD)"), "{e}");
        // icUSD together with another currency is refused too.
        let mut both = P::mainnet(500, "5");
        both.obligations = Some(json!({ ICUSD_CURRENCY_HEX: "5", "EUR": "1" }));
        let e = failure(read(&mock(vec![both.clone(), both]), "xrpl-mainnet"));
        assert!(e.contains("other currencies (EUR)"), "{e}");
        // A look-alike code (right letters, wrong padding) is another currency.
        let mut near = P::mainnet(500, "5");
        near.obligations = Some(json!({ "6963555344000000000000000000000000000001": "5" }));
        assert!(read(&mock(vec![near.clone(), near]), "xrpl-mainnet").is_err());
        // The code is matched without regard to hex case.
        let mut lower = P::mainnet(500, "5");
        lower.obligations = Some(json!({ ICUSD_CURRENCY_HEX.to_ascii_lowercase(): "5" }));
        assert_eq!(
            read(&mock(vec![lower.clone(), lower]), "xrpl-mainnet").unwrap().issued_outstanding,
            5 * E8
        );
    }

    #[test]
    fn nothing_issued_reads_as_exactly_zero_only_when_the_answer_says_so() {
        let mut absent = P::mainnet(500, "0");
        absent.obligations = None;
        let mut empty = P::mainnet(500, "0");
        empty.obligations = Some(json!({}));
        assert_eq!(read(&mock(vec![absent.clone(), absent.clone()]), "xrpl-mainnet").unwrap().issued_outstanding, 0);
        assert_eq!(read(&mock(vec![empty.clone(), empty.clone()]), "xrpl-mainnet").unwrap().issued_outstanding, 0);
        // A provider that omits the field while the others report an amount is a
        // disagreement, never a quiet zero.
        let m = mock(vec![absent.clone(), P::mainnet(500, "5"), P::mainnet(500, "5")]);
        assert_eq!(read(&m, "xrpl-mainnet").unwrap().issued_outstanding, 5 * E8);
        let m = mock(vec![absent, P::mainnet(500, "5")]);
        assert!(failure(read(&m, "xrpl-mainnet")).contains("no quorum"));
    }

    #[test]
    fn an_inexact_amount_is_a_failure_and_is_never_rounded() {
        for (text, needle) in [
            ("1.123456789", "beyond 8 decimals"),
            ("12345678.12345678", "15 significant digits"),
            ("1e-9", "beyond 8 decimals"),
            ("-3", "negative"),
            ("10000000", "lane ceiling"),
        ] {
            let m = mock(vec![P::mainnet(500, text), P::mainnet(500, text)]);
            let e = failure(read(&m, "xrpl-mainnet"));
            assert!(e.contains(needle), "{text}: {e}");
        }
        // An amount that is exact but unequal between providers does not average out.
        let m = mock(vec![P::mainnet(500, "1.00000001"), P::mainnet(500, "1.00000002")]);
        assert!(failure(read(&m, "xrpl-mainnet")).contains("no quorum"));
    }

    #[test]
    fn transport_and_rpc_failures_are_named_by_position_and_never_a_value() {
        let down: Script = Box::new(|_, _| Err("TRANSPORT".into()));
        let m = Mock::new(vec![down, P::mainnet(500, "5").script()]);
        let e = failure(read(&m, "xrpl-mainnet"));
        assert!(e.contains("p1"), "{e}");
        assert!(e.contains("connection refused"), "{e}");
        // A provider that errors at the numbered ledger is just not a voter.
        let bad: Script = Box::new(|method, _| match method {
            "server_info" => Ok(json!({"info": {"validated_ledger": {"seq": 500, "hash": hash(1)}}})),
            _ => Err("actNotFound".into()),
        });
        let m = Mock::new(vec![bad, P::mainnet(500, "5").script(), P::mainnet(500, "5").script()]);
        assert_eq!(read(&m, "xrpl-mainnet").unwrap().issued_outstanding, 5 * E8);
    }

    #[test]
    fn a_malformed_issuer_or_quorum_fails_before_any_request() {
        for issuer in ["", "notanaddress", "rO0Il", "xFixtureTestAccountzzzzzzzzzzzzz"] {
            let m = mock(vec![P::mainnet(500, "1"), P::mainnet(500, "1")]);
            let e = failure(read_issued_outstanding(&m, issuer, &network("xrpl-mainnet")));
            assert!(e.contains("classic address"), "{issuer}: {e}");
            assert!(m.log.borrow().is_empty(), "no request for {issuer:?}");
        }
        // One provider cannot meet a quorum of two.
        let m = mock(vec![P::mainnet(500, "1")]);
        let e = failure(read(&m, "xrpl-mainnet"));
        assert!(e.contains("quorum 2 is not in 1..=1"), "{e}");
        assert!(m.log.borrow().is_empty());
    }

    #[test]
    fn no_real_issuer_endpoint_or_canister_id_is_built_in() {
        // The only account text in this module is the synthetic fixture shape.
        assert!(check_issuer(ISSUER).is_ok());
        assert_eq!(ICUSD_CURRENCY_HEX.len(), 40);
        // ASCII "icUSD" right-padded with zeros to 160 bits.
        let mut code = b"icUSD".to_vec();
        code.resize(20, 0);
        let hex: String = code.iter().map(|b| format!("{b:02X}")).collect();
        assert_eq!(hex, ICUSD_CURRENCY_HEX);
    }
}
