//! Rumi icUSD bridge, Sui instance: canister FOUNDATION (card BB-52). Source and local
//! capability only: nothing is deployed, signed, funded, broadcast or read from a network, and
//! the Sui profile stays `disabled`. This is NOT a Sui bridge: there is no mint lane, no
//! observer, no custody, no accounting and no wallet surface (BB-53 and BB-54).
//!
//! One canister instance serves one network (settled topology D1). The design is
//! `docs/coordination/multichain/chain-sui.md`; the Move package it talks to is
//! `move/rumi_icusd_gate` at 23f75ef1 (accepted). The instance-skeleton pattern is the frozen
//! Solana instance at 23dfe841 (`rumi_icusd_sol_bridge`); none of its later or unaccepted work
//! (49c, 49d) is copied.
//!
//! What exists in this crate:
//!
//! * `ids`: the two accepted network identities, the three role key paths and the id domains
//!   (mint id, the return `transfer_id` shared with the Move gate);
//! * `hash`, `bcs`, `sui`: BLAKE2b-256 (the `blake2` crate), the BCS subset and Sui address and object primitives;
//! * `tx`: the sponsored `rumi_icusd_gate::mint` `TransactionData` builder with BOTH digests
//!   (signing digest and typed transaction digest), the Ed25519 wire signature, and a strict
//!   decoder used as a shape guard before any signature;
//! * `gate`: the configured gate identities and the decoders for the `Gate` object and its
//!   `mints` and `returns` table entries;
//! * `graphql`: bounded queries, parsers with per-provider network and identity checks, the
//!   strict quorum and the deterministic response transform. Pure; nothing is sent;
//! * `schnorr`: the threshold-Ed25519 signer seam; `state`, `types`, `env`: the skeleton.
//!
//! # Sources (pinned, official; read 2026-10-31)
//!
//! * `MystenLabs/sui` at `mainnet-v1.82.3`: `crates/sui-types/src/transaction.rs` (the BCS enums
//!   and structs), `crypto.rs` (`BcsSignable`/`Signable`, `default_hash`, `Signature::new_secure`
//!   and the signature byte layout), `digests.rs` (`ChainIdentifier` and the mainnet and testnet
//!   genesis digests), `crates/sui-indexer-alt-graphql/schema.graphql` (the SDL), and
//!   `crates/sui-sdk/src/sui_client_config.rs` (not used by the crate; tooling only).
//! * `MystenLabs/sui-rust-sdk`, `sui-sdk-types` 0.3.2 (docs.rs): `hash.rs` for the two digests and
//!   `digest.rs` for the length-prefixed digest encoding.
//! * `MystenLabs/ts-sdks`, `packages/sui/src/bcs/bcs.ts`: an independent declaration of the same
//!   BCS orders.
//!
//! # Authority and upgrade limits
//!
//! * Controllers (the IC canister controllers) are the root of trust: every role key is
//!   canister-derived, so whoever controls the canister reaches all signing and, later, the
//!   escrow. The set is not established here (D6 is held).
//! * `set_gate_binding` and `derive_role_keys` are controller-only and set-once: the network,
//!   the binding and the role keys never change after they are set. A wrong binding needs a
//!   reinstall of this instance, which is a decision for the controllers, not an endpoint.
//! * An upgrade can only replace the provider set (`UpgradeArgs.mirror`); it re-validates the
//!   stored network, so a bad upgrade traps and the old module stays.
//! * No endpoint signs or broadcasts: `signing_enabled` is false by construction.
//!
//! # Open items this crate carries (honest list)
//!
//! * `sui-signing-vectors-unverified`: the golden vectors are an independent Python reference
//!   written from the pinned sources, plus the pinned sources themselves. They are NOT checked
//!   against the official Sui Rust SDK or CLI binary (running either was out of scope and the SDK
//!   is not a dependency). The IC `sign_with_schnorr` mapping for Sui is unrun.
//! * `sui-graphql-schema-unmeasured`: fixtures are schema-derived, not captured from a provider;
//!   `UInt53` number versus string and the `repr` address form are tolerated both ways.
//! * `sui-rpc-provider-contract`: no provider is contracted; the mirror is disabled by default
//!   and the foundation has no HTTP transport.
//! * `sui-epoch-expiry-nonexecution-proof`: the builder binds `Epoch(e)` to bound signing; it
//!   never infers expiry or non-execution from an epoch passing or a missing digest.
//! * Ed25519 signature verification is not in the production build (tests verify with
//!   `ed25519-dalek`); `assemble_signed_mint` checks that each key matches its role address.
//! * BLAKE2b-256 is the RustCrypto `blake2` crate, the one dependency added to `Cargo.lock` for this card.

