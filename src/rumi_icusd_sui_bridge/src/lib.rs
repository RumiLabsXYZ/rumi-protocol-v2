//! Rumi icUSD bridge, Sui instance (card BB-53; the foundation is BB-52). Source and local
//! capability only: nothing is deployed, signed, funded, broadcast or read from a network, and the
//! Sui profile stays `disabled`. 53a is the ICP-side saga and accounting (escrow pull, refund,
//! release, fee sweep); 53b adds the observed binding, the HTTPS-outcall transport (disabled by
//! default, exercised only against a simulated chain) and the mint lane; the return observer and
//! the monitors (53c) and the wallet surface (BB-54) do not exist yet.
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
//! * `schnorr`: the threshold-Ed25519 signer seam; `state`, `types`, `env`: the skeleton;
//! * 53b: `reads` (the head, checkpoint-with-gas, owned-object, coin-metadata, transaction and
//!   `executeTransaction` shapes), `mirror` (the transport trait, the real outcall transport and the
//!   per-provider quorum), `bind` (the observed deployment), `flows::mint` (the lane) and
//!   `flows::remint` (held releases back to their Sui sender).
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
//! * `signing_enabled` in the config view is a legacy, non-authoritative placeholder (always false); it
//!   enforces nothing and does not say whether the lane can sign. The mint lane signs and broadcasts
//!   only when the controllers have configured the gas coin, an admin has enabled a provider mirror and
//!   observed the deployment, and mints are not paused; `get_mint_lane` (`ready`, `blocked_by`) is the
//!   readiness seam.
//!
//! # Open items this crate carries (honest list)
//!
//! * `sui-signing-vectors-unverified`: the golden vectors are an independent Python reference
//!   written from the pinned sources, plus the pinned sources themselves. They are NOT checked
//!   against the official Sui Rust SDK or CLI binary (running either was out of scope and the SDK
//!   is not a dependency). The IC `sign_with_schnorr` mapping for Sui is unrun.
//! * `sui-graphql-schema-unmeasured`: fixtures are schema-derived, not captured from a provider;
//!   `UInt53` number versus string and the `repr` address form are tolerated both ways.
//! * `sui-rpc-provider-contract`: no provider is contracted; the mirror is disabled by default. The
//!   HTTPS-outcall transport exists (53b) but has only ever answered a simulated chain: cycles per
//!   call, keyless availability, retention and real response shapes are unmeasured.
//! * `sui-epoch-expiry-nonexecution-proof`: the builder binds `Epoch(e)` to bound signing; it
//!   never infers expiry or non-execution from an epoch passing or a missing digest.
//! * Ed25519 signatures are verified against the role key before the mint lane stores one
//!   (`ed25519-dalek`, a normal dependency since 53b); the transaction itself is only ever checked
//!   against a simulated chain.
//! * BLAKE2b-256 is the RustCrypto `blake2` crate, the one dependency added to `Cargo.lock` for this card.

pub mod accounting;
pub mod admin;
pub mod bcs;
pub mod bind;
pub mod env;
pub mod flows;
pub mod gate;
pub mod graphql;
pub mod hash;
pub mod icp;
pub mod icrc21;
pub mod ids;
pub mod ledger;
pub mod mirror;
pub mod reads;
pub mod runtime;
pub mod schnorr;
pub mod state;
pub mod sui;
pub mod timer;
pub mod tx;
pub mod types;
pub mod views;
#[cfg(test)]
#[allow(dead_code)]
mod vectors;
#[cfg(test)]
mod graphql_identity_tests;
#[cfg(test)]
#[allow(dead_code)]
mod testkit;
#[cfg(test)]
mod saga_tests;
#[cfg(test)]
#[allow(dead_code)]
mod chain_sim;
#[cfg(test)]
mod lane_tests;

