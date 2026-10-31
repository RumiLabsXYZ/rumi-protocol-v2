//! The bounded HTTP transport and the provider quorum (53b): every read the observed binding and the
//! mint lane make, as an async method returning a [`Verdict`] that is never zero, absent or success
//! unless the strict quorum says so, and the broadcast of one signed transaction.
//!
//! The transport is a trait ([`SuiTransport`]). The real implementation ([`CanisterTransport`],
//! wasm only) makes one HTTPS-outcall POST per provider through the IC management canister with a
//! transform named by the call kind; the tests use a scripted provider set and a fake chain that
//! answers the exact query shapes (`testkit`). Nothing in this crate sends a request to a public
//! provider: the mirror is disabled by default (`MirrorSettings.enabled`), no provider is contracted,
//! and the outcall cycle cost, retention and keyless availability are unmeasured
//! (`sui-rpc-provider-contract`, MC-DEF-3 and MC-DEF-4).
//!
//! Rules this module enforces on top of `graphql.rs`:
//!
//! * a provider whose answer breaks identity (wrong network, another object, another version, a
//!   malformed shape, an unrounded head) is set aside BEFORE the quorum counts it;
//! * exact-version object reads go through [`Mirror::object_at_version`], the one composed helper
//!   that parses with `parse_object_at_version` before the tally;
//! * `Missing` is a reading, not zero: only the caller decides what an agreed absence means, and for
//!   a table entry or a transaction it never means "did not happen";
//! * a send is "maybe sent" by construction: its outcome is never evidence.

use crate::gate::{ConfiguredGate, MintRecord};
use crate::graphql::{
    self, exclusions, interpret_gate, interpret_mint_entry, outcome_from_body, parse_checkpoint, parse_object, parse_object_at_version, parse_table_entry,
    request_chain_identifier, request_checkpoint, request_object_at_checkpoint, request_object_at_version, request_table_entry, tally, CheckpointFact,
    Exclusion, GateFact, GraphqlRequest, MirrorSettings, ObjectFact, Outcome, ProviderConfig, QuorumRule, Verdict,
};
use crate::reads::*;
use crate::sui::{format_digest_base58, Bytes32};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::Value;

/// Reads are limited by the GraphQL service to 5 KB; the lane's reads are far below it.
pub const MAX_READ_REQUEST_BYTES: usize = 5 * 1024;
/// A signed mint is a few hundred bytes; the cap is generous and still far below the service's 175 KB.
pub const MAX_SEND_REQUEST_BYTES: usize = 8 * 1024;
/// The subnet size the cycle estimate assumes (the largest, so it is an upper bound).
pub const OUTCALL_SUBNET_NODES: u128 = 34;

/// Which transform a call goes through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallKind {
    /// A pinned read: `data` canonicalized (`transform_sui_graphql`).
    Read,
    /// The head query: rounded down (`transform_sui_graphql_head`).
    Head,
    /// `executeTransaction`: reduced to whether the provider HTTP-acknowledged it (`transform_sui_send`).
    Send,
}

impl CallKind {
    /// The exported query method that transforms this kind of response.
    pub fn transform(self) -> &'static str {
        match self {
            CallKind::Read => "transform_sui_graphql",
            CallKind::Head => "transform_sui_graphql_head",
            CallKind::Send => "transform_sui_send",
        }
    }
}

/// The HTTP request an outcall makes, as plain data (validated; the wasm transport converts it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpSpec {
    pub url: String,
    pub body: Vec<u8>,
    pub max_response_bytes: u32,
    pub transform: &'static str,
    pub headers: Vec<(String, String)>,
}

