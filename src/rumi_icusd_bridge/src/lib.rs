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
    timer::run_tick().await;
    Ok(())
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