pub mod bcs;
pub mod env;
pub mod gate;
pub mod graphql;
pub mod hash;
pub mod ids;
pub mod schnorr;
pub mod state;
pub mod sui;
pub mod tx;
pub mod types;
#[cfg(test)]
#[allow(dead_code)]
mod vectors;
#[cfg(test)]
mod graphql_identity_tests;

use candid::Principal;
use ids::SuiRole;
use types::*;

// ---------------------------------------------------------------------------
// Guards
// ---------------------------------------------------------------------------

fn require_controller() -> SuiResult<Principal> {
    let caller = env::caller();
    if caller == Principal::anonymous() {
        return Err(SuiBridgeError::Anonymous);
    }
    if env::is_controller(&caller) {
        Ok(caller)
    } else {
        Err(SuiBridgeError::Unauthorized)
    }
}

// ---------------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------------

fn binding_view(b: &state::StoredBinding) -> GateBindingView {
    let cfg = b.configured();
    GateBindingView {
        package_id: sui::format_address(&b.package),
        gate_id: sui::format_address(&b.gate),
        mint_cap_id: sui::format_address(&b.mint_cap),
        admin_cap_id: sui::format_address(&b.admin_cap),
        gate_type: cfg.gate_type(),
        coin_type: cfg.coin_type(),
        decimals: gate::DECIMALS,
    }
}

fn config_view(c: &state::SuiCore) -> ConfigView {
    ConfigView {
        initialized: c.initialized,
        network: c.network,
        chain_identifier: hex::encode(c.chain_identifier),
        short_chain_id: sui::short_chain_id(&c.chain_identifier),
        schnorr_key_name: c.network.key_name().to_string(),
        instance_salt: hex::encode(c.instance_salt),
        binding: c.binding.as_ref().map(binding_view),
        mirror: c.mirror.clone(),
        bridge_version: ids::BRIDGE_VERSION.to_string(),
        schema_version: c.schema_version,
        signing_enabled: false,
    }
}