use candid::Principal;
use ids::SuiRole;
use types::*;

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
    let r = state::read_runtime(|r| r.clone());
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
        icusd_ledger: r.config.icusd_ledger,
        admins: r.config.admins,
        fee_recipient: r.config.fee_recipient,
        fees: r.config.fees,
        limits: r.config.limits,
        pauses: r.config.pauses,
        halted: r.halted.map(|h| h.reason),
        escrow_subaccount: hex::encode(ids::escrow_subaccount()),
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
    let runtime = state::validate_runtime_init(&args).unwrap_or_else(|e| ic_cdk::trap(&format!("invalid init args: {e}")));
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
    state::mutate_runtime(|c| {
        c.initialized = true;
        c.config = runtime;
    });
    state::append_audit(None, state::AuditCtx::note("initialized"));
    timer::schedule_soon();
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
            if let Some(admins) = u.admins {
                if admins.is_empty() || admins.iter().any(|p| *p == Principal::anonymous()) {
                    ic_cdk::trap("upgrade admins must be non-empty and non-anonymous");
                }
                state::mutate_runtime(|c| c.config.admins = admins.clone());
                state::append_audit(None, state::AuditCtx::note(format!("admins replaced at upgrade: {admins:?}")));
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
    // An image written by the BB-52 foundation has no runtime record: the runtime starts from its
    // defaults and must then be initialized from the identity that is already stored. A refused
    // runtime config traps here, so the previous module stays.
    if !state::read_runtime(|c| c.initialized) {
        ic_cdk::trap("upgrade refused: the saga runtime is not initialized; reinstall with the saga install arguments");
    }
    if let Err(e) = state::read_runtime(|c| state::validate_stored_runtime(&c.config)) {
        ic_cdk::trap(&format!("upgrade refused, the previous module stays installed: {e}"));
    }
    state::migrate_core();
    if state::migrate_runtime() {
        state::append_audit(None, state::AuditCtx::note("runtime schema migrated"));
    }
    state::append_audit(None, state::AuditCtx::note("upgraded"));
    timer::schedule_soon();
}

// ---------------------------------------------------------------------------
// User methods
// ---------------------------------------------------------------------------

#[ic_cdk::update]
async fn bridge_to_sui(args: BridgeToSuiArgs) -> BridgeResult<DepositView> {
    flows::deposit::bridge_to_sui(&icp::CanisterLedger::from_config(), args).await
}

#[ic_cdk::update]
async fn request_deposit_refund(id: u64) -> BridgeResult<DepositView> {
    flows::deposit::request_refund(&icp::CanisterLedger::from_config(), id).await
}

/// "Look at this Sui transaction sooner" (base58 typed transaction digest). Stores a bounded hint;
/// never settles anything.
#[ic_cdk::update]
fn notify_sui_return(digest: String) -> BridgeResult<()> {
    flows::release::record_hint(digest)
}

#[ic_cdk::query]
fn get_deposit(id: u64) -> Option<DepositView> {
    state::get_deposit(id).map(|d| views::deposit_view(&d))
}

#[ic_cdk::query]
fn list_my_deposits(args: ListDepositsArgs) -> Vec<DepositView> {
    let caller = env::caller();
    state::list_owner_deposits(&caller, args.start_after, args.limit).iter().map(views::deposit_view).collect()
}

/// Public: deposits are on-chain facts. Lets wallets whose signer upgrades queries to signed calls
/// read history without a consent prompt.
#[ic_cdk::query]
fn list_deposits_for_owner(args: ListDepositsForOwnerArgs) -> Vec<DepositView> {
    state::list_owner_deposits(&args.owner, args.start_after, args.limit).iter().map(views::deposit_view).collect()
}

/// Public lookup of a deposit by its owner and client request id (recovery after a lost
/// `bridge_to_sui` reply).
#[ic_cdk::query]
fn get_deposit_by_request(owner: Principal, client_request_id: serde_bytes::ByteBuf) -> Option<DepositView> {
    let key = state::request_key(&owner, &client_request_id).ok()?;
    state::lookup_request(&key).and_then(state::get_deposit).map(|d| views::deposit_view(&d))
}

/// A release by the Gate's `transfer_id` (64 hex).
#[ic_cdk::query]
fn get_release(return_id: String) -> Option<ReleaseView> {
    let id: [u8; 32] = hex::decode(return_id.trim()).ok()?.try_into().ok()?;
    state::get_release(&id).map(|r| views::release_view(&r))
}

