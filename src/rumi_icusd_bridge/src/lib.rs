//! Rumi icUSD bridge canister: canonical ICP icUSD escrow <-> Base icUSD.
//!
//! Design: docs/superpowers/specs/2026-10-24-base-icusd-bridge-design.md.
//! This canister is not the icUSD minting account. It can move only icUSD it holds in
//! its escrow subaccount, and it is the sole minter of the Base `BridgedIcUSD` token
//! through its threshold-ECDSA minter address.

pub mod accounting;
pub mod admin;
pub mod env;
pub mod evm;
pub mod flows;
pub mod icrc21;
pub mod ids;
pub mod ledger;
pub mod observer;
pub mod state;
pub mod timer;
pub mod types;
pub mod views;

use candid::Principal;
use sha2::{Digest as _, Sha256};
use state::AuditCtx;
use types::*;

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

#[ic_cdk::init]
fn init(arg: BridgeArg) {
    let args = match arg {
        BridgeArg::Init(a) => a,
        BridgeArg::Upgrade(_) => ic_cdk::trap("install requires BridgeArg::Init"),
    };
    let config = state::validate_init(&args).unwrap_or_else(|e| ic_cdk::trap(&format!("invalid init args: {e}")));
    let now = env::now_ns();
    let salt: [u8; 32] = {
        let mut h = Sha256::new();
        h.update(b"rumi-icusd-base-bridge:instance-salt:v1");
        h.update(env::canister_id().as_slice());
        h.update(now.to_be_bytes());
        h.finalize().into()
    };
    state::mutate_core(|c| {
        if c.initialized {
            ic_cdk::trap("bridge state already initialized");
        }
        c.initialized = true;
        c.config = config;
        c.instance_salt = salt;
        c.windows.window_start_ns = now;
    });
    state::append_audit(None, AuditCtx::note("initialized"));
    timer::schedule_soon();
}

#[ic_cdk::post_upgrade]
fn post_upgrade(arg: Option<BridgeArg>) {
    match arg {
        None => {}
        Some(BridgeArg::Upgrade(u)) => {
            if let Some(admins) = u.admins {
                if admins.is_empty() || admins.iter().any(|p| *p == Principal::anonymous()) {
                    ic_cdk::trap("upgrade admins must be non-empty and non-anonymous");
                }
                state::mutate_core(|c| c.config.admins = admins.clone());
                state::append_audit(None, AuditCtx::note(format!("admins replaced at upgrade: {admins:?}")));
            }
        }
        Some(BridgeArg::Init(_)) => ic_cdk::trap("upgrade requires BridgeArg::Upgrade or no argument"),
    }
    if !state::read_core(|c| c.initialized) {
        ic_cdk::trap("upgrade of an uninitialized bridge");
    }
    state::append_audit(None, AuditCtx::note("upgraded"));
    timer::schedule_soon();
}

// ---------------------------------------------------------------------------
// User methods
// ---------------------------------------------------------------------------

