//! Explicit XRPL network selection, fixture isolation, and artifact-environment
//! inspection.
//!
//! * Normal builds: native-XRP collateral still requires the production Schnorr key
//!   (`key_1`) and routes to XRPL mainnet, byte-for-byte as before.
//! * `xrpl-testnet-fixture` builds (default OFF): only `test_key_1` is accepted for
//!   collateral and EVERY XRPL read/write is pinned to the public testnet. Mainnet
//!   cannot be selected for either the collateral rail or the token lane
//!   (`fixture_cannot_select_mainnet`).
//! * The token lane (`issued_rpc`) always takes an explicit [`XrplNetwork`]; it never
//!   infers the network from a signing-key name.
//!
//! The build environment is stamped into the final Wasm as a public custom section
//! `rumi:xrpl-environment` (see `main.rs`), and a fixture build additionally
//! exports the `xrpl_fixture_environment_marker` query. The pure Wasm inspector
//! below is mirrored by `scripts/verify-xrpl-icusd-artifact.sh`, which the normal
//! backend build runs (`--require-mainnet`) so a fixture artifact can never be
//! packaged for a mainnet target.

use candid::CandidType;
use serde::{Deserialize, Serialize};

use super::config::{
    is_xrp_production_key_name, XRP_PRODUCTION_SCHNORR_KEY_NAME, XRP_TEST_SCHNORR_KEY_NAME,
};

/// True only in the feature-gated test artifact.
pub const FIXTURE_BUILD: bool = cfg!(feature = "xrpl-testnet-fixture");

pub const ENV_PRODUCTION: &str = "production";
pub const ENV_TESTNET_FIXTURE: &str = "testnet-fixture";
/// Public Wasm metadata name (stored as custom section `icp:public <name>`).
pub const ENVIRONMENT_METADATA_NAME: &str = "rumi:xrpl-environment";
/// Export (query method) present ONLY in fixture builds.
pub const FIXTURE_MARKER_QUERY: &str = "xrpl_fixture_environment_marker";

/// The environment this binary was compiled for.
pub const fn build_environment() -> &'static str {
    if FIXTURE_BUILD {
        ENV_TESTNET_FIXTURE
    } else {
        ENV_PRODUCTION
    }
}

#[derive(CandidType, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum XrplNetwork {
    Mainnet,
    Testnet,
}

impl XrplNetwork {
    pub fn name(&self) -> &'static str {
        match self {
            XrplNetwork::Mainnet => "mainnet",
            XrplNetwork::Testnet => "testnet",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetworkError {
    /// A fixture artifact must never talk to (or sign for) mainnet.
    FixtureCannotSelectMainnet,
}

impl std::fmt::Display for NetworkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetworkError::FixtureCannotSelectMainnet => {
                write!(f, "testnet fixture build cannot select XRPL mainnet")
            }
        }
    }
}

const MAINNET_URL: &str = "https://xrplcluster.com/";
const TESTNET_URL: &str = "https://s.altnet.rippletest.net:51234/";
const MAINNET_READ_URLS: &[&str] = &[
    MAINNET_URL,
    "https://xrpl.link/",
    "https://s1.ripple.com:51234/",
];
const TESTNET_READ_URLS: &[&str] = &[TESTNET_URL, "https://testnet.xrpl-labs.com/"];

/// Provider URLs with no fixture check (internal; callers go through the checked
/// functions below).
pub(super) fn provider_urls_unchecked(network: XrplNetwork) -> &'static [&'static str] {
    match network {
        XrplNetwork::Mainnet => MAINNET_READ_URLS,
        XrplNetwork::Testnet => TESTNET_READ_URLS,
    }
}

/// Reject any network this build is not allowed to use.
pub fn check_network_allowed(network: XrplNetwork) -> Result<XrplNetwork, NetworkError> {
    if FIXTURE_BUILD && network == XrplNetwork::Mainnet {
        return Err(NetworkError::FixtureCannotSelectMainnet);
    }
    Ok(network)
}

/// Provider quorum URLs for an explicit network.
pub fn read_provider_urls_for(
    network: XrplNetwork,
) -> Result<&'static [&'static str], NetworkError> {
    check_network_allowed(network).map(provider_urls_unchecked)
}

/// Primary provider URL for an explicit network.
pub fn rpc_url_for(network: XrplNetwork) -> Result<&'static str, NetworkError> {
    read_provider_urls_for(network).map(|u| u[0])
}