/// Build the request for one provider: an `https` URL with no credentials, a JSON body within the
/// size cap for its kind, a bounded response cap, no authorization header of any kind.
pub fn http_spec(provider: &ProviderConfig, kind: CallKind, body: Vec<u8>, max_response_bytes: u32) -> Result<HttpSpec, String> {
    graphql::validate_provider_url(&provider.url).map_err(|e| format!("provider {}: {e}", provider.id))?;
    let cap = if kind == CallKind::Send { MAX_SEND_REQUEST_BYTES } else { MAX_READ_REQUEST_BYTES };
    if body.len() > cap {
        return Err(format!("the request is {} bytes, over the {cap} cap", body.len()));
    }
    if !(graphql::MIN_RESPONSE_BYTES..=graphql::MAX_RESPONSE_BYTES).contains(&max_response_bytes) {
        return Err("max_response_bytes is outside the configured bounds".into());
    }
    Ok(HttpSpec {
        url: provider.url.clone(),
        body,
        max_response_bytes,
        transform: kind.transform(),
        headers: vec![("content-type".into(), "application/json".into()), ("accept".into(), "application/json".into())],
    })
}

/// An upper bound on the cycles one outcall costs, attached up front (unused cycles are refunded):
/// the published formula `(3_000_000 + 60_000 n) n + 400 n request_bytes + 800 n response_bytes` at the
/// largest subnet size. The real cost per call is unmeasured here.
pub fn outcall_cycles(request_bytes: usize, max_response_bytes: u32) -> u128 {
    let n = OUTCALL_SUBNET_NODES;
    (3_000_000 + 60_000 * n) * n + 400 * n * request_bytes as u128 + 800 * n * max_response_bytes as u128
}

#[allow(async_fn_in_trait)]
pub trait SuiTransport {
    /// POST `body` to `provider` and return the transformed response body. An `Err` is a transport
    /// failure (non-200 status, a replica disagreement, a timeout): the provider's reading is
    /// excluded, and for a send the transaction is "maybe sent".
    async fn post(&self, provider: &ProviderConfig, kind: CallKind, body: Vec<u8>, max_response_bytes: u32) -> Result<Vec<u8>, String>;
}

/// The real transport: the IC management canister's HTTPS outcall (wasm only).
#[derive(Clone, Copy, Debug, Default)]
pub struct CanisterTransport;

#[cfg(target_arch = "wasm32")]
impl SuiTransport for CanisterTransport {
    async fn post(&self, provider: &ProviderConfig, kind: CallKind, body: Vec<u8>, max_response_bytes: u32) -> Result<Vec<u8>, String> {
        use ic_cdk::api::management_canister::http_request::{http_request, CanisterHttpRequestArgument, HttpHeader, HttpMethod, TransformContext};
        let spec = http_spec(provider, kind, body, max_response_bytes)?;
        let cycles = outcall_cycles(spec.body.len(), spec.max_response_bytes);
        let arg = CanisterHttpRequestArgument {
            url: spec.url,
            max_response_bytes: Some(u64::from(spec.max_response_bytes)),
            method: HttpMethod::POST,
            headers: spec.headers.into_iter().map(|(name, value)| HttpHeader { name, value }).collect(),
            body: Some(spec.body),
            transform: Some(TransformContext::from_name(spec.transform.to_string(), vec![])),
        };
        match http_request(arg, cycles).await {
            Ok((response,)) if response.status == 200u64 => Ok(response.body),
            Ok((response,)) => Err(format!("{}: HTTP status {}", provider.id, response.status)),
            Err((code, message)) => Err(format!("{}: {code:?}: {message}", provider.id)),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl SuiTransport for CanisterTransport {
    async fn post(&self, provider: &ProviderConfig, _kind: CallKind, _body: Vec<u8>, _max_response_bytes: u32) -> Result<Vec<u8>, String> {
        Err(format!("{}: HTTPS outcalls are unavailable off-chain", provider.id))
    }
}

/// What a broadcast did, per provider. Never evidence of execution or of non-execution.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SendReport {
    /// Providers that HTTP-acknowledged the post (a 200 after the transform): "maybe sent", NOT "executed"
    /// and not "the network accepted it". The body is never evidence either way.
    pub accepted: u32,
    pub failed: u32,
    pub last_error: Option<String>,
}

/// What a verdict means to a caller that has to act: `Ok(Some)` an agreed fact, `Ok(None)` an agreed
/// absence (which proves nothing about whether a thing happened), `Err` that the providers disagree
/// or too few usable ones answered (wait and retry; never zero, never success).
pub fn settle<X>(what: &str, v: Verdict<X>) -> Result<Option<X>, String> {
    match v {
        Verdict::Agreed(x) => Ok(Some(x)),
        Verdict::AgreedMissing => Ok(None),
        Verdict::Held(reason) => Err(format!("{what}: the providers disagree ({reason:?})")),
        Verdict::Unavailable(why) => Err(format!("{what}: unavailable ({why})")),
    }
}

pub struct Mirror<'a, T: SuiTransport> {
    transport: &'a T,
    settings: MirrorSettings,
    genesis_b58: String,
    chain_identifier: Bytes32,
}

fn describe_exclusions<X>(readings: &[(String, Outcome<X>)]) -> String {
    exclusions(readings)
        .into_iter()
        .map(|(id, e)| format!("{id}: {}", match e {
            Exclusion::WrongNetwork(_) => "wrong network".to_string(),
            Exclusion::Identity(m) | Exclusion::Malformed(m) | Exclusion::Transport(m) => m.chars().take(80).collect(),
        }))
        .collect::<Vec<_>>()
        .join("; ")
}

impl<'a, T: SuiTransport> Mirror<'a, T> {
    /// A mirror for explicit settings (tests, and the stored configuration through [`Self::from_state`]).
    pub fn new(transport: &'a T, settings: MirrorSettings, chain_identifier: Bytes32) -> Result<Self, String> {
        graphql::validate_settings(&settings)?;
        if !settings.enabled {
            return Err("the Sui provider mirror is disabled".into());
        }
        Ok(Mirror { transport, settings, genesis_b58: format_digest_base58(&chain_identifier), chain_identifier })
    }