fn role_views(c: &state::SuiCore) -> Vec<RoleView> {
    SuiRole::ALL
        .iter()
        .map(|role| {
            let key = c.roles.iter().find(|k| k.role == *role).map(|k| k.public_key);
            RoleView {
                role: *role,
                derivation_path: ids::derivation_path(*role, &c.chain_identifier).iter().map(hex::encode).collect(),
                public_key: key.as_ref().map(hex::encode),
                address: key.as_ref().map(|k| sui::format_address(&sui::address_from_ed25519(k))),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

#[ic_cdk::init]
fn init(arg: BridgeArg) {
    let args = match arg {
        BridgeArg::Init(a) => a,
        BridgeArg::Upgrade(_) => ic_cdk::trap("install requires BridgeArg::Init"),
    };
    let (chain, mirror) = state::validate_init(&args).unwrap_or_else(|e| ic_cdk::trap(&format!("invalid init args: {e}")));
    let salt = ids::instance_salt(env::canister_id().as_slice(), env::now_ns());
    state::mutate_core(|c| {
        if c.initialized {
            ic_cdk::trap("bridge state already initialized");
        }
        c.initialized = true;
        c.network = args.network;
        c.chain_identifier = chain;
        c.instance_salt = salt;
        c.mirror = mirror;
    });
}

#[ic_cdk::post_upgrade]
fn post_upgrade(arg: Option<BridgeArg>) {
    match arg {
        None => {}
        Some(BridgeArg::Upgrade(u)) => {
            if let Some(mirror) = u.mirror {
                if let Err(e) = graphql::validate_settings(&mirror) {
                    ic_cdk::trap(&format!("upgrade refused, the previous module stays installed: {e}"));
                }
                state::mutate_core(|c| c.mirror = mirror);
            }
        }
        Some(BridgeArg::Init(_)) => ic_cdk::trap("upgrade requires BridgeArg::Upgrade or no argument"),
    }
    if !state::read_core(|c| c.initialized) {
        ic_cdk::trap("upgrade of an uninitialized bridge");
    }
    // A trap in `post_upgrade` rolls the upgrade back: the old module and its state stay in place.
    if let Err(e) = state::read_core(state::validate_stored_config) {
        ic_cdk::trap(&format!("upgrade refused, the previous module stays installed: {e}"));
    }
    state::migrate_core();
}

// ---------------------------------------------------------------------------
// Public queries (no secrets: the configuration is public by design)
// ---------------------------------------------------------------------------

#[ic_cdk::query]
fn bridge_version() -> String {
    ids::BRIDGE_VERSION.to_string()
}

#[ic_cdk::query]
fn get_config() -> ConfigView {
    state::read_core(config_view)
}

#[ic_cdk::query]
fn get_roles() -> Vec<RoleView> {
    state::read_core(role_views)
}

// ---------------------------------------------------------------------------
// Controller methods
// ---------------------------------------------------------------------------

/// Record the exact identities of the deployed gate. Set once; the same ids again are a no-op
/// and different ids are a conflict.
#[ic_cdk::update]
fn set_gate_binding(arg: GateBindingArg) -> SuiResult<ConfigView> {
    require_controller()?;
    let binding = state::validate_binding(&arg).map_err(SuiBridgeError::InvalidArgument)?;
    state::set_binding(binding)?;
    Ok(state::read_core(config_view))
}

/// Derive the three role public keys from the threshold key (free calls, no signing) and store
/// them once. Any rejected or malformed reply, a duplicate key or a role set that is not three
/// distinct keys is refused and nothing is stored.
#[ic_cdk::update]
async fn derive_role_keys() -> SuiResult<Vec<RoleView>> {
    require_controller()?;
    let (network, chain, initialized) = state::read_core(|c| (c.network, c.chain_identifier, c.initialized));
    if !initialized {
        return Err(SuiBridgeError::NotConfigured("the instance is not initialized".into()));
    }
    let signer = schnorr::SuiSigner::new(network, chain).map_err(SuiBridgeError::InvalidArgument)?;
    let mut keys = Vec::with_capacity(SuiRole::ALL.len());
    for role in SuiRole::ALL {
        let key = signer.fetch_public_key(role).await.map_err(SuiBridgeError::SignerFailed)?;
        keys.push((role, key));
    }
    state::set_role_keys(keys)?;
    Ok(state::read_core(role_views))
}

// ---------------------------------------------------------------------------
// HTTP outcall transform (the transport itself is BB-53)
// ---------------------------------------------------------------------------

use ic_cdk::api::management_canister::http_request::{HttpResponse, TransformArgs};

/// Keep only the deterministic part of a GraphQL response (`data` and truncated error
/// messages, canonically ordered). A non-200 status keeps its code and drops the body; a 200
/// whose body does not canonicalize becomes a 502 with no body.
#[ic_cdk::query]
fn transform_sui_graphql(args: TransformArgs) -> HttpResponse {
    let status = args.response.status.clone();
    if status != 200u64 {
        return HttpResponse { status, headers: vec![], body: vec![] };
    }
    match graphql::canonicalize_response(&args.response.body, graphql::MAX_RESPONSE_BYTES as usize) {
        Ok(body) => HttpResponse { status, headers: vec![], body },
        Err(_) => HttpResponse { status: candid::Nat::from(502u64), headers: vec![], body: vec![] },
    }
}

// ---------------------------------------------------------------------------
// Ingress filtering
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IngressAccess {
    /// Authenticated controllers only.
    Controller,
}

/// Every method that may legitimately arrive as an ingress UPDATE call. Anything not listed is
/// refused before execution. A new update endpoint must be added here; the test
/// `every_update_endpoint_has_an_ingress_decision` fails until it is.
const INGRESS_METHODS: &[(&str, IngressAccess)] =
    &[("set_gate_binding", IngressAccess::Controller), ("derive_role_keys", IngressAccess::Controller)];

/// Largest accepted argument.
pub const MAX_INGRESS_BYTES: usize = 8 * 1024;

fn ingress_access(method: &str) -> Option<IngressAccess> {
    INGRESS_METHODS.iter().find(|(name, _)| *name == method).map(|(_, access)| *access)
}

/// Whether an ingress update call is worth executing. Pure, so the table is unit tested. NOT a
/// security boundary: it runs on one replica without consensus. Every method keeps its own
/// caller check, which is what protects state.
fn decide(method: &str, arg_bytes: usize, caller_is_anonymous: bool, is_controller: bool) -> bool {
    let Some(access) = ingress_access(method) else {
        return false;
    };
    if arg_bytes > MAX_INGRESS_BYTES || caller_is_anonymous {
        return false;
    }
    match access {
        IngressAccess::Controller => is_controller,
    }
}

#[ic_cdk::inspect_message]
fn inspect_message() {
    let method = ic_cdk::api::call::method_name();
    let arg_bytes = ic_cdk::api::call::arg_data_raw_size();
    let caller = ic_cdk::api::caller();
    let anonymous = caller == Principal::anonymous();
    let is_controller = !anonymous && ic_cdk::api::is_controller(&caller);
    if decide(&method, arg_bytes, anonymous, is_controller) {
        ic_cdk::api::call::accept_message();
    }
}

ic_cdk::export_candid!();

#[cfg(test)]
mod candid_tests {
    use candid_parser::utils::{service_equal, CandidSource};

    /// The checked-in `.did` must equal the interface generated from Rust.
    /// Regenerate with: `RUMI_SUI_BRIDGE_REGEN_DID=1 cargo test -p rumi_icusd_sui_bridge --lib candid_interface`.
    #[test]
    fn candid_interface_matches_checked_in_did() {
        let generated = super::__export_service();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("rumi_icusd_sui_bridge.did");
        if std::env::var("RUMI_SUI_BRIDGE_REGEN_DID").is_ok() {
            std::fs::write(&path, &generated).expect("write did");
        }
        let checked_in = std::fs::read_to_string(&path).expect("read the checked-in .did");
        service_equal(CandidSource::Text(&generated), CandidSource::Text(&checked_in))
            .unwrap_or_else(|e| panic!("rumi_icusd_sui_bridge.did is out of date; regenerate it: {e:?}"));
    }
}

#[cfg(test)]
mod endpoint_tests {
    use super::*;
    use crate::env::test_env;
    use crate::ids::{SuiNetwork, MAINNET_CHAIN_IDENTIFIER, TESTNET_CHAIN_IDENTIFIER};
    use crate::vectors::*;
    use std::collections::BTreeSet;

    const SRC_LIB: &str = include_str!("lib.rs");

    /// Function names of every endpoint in `lib.rs` carrying `attr`.
    fn endpoints(attr: &str) -> Vec<String> {
        let lines: Vec<&str> = SRC_LIB.lines().collect();
        let mut out = vec![];
        for (i, line) in lines.iter().enumerate() {
            if line.trim() == attr {
                let mut j = i + 1;
                while lines[j].trim().starts_with("#[") {
                    j += 1;
                }
                let sig = lines[j].trim();
                let rest = sig.strip_prefix("pub ").unwrap_or(sig);
                let rest = rest.strip_prefix("async ").unwrap_or(rest);
                let rest = rest.strip_prefix("fn ").unwrap_or_else(|| panic!("fn expected after {attr}: {sig}"));
                out.push(rest.split(['(', '<']).next().unwrap().to_string());
            }
        }
        out
    }

    #[test]
    fn every_update_endpoint_has_an_ingress_decision() {
        let updates: BTreeSet<String> = endpoints("#[ic_cdk::update]").into_iter().collect();
        assert_eq!(updates.len(), 2, "the update endpoints are exactly the listed ones: {updates:?}");
        let listed: BTreeSet<String> = INGRESS_METHODS.iter().map(|(n, _)| n.to_string()).collect();
        assert_eq!(updates, listed);
        assert_eq!(listed.len(), INGRESS_METHODS.len(), "no duplicates");
        // The queries expose no signing or broadcast capability.
        let queries: BTreeSet<String> = endpoints("#[ic_cdk::query]").into_iter().collect();
        let expected: BTreeSet<String> = ["bridge_version", "get_config", "get_roles", "transform_sui_graphql"].iter().map(|s| s.to_string()).collect();
        assert_eq!(queries, expected);
    }

    #[test]
    fn ingress_decisions() {
        assert!(decide("set_gate_binding", 100, false, true));
        assert!(decide("derive_role_keys", 0, false, true));
        assert!(!decide("set_gate_binding", 100, true, true), "anonymous");
        assert!(!decide("set_gate_binding", 100, false, false), "not a controller");
        assert!(!decide("set_gate_binding", MAX_INGRESS_BYTES + 1, false, true), "oversized");
        assert!(!decide("get_config", 0, false, true), "a query name is not an update");
        assert!(!decide("anything_else", 0, false, true));
    }

    fn controller() -> Principal {
        Principal::from_slice(&[1, 2, 3])
    }

    fn start(network: SuiNetwork, chain: &sui::Bytes32) {
        state::reset_for_tests();
        let (chain, mirror) = state::validate_init(&InitArgs { network, chain_identifier: hex::encode(chain), mirror: None }).unwrap();
        state::mutate_core(|c| {
            c.initialized = true;
            c.network = network;
            c.chain_identifier = chain;
            c.instance_salt = ids::instance_salt(env::canister_id().as_slice(), env::now_ns());
            c.mirror = mirror;
        });
    }

    fn binding_arg() -> GateBindingArg {
        GateBindingArg {
            package_id: sui::format_address(&PACKAGE),
            gate_id: sui::format_address(&GATE_ID),
            mint_cap_id: sui::format_address(&MINT_CAP_ID),
            admin_cap_id: sui::format_address(&ADMIN_CAP_ID),
        }
    }

    #[test]
    fn controller_methods_check_the_caller() {
        start(SuiNetwork::Testnet, &TESTNET_CHAIN_IDENTIFIER);
        test_env::set_controllers(vec![controller()]);

        test_env::set_caller(Principal::anonymous());
        assert_eq!(set_gate_binding(binding_arg()).unwrap_err(), SuiBridgeError::Anonymous);
        test_env::set_caller(Principal::from_slice(&[9, 9]));
        assert_eq!(set_gate_binding(binding_arg()).unwrap_err(), SuiBridgeError::Unauthorized);
        assert!(state::read_core(|c| c.binding.is_none()), "an unauthorized call stored nothing");

        test_env::set_caller(controller());
        let view = set_gate_binding(binding_arg()).unwrap();
        let b = view.binding.expect("binding");
        assert_eq!(b.gate_type, format!("0x{}::rumi_icusd_gate::Gate", hex::encode(PACKAGE)));
        assert_eq!(b.coin_type, format!("0x{}::rumi_icusd_gate::RUMI_ICUSD_GATE", hex::encode(PACKAGE)));
        assert_eq!(b.decimals, 8);
        assert!(!view.signing_enabled);
        assert_eq!(set_gate_binding(binding_arg()).unwrap().binding.unwrap(), b, "the same binding again is a no-op");
        let mut different = binding_arg();
        different.gate_id = sui::format_address(&[0x6b; 32]);
        assert!(matches!(set_gate_binding(different), Err(SuiBridgeError::Conflict(_))));
        let mut short = binding_arg();
        short.package_id = "0x2".into();
        assert!(matches!(set_gate_binding(short), Err(SuiBridgeError::InvalidArgument(_))));

        // derive_role_keys is a controller method too; off-chain it cannot reach the signer.
        test_env::set_caller(Principal::anonymous());
        assert_eq!(schnorr::block_on(derive_role_keys()).unwrap_err(), SuiBridgeError::Anonymous);
        test_env::set_caller(controller());
        assert!(matches!(schnorr::block_on(derive_role_keys()), Err(SuiBridgeError::SignerFailed(m)) if m.contains("unavailable off-chain")));
        assert!(state::read_core(|c| c.roles.is_empty()), "a failed derivation stored nothing");
        test_env::set_controllers(vec![]);
        test_env::set_caller(Principal::anonymous());
    }

    #[test]
    fn views_show_configuration_and_the_role_paths() {
        start(SuiNetwork::Mainnet, &MAINNET_CHAIN_IDENTIFIER);
        let c = get_config();
        assert!(c.initialized);
        assert_eq!(c.network, SuiNetwork::Mainnet);
        assert_eq!(c.short_chain_id, "35834a8a");
        assert_eq!(c.schnorr_key_name, "key_1");
        assert_eq!(c.chain_identifier, hex::encode(MAINNET_CHAIN_IDENTIFIER));
        assert_eq!(c.bridge_version, "rumi-sui-icusd-bridge-v1");
        assert!(!c.signing_enabled && c.binding.is_none() && !c.mirror.enabled);
        let roles = get_roles();
        assert_eq!(roles.len(), 3);
        assert!(roles.iter().all(|r| r.public_key.is_none() && r.address.is_none()));
        let t = roles.iter().find(|r| r.role == SuiRole::Treasury).unwrap();
        assert_eq!(t.derivation_path.len(), 4);
        assert_eq!(hex::decode(&t.derivation_path[2]).unwrap(), b"treasury");
        assert_eq!(hex::decode(&t.derivation_path[3]).unwrap(), MAINNET_CHAIN_IDENTIFIER.to_vec());

        state::set_role_keys(vec![(SuiRole::Treasury, h32(PUB_TREASURY)), (SuiRole::Gas, h32(PUB_GAS)), (SuiRole::UpgradeCustody, h32(PUB_CUSTODY))]).unwrap();
        let roles = get_roles();
        let addr = |r: SuiRole| roles.iter().find(|v| v.role == r).unwrap().address.clone().unwrap();
        assert_eq!(addr(SuiRole::Treasury), format!("0x{ADDR_TREASURY}"));
        assert_eq!(addr(SuiRole::Gas), format!("0x{ADDR_GAS}"));
        assert_eq!(addr(SuiRole::UpgradeCustody), format!("0x{ADDR_CUSTODY}"));
        state::reset_for_tests();
    }

    #[test]
    fn the_transform_keeps_status_and_drops_non_deterministic_parts() {
        use ic_cdk::api::management_canister::http_request::HttpResponse;
        let args = |status: u64, body: &[u8]| TransformArgs {
            response: HttpResponse { status: candid::Nat::from(status), headers: vec![], body: body.to_vec() },
            context: vec![],
        };
        let ok = transform_sui_graphql(args(200, br#"{"extensions":{"x":1},"data":{"b":1,"a":2}}"#));
        assert_eq!(ok.status, candid::Nat::from(200u64));
        assert_eq!(String::from_utf8(ok.body).unwrap(), r#"{"data":{"a":2,"b":1}}"#);
        assert!(ok.headers.is_empty());
        let rate = transform_sui_graphql(args(429, b"slow down, request 8c1e"));
        assert_eq!(rate.status, candid::Nat::from(429u64));
        assert!(rate.body.is_empty());
        let bad = transform_sui_graphql(args(200, b"<html>"));
        assert_eq!(bad.status, candid::Nat::from(502u64));
        assert!(bad.body.is_empty());
    }
}