/// Network the native-XRP COLLATERAL rail talks to for a configured key name.
/// Normal builds: mainnet for the production key, testnet otherwise (unchanged).
/// Fixture builds: always testnet.
pub fn collateral_network_for_key(chain_key_name: &str) -> XrplNetwork {
    if FIXTURE_BUILD {
        return XrplNetwork::Testnet;
    }
    if is_xrp_production_key_name(chain_key_name) {
        XrplNetwork::Mainnet
    } else {
        XrplNetwork::Testnet
    }
}

/// The Schnorr key name this build's native-XRP collateral gate accepts.
pub const fn expected_collateral_key_name() -> &'static str {
    if FIXTURE_BUILD {
        XRP_TEST_SCHNORR_KEY_NAME
    } else {
        XRP_PRODUCTION_SCHNORR_KEY_NAME
    }
}

/// Key-name gate for native-XRP collateral operations. Normal builds require the
/// production key; ONLY the fixture build accepts `test_key_1` (and then rejects
/// `key_1`, so the fixture can never sign with the production key).
pub fn require_collateral_key(configured_key: &str) -> Result<(), String> {
    if FIXTURE_BUILD {
        if configured_key == XRP_TEST_SCHNORR_KEY_NAME {
            return Ok(());
        }
        return Err(format!(
            "xrpl-testnet-fixture build requires Schnorr key {XRP_TEST_SCHNORR_KEY_NAME} (configured: {configured_key})"
        ));
    }
    if is_xrp_production_key_name(configured_key) {
        return Ok(());
    }
    Err(format!(
        "native-XRP operations require production Schnorr key {XRP_PRODUCTION_SCHNORR_KEY_NAME} (configured: {configured_key})"
    ))
}

// ---------------------------------------------------------------------------
// Wasm environment inspection (pure; mirrored by verify-xrpl-icusd-artifact.sh)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WasmEnvironment {
    /// Values of every `rumi:xrpl-environment` custom section found.
    pub markers: Vec<String>,
    /// True when the module exports `canister_query xrpl_fixture_environment_marker`.
    pub has_fixture_query: bool,
}

fn read_leb_u32(b: &[u8], pos: &mut usize) -> Result<u32, String> {
    let mut result: u64 = 0;
    let mut shift = 0u32;
    loop {
        let byte = *b.get(*pos).ok_or("truncated LEB128")?;
        *pos += 1;
        result |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift > 35 {
            return Err("LEB128 too long".into());
        }
    }
    u32::try_from(result).map_err(|_| "LEB128 overflow".to_string())
}

fn read_name<'a>(b: &'a [u8], pos: &mut usize) -> Result<&'a str, String> {
    let len = read_leb_u32(b, pos)? as usize;
    let end = pos
        .checked_add(len)
        .filter(|e| *e <= b.len())
        .ok_or("name overflows")?;
    let s = std::str::from_utf8(&b[*pos..end]).map_err(|_| "non-utf8 name".to_string())?;
    *pos = end;
    Ok(s)
}

/// Parse the module's section list (no full validation): collects environment
/// markers from custom sections and detects the fixture export.
pub fn inspect_wasm_environment(wasm: &[u8]) -> Result<WasmEnvironment, String> {
    if wasm.len() < 8 || &wasm[..4] != b"\0asm" {
        return Err("not a Wasm module".into());
    }
    let mut env = WasmEnvironment::default();
    let mut pos = 8usize;
    while pos < wasm.len() {
        let id = wasm[pos];
        pos += 1;
        let size = read_leb_u32(wasm, &mut pos)? as usize;
        let end = pos
            .checked_add(size)
            .filter(|e| *e <= wasm.len())
            .ok_or("section overflows module")?;
        let payload = &wasm[pos..end];
        match id {
            0 => {
                let mut p = 0usize;
                let name = read_name(payload, &mut p)?;
                let bare = name
                    .strip_prefix("icp:public ")
                    .or_else(|| name.strip_prefix("icp:private "))
                    .unwrap_or(name);
                if bare == ENVIRONMENT_METADATA_NAME {
                    env.markers
                        .push(String::from_utf8_lossy(&payload[p..]).into_owned());
                }
            }
            7 => {
                let mut p = 0usize;
                let count = read_leb_u32(payload, &mut p)?;
                for _ in 0..count {
                    let name = read_name(payload, &mut p)?;
                    p += 1; // kind
                    read_leb_u32(payload, &mut p)?; // index
                    if name == format!("canister_query {FIXTURE_MARKER_QUERY}") {
                        env.has_fixture_query = true;
                    }
                }
            }
            _ => {}
        }
        pos = end;
    }
    Ok(env)
}