    /// From the stored identity and provider set; refuses a disabled or unconfigured mirror.
    pub fn from_state(transport: &'a T) -> Result<Self, String> {
        let (settings, chain) = crate::state::read_core(|c| (c.mirror.clone(), c.chain_identifier));
        Self::new(transport, settings, chain)
    }

    pub fn chain_identifier(&self) -> Bytes32 {
        self.chain_identifier
    }

    fn rule(&self) -> QuorumRule {
        QuorumRule::from(&self.settings)
    }

    /// One request to every configured provider, each reading parsed and identity-checked, then the
    /// strict tally. Providers are asked in order; each call is bounded by `max_response_bytes`.
    async fn read<X: Clone + Eq>(&self, kind: CallKind, req: &GraphqlRequest, parse: impl Fn(&Value) -> Result<Option<X>, Exclusion>) -> Verdict<X> {
        let body = req.body();
        let mut readings: Vec<(String, Outcome<X>)> = Vec::with_capacity(self.settings.providers.len());
        for p in &self.settings.providers {
            let outcome = match self.transport.post(p, kind, body.clone(), self.settings.max_response_bytes).await {
                Ok(bytes) => outcome_from_body(&bytes, &parse),
                Err(e) => Outcome::Excluded(Exclusion::Transport(e)),
            };
            readings.push((p.id.clone(), outcome));
        }
        match tally(&readings, self.rule()) {
            Verdict::Unavailable(why) => {
                let detail = describe_exclusions(&readings);
                Verdict::Unavailable(if detail.is_empty() { why } else { format!("{why}; excluded: {detail}") })
            }
            other => other,
        }
    }

    /// The chain identifier every provider reports (a connectivity and identity check).
    pub async fn chain(&self) -> Verdict<()> {
        let expected = self.genesis_b58.clone();
        self.read(CallKind::Read, &request_chain_identifier(), |d| graphql::check_chain_identifier(d, &expected).map(|_| Some(()))).await
    }

    /// The head, rounded down by the transform: a checkpoint number all providers within a window agree on.
    pub async fn head(&self) -> Verdict<u64> {
        let expected = self.genesis_b58.clone();
        self.read(CallKind::Head, &request_head(), |d| parse_head(d, &expected)).await
    }

