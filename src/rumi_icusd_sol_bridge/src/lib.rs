//! Rumi icUSD bridge, Solana instance: canonical ICP icUSD escrow <-> escrow-backed icUSD on
//! Solana. SKELETON (card BB-47): nothing is deployed, signed, funded or run against any
//! network, and the Solana profile stays `disabled`.
//!
//! One canister instance serves one destination cluster (settled topology D1). The guard is
//! memo plus durable nonce (D3): there is no Solana program, so returns are custodied in a
//! canister-owned token account until a canister-signed burn is final (`accounting.rs`).
//!
//! Design: `docs/coordination/multichain/chain-solana.md` and
//! `docs/superpowers/specs/2026-10-30-multichain-icusd-groundwork-design.md`.
//!
//! What exists in this crate:
//!
//! * `ids`: domains, key paths and ids (mint, re-mint, return, burn, refund);
//! * `memo`: the return memo grammar and the canister memos;
//! * `accounting`: the escrow buckets with the `RC`, `BU` and `stray` additions, and the
//!   coverage, supply and custody checks;
//! * `tx`: legacy message builders (mint, burn, refund, reference return), the return-shape
//!   checker and golden vectors; no Solana SDK;
//! * `state`, `env`, `admin`, `icrc21`, `types`, `ledger`: the copied instance skeleton.
//!
//! Deferred to BB-48 (nonce pool, mint saga) and BB-49 (return observer, burn lane): SOL RPC
//! client, durable-nonce pool and reconcile, mint saga, mint creation and bind checks, return
//! observer, burn lane, refund lane, timers, the deposit/return/admin endpoints and the
//! deposit and release flows built on `ledger.rs`.
//!
//! # Open (U) items this crate carries, by profile blocker id
//!
//! * `solana-memo-program-version-unconfirmed`: the Memo program id is a parameter of every
//!   builder and of the shape checker (`tx::builders::TxProfile::memo_program`,
//!   `tx::shape::ReturnShapeContext::memo_program`, `state::BridgeConfig::memo_program`).
//!   `tx::MEMO_PROGRAM_V3_ID` is a default, not a confirmed fact.
//! * `solana-unattributed-burn-policy`: `accounting::check_destination_supply` takes
//!   `unattributed_burn: Option<u128>`; `None` is the missing `UB` seam (no burn observer), so
//!   only `supply <= O` is enforced and the gap is reported, never acted on.
//! * `solana-token-program-wallet-check`: the token program is a parameter
//!   (`tx::TokenProgram`, exactly two values) and every golden vector exists for both.
//! * Token tag 255 (`Batch`) deployed or not: `tx::TAG_BATCH` is rejected either way.
//! * Wallet-added guard instructions in a return are refused (default deny in
//!   `tx::shape`), to be probed per wallet at activation.
//! * Not touched here (BB-48 and later): `sol-rpc-canister-id-conflict` (the id is an install
//!   argument), `solana-rpc-consensus-unmeasured`, `solana-history-availability-unmeasured`,
//!   Ed25519 signing determinism, `CreateAccountWithSeed` for nonce accounts, and whether the
//!   Token-2022 metadata authorities can be set to `None`.

pub mod accounting;
pub mod admin;
pub mod env;
pub mod icrc21;
pub mod ids;
pub mod ledger;
pub mod memo;
pub mod state;
pub mod tx;
pub mod types;

use candid::Principal;
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
    let salt = ids::instance_salt(env::canister_id().as_slice(), now);
    state::mutate_core(|c| {
        if c.initialized {
            ic_cdk::trap("bridge state already initialized");
        }
        c.initialized = true;
        c.config = config;
        c.instance_salt = salt;
    });
    state::append_audit(None, AuditCtx::note("initialized"));
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
    // The stored cluster and key must still satisfy the install-time rule. A trap in
    // `post_upgrade` rolls the upgrade back: the old module and its state stay in place.
    if let Err(e) = state::read_core(|c| state::validate_stored_config(&c.config)) {
        ic_cdk::trap(&format!("upgrade refused, the previous module stays installed: {e}"));
    }
    state::append_audit(None, AuditCtx::note("upgraded"));
}

// ---------------------------------------------------------------------------
// Controller methods
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Minimal query surface
// ---------------------------------------------------------------------------

#[ic_cdk::query]
fn bridge_version() -> String {
    ids::BRIDGE_VERSION.to_string()
}

#[ic_cdk::query]
fn get_bridge_info() -> BridgeInfo {
    state::read_core(|c| BridgeInfo {
        schema_version: c.schema_version,
        initialized: c.initialized,
        genesis_hash: tx::pubkey_to_base58(&c.config.genesis_hash),
        token_program: c.config.token_program,
        memo_program: tx::pubkey_to_base58(&c.config.memo_program),
        icusd_ledger: c.config.icusd_ledger,
        sol_rpc: c.config.sol_rpc,
        schnorr_key_name: c.config.schnorr_key_name.clone(),
        admins: c.config.admins.clone(),
        pauses: c.config.pauses.clone(),
        fees: c.config.fees.clone(),
        halted: c.halted.as_ref().map(|h| h.reason.clone()),
        instance_salt: hex::encode(c.instance_salt),
        escrow_subaccount: hex::encode(ids::escrow_subaccount()),
    })
}