/// Releases paid to an ICP principal, newest first.
#[ic_cdk::query]
fn list_releases_for_owner(args: ListReleasesArgs) -> Vec<ReleaseView> {
    state::list_owner_releases(&args.owner, args.start_after_seq, args.limit).iter().map(views::release_view).collect()
}

/// Releases created by returns from a Sui address (`0x` and 64 hex), including held claims.
#[ic_cdk::query]
fn list_releases_for_sui_sender(args: ListReleasesForSenderArgs) -> Vec<ReleaseView> {
    let Ok(sender) = sui::parse_address_strict(&args.sui_sender) else { return vec![] };
    state::list_sender_releases(&sender, args.start_after_seq, args.limit).iter().map(views::release_view).collect()
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

#[ic_cdk::query]
fn get_buckets() -> BucketsView {
    views::buckets_view()
}

/// What the providers last agreed the configured deployment is (`observe_deployment`).
#[ic_cdk::query]
fn get_observed_deployment() -> Option<ObservedDeploymentView> {
    state::read_runtime(|c| c.observed.as_ref().map(views::observed_view))
}

/// Whether the mint lane would run now, and what it is waiting for.
#[ic_cdk::query]
fn get_mint_lane() -> MintLaneView {
    views::mint_lane_view()
}

#[ic_cdk::query]
fn get_mint_op(id: u64) -> Option<MintOpView> {
    state::get_mint_op(id).map(|o| views::mint_op_view(&o))
}

#[ic_cdk::query]
fn list_mint_ops(args: ListDepositsArgs) -> Vec<MintOpView> {
    state::list_mint_ops(args.start_after, args.limit).iter().map(views::mint_op_view).collect()
}

#[ic_cdk::query]
fn get_audit_log(start: u64, limit: u32) -> Vec<AuditEntry> {
    state::audit_page(start, limit).iter().map(views::audit_entry).collect()
}

// ---------------------------------------------------------------------------
// Admin methods
// ---------------------------------------------------------------------------

#[ic_cdk::update]
fn admin_set_pauses(p: Pauses) -> BridgeResult<()> {
    admin::set_pauses(p)?;
    timer::schedule_soon();
    Ok(())
}

#[ic_cdk::update]
fn admin_set_limits(l: BridgeLimits) -> BridgeResult<()> {
    admin::set_limits(l)
}

#[ic_cdk::update]
fn admin_set_fees(f: BridgeFees) -> BridgeResult<()> {
    admin::set_fees(f)
}

#[ic_cdk::update]
async fn admin_sweep_fees(amount_e8s: u64) -> BridgeResult<u64> {
    admin::require_admin()?;
    flows::deposit::sweep_fees(&icp::CanisterLedger::from_config(), amount_e8s).await
}

/// Put a held deposit (no mint operation, none abandoned) back in the mint queue.
#[ic_cdk::update]
fn admin_retry_deposit(id: u64) -> BridgeResult<DepositView> {
    admin::require_admin()?;
    flows::deposit::retry_held(id)
}

/// Clear the bridge halt after resolving its cause; the note says how.
#[ic_cdk::update]
fn admin_clear_halt(note: String) -> BridgeResult<()> {
    admin::clear_halt(&note)?;
    timer::schedule_soon();
    Ok(())
}

/// Read the configured deployment through the provider quorum and store what they agree on. Needs the
/// gate binding, the role keys and an enabled mirror; stores nothing unless the observation is agreed
/// and acceptable.
#[ic_cdk::update]
async fn observe_deployment() -> SuiResult<ObservedDeploymentView> {
    admin::require_admin()?;
    match bind::observe_deployment(&mirror::CanisterTransport).await {
        Ok(o) => Ok(views::observed_view(&o)),
        Err(bind::BindError::NotReady(m)) => Err(SuiBridgeError::NotConfigured(m)),
        Err(bind::BindError::Unavailable(m)) => Err(SuiBridgeError::Unavailable(m)),
        Err(bind::BindError::Mismatch(m)) => Err(SuiBridgeError::DeploymentMismatch(m)),
    }
}

#[ic_cdk::update]
fn admin_set_lane_settings(a: LaneSettingsArg) -> BridgeResult<()> {
    admin::set_lane_settings(a)
}

/// Abandon a mint operation whose current attempt is proven never signed. Refused for any attempt that
/// may be a complete signed transaction.
#[ic_cdk::update]
fn admin_abandon_mint_op(op_id: u64, note: String) -> BridgeResult<()> {
    admin::abandon_mint_op(op_id, &note)?;
    timer::schedule_soon();
    Ok(())
}

/// Clear the mint lane's halt after resolving its cause; the note says how.
#[ic_cdk::update]
fn admin_clear_mint_lane_halt(note: String) -> BridgeResult<()> {
    admin::clear_mint_lane_halt(&note)?;
    timer::schedule_soon();
    Ok(())
}

#[ic_cdk::update]
async fn admin_run_tick() -> BridgeResult<()> {
    admin::require_admin()?;
    timer::schedule_soon();
    if timer::run_now().await {
        Ok(())
    } else {
        Err(BridgeError::InvalidState("a pass is already running, or the instance is not fully configured".into()))
    }
}

// ---------------------------------------------------------------------------
// Controller methods
// ---------------------------------------------------------------------------

#[ic_cdk::update]
fn set_admins(admins: Vec<Principal>) -> BridgeResult<()> {
    admin::set_admins(admins)
}

/// The one SUI coin object (owned by the gas role address) that pays for mints.
#[ic_cdk::update]
fn set_gas_coin(id: String) -> BridgeResult<()> {
    admin::set_gas_coin(&id)
}

/// Record the exact identities of the deployed gate. Set once; the same ids again are a no-op
/// and different ids are a conflict.
#[ic_cdk::update]
fn set_gate_binding(arg: GateBindingArg) -> SuiResult<ConfigView> {
    admin::require_controller()?;
    let binding = state::validate_binding(&arg).map_err(SuiBridgeError::InvalidArgument)?;
    state::set_binding(binding)?;
    Ok(state::read_core(config_view))
}

/// Derive the three role public keys from the threshold key (free calls, no signing) and store
/// them once. Any rejected or malformed reply, a duplicate key or a role set that is not three
/// distinct keys is refused and nothing is stored.
#[ic_cdk::update]
async fn derive_role_keys() -> SuiResult<Vec<RoleView>> {
    admin::require_controller()?;
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
// Wallet standards
// ---------------------------------------------------------------------------

#[ic_cdk::update]
fn icrc21_canister_call_consent_message(request: icrc21::ConsentMessageRequest) -> Result<icrc21::ConsentInfo, icrc21::Icrc21Error> {
    icrc21::consent_message(request)
}

#[ic_cdk::query]
fn icrc28_trusted_origins() -> icrc21::Icrc28TrustedOriginsResponse {
    icrc21::trusted_origins()
}

#[ic_cdk::query]
fn icrc10_supported_standards() -> Vec<icrc21::StandardRecord> {
    icrc21::supported_standards()
}

// ---------------------------------------------------------------------------
// HTTP outcall transform (the transport itself is 53b)
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

/// The head query's transform: the checkpoint number rounded down to a multiple of
/// `reads::HEAD_ROUNDING`, everything else about the head dropped. A non-200 status keeps its code.
#[ic_cdk::query]
fn transform_sui_graphql_head(args: TransformArgs) -> HttpResponse {
    let status = args.response.status.clone();
    if status != 200u64 {
        return HttpResponse { status, headers: vec![], body: vec![] };
    }
    match reads::canonicalize_head(&args.response.body, graphql::MAX_RESPONSE_BYTES as usize) {
        Ok(body) => HttpResponse { status, headers: vec![], body },
        Err(_) => HttpResponse { status: candid::Nat::from(502u64), headers: vec![], body: vec![] },
    }
}

/// The `executeTransaction` transform: whether the provider answered 200 is all a replica
/// contributes (a duplicate submission of the same signed bytes may answer differently), and the body
/// is replaced by a constant. The canister confirms by transaction lookup, never from this reply.
#[ic_cdk::query]
fn transform_sui_send(args: TransformArgs) -> HttpResponse {
    let status = args.response.status.clone();
    if status != 200u64 {
        return HttpResponse { status, headers: vec![], body: vec![] };
    }
    HttpResponse { status, headers: vec![], body: reads::SEND_ACCEPTED_BODY.to_vec() }
}

// ---------------------------------------------------------------------------
// Ingress filtering
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IngressAccess {
    /// Anyone, including anonymous callers (wallet standards only).
    Anyone,
    /// Any authenticated principal.
    Authenticated,
    /// An admin or a controller.
    Admin,
    /// A controller.
    Controller,
}

/// Every method that may legitimately arrive as an ingress UPDATE call, with the callers it is worth
/// running for. Anything not listed is refused before execution. A new update endpoint must be added
/// here; `every_update_endpoint_has_an_ingress_decision` fails until it is.
const INGRESS_METHODS: &[(&str, IngressAccess)] = &[
    ("icrc21_canister_call_consent_message", IngressAccess::Anyone),
    // User methods.
    ("bridge_to_sui", IngressAccess::Authenticated),
    ("request_deposit_refund", IngressAccess::Authenticated),
    ("notify_sui_return", IngressAccess::Authenticated),
    // Admin methods.
    ("admin_set_pauses", IngressAccess::Admin),
    ("admin_set_limits", IngressAccess::Admin),
    ("admin_set_fees", IngressAccess::Admin),
    ("admin_sweep_fees", IngressAccess::Admin),
    ("admin_retry_deposit", IngressAccess::Admin),
    ("admin_clear_halt", IngressAccess::Admin),
    ("admin_run_tick", IngressAccess::Admin),
    ("observe_deployment", IngressAccess::Admin),
    ("admin_set_lane_settings", IngressAccess::Admin),
    ("admin_abandon_mint_op", IngressAccess::Admin),
    ("admin_clear_mint_lane_halt", IngressAccess::Admin),
    // Controller methods.
    ("set_admins", IngressAccess::Controller),
    ("set_gate_binding", IngressAccess::Controller),
    ("derive_role_keys", IngressAccess::Controller),
    ("set_gas_coin", IngressAccess::Controller),
];

/// Largest accepted argument for open methods (wallet standards and user methods). The biggest
/// legitimate one, a `bridge_to_sui` consent request, is well under 1 KiB; without a cap any caller
/// could make the canister pay the ingress fee for about 2 MiB.
pub const MAX_OPEN_INGRESS_BYTES: usize = 4 * 1024;
/// Largest accepted argument for admin and controller methods.
pub const MAX_ADMIN_INGRESS_BYTES: usize = 16 * 1024;

fn ingress_access(method: &str) -> Option<IngressAccess> {
    INGRESS_METHODS.iter().find(|(name, _)| *name == method).map(|(_, access)| *access)
}

/// Whether an ingress update call is worth executing. Pure, so the table above is unit tested. NOT a
/// security boundary: it runs on one replica without consensus. Every method keeps its own caller
/// checks, which are what protect state and funds.
fn decide(method: &str, arg_bytes: usize, caller_is_anonymous: bool, is_admin: bool, is_controller: bool) -> bool {
    let Some(access) = ingress_access(method) else {
        // Not a known update method (a query name, a typo, a probe): nothing to run.
        return false;
    };
    let max_bytes = match access {
        IngressAccess::Anyone | IngressAccess::Authenticated => MAX_OPEN_INGRESS_BYTES,
        IngressAccess::Admin | IngressAccess::Controller => MAX_ADMIN_INGRESS_BYTES,
    };
    if arg_bytes > max_bytes {
        return false;
    }
    match access {
        IngressAccess::Anyone => true,
        _ if caller_is_anonymous => false,
        IngressAccess::Authenticated => true,
        IngressAccess::Admin => is_admin || is_controller,
        IngressAccess::Controller => is_controller,
    }
}

#[ic_cdk::inspect_message]
fn inspect_message() {
    let method = ic_cdk::api::call::method_name();
    let arg_bytes = ic_cdk::api::call::arg_data_raw_size();
    let caller = ic_cdk::api::caller();
    let anonymous = caller == Principal::anonymous();
    // Only the role checks the method's table entry might need; skipped for anonymous.
    let (is_admin, is_controller) =
        if anonymous { (false, false) } else { (state::read_runtime(|c| c.is_admin(&caller)), ic_cdk::api::is_controller(&caller)) };
    if decide(&method, arg_bytes, anonymous, is_admin, is_controller) {
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
        assert_eq!(updates.len(), 19, "the update endpoints are exactly the listed ones: {updates:?}");
        let listed: BTreeSet<String> = INGRESS_METHODS.iter().map(|(n, _)| n.to_string()).collect();
        assert_eq!(updates, listed, "every update endpoint is listed and nothing else is");
        assert_eq!(listed.len(), INGRESS_METHODS.len(), "no duplicates");
        // The queries expose no signing or broadcast capability and no mutation.
        let queries: BTreeSet<String> = endpoints("#[ic_cdk::query]").into_iter().collect();
        let expected: BTreeSet<String> = [
            "bridge_version",
            "get_config",
            "get_roles",
            "get_buckets",
            "get_audit_log",
            "get_observed_deployment",
            "get_mint_lane",
            "get_mint_op",
            "list_mint_ops",
            "get_deposit",
            "list_my_deposits",
            "list_deposits_for_owner",
            "get_deposit_by_request",
            "get_release",
            "list_releases_for_owner",
            "list_releases_for_sui_sender",
            "icrc28_trusted_origins",
            "icrc10_supported_standards",
            "transform_sui_graphql",
            "transform_sui_graphql_head",
            "transform_sui_send",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(queries, expected);
    }

    #[test]
    fn ingress_decisions() {
        // (method, anonymous, admin, controller) -> accepted
        assert!(decide("icrc21_canister_call_consent_message", 100, true, false, false), "wallet standards stay open");
        assert!(decide("bridge_to_sui", 100, false, false, false));
        assert!(!decide("bridge_to_sui", 100, true, false, false), "anonymous");
        assert!(!decide("bridge_to_sui", MAX_OPEN_INGRESS_BYTES + 1, false, false, false), "oversized open call");
        assert!(decide("admin_set_fees", 100, false, true, false));
        assert!(decide("admin_set_fees", 100, false, false, true), "a controller may run admin methods");
        assert!(!decide("admin_set_fees", 100, false, false, false), "not an admin");
        assert!(!decide("admin_set_fees", MAX_ADMIN_INGRESS_BYTES + 1, false, true, false));
        assert!(decide("set_gate_binding", 100, false, false, true));
        assert!(!decide("set_gate_binding", 100, false, true, false), "an admin is not a controller");
        assert!(!decide("derive_role_keys", 100, true, false, true), "anonymous");
        assert!(!decide("get_config", 0, false, true, true), "a query name is not an update");
        assert!(!decide("bridge_to_solana", 0, false, true, true), "another chain's method");
    }

    fn controller() -> Principal {
        Principal::from_slice(&[1, 2, 3])
    }

    fn start(network: SuiNetwork, chain: &sui::Bytes32) {
        state::reset_for_tests();
        let args = InitArgs {
            network,
            chain_identifier: hex::encode(chain),
            mirror: None,
            icusd_ledger: Principal::from_slice(&[0x1c; 10]),
            admins: vec![Principal::from_slice(&[3; 10])],
            fee_recipient: None,
            fees: None,
            limits: None,
        };
        let (chain, mirror) = state::validate_init(&args).unwrap();
        let runtime = state::validate_runtime_init(&args).unwrap();
        state::mutate_core(|c| {
            c.initialized = true;
            c.network = network;
            c.chain_identifier = chain;
            c.instance_salt = ids::instance_salt(env::canister_id().as_slice(), env::now_ns());
            c.mirror = mirror;
        });
        state::mutate_runtime(|c| {
            c.initialized = true;
            c.config = runtime;
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
        assert!(c.pauses.deposits, "deposits start paused");
        assert_eq!(c.icusd_ledger, Principal::from_slice(&[0x1c; 10]));
        assert_eq!(c.escrow_subaccount, hex::encode(ids::escrow_subaccount()));
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