    /// A checkpoint by number, with its epoch and the epoch's reference gas price.
    pub async fn checkpoint_gas(&self, sequence: u64) -> Verdict<CheckpointGasFact> {
        let expected = self.genesis_b58.clone();
        self.read(CallKind::Read, &request_checkpoint_gas(sequence), |d| parse_checkpoint_gas(d, sequence, &expected)).await
    }

    /// A checkpoint by number (digest and epoch only: the accepted BB-52 shape).
    pub async fn checkpoint(&self, sequence: u64) -> Verdict<CheckpointFact> {
        let expected = self.genesis_b58.clone();
        self.read(CallKind::Read, &request_checkpoint(sequence), |d| parse_checkpoint(d, sequence, &expected)).await
    }

    /// The configured gate as of `checkpoint`, after every identity and invariant check.
    pub async fn gate(&self, cfg: &ConfiguredGate, checkpoint: u64) -> Verdict<GateFact> {
        let expected = self.genesis_b58.clone();
        let chain = self.chain_identifier;
        let cfg = *cfg;
        self.read(CallKind::Read, &request_object_at_checkpoint(&cfg.gate, checkpoint), move |d| {
            match parse_object(d, &cfg.gate, &expected)? {
                None => Ok(None),
                Some(obj) => interpret_gate(&obj, &cfg, &chain).map(Some),
            }
        })
        .await
    }

    /// An object as of `checkpoint`, with the owner's address asked for. The caller interprets it.
    pub async fn owned_object(&self, address: &Bytes32, checkpoint: u64) -> Verdict<ObjectFact> {
        let expected = self.genesis_b58.clone();
        let address = *address;
        self.read(CallKind::Read, &request_owned_object(&address, checkpoint), move |d| parse_owned_object(d, &address, &expected)).await
    }

    /// An object at an EXACT version. The ONE composed helper for exact-version reads: the response
    /// is parsed with `parse_object_at_version`, so a provider that answers with another version is
    /// excluded before the tally and can never count as agreeing.
    pub async fn object_at_version(&self, address: &Bytes32, version: u64) -> Verdict<ObjectFact> {
        let expected = self.genesis_b58.clone();
        let address = *address;
        self.read(CallKind::Read, &request_object_at_version(&address, version), move |d| parse_object_at_version(d, &address, version, &expected)).await
    }

    /// `mints[mint_id]` as of `checkpoint`: the record, with the value type checked. `AgreedMissing`
    /// means the providers agree the entry is not there at that checkpoint, nothing more.
    pub async fn mint_entry(&self, cfg: &ConfiguredGate, table: &Bytes32, checkpoint: u64, mint_id: &Bytes32) -> Verdict<MintRecord> {
        let expected = self.genesis_b58.clone();
        let cfg = *cfg;
        let key = *mint_id;
        self.read(CallKind::Read, &request_table_entry(table, checkpoint, &key), move |d| match parse_table_entry(d, &key, &expected)? {
            None => Ok(None),
            Some(entry) => interpret_mint_entry(&entry, &cfg).map(Some),
        })
        .await
    }

    pub async fn transaction(&self, digest: &Bytes32) -> Verdict<TransactionFact> {
        let expected = self.genesis_b58.clone();
        let digest = *digest;
        self.read(CallKind::Read, &request_transaction(&digest), move |d| parse_transaction(d, &digest, &expected)).await
    }

    pub async fn coin_metadata(&self, coin_type: &str) -> Verdict<CoinMetadataFact> {
        let expected = self.genesis_b58.clone();
        self.read(CallKind::Read, &request_coin_metadata(coin_type), |d| parse_coin_metadata(d, &expected)).await
    }

