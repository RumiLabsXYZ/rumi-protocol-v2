//! Rumi icUSD bridge, Solana instance: canonical ICP icUSD escrow <-> escrow-backed icUSD on
//! Solana. Source only (cards BB-47 and BB-48): nothing is deployed, signed, funded or run
//! against any network, and the Solana profile stays `disabled`.
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
//! * `tx`: legacy message builders (mint, burn, refund, the advance-only cancel, the pool
//!   bootstrap, the reference return), the return-shape checker and golden vectors; no Solana SDK;
//! * `sol_rpc` (BB-48): the typed SOL RPC mirror, checked offline against the pinned upstream
//!   canister interface, the [`sol_rpc::SolRpc`] transport trait and the wasm transport;
//! * `nonce`, `records`, `lane` (BB-48): the durable-nonce pool, the stable records, and the
//!   persist-before-sign, same-attempt reconcile engine run against a transport trait and a
//!   signer trait (mocks in `testkit`, unit tests only);
//! * `state`, `env`, `admin`, `icrc21`, `types`, `ledger`: the copied instance skeleton.
//!
//! BB-49 49a added the real `sign_with_schnorr` signer, the timer, mint creation and bind checks
//! and the pool bootstrap driver. 49b adds the ICP-side ledger saga (`flows`, `icp`): deposit
//! admission and the ICRC-2 pull, the ICP refund, releases opened only from a delivered burn, fee
//! sweeps, the persisted-tuple lifecycle with bounded ICRC-3 proofs, the user, hint and admin
//! endpoints and the admin resolution of held operations.
//!
//! Still deferred (49c and 49d): the mint saga that builds a mint operation for an escrowed
//! deposit and applies `OpEvent::Delivered` to the accounting buckets (49c), the return observer
//! (49c), and the burn lane that custodies, burns and calls `flows::release::open_release` (49d).
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
//! * `solana-history-availability-unmeasured`: `LaneSettings::history_horizon_secs` is an
//!   UNMEASURED placeholder (`types::DEFAULT_HISTORY_HORIZON_SECS`); the lane halts when the nonce
//!   advanced and no consumer was found within it.
//! * `solana-rpc-consensus-unmeasured`: the consensus settings (`RpcSettings`, strict `verify`,
//!   lenient `discover`) are configuration, not measurements; the first discovery page is
//!   requested without `before` (the interface requires it only for an idempotent reply), which
//!   is workable because a nonce account is quiet.
//! * `sol-rpc-canister-id-conflict`: the id is an install argument. The mirror was checked against
//!   `tests/fixtures/sol_rpc_canister.did`, the exact upstream file (git blob `cd456a3b`) of
//!   `dfinity/sol-rpc-canister` at commit `5ac94de2` (see `sol_rpc.rs`). That proves
//!   source-fixture equality only, not the deployed canister's interface or provider behavior.
//! * Ed25519 signing determinism: the lane records every signature and every transaction id and
//!   does not assume either way.
//! * `CreateAccountWithSeed` for the nonce accounts is now read from `solana-system-interface`
//!   3.2.0 (U resolved on paper); the resulting message is golden-vectored like the others.
//! * Token-2022 metadata: the bind requires every metadata authority to be none or the canister's
//!   `mint-authority` key (`mint::check_mint`). The bootstrap creates the mint WITHOUT the metadata
//!   extensions (name, symbol and URI creation is a disclosed deferred omission), so whether the
//!   program accepts `None` authorities at creation is not exercised here. Extension and tag ids
//!   carry source pins (`mint.rs`, `tx/mod.rs`) that are a reading of source, not installed-program
//!   or wallet proof.

pub mod accounting;
pub mod admin;
pub mod bootstrap;
pub mod env;
pub mod flows;
pub mod history_probe;
pub mod icp;
pub mod icrc21;
pub mod ids;
pub mod lane;
#[cfg(test)]
mod bootstrap_tests;
#[cfg(test)]
mod lane_tests;
pub mod ledger;
pub mod memo;
pub mod mint;
pub mod nonce;
pub mod records;
pub mod schnorr;
pub mod sol_rpc;
pub mod state;
#[cfg(test)]
#[allow(dead_code)]
mod testkit;
pub mod timer;
pub mod tx;
pub mod types;
pub mod views;