/// Release gate (`--require-mainnet`): exactly one marker equal to `production`,
/// no fixture marker, no fixture query.
pub fn verify_release_artifact(wasm: &[u8]) -> Result<(), String> {
    let env = inspect_wasm_environment(wasm)?;
    if env.has_fixture_query {
        return Err(format!(
            "fixture marker query `{FIXTURE_MARKER_QUERY}` is exported"
        ));
    }
    if env.markers.iter().any(|m| m == ENV_TESTNET_FIXTURE) {
        return Err("artifact carries the testnet-fixture environment marker".into());
    }
    match env.markers.as_slice() {
        [m] if m == ENV_PRODUCTION => Ok(()),
        [] => Err(format!("missing `{ENVIRONMENT_METADATA_NAME}` marker")),
        other => Err(format!("unexpected environment markers: {other:?}")),
    }
}

/// Fixture-runner gate: exactly one `testnet-fixture` marker AND the marker query.
pub fn verify_fixture_artifact(wasm: &[u8]) -> Result<(), String> {
    let env = inspect_wasm_environment(wasm)?;
    if !env.has_fixture_query {
        return Err(format!(
            "missing fixture marker query `{FIXTURE_MARKER_QUERY}`"
        ));
    }
    match env.markers.as_slice() {
        [m] if m == ENV_TESTNET_FIXTURE => Ok(()),
        other => Err(format!(
            "expected a single testnet-fixture marker, found {other:?}"
        )),
    }
}

#[cfg(test)]
pub(crate) mod test_wasm {
    //! Tiny hand-built Wasm modules for inspector tests.
    fn leb(mut v: u32, out: &mut Vec<u8>) {
        loop {
            let b = (v & 0x7F) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                break;
            }
            out.push(b | 0x80);
        }
    }
    fn section(id: u8, payload: &[u8], out: &mut Vec<u8>) {
        out.push(id);
        leb(payload.len() as u32, out);
        out.extend_from_slice(payload);
    }
    fn name(s: &str, out: &mut Vec<u8>) {
        leb(s.len() as u32, out);
        out.extend_from_slice(s.as_bytes());
    }
    /// Build a valid module exporting the given function names and carrying the given
    /// `(section_name, bytes)` custom sections.
    pub fn build(exports: &[&str], customs: &[(&str, &[u8])]) -> Vec<u8> {
        let mut m = b"\0asm\x01\x00\x00\x00".to_vec();
        section(1, &[1, 0x60, 0, 0], &mut m); // one () -> () type
        section(3, &[1, 0], &mut m); // one function of type 0
        let mut ex = Vec::new();
        leb(exports.len() as u32, &mut ex);
        for e in exports {
            name(e, &mut ex);
            ex.push(0); // func
            leb(0, &mut ex);
        }
        section(7, &ex, &mut m);
        section(10, &[1, 2, 0, 0x0B], &mut m); // body: no locals, end
        for (n, data) in customs {
            let mut p = Vec::new();
            name(n, &mut p);
            p.extend_from_slice(data);
            section(0, &p, &mut m);
        }
        m
    }
}

#[cfg(test)]
mod tests {
    use super::test_wasm::build;
    use super::*;

    #[test]
    fn network_names_and_urls() {
        assert_eq!(XrplNetwork::Mainnet.name(), "mainnet");
        let t = provider_urls_unchecked(XrplNetwork::Testnet);
        assert!(t
            .iter()
            .all(|u| u.contains("rippletest") || u.contains("testnet")));
        let m = provider_urls_unchecked(XrplNetwork::Mainnet);
        assert!(m.iter().all(|u| !u.contains("test")));
    }

    #[cfg(not(feature = "xrpl-testnet-fixture"))]
    #[test]
    fn normal_build_requires_production_collateral_key_and_mainnet() {
        assert!(!FIXTURE_BUILD);
        assert_eq!(build_environment(), ENV_PRODUCTION);
        assert!(require_collateral_key("key_1").is_ok());
        assert!(require_collateral_key("test_key_1").is_err());
        assert!(require_collateral_key("key_10").is_err());
        assert!(require_collateral_key("").is_err());
        assert_eq!(collateral_network_for_key("key_1"), XrplNetwork::Mainnet);
        // Unchanged legacy behaviour: any other key name routes to the testnet.
        assert_eq!(
            collateral_network_for_key("test_key_1"),
            XrplNetwork::Testnet
        );
        // Mainnet is selectable explicitly for the token lane in a normal build.
        assert_eq!(
            check_network_allowed(XrplNetwork::Mainnet),
            Ok(XrplNetwork::Mainnet)
        );
        assert_eq!(
            rpc_url_for(XrplNetwork::Mainnet).unwrap(),
            "https://xrplcluster.com/"
        );
        // The collateral rail's URL selection is byte-for-byte the legacy one.
        assert_eq!(
            super::super::xrp_rpc::rpc_url("key_1"),
            "https://xrplcluster.com/"
        );
        assert_eq!(
            super::super::xrp_rpc::rpc_url("test_key_1"),
            "https://s.altnet.rippletest.net:51234/"
        );
    }