#[ic_cdk::update]
async fn bridge_to_base(args: BridgeToBaseArgs) -> BridgeResult<DepositView> {
    flows::deposit::bridge_to_base(args).await
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

/// Public: deposits are on-chain facts. Lets wallets whose signer upgrades queries to
/// signed calls (Oisy) read history without a consent prompt.
#[ic_cdk::query]
fn list_deposits_for_owner(args: ListDepositsForOwnerArgs) -> Vec<DepositView> {
    state::list_owner_deposits(&args.owner, args.start_after, args.limit).iter().map(views::deposit_view).collect()
}

/// Public lookup of a deposit by its owner and client request id (recovery after a
/// lost `bridge_to_base` reply).
#[ic_cdk::query]
fn get_deposit_by_request(owner: Principal, client_request_id: serde_bytes::ByteBuf) -> Option<DepositView> {
    let key = state::request_key(&owner, &client_request_id).ok()?;
    state::lookup_request(&key).and_then(state::get_deposit).map(|d| views::deposit_view(&d))
}

/// Public: releases created by a given Base sender, including held claims whose ICP
/// destination was invalid.
#[ic_cdk::query]
fn list_releases_for_base_sender(args: ListReleasesForSenderArgs) -> Vec<ReleaseView> {
    let Ok(sender) = ids::parse_address(&args.base_sender) else { return vec![] };
    state::list_sender_releases(&sender, args.start_after_sequence, args.limit).iter().map(views::release_view).collect()
}

#[ic_cdk::query]
fn get_release(transfer_id: String) -> Option<ReleaseView> {
    let tid = ids::parse_bytes32(&transfer_id).ok()?;
    state::get_release(&tid).map(|r| views::release_view(&r))
}

#[ic_cdk::query]
fn list_releases_for_owner(args: ListReleasesArgs) -> Vec<ReleaseView> {
    state::list_owner_releases(&args.owner, args.start_after_sequence, args.limit).iter().map(views::release_view).collect()
}

#[ic_cdk::update]
fn notify_base_bridge_out(tx_hash: String) -> BridgeResult<()> {
    flows::release::record_hint(tx_hash)
}

#[ic_cdk::update]
async fn request_deposit_refund(id: u64) -> BridgeResult<DepositView> {
    flows::deposit::request_refund(id).await
}

#[ic_cdk::query]
fn get_bridge_info() -> BridgeInfo {
    views::bridge_info()
}

#[ic_cdk::query]
fn get_supply_audit() -> SupplyAudit {
    views::supply_audit()
}

#[ic_cdk::update]
async fn get_base_addresses() -> BridgeResult<BaseAddresses> {
    evm::signer::ensure_lane_addresses().await
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
    timer::ensure_scheduled();
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
fn admin_set_gas(g: GasConfig) -> BridgeResult<()> {
    admin::set_gas(g)
}

#[ic_cdk::update]
fn admin_set_observer_config(o: ObserverConfig) -> BridgeResult<()> {
    admin::set_observer_config(o)
}

#[ic_cdk::update]
async fn admin_set_rpc_services(services: Vec<RpcServiceConfig>, min_quorum: u8) -> BridgeResult<()> {
    admin::set_rpc_services(services, min_quorum).await
}

#[ic_cdk::update]
async fn admin_bind_contract(address: String) -> BridgeResult<ContractBindingView> {
    admin::require_admin()?;
    observer::bind_contract(address).await
}

#[ic_cdk::update]
async fn admin_sweep_fees(amount_e8s: u64) -> BridgeResult<u64> {
    admin::require_admin()?;
    flows::deposit::sweep_fees(amount_e8s).await
}

#[ic_cdk::update]
fn admin_submit_contract_action(action: ContractAdminAction) -> BridgeResult<u64> {
    admin::require_admin()?;
    evm::lane::enqueue_admin_action(action)
}

#[ic_cdk::update]
fn admin_cancel_stuck_tx(tx_id: u64) -> BridgeResult<()> {
    admin::require_admin()?;
    evm::lane::request_cancel(tx_id)
}

#[ic_cdk::update]
async fn admin_clear_lane_halt(lane: LaneKind) -> BridgeResult<()> {
    admin::require_admin()?;
    timer::ensure_scheduled();
    evm::lane::clear_lane_halt(lane).await
}

#[ic_cdk::update]
fn admin_retry_deposit(id: u64) -> BridgeResult<DepositView> {
    admin::require_admin()?;
    flows::deposit::retry_held(id)
}

#[ic_cdk::update]
async fn admin_reconcile() -> BridgeResult<SupplyAudit> {
    admin::require_admin()?;
    timer::ensure_scheduled();
    timer::reconcile_supply().await?;
    Ok(views::supply_audit())
}

#[ic_cdk::update]
async fn admin_clear_halt() -> BridgeResult<SupplyAudit> {
    admin::require_admin()?;
    timer::ensure_scheduled();
    timer::clear_halt_after_checks().await
}

#[ic_cdk::update]
async fn admin_run_tick() -> BridgeResult<()> {
    admin::require_admin()?;
    timer::ensure_scheduled();
    if timer::run_tick().await {
        Ok(())
    } else {
        Err(BridgeError::InvalidState("a tick is already running".into()))
    }
}

#[ic_cdk::update]
fn set_admins(admins: Vec<Principal>) -> BridgeResult<()> {
    admin::set_admins(admins)
}

// ---------------------------------------------------------------------------
// Wallet standards
// ---------------------------------------------------------------------------

#[ic_cdk::update]
fn icrc21_canister_call_consent_message(
    request: icrc21::ConsentMessageRequest,
) -> Result<icrc21::ConsentInfo, icrc21::Icrc21Error> {
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

#[ic_cdk::query]
fn bridge_version() -> String {
    ids::CONTRACT_VERSION.to_string()
}

// ---------------------------------------------------------------------------
// Ingress pre-filter (cycle-drain protection)
// ---------------------------------------------------------------------------

/// Who an ingress call to a method is worth executing for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IngressAccess {
    /// Wallet standards: anyone, anonymous included (a wallet asks before the user signs).
    Anyone,
    /// Any authenticated principal. The method decides the rest.
    Authenticated,
    /// A configured admin. Controllers also pass this filter (the method itself still
    /// requires an admin).
    Admin,
    /// A canister controller.
    Controller,
}

/// Every method that may legitimately arrive as an ingress UPDATE call, with the callers
/// it is worth running for. Anything not listed is refused before execution. A new update
/// endpoint must be added here; `every_update_endpoint_has_an_ingress_decision` fails
/// until it is.
///
/// The two `Anyone` entries that are queries here (ICRC-28, ICRC-10) are listed so the
/// standards stay open to anonymous callers when a client sends them as update calls.
/// Every other query (`get_*`, `list_*`, `bridge_version`) must be called as a query:
/// a query name sent as an update call is refused, so replicated reads cost nothing.
const INGRESS_METHODS: &[(&str, IngressAccess)] = &[
    // Wallet standards.
    ("icrc21_canister_call_consent_message", IngressAccess::Anyone),
    ("icrc28_trusted_origins", IngressAccess::Anyone),
    ("icrc10_supported_standards", IngressAccess::Anyone),
    // User methods.
    ("bridge_to_base", IngressAccess::Authenticated),
    ("notify_base_bridge_out", IngressAccess::Authenticated),
    ("request_deposit_refund", IngressAccess::Authenticated),
    ("get_base_addresses", IngressAccess::Authenticated),
    // Admin methods.
    ("admin_set_pauses", IngressAccess::Admin),
    ("admin_set_limits", IngressAccess::Admin),
    ("admin_set_fees", IngressAccess::Admin),
    ("admin_set_gas", IngressAccess::Admin),
    ("admin_set_observer_config", IngressAccess::Admin),
    ("admin_bind_contract", IngressAccess::Admin),
    ("admin_sweep_fees", IngressAccess::Admin),
    ("admin_submit_contract_action", IngressAccess::Admin),
    ("admin_cancel_stuck_tx", IngressAccess::Admin),
    ("admin_clear_lane_halt", IngressAccess::Admin),
    ("admin_retry_deposit", IngressAccess::Admin),
    ("admin_reconcile", IngressAccess::Admin),
    ("admin_clear_halt", IngressAccess::Admin),
    ("admin_run_tick", IngressAccess::Admin),
    // Controller methods.
    ("admin_set_rpc_services", IngressAccess::Controller),
    ("set_admins", IngressAccess::Controller),
];

/// Largest accepted argument for open methods (wallet standards and user methods). The
/// biggest legitimate one, a `bridge_to_base` consent request, is well under 1 KiB.
/// Without a cap any caller could make the canister pay the ingress fee for about 2 MiB.
pub const MAX_OPEN_INGRESS_BYTES: usize = 4 * 1024;
/// Largest accepted argument for admin and controller methods (provider lists, admin lists).
pub const MAX_ADMIN_INGRESS_BYTES: usize = 16 * 1024;

fn ingress_access(method: &str) -> Option<IngressAccess> {
    INGRESS_METHODS.iter().find(|(name, _)| *name == method).map(|(_, access)| *access)
}

/// Whether an ingress update call is worth executing. Pure, so the table above is unit
/// tested. NOT a security boundary: it runs on one replica without consensus. Every
/// method keeps its own caller checks, which are what protect state and funds.
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

/// Runs on a single replica before consensus and costs nothing if it refuses, so
/// anonymous callers and callers that cannot pass the method's own check do not make the
/// canister pay for Candid decoding and execution.
#[ic_cdk::inspect_message]
fn inspect_message() {
    let method = ic_cdk::api::call::method_name();
    let arg_bytes = ic_cdk::api::call::arg_data_raw_size();
    let caller = ic_cdk::api::caller();
    let anonymous = caller == Principal::anonymous();
    // Only the role checks the method's table entry might need; skipped for anonymous.
    let (is_admin, is_controller) = if anonymous {
        (false, false)
    } else {
        (state::read_core(|c| c.is_admin(&caller)), ic_cdk::api::is_controller(&caller))
    };
    if decide(&method, arg_bytes, anonymous, is_admin, is_controller) {
        ic_cdk::api::call::accept_message();
    }
}

ic_cdk::export_candid!();

#[cfg(test)]
mod candid_tests {
    use candid_parser::utils::{service_equal, CandidSource};

    /// The checked-in `.did` must equal the interface generated from Rust.
    /// Regenerate with: `RUMI_BRIDGE_REGEN_DID=1 cargo test -p rumi_icusd_bridge --lib candid_interface`.
    #[test]
    fn candid_interface_matches_checked_in_did() {
        let generated = super::__export_service();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("rumi_icusd_bridge.did");
        if std::env::var("RUMI_BRIDGE_REGEN_DID").is_ok() {
            std::fs::write(&path, &generated).expect("write did");
        }
        let checked_in = std::fs::read_to_string(&path).expect("read rumi_icusd_bridge.did");
        service_equal(CandidSource::Text(&generated), CandidSource::Text(&checked_in))
            .expect("rumi_icusd_bridge.did is out of date; regenerate it");
    }
}

#[cfg(test)]
mod ingress_tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    const SRC_LIB: &str = include_str!("lib.rs");
    const SRC_ADMIN: &str = include_str!("admin.rs");

    /// `(attribute, fn name, body)` of every endpoint in `lib.rs` carrying `attr`.
    fn endpoints(attr: &str) -> Vec<(String, String)> {
        let lines: Vec<&str> = SRC_LIB.lines().collect();
        let mut out = vec![];
        let mut i = 0;
        while i < lines.len() {
            if lines[i].trim() == attr {
                let mut j = i + 1;
                while lines[j].trim().starts_with("#[") {
                    j += 1;
                }
                let sig = lines[j].trim();
                let rest = sig.strip_prefix("pub ").unwrap_or(sig);
                let rest = rest.strip_prefix("async ").unwrap_or(rest);
                let rest = rest.strip_prefix("fn ").unwrap_or_else(|| panic!("fn expected after {attr}: {sig}"));
                let name = rest.split(['(', '<']).next().unwrap().to_string();
                let mut body = String::new();
                let mut k = j;
                loop {
                    body.push_str(lines[k]);
                    body.push('\n');
                    if lines[k] == "}" {
                        break;
                    }
                    k += 1;
                }
                out.push((name, body));
                i = k;
            }
            i += 1;
        }
        out
    }

    /// Top-level function bodies of `admin.rs`, without the test module.
    fn admin_fn_bodies() -> BTreeMap<String, String> {
        let src = SRC_ADMIN.split("#[cfg(test)]").next().unwrap();
        let mut out: BTreeMap<String, String> = BTreeMap::new();
        let mut current: Option<String> = None;
        for line in src.lines() {
            let head = ["pub async fn ", "pub fn ", "async fn ", "fn "].iter().find_map(|p| line.strip_prefix(p));
            if let Some(rest) = head {
                let name = rest.split(['(', '<']).next().unwrap().to_string();
                out.insert(name.clone(), String::new());
                current = Some(name);
            }
            if let Some(name) = &current {
                let body = out.get_mut(name).unwrap();
                body.push_str(line);
                body.push('\n');
            }
        }
        out
    }

    /// What the method itself demands, read from the source: the strongest of
    /// `require_controller` and `require_admin` reached directly or through `admin::f(..)`.
    fn demanded_role(body: &str, admin_fns: &BTreeMap<String, String>) -> Option<IngressAccess> {
        let mut texts = vec![body.to_string()];
        for (name, text) in admin_fns {
            if body.contains(&format!("admin::{name}(")) {
                texts.push(text.clone());
            }
        }
        if texts.iter().any(|t| t.contains("require_controller()")) {
            Some(IngressAccess::Controller)
        } else if texts.iter().any(|t| t.contains("require_admin()")) {
            Some(IngressAccess::Admin)
        } else {
            None
        }
    }

    #[test]
    fn every_update_endpoint_has_an_ingress_decision() {
        let updates: BTreeSet<String> = endpoints("#[ic_cdk::update]").into_iter().map(|(n, _)| n).collect();
        assert!(updates.len() >= 20, "the scan found {} update endpoints", updates.len());
        let listed: BTreeSet<String> = INGRESS_METHODS.iter().map(|(n, _)| n.to_string()).collect();
        let missing: Vec<_> = updates.difference(&listed).collect();
        assert!(missing.is_empty(), "update endpoints without an ingress decision (add them to INGRESS_METHODS): {missing:?}");
        // Anything listed that is not an update must be one of the two standards queries.
        let queries: BTreeSet<String> = endpoints("#[ic_cdk::query]").into_iter().map(|(n, _)| n).collect();
        let extra: Vec<_> = listed.difference(&updates).cloned().collect();
        assert_eq!(extra, vec!["icrc10_supported_standards".to_string(), "icrc28_trusted_origins".to_string()]);
        assert!(extra.iter().all(|n| queries.contains(n)), "{extra:?}");
        // No duplicates.
        assert_eq!(listed.len(), INGRESS_METHODS.len());
        // The explicit list of update endpoints this decision table was written for.
        let expected: BTreeSet<String> = [
            "bridge_to_base",
            "notify_base_bridge_out",
            "request_deposit_refund",
            "get_base_addresses",
            "admin_set_pauses",
            "admin_set_limits",
            "admin_set_fees",
            "admin_set_gas",
            "admin_set_observer_config",
            "admin_set_rpc_services",
            "admin_bind_contract",
            "admin_sweep_fees",
            "admin_submit_contract_action",
            "admin_cancel_stuck_tx",
            "admin_clear_lane_halt",
            "admin_retry_deposit",
            "admin_reconcile",
            "admin_clear_halt",
            "admin_run_tick",
            "set_admins",
            "icrc21_canister_call_consent_message",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(updates, expected);
    }

    #[test]
    fn the_filter_matches_what_each_method_demands_of_its_caller() {
        let admin_fns = admin_fn_bodies();
        for (name, body) in endpoints("#[ic_cdk::update]") {
            let access = ingress_access(&name).unwrap_or_else(|| panic!("{name} has no decision"));
            let demanded = demanded_role(&body, &admin_fns);
            match demanded {
                Some(IngressAccess::Controller) => assert_eq!(access, IngressAccess::Controller, "{name} requires a controller"),
                Some(IngressAccess::Admin) => assert_eq!(access, IngressAccess::Admin, "{name} requires an admin"),
                _ => assert!(
                    matches!(access, IngressAccess::Anyone | IngressAccess::Authenticated),
                    "{name} demands no role, so the filter must not demand one either"
                ),
            }
            if name.starts_with("admin_") {
                assert!(demanded.is_some(), "{name} is named admin_ but its body checks no role");
            }
        }
    }

    const ANON: (bool, bool, bool) = (true, false, false);
    const USER: (bool, bool, bool) = (false, false, false);
    const ADMIN: (bool, bool, bool) = (false, true, false);
    const CONTROLLER: (bool, bool, bool) = (false, false, true);
    const BOTH: (bool, bool, bool) = (false, true, true);

    /// A typical small argument (a `bridge_to_base` call is a few hundred bytes).
    const SMALL: usize = 300;

    fn d(method: &str, who: (bool, bool, bool)) -> bool {
        decide(method, SMALL, who.0, who.1, who.2)
    }

    #[test]
    fn standards_are_open_to_everyone_including_anonymous() {
        for m in ["icrc21_canister_call_consent_message", "icrc28_trusted_origins", "icrc10_supported_standards"] {
            for who in [ANON, USER, ADMIN, CONTROLLER, BOTH] {
                assert!(d(m, who), "{m} {who:?}");
            }
        }
    }

    #[test]
    fn user_methods_need_an_authenticated_caller() {
        for m in ["bridge_to_base", "notify_base_bridge_out", "request_deposit_refund", "get_base_addresses"] {
            assert!(!d(m, ANON), "{m} anonymous");
            for who in [USER, ADMIN, CONTROLLER, BOTH] {
                assert!(d(m, who), "{m} {who:?}");
            }
        }
    }

    #[test]
    fn admin_methods_need_an_admin_or_a_controller() {
        for (m, access) in INGRESS_METHODS {
            if !m.starts_with("admin_") || *access != IngressAccess::Admin {
                continue;
            }
            assert!(!d(m, ANON), "{m} anonymous");
            assert!(!d(m, USER), "{m} plain user");
            for who in [ADMIN, CONTROLLER, BOTH] {
                assert!(d(m, who), "{m} {who:?}");
            }
        }
    }

    #[test]
    fn controller_methods_need_a_controller() {
        for m in ["admin_set_rpc_services", "set_admins"] {
            assert!(!d(m, ANON), "{m} anonymous");
            assert!(!d(m, USER), "{m} plain user");
            assert!(!d(m, ADMIN), "{m} an admin that is not a controller");
            assert!(d(m, CONTROLLER), "{m} controller");
            assert!(d(m, BOTH), "{m} controller and admin");
        }
    }

    #[test]
    fn a_controller_flag_never_lets_an_anonymous_caller_through() {
        // The anonymous principal can be a controller (PocketIC creates canisters that
        // way). Anonymous ingress is still refused for every role-bound method.
        for (m, access) in INGRESS_METHODS {
            if *access != IngressAccess::Anyone {
                assert!(!decide(m, SMALL, true, true, true), "{m}");
            }
        }
    }

    #[test]
    fn unknown_methods_are_refused_for_everyone() {
        for m in ["get_bridge_info", "get_deposit", "list_my_deposits", "bridge_version", "no_such_method", "", "Admin_set_pauses", "admin_set_pauses "] {
            for who in [ANON, USER, ADMIN, CONTROLLER, BOTH] {
                assert!(!d(m, who), "{m:?} {who:?}");
            }
        }
        // Query endpoints are refused as updates: they have no update entry point.
        for (m, _) in endpoints("#[ic_cdk::query]") {
            if m == "icrc28_trusted_origins" || m == "icrc10_supported_standards" {
                continue;
            }
            assert!(!d(&m, BOTH), "query {m} must not be accepted as an update");
        }
    }

    #[test]
    fn oversized_arguments_are_refused_before_execution_for_every_caller() {
        for (m, access) in INGRESS_METHODS {
            let max = match access {
                IngressAccess::Anyone | IngressAccess::Authenticated => MAX_OPEN_INGRESS_BYTES,
                IngressAccess::Admin | IngressAccess::Controller => MAX_ADMIN_INGRESS_BYTES,
            };
            // BOTH passes every role check, so only the size decides.
            assert!(decide(m, max, false, true, true), "{m} at exactly {max} bytes");
            assert!(!decide(m, max + 1, false, true, true), "{m} at {} bytes", max + 1);
            assert!(!decide(m, 2 * 1024 * 1024, false, true, true), "{m} near the 2 MiB ingress limit");
        }
        // Anonymous consent requests are open, but only small ones.
        assert!(decide("icrc21_canister_call_consent_message", MAX_OPEN_INGRESS_BYTES, true, false, false));
        assert!(!decide("icrc21_canister_call_consent_message", MAX_OPEN_INGRESS_BYTES + 1, true, false, false));
    }

    #[test]
    fn real_arguments_fit_under_their_caps() {
        use candid::Encode;
        let consent = icrc21::ConsentMessageRequest {
            method: "bridge_to_base".into(),
            arg: Encode!(&types::BridgeToBaseArgs {
                amount_e8s: u64::MAX,
                base_recipient: format!("0x{}", "ab".repeat(20)),
                from_subaccount: Some(serde_bytes::ByteBuf::from(vec![0xff; 32])),
                client_request_id: serde_bytes::ByteBuf::from(vec![0xff; 32]),
                expected_deposit_fee_e8s: u64::MAX,
            })
            .unwrap(),
            user_preferences: icrc21::ConsentMessageSpec {
                metadata: icrc21::ConsentMessageMetadata { language: "en-US".into(), utc_offset_minutes: Some(-420) },
                device_spec: Some(icrc21::DeviceSpec::LineDisplay { characters_per_line: 40, lines_per_page: 10 }),
            },
        };
        let consent_len = Encode!(&consent).unwrap().len();
        assert!(consent_len * 4 < MAX_OPEN_INGRESS_BYTES, "consent request {consent_len} B");
        let services: Vec<types::RpcServiceConfig> =
            (0..8).map(|i| types::RpcServiceConfig::Custom { url: format!("https://provider-{i}.example.org/{}", "x".repeat(200)) }).collect();
        let rpc_len = Encode!(&services, &2u8).unwrap().len();
        assert!(rpc_len * 4 < MAX_ADMIN_INGRESS_BYTES, "rpc services {rpc_len} B");
        let admins: Vec<Principal> = (0..50u8).map(|i| Principal::from_slice(&[i; 29])).collect();
        let admins_len = Encode!(&admins).unwrap().len();
        assert!(admins_len * 4 < MAX_ADMIN_INGRESS_BYTES, "admins {admins_len} B");
    }

    /// `icp canister call` sends an update unless `--query` is given, and query names sent
    /// as updates are refused. Every runbook call to a bridge query must pass `--query`,
    /// and no update call may.
    #[test]
    fn the_runbook_calls_queries_as_queries_and_updates_as_updates() {
        const RUNBOOK: &str = include_str!("../../../foundry/DEPLOY-BASE.md");
        let queries: BTreeSet<String> = endpoints("#[ic_cdk::query]").into_iter().map(|(n, _)| n).collect();
        let updates: BTreeSet<String> = endpoints("#[ic_cdk::update]").into_iter().map(|(n, _)| n).collect();
        // Join backslash-continued lines so a wrapped command is checked as one.
        let joined = RUNBOOK.replace("\\\n", " ");
        let mut seen = 0;
        for line in joined.lines() {
            // Whitespace tokens, with a `<placeholder with spaces>` kept as one token.
            let mut tokens: Vec<String> = vec![];
            for t in line.split_whitespace() {
                match tokens.last_mut() {
                    Some(last) if last.starts_with('<') && !last.ends_with('>') => {
                        last.push(' ');
                        last.push_str(t);
                    }
                    _ => tokens.push(t.to_string()),
                }
            }
            let tokens: Vec<&str> = tokens.iter().map(String::as_str).collect();
            let Some(call) = tokens.windows(2).position(|w| w == ["canister", "call"]) else { continue };
            // Positional arguments after `call`, whatever the flags and their order: the
            // canister (name or id), then the method.
            let positional: Vec<&str> = tokens[call + 2..]
                .iter()
                .filter(|t| !t.starts_with('-') && !t.starts_with('\'') && !t.starts_with('('))
                .map(|t| t.trim_matches('"'))
                .collect();
            let Some(method) = positional.get(1).copied() else { continue };
            let is_query = queries.contains(method);
            if !is_query && !updates.contains(method) {
                // A ledger or other canister method (`icrc2_approve`, `icrc1_balance_of`, ...).
                continue;
            }
            seen += 1;
            let has_query_flag = tokens.contains(&"--query");
            assert_eq!(has_query_flag, is_query, "runbook call to {method} must{} pass --query: {line}", if is_query { "" } else { " not" });
        }
        assert!(seen >= 16, "the scan found only {seen} runbook calls to bridge methods");
    }
}