#[cfg(test)]
mod saga_tests;

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
    // The stored cluster and key must still satisfy the install-time rule. A trap in
    // `post_upgrade` rolls the upgrade back: the old module and its state stay in place.
    if let Err(e) = state::read_core(|c| state::validate_stored_config(&c.config)) {
        ic_cdk::trap(&format!("upgrade refused, the previous module stays installed: {e}"));
    }
    if state::migrate_core() {
        state::append_audit(None, AuditCtx::note("core schema migrated"));
    }
    state::append_audit(None, AuditCtx::note("upgraded"));
    timer::schedule_soon();
}

// ---------------------------------------------------------------------------
// User methods
// ---------------------------------------------------------------------------

#[ic_cdk::update]
async fn bridge_to_solana(args: BridgeToSolanaArgs) -> BridgeResult<DepositView> {
    flows::deposit::bridge_to_solana(&icp::CanisterLedger::from_config(), args).await
}

#[ic_cdk::update]
async fn request_deposit_refund(id: u64) -> BridgeResult<DepositView> {
    flows::deposit::request_refund(&icp::CanisterLedger::from_config(), id).await
}

/// "Look at this Solana transaction sooner" (base58 signature). Stores a bounded hint; never
/// settles anything.
#[ic_cdk::update]
fn notify_solana_return(signature: String) -> BridgeResult<()> {
    flows::release::record_hint(signature)
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
/// `bridge_to_solana` reply).
#[ic_cdk::query]
fn get_deposit_by_request(owner: Principal, client_request_id: serde_bytes::ByteBuf) -> Option<DepositView> {
    let key = state::request_key(&owner, &client_request_id).ok()?;
    state::lookup_request(&key).and_then(state::get_deposit).map(|d| views::deposit_view(&d))
}

/// A release by its return id (64 hex).
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

/// Releases created by returns from a Solana wallet (base58), including held claims.
#[ic_cdk::query]
fn list_releases_for_solana_sender(args: ListReleasesForSenderArgs) -> Vec<ReleaseView> {
    let Ok(sender) = tx::pubkey_from_base58(&args.solana_sender) else { return vec![] };
    state::list_sender_releases(&sender, args.start_after_seq, args.limit).iter().map(views::release_view).collect()
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

/// Resolve a held lane operation by hand: look again at the same bytes, or abandon a held mint.
#[ic_cdk::update]
fn admin_resolve_held_op(op_id: u64, resolution: HeldOpResolution) -> BridgeResult<OpView> {
    flows::held::resolve_held_op(op_id, resolution)
}

/// Clear a lane halt after resolving its cause; the note says how.
#[ic_cdk::update]
fn admin_clear_lane_halt(lane: records::Lane, note: String) -> BridgeResult<()> {
    admin::clear_lane_halt(lane, &note)?;
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

#[ic_cdk::query]
fn get_audit_log(start: u64, limit: u32) -> Vec<AuditEntry> {
    state::audit_page(start, limit).iter().map(views::audit_entry).collect()
}

/// One lane operation (mint, burn or stray refund) by id.
#[ic_cdk::query]
fn get_sol_op(id: u64) -> Option<OpView> {
    state::get_op(id).map(|o| views::op_view(&o))
}

/// Operations that are not terminal (including held ones), ascending by id after `start_after`.
#[ic_cdk::query]
fn list_unresolved_ops(args: ListOpsArgs) -> Vec<OpView> {
    views::unresolved_ops(args.start_after, args.limit)
}

/// Pending return hints (what the observer will fetch early).
#[ic_cdk::query]
fn list_return_hints() -> Vec<HintView> {
    flows::release::live_hints(env::now_ns()).iter().map(views::hint_view).collect()
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
        limits: c.config.limits.clone(),
        rpc: c.config.rpc.clone(),
        lane: c.config.lane.clone(),
        halted: c.halted.as_ref().map(|h| h.reason.clone()),
        instance_salt: hex::encode(c.instance_salt),
        escrow_subaccount: hex::encode(ids::escrow_subaccount()),
    })
}

#[ic_cdk::query]
fn get_bootstrap_status() -> BootstrapStatus {
    use records::{Lane, SlotState};
    let (keys, mint_state, bind_note, token_program) =
        state::read_core(|c| (c.keys.clone(), c.mint_state, c.bind_note.clone(), c.config.token_program));
    let slots: Vec<_> = Lane::ALL.iter().flat_map(|l| state::slots_in_lane(*l)).collect();
    let count = |f: &dyn Fn(&SlotState) -> bool| slots.iter().filter(|s| f(&s.state)).count() as u32;
    let return_account = keys.as_ref().and_then(|k| {
        let mint = k.mint_account?;
        tx::derive_ata(&k.return_owner, &token_program.id(), &mint).ok().map(|a| tx::pubkey_to_base58(&a))
    });
    BootstrapStatus {
        keys_derived: keys.as_ref().is_some_and(|k| k.mint_account.is_some()),
        fee_payer: keys.as_ref().map(|k| tx::pubkey_to_base58(&k.fee_payer)),
        mint_authority: keys.as_ref().map(|k| tx::pubkey_to_base58(&k.mint_authority)),
        return_owner: keys.as_ref().map(|k| tx::pubkey_to_base58(&k.return_owner)),
        mint_account: keys.as_ref().and_then(|k| k.mint_account).map(|m| tx::pubkey_to_base58(&m)),
        return_account,
        mint_state,
        bind_note,
        pool_registered: slots.len() as u32,
        pool_unverified: count(&|s| *s == SlotState::Unverified),
        pool_free: count(&|s| *s == SlotState::Free),
        pool_reserved: count(&|s| matches!(s, SlotState::Reserved { .. })),
        pending_bootstrap_txs: state::bootstrap_tx_count() as u32,
    }
}

#[ic_cdk::query]
fn get_buckets() -> BucketsView {
    state::read_core(|c| BucketsView::from(&c.buckets))
}

// ---------------------------------------------------------------------------
// History-availability probe (feature `history_probe` only; never in the production wasm)
// ---------------------------------------------------------------------------

/// Run the P5 probe (`history_probe.rs`, `HISTORY_PROBE.md`) with an explicit configuration. Reads
/// and reports; changes no bridge state. Controller only. It spends cycles on the SOL RPC canister
/// named in the arguments, so it exists only in a throwaway build of this crate.
#[cfg(feature = "history_probe")]
#[ic_cdk::update]
async fn probe_history_availability(args: history_probe::ProbeArgs) -> BridgeResult<history_probe::ProbeReport> {
    admin::require_controller()?;
    let rpc = sol_rpc::CandidRpc::from_config(args.sol_rpc, args.rpc.clone()).map_err(|e| BridgeError::InvalidArgument(format!("{e:?}")))?;
    history_probe::run(&rpc, &args).await.map_err(BridgeError::InvalidArgument)
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
    /// A configured admin. Controllers also pass this filter (the method itself still requires an
    /// admin).
    Admin,
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
    // User methods.
    ("bridge_to_solana", IngressAccess::Authenticated),
    ("request_deposit_refund", IngressAccess::Authenticated),
    ("notify_solana_return", IngressAccess::Authenticated),
    // Admin methods.
    ("admin_set_pauses", IngressAccess::Admin),
    ("admin_set_limits", IngressAccess::Admin),
    ("admin_set_fees", IngressAccess::Admin),
    ("admin_sweep_fees", IngressAccess::Admin),
    ("admin_retry_deposit", IngressAccess::Admin),
    ("admin_resolve_held_op", IngressAccess::Admin),
    ("admin_clear_lane_halt", IngressAccess::Admin),
    ("admin_run_tick", IngressAccess::Admin),
    // Controller methods.
    ("set_admins", IngressAccess::Controller),
    #[cfg(feature = "history_probe")]
    ("probe_history_availability", IngressAccess::Controller),
];

/// Largest accepted argument for open methods (wallet standards and user methods). The biggest
/// legitimate one, a `bridge_to_solana` consent request, is well under 1 KiB; without a cap any
/// caller could make the canister pay the ingress fee for about 2 MiB.
pub const MAX_OPEN_INGRESS_BYTES: usize = 4 * 1024;
/// Largest accepted argument for admin and controller methods.
pub const MAX_ADMIN_INGRESS_BYTES: usize = 16 * 1024;

fn ingress_access(method: &str) -> Option<IngressAccess> {
    INGRESS_METHODS.iter().find(|(name, _)| *name == method).map(|(_, access)| *access)
}

/// Whether an ingress update call is worth executing. Pure, so the table above is unit tested.
/// NOT a security boundary: it runs on one replica without consensus. Every method keeps its
/// own caller checks, which are what protect state and funds.
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
        if anonymous { (false, false) } else { (state::read_core(|c| c.is_admin(&caller)), ic_cdk::api::is_controller(&caller)) };
    if decide(&method, arg_bytes, anonymous, is_admin, is_controller) {
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
        // The probe build has one more endpoint and its own checked-in interface.
        let file = if cfg!(feature = "history_probe") { "rumi_icusd_sol_bridge.history_probe.did" } else { "rumi_icusd_sol_bridge.did" };
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(file);
        if std::env::var("RUMI_SOL_BRIDGE_REGEN_DID").is_ok() {
            std::fs::write(&path, &generated).expect("write did");
        }
        let checked_in = std::fs::read_to_string(&path).expect("read the checked-in .did");
        service_equal(CandidSource::Text(&generated), CandidSource::Text(&checked_in))
            .unwrap_or_else(|e| panic!("{file} is out of date; regenerate it: {e:?}"));
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
                // A feature-gated endpoint exists only when its feature is on.
                let gated = i > 0 && lines[i - 1].trim() == r#"#[cfg(feature = "history_probe")]"#;
                if gated && !cfg!(feature = "history_probe") {
                    continue;
                }
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
        let expected = if cfg!(feature = "history_probe") { 14 } else { 13 };
        assert_eq!(updates.len(), expected, "the update endpoints are exactly the listed ones: {updates:?}");
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
        // decide(method, bytes, anonymous, admin, controller)
        // Wallet standards: anyone, anonymous included, within the size cap.
        assert!(decide("icrc21_canister_call_consent_message", 100, true, false, false));
        assert!(!decide("icrc21_canister_call_consent_message", MAX_OPEN_INGRESS_BYTES + 1, true, false, false));
        // User methods: any authenticated principal, never anonymous, within the open cap.
        for m in ["bridge_to_solana", "request_deposit_refund", "notify_solana_return"] {
            assert!(decide(m, 100, false, false, false), "{m}");
            assert!(!decide(m, 100, true, false, false), "{m}: anonymous");
            assert!(!decide(m, MAX_OPEN_INGRESS_BYTES + 1, false, false, false), "{m}: size");
        }
        // Admin methods: an admin or a controller, never anonymous.
        for m in [
            "admin_set_pauses",
            "admin_set_limits",
            "admin_set_fees",
            "admin_sweep_fees",
            "admin_retry_deposit",
            "admin_resolve_held_op",
            "admin_clear_lane_halt",
            "admin_run_tick",
        ] {
            assert!(decide(m, 100, false, true, false), "{m}: admin");
            assert!(decide(m, 100, false, false, true), "{m}: controller");
            assert!(!decide(m, 100, false, false, false), "{m}: neither");
            assert!(!decide(m, 100, true, true, true), "{m}: anonymous");
            assert!(!decide(m, MAX_ADMIN_INGRESS_BYTES + 1, false, true, false), "{m}: size");
        }
        // Controller methods: a controller only, never anonymous (an admin is not enough).
        assert!(decide("set_admins", 100, false, false, true));
        assert!(!decide("set_admins", 100, false, true, false));
        assert!(!decide("set_admins", 100, true, false, true));
        assert!(!decide("set_admins", MAX_ADMIN_INGRESS_BYTES + 1, false, false, true));
        // Unknown methods and query names sent as updates are refused.
        assert!(!decide("get_bridge_info", 0, false, true, true));
        assert!(!decide("get_deposit", 0, false, true, true));
        assert!(!decide("list_unresolved_ops", 0, false, true, true));
    }
}