    #[cfg(feature = "xrpl-testnet-fixture")]
    #[test]
    fn fixture_cannot_select_mainnet() {
        assert!(FIXTURE_BUILD);
        assert_eq!(build_environment(), ENV_TESTNET_FIXTURE);
        assert_eq!(
            check_network_allowed(XrplNetwork::Mainnet),
            Err(NetworkError::FixtureCannotSelectMainnet)
        );
        assert_eq!(
            read_provider_urls_for(XrplNetwork::Mainnet),
            Err(NetworkError::FixtureCannotSelectMainnet)
        );
        assert_eq!(
            rpc_url_for(XrplNetwork::Mainnet),
            Err(NetworkError::FixtureCannotSelectMainnet)
        );
        assert!(rpc_url_for(XrplNetwork::Testnet).is_ok());
        // Collateral: even the production key name is clamped to testnet and refused.
        assert_eq!(collateral_network_for_key("key_1"), XrplNetwork::Testnet);
        assert_eq!(
            collateral_network_for_key("test_key_1"),
            XrplNetwork::Testnet
        );
        assert!(require_collateral_key("key_1").is_err());
        assert!(require_collateral_key("test_key_1").is_ok());
    }

    #[cfg(feature = "xrpl-testnet-fixture")]
    #[test]
    fn public_testnet_collateral_rpc_uses_fixture_network() {
        use super::super::xrp_rpc;
        for key in ["test_key_1", "key_1"] {
            // Every collateral request builder derives its URL from `rpc_url(key)`
            // (and its provider quorum from the same mapping): all testnet.
            assert_eq!(
                xrp_rpc::rpc_url(key),
                "https://s.altnet.rippletest.net:51234/"
            );
            assert_eq!(
                provider_urls_unchecked(collateral_network_for_key(key)),
                TESTNET_READ_URLS
            );
        }
    }

    #[test]
    fn release_rejects_fixture_artifact() {
        let prod = build(
            &["canister_query foo"],
            &[("icp:public rumi:xrpl-environment", b"production")],
        );
        assert_eq!(verify_release_artifact(&prod), Ok(()));
        assert!(verify_fixture_artifact(&prod).is_err());

        // Fixture marker section.
        let fixture = build(
            &["canister_query xrpl_fixture_environment_marker"],
            &[("icp:public rumi:xrpl-environment", b"testnet-fixture")],
        );
        assert!(verify_release_artifact(&fixture).is_err());
        assert_eq!(verify_fixture_artifact(&fixture), Ok(()));

        // Marker section stripped but the fixture query is still exported.
        let stripped = build(&["canister_query xrpl_fixture_environment_marker"], &[]);
        assert!(verify_release_artifact(&stripped).is_err());
        // Query missing but fixture marker present.
        let marker_only = build(
            &[],
            &[("icp:public rumi:xrpl-environment", b"testnet-fixture")],
        );
        assert!(verify_release_artifact(&marker_only).is_err());
        assert!(verify_fixture_artifact(&marker_only).is_err());
        // Unmarked and conflicting artifacts are not releasable either.
        assert!(verify_release_artifact(&build(&[], &[])).is_err());
        let conflict = build(
            &[],
            &[
                ("icp:public rumi:xrpl-environment", b"production"),
                ("icp:private rumi:xrpl-environment", b"testnet-fixture"),
            ],
        );
        assert!(verify_release_artifact(&conflict).is_err());
        let dup = build(
            &[],
            &[
                ("icp:public rumi:xrpl-environment", b"production"),
                ("rumi:xrpl-environment", b"production"),
            ],
        );
        assert!(verify_release_artifact(&dup).is_err());
        assert!(verify_release_artifact(b"not wasm").is_err());
        assert!(verify_release_artifact(b"\0asm\x01\0\0\0\x07\xff").is_err());
        // An unrelated export that merely contains the query text is not the export.
        let near = build(
            &["xrpl_fixture_environment_marker"],
            &[("icp:public rumi:xrpl-environment", b"production")],
        );
        assert_eq!(verify_release_artifact(&near), Ok(()));
    }
}