    /// Broadcast one signed transaction to every provider. The reply of each is reduced to "accepted
    /// or not" by the transform and is not evidence of anything: the caller confirms by lookup.
    pub async fn send(&self, tx_bytes: &[u8], signatures_base64: &[String]) -> SendReport {
        let body = request_execute(&STANDARD.encode(tx_bytes), signatures_base64).body();
        let mut report = SendReport::default();
        for p in &self.settings.providers {
            match self.transport.post(p, CallKind::Send, body.clone(), self.settings.max_response_bytes).await {
                Ok(_) => report.accepted += 1,
                Err(e) => {
                    report.failed += 1;
                    report.last_error = Some(e.chars().take(200).collect());
                }
            }
        }
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(id: &str) -> ProviderConfig {
        ProviderConfig { id: id.into(), url: format!("https://{id}.example.com/graphql") }
    }

    #[test]
    fn the_request_is_https_keyless_bounded_and_carries_no_authorization() {
        let p = provider("alpha");
        let spec = http_spec(&p, CallKind::Read, b"{}".to_vec(), 16_384).unwrap();
        assert_eq!(spec.url, "https://alpha.example.com/graphql");
        assert_eq!(spec.transform, "transform_sui_graphql");
        assert!(spec.headers.iter().all(|(k, _)| !k.eq_ignore_ascii_case("authorization") && !k.eq_ignore_ascii_case("x-api-key")));
        assert_eq!(http_spec(&p, CallKind::Head, b"{}".to_vec(), 2_048).unwrap().transform, "transform_sui_graphql_head");
        assert_eq!(http_spec(&p, CallKind::Send, vec![0; 100], 2_048).unwrap().transform, "transform_sui_send");
        // Bounds.
        assert!(http_spec(&p, CallKind::Read, vec![0; MAX_READ_REQUEST_BYTES + 1], 16_384).is_err());
        assert!(http_spec(&p, CallKind::Send, vec![0; MAX_SEND_REQUEST_BYTES], 16_384).is_ok());
        assert!(http_spec(&p, CallKind::Send, vec![0; MAX_SEND_REQUEST_BYTES + 1], 16_384).is_err());
        assert!(http_spec(&p, CallKind::Read, b"{}".to_vec(), 10).is_err(), "response cap below the minimum");
        assert!(http_spec(&p, CallKind::Read, b"{}".to_vec(), graphql::MAX_RESPONSE_BYTES + 1).is_err());
        // The provider URL rules are re-checked at the moment of the call.
        for url in ["http://alpha.example.com/", "https://u:p@alpha.example.com/", "https://alpha.example.com/graphql?key=1", "https://alpha.example.com/v1/0123456789abcdefghij0123456789"] {
            let bad = ProviderConfig { id: "alpha".into(), url: url.into() };
            assert!(http_spec(&bad, CallKind::Read, b"{}".to_vec(), 16_384).is_err(), "{url}");
        }
    }

    #[test]
    fn the_cycle_estimate_grows_with_both_sizes_and_is_an_upper_bound_formula() {
        let base = outcall_cycles(0, 0);
        assert_eq!(base, (3_000_000 + 60_000 * 34) * 34);
        assert_eq!(outcall_cycles(1_000, 0) - base, 400 * 34 * 1_000);
        assert_eq!(outcall_cycles(0, 16_384) - base, 800 * 34 * 16_384);
        assert!(outcall_cycles(500, 16_384) < 1_000_000_000, "one bounded call stays below a billion cycles");
    }

    #[test]
    fn the_real_transport_is_unavailable_off_chain_and_a_disabled_mirror_refuses() {
        struct Fixed;
        impl SuiTransport for Fixed {
            async fn post(&self, _p: &ProviderConfig, _k: CallKind, _b: Vec<u8>, _m: u32) -> Result<Vec<u8>, String> {
                Ok(vec![])
            }
        }
        let t = Fixed;
        let disabled = MirrorSettings { providers: vec![provider("alpha"), provider("beta")], enabled: false, ..MirrorSettings::default() };
        assert!(Mirror::new(&t, disabled, crate::ids::TESTNET_CHAIN_IDENTIFIER).is_err());
        let r = crate::testkit::block_on(CanisterTransport.post(&provider("alpha"), CallKind::Read, vec![], 2_048));
        assert!(r.is_err());
    }
}