#[ic_cdk::query]
fn get_buckets() -> BucketsView {
    state::read_core(|c| BucketsView::from(&c.buckets))
}

// ---------------------------------------------------------------------------
// Ingress pre-filter (cycle-drain protection)
// ---------------------------------------------------------------------------

/// Who an ingress call to a method is worth executing for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IngressAccess {
    /// Wallet standards: anyone, anonymous included (a wallet asks before the user signs).
    Anyone,
    /// A canister controller.
    Controller,
}

/// Every method that may legitimately arrive as an ingress UPDATE call, with the callers it is
/// worth running for. Anything not listed is refused before execution. A new update endpoint
/// must be added here; `every_update_endpoint_has_an_ingress_decision` fails until it is.
///
/// The two `Anyone` entries that are queries here (ICRC-28, ICRC-10) are listed so the
/// standards stay open to anonymous callers when a client sends them as update calls.
const INGRESS_METHODS: &[(&str, IngressAccess)] = &[
    ("icrc21_canister_call_consent_message", IngressAccess::Anyone),
    ("icrc28_trusted_origins", IngressAccess::Anyone),
    ("icrc10_supported_standards", IngressAccess::Anyone),
    ("set_admins", IngressAccess::Controller),
];

/// Largest accepted argument for wallet-standard calls.
pub const MAX_OPEN_INGRESS_BYTES: usize = 4 * 1024;
/// Largest accepted argument for admin and controller methods.
pub const MAX_ADMIN_INGRESS_BYTES: usize = 16 * 1024;

fn ingress_access(method: &str) -> Option<IngressAccess> {
    INGRESS_METHODS.iter().find(|(name, _)| *name == method).map(|(_, access)| *access)
}

/// Whether an ingress update call is worth executing. Pure, so the table above is unit tested.
/// NOT a security boundary: it runs on one replica without consensus. Every method keeps its
/// own caller checks, which are what protect state and funds.
fn decide(method: &str, arg_bytes: usize, caller_is_anonymous: bool, is_controller: bool) -> bool {
    let Some(access) = ingress_access(method) else {
        return false;
    };
    let max_bytes = match access {
        IngressAccess::Anyone => MAX_OPEN_INGRESS_BYTES,
        IngressAccess::Controller => MAX_ADMIN_INGRESS_BYTES,
    };
    if arg_bytes > max_bytes {
        return false;
    }
    match access {
        IngressAccess::Anyone => true,
        IngressAccess::Controller => !caller_is_anonymous && is_controller,
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
    /// Regenerate with: `RUMI_SOL_BRIDGE_REGEN_DID=1 cargo test -p rumi_icusd_sol_bridge --lib candid_interface`.
    #[test]
    fn candid_interface_matches_checked_in_did() {
        let generated = super::__export_service();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("rumi_icusd_sol_bridge.did");
        if std::env::var("RUMI_SOL_BRIDGE_REGEN_DID").is_ok() {
            std::fs::write(&path, &generated).expect("write did");
        }
        let checked_in = std::fs::read_to_string(&path).expect("read rumi_icusd_sol_bridge.did");
        service_equal(CandidSource::Text(&generated), CandidSource::Text(&checked_in))
            .expect("rumi_icusd_sol_bridge.did is out of date; regenerate it");
    }
}

#[cfg(test)]
mod ingress_tests {
    use super::*;
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
        assert_eq!(updates.len(), 2, "the skeleton has exactly two update endpoints: {updates:?}");
        let listed: BTreeSet<String> = INGRESS_METHODS.iter().map(|(n, _)| n.to_string()).collect();
        let missing: Vec<_> = updates.difference(&listed).collect();
        assert!(missing.is_empty(), "update endpoints without an ingress decision: {missing:?}");
        // Anything listed that is not an update must be one of the two standards queries.
        let queries: BTreeSet<String> = endpoints("#[ic_cdk::query]").into_iter().collect();
        let extra: Vec<_> = listed.difference(&updates).cloned().collect();
        assert_eq!(extra, vec!["icrc10_supported_standards".to_string(), "icrc28_trusted_origins".to_string()]);
        assert!(extra.iter().all(|n| queries.contains(n)), "{extra:?}");
        assert_eq!(listed.len(), INGRESS_METHODS.len(), "no duplicates");
    }

    #[test]
    fn the_ingress_table_admits_the_right_callers() {
        // Wallet standards: anyone, anonymous included, within the size cap.
        assert!(decide("icrc21_canister_call_consent_message", 100, true, false));
        assert!(!decide("icrc21_canister_call_consent_message", MAX_OPEN_INGRESS_BYTES + 1, true, false));
        // Controller methods: a controller only, never anonymous.
        assert!(decide("set_admins", 100, false, true));
        assert!(!decide("set_admins", 100, false, false));
        assert!(!decide("set_admins", 100, true, true));
        assert!(!decide("set_admins", MAX_ADMIN_INGRESS_BYTES + 1, false, true));
        // Unknown methods and query names sent as updates are refused.
        assert!(!decide("get_bridge_info", 0, false, true));
        assert!(!decide("bridge_to_solana", 0, false, true), "not an endpoint yet");
    }
}
