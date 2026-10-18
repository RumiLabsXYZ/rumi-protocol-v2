//! XRPL icUSD lane: durable claims, canonical escrow, and the issuer sequencer.
//!
//! This is the FOUNDATION (Task 3): configuration (disabled by default), the
//! operation/job/attempt state machines, the IC escrow ledger jobs, the single-flight
//! issuer sequencer, canonical-to-XRPL issuance, owner claim resolution, and the
//! idle-free background tick. The borrow and repayment flows (Tasks 4 and 5) add
//! `borrow.rs` / `repay.rs` / `observer.rs` on top of these types and transitions.
//!
//! Module map: `types` (wire/stable types) -> `state` (pure transitions, buckets,
//! coverage, event application) -> `ledger` (IC escrow jobs) and `settlement`
//! (issuer sequencer) -> `issuance` (backed claims, resolution) -> this module
//! (guards, configuration, public async entry points, the tick).

pub mod issuance;
pub mod ledger;
pub mod settlement;
pub mod state;
pub mod types;

#[cfg(test)]
mod tests;

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;

use candid::Principal;

pub use state::{
    apply_xrpl_icusd_event, operation_view, owner_page, public_config, xrpl_accrual_frozen,
    xrpl_vault_excluded, XrplIcusdState,
};
pub use types::*;

use crate::chains::xrp::address::classic_address_from_ed25519_pubkey;
use crate::chains::xrp::issued_amount::IssuedAsset;
use crate::chains::xrp::issued_rpc::fetch_trust_line;
use crate::chains::xrp::network::{
    check_network_allowed, collateral_network_for_key, require_collateral_key, XrplNetwork,
};
use crate::chains::xrp::ted25519;
use crate::state::{mutate_state, read_state};

fn now() -> u64 {
    ic_cdk::api::time()
}

// ----------------------------------------------------------------------------------
// Re-entrancy guards (transient; reset by an upgrade, which also normalises jobs)
// ----------------------------------------------------------------------------------

thread_local! {
    static OPS_BUSY: RefCell<BTreeSet<OperationId>> = const { RefCell::new(BTreeSet::new()) };
    static ISSUER_BUSY: Cell<bool> = const { Cell::new(false) };
    static TICK_RUNNING: Cell<bool> = const { Cell::new(false) };
    static TICK_SCHEDULED: Cell<bool> = const { Cell::new(false) };
    static TICK_DELAY_SECS: Cell<u64> = const { Cell::new(TICK_ACTIVE_SECS) };
    static TICK_CURSOR: Cell<u64> = const { Cell::new(0) };
    /// `(kind, key) -> last accepted ns`: bounds the cycle cost any single caller can
    /// impose through the user-triggered outcall paths. Transient by design.
    static THROTTLE: RefCell<std::collections::BTreeMap<(u8, Vec<u8>), u64>> =
        const { RefCell::new(std::collections::BTreeMap::new()) };
}

/// Minimum gap between two accepted new-request/resume calls per caller/operation.
pub const NEW_REQUEST_GAP_NS: u64 = 10_000_000_000;
pub const RESUME_GAP_NS: u64 = 30_000_000_000;
const THROTTLE_KIND_NEW: u8 = 1;
const THROTTLE_KIND_RESUME: u8 = 2;
const THROTTLE_MAX_ENTRIES: usize = 4_096;

/// True (and records `now_ns`) when `min_gap_ns` has elapsed since the last accepted
/// call for `(kind, key)`.
pub fn throttle_ok(kind: u8, key: &[u8], now_ns: u64, min_gap_ns: u64) -> bool {
    THROTTLE.with(|t| {
        let mut t = t.borrow_mut();
        if t.len() >= THROTTLE_MAX_ENTRIES {
            // Entries older than the largest gap carry no information.
            t.retain(|_, last| now_ns.saturating_sub(*last) < RESUME_GAP_NS);
            if t.len() >= THROTTLE_MAX_ENTRIES {
                return false;
            }
        }
        let k = (kind, key.to_vec());
        match t.get(&k) {
            Some(last) if now_ns.saturating_sub(*last) < min_gap_ns => false,
            _ => {
                t.insert(k, now_ns);
                true
            }
        }
    })
}

/// One driver per operation at a time.
#[must_use]
pub struct OpGuard(OperationId);

impl OpGuard {
    pub fn acquire(id: OperationId) -> Result<Self, XrplIcusdError> {
        OPS_BUSY.with(|b| {
            if b.borrow_mut().insert(id) {
                Ok(OpGuard(id))
            } else {
                Err(XrplIcusdError::Unavailable(format!(
                    "operation {id} is already being processed"
                )))
            }
        })
    }
}

impl Drop for OpGuard {
    fn drop(&mut self) {
        OPS_BUSY.with(|b| {
            b.borrow_mut().remove(&self.0);
        });
    }
}

/// One issuer-sequencer step at a time (setup or a new delivery attempt).
#[must_use]
pub struct IssuerGuard(());

impl IssuerGuard {
    pub fn acquire() -> Result<Self, XrplIcusdError> {
        ISSUER_BUSY.with(|b| {
            if b.replace(true) {
                Err(XrplIcusdError::Unavailable(
                    "the issuer sequencer is busy".into(),
                ))
            } else {
                Ok(IssuerGuard(()))
            }
        })
    }
}

impl Drop for IssuerGuard {
    fn drop(&mut self) {
        ISSUER_BUSY.with(|b| b.set(false));
    }
}

// ----------------------------------------------------------------------------------
// Configuration (controller-only at the endpoint)
// ----------------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct ConfigUpdate {
    pub network: Option<XrplNetwork>,
    /// Derived issuer `(classic address, 32-byte public key)` for `network`.
    pub derived: Option<(String, Vec<u8>)>,
    pub policy: Option<IssuerPolicy>,
    pub enabled: Option<bool>,
    pub outstanding_cap_e8s: Option<u64>,
    pub per_operation_cap_e8s: Option<u64>,
    pub rounding_buffer_e8s: Option<u64>,
    pub max_fee_drops: Option<u64>,
}

impl XrplIcusdState {
    /// Apply a configuration change. The issuer policy and network are locked once
    /// issuer setup has started (the first AccountSet persisted).
    pub fn apply_config_update(&mut self, u: ConfigUpdate) -> Result<(), XrplIcusdError> {
        let started = self.config.setup_started();
        if let Some(p) = u.policy {
            p.validate()?;
            if started && self.config.policy != Some(p) {
                return Err(XrplIcusdError::Policy(
                    "the issuer policy cannot change after setup has started".into(),
                ));
            }
        }
        if let Some(n) = u.network {
            if started && self.config.network != Some(n) {
                return Err(XrplIcusdError::Policy(
                    "the network cannot change after setup has started".into(),
                ));
            }
        }
        if let Some(n) = u.network {
            if self.config.network != Some(n) {
                self.config.network = Some(n);
                self.config.issuer_address = None;
                self.config.issuer_public_key = None;
            }
        }
        if let Some((addr, pk)) = u.derived {
            if started && self.config.issuer_address.as_deref().is_some_and(|a| a != addr) {
                return Err(XrplIcusdError::Policy(
                    "the issuer address cannot change after setup has started".into(),
                ));
            }
            self.config.issuer_address = Some(addr);
            self.config.issuer_public_key = Some(pk);
        }
        if let Some(p) = u.policy {
            self.config.policy = Some(p);
        }
        if let Some(v) = u.enabled {
            self.config.enabled = v;
        }
        if let Some(v) = u.outstanding_cap_e8s {
            self.config.outstanding_cap_e8s = v;
        }
        if let Some(v) = u.per_operation_cap_e8s {
            self.config.per_operation_cap_e8s = v;
        }
        if let Some(v) = u.rounding_buffer_e8s {
            self.config.rounding_buffer_e8s = v;
        }
        if let Some(v) = u.max_fee_drops {
            self.config.max_fee_drops = v;
        }
        self.config.config_version += 1;
        Ok(())
    }

    pub fn set_pauses(&mut self, issuance: Option<bool>, settlement: Option<bool>) {
        if let Some(v) = issuance {
            self.config.issuance_paused = v;
        }
        if let Some(v) = settlement {
            self.config.settlement_paused = v;
        }
        self.config.config_version += 1;
    }
}

/// The token lane's network must match the signing key's environment: the production
/// key signs for mainnet only, the test key for the testnet fixture only.
pub fn check_network_for_key(
    network: XrplNetwork,
    configured_key: &str,
) -> Result<(), XrplIcusdError> {
    require_collateral_key(configured_key).map_err(XrplIcusdError::Disabled)?;
    check_network_allowed(network).map_err(|e| XrplIcusdError::WrongNetwork(e.to_string()))?;
    if network != collateral_network_for_key(configured_key) {
        return Err(XrplIcusdError::WrongNetwork(format!(
            "network {} does not match signing key {configured_key}",
            network.name()
        )));
    }
    Ok(())
}

pub async fn configure(arg: ConfigureXrplIcusdArg) -> Result<PublicXrplIcusdConfig, XrplIcusdError> {
    let key = crate::chains::xrp::config::xrp_schnorr_key_name();
    let mut derived = None;
    if let Some(n) = arg.network {
        check_network_for_key(n, &key)?;
        let needs_derivation = read_state(|s| {
            s.xrpl_icusd.config.network != Some(n) || s.xrpl_icusd.config.issuer_address.is_none()
        });
        if needs_derivation {
            let (pk, addr) = ted25519::derive_xrp_address(issuer_derivation_path(n))
                .await
                .map_err(XrplIcusdError::Unavailable)?;
            debug_assert_eq!(addr, classic_address_from_ed25519_pubkey(&pk));
            derived = Some((addr, pk.to_vec()));
        }
    }
    let backend = ic_cdk::id();
    mutate_state(|s| {
        s.xrpl_icusd.apply_config_update(ConfigUpdate {
            network: arg.network,
            derived,
            policy: arg.policy,
            enabled: arg.enabled,
            outstanding_cap_e8s: arg.outstanding_cap_e8s,
            per_operation_cap_e8s: arg.per_operation_cap_e8s,
            rounding_buffer_e8s: arg.rounding_buffer_e8s,
            max_fee_drops: arg.max_fee_drops,
        })?;
        Ok(public_config(&s.xrpl_icusd, backend))
    })
}

pub fn set_pauses(arg: SetPausesArg) -> PublicXrplIcusdConfig {
    let backend = ic_cdk::id();
    mutate_state(|s| {
        s.xrpl_icusd
            .set_pauses(arg.issuance_paused, arg.settlement_paused);
        public_config(&s.xrpl_icusd, backend)
    })
}

/// Prove the ledger's minting account is the backend default account and not escrow,
/// then store it. Required before the lane can become ready.
pub async fn verify_minting_account() -> Result<Account, XrplIcusdError> {
    let acct = ledger::query_minting_account().await?;
    mutate_state(|s| {
        s.xrpl_icusd.config.verified_minting_account = Some(acct);
        s.xrpl_icusd.config.config_version += 1;
    });
    Ok(acct)
}

use icrc_ledger_types::icrc1::account::Account;

pub async fn run_issuer_setup_step() -> Result<IssuerSetupStatus, XrplIcusdError> {
    settlement::run_setup_step().await
}

pub async fn coverage_report() -> Result<CoverageReport, XrplIcusdError> {
    settlement::refresh_coverage().await
}

pub fn get_public_config() -> PublicXrplIcusdConfig {
    let backend = ic_cdk::id();
    read_state(|s| public_config(&s.xrpl_icusd, backend))
}

pub fn get_operations(caller: Principal, cursor: Option<u64>, limit: u32) -> OperationPage {
    if caller == Principal::anonymous() {
        return OperationPage {
            items: vec![],
            next_cursor: None,
        };
    }
    read_state(|s| owner_page(&s.xrpl_icusd, &caller, cursor, limit))
}

fn view_for(caller: &Principal, id: OperationId) -> Result<OperationView, XrplIcusdError> {
    read_state(|s| {
        s.xrpl_icusd
            .op_for_owner(caller, id)
            .map(operation_view)
    })
}

// ----------------------------------------------------------------------------------
// Operation driver
// ----------------------------------------------------------------------------------

/// Run every pending piece of one operation: ledger jobs (and their block proofs),
/// phase advancement, and the issuer delivery. Safe to call at any time: every step
/// is idempotent and loads its inputs from state.
pub async fn run_op_step(op_id: OperationId) -> Result<(), XrplIcusdError> {
    let job_count = read_state(|s| s.xrpl_icusd.op(op_id).map(|o| o.ic_jobs.len()))
        .ok_or(XrplIcusdError::NotFound)?;
    for idx in 0..job_count {
        let (resolved, done, verified) = read_state(|s| {
            s.xrpl_icusd
                .job(op_id, idx)
                .map(|j| (j.is_resolved(), j.done_block().is_some(), j.proof_verified))
                .unwrap_or((true, false, true))
        });
        if !resolved {
            // A busy/in-flight/transport error leaves the persisted job as-is.
            let _ = ledger::execute_job(op_id, idx).await;
        }
        let (done_now, verified_now) = read_state(|s| {
            s.xrpl_icusd
                .job(op_id, idx)
                .map(|j| (j.done_block().is_some(), j.proof_verified))
                .unwrap_or((done, verified))
        });
        if done_now && !verified_now {
            let _ = ledger::verify_done_job(op_id, idx).await;
        }
    }
    mutate_state(|s| {
        let st = &mut s.xrpl_icusd;
        let _ = issuance::advance_import(st, op_id, now());
        let _ = issuance::advance_resolution(st, op_id, now());
    });
    if let Ok(_g) = OpGuard::acquire(op_id) {
        let _ = settlement::drive_delivery(op_id).await;
    }
    mutate_state(|s| {
        let _ = issuance::advance_import(&mut s.xrpl_icusd, op_id, now());
    });
    Ok(())
}

pub async fn resume_operation(
    caller: Principal,
    op_id: OperationId,
) -> Result<OperationView, XrplIcusdError> {
    if caller == Principal::anonymous() {
        return Err(XrplIcusdError::AnonymousCaller);
    }
    // Ownership first: another principal's operation is simply not found.
    view_for(&caller, op_id)?;
    // Outcalls cost the canister cycles: only operations with work, and at most once
    // per gap per operation (the background tick keeps driving them regardless).
    let has_work = read_state(|s| s.xrpl_icusd.op(op_id).is_some_and(|o| o.needs_work()));
    if has_work && throttle_ok(THROTTLE_KIND_RESUME, &op_id.to_be_bytes(), now(), RESUME_GAP_NS) {
        // Reconciliation stays available while issuance or settlement is paused.
        run_op_step(op_id).await?;
    }
    ensure_xrpl_tick_scheduled();
    view_for(&caller, op_id)
}

// ----------------------------------------------------------------------------------
// Canonical -> XRPL issuance
// ----------------------------------------------------------------------------------

pub async fn issue_from_canonical(
    caller: Principal,
    arg: IssueFromCanonicalArg,
) -> Result<OperationView, XrplIcusdError> {
    if caller == Principal::anonymous() {
        return Err(XrplIcusdError::AnonymousCaller);
    }
    let nonce = issuance::parse_request_nonce(&arg.request_nonce)?;
    let kind = OperationKind::Import {
        amount_e8s: arg.amount_e8s,
        recipient: arg.recipient.clone(),
    };
    // An already-accepted request is idempotent, even while issuance is paused.
    let existing = read_state(|s| s.xrpl_icusd.admit(&caller, &nonce, arg.network, &kind));
    if let state::Admission::Existing(id) = existing? {
        let _ = run_op_step(id).await;
        ensure_xrpl_tick_scheduled();
        return view_for(&caller, id);
    }
    if !throttle_ok(THROTTLE_KIND_NEW, caller.as_slice(), now(), NEW_REQUEST_GAP_NS) {
        return Err(XrplIcusdError::LimitExceeded(
            "too many requests; retry in a few seconds".into(),
        ));
    }
    let issuer = read_state(|s| {
        s.xrpl_icusd.ensure_open_for_issuance()?;
        if Some(arg.network) != s.xrpl_icusd.config.network {
            return Err(XrplIcusdError::WrongNetwork(
                "request network differs from the configured network".into(),
            ));
        }
        s.xrpl_icusd.check_caps(arg.amount_e8s)?;
        s.xrpl_icusd
            .config
            .issuer_address
            .clone()
            .ok_or_else(|| XrplIcusdError::Disabled("issuer address not derived".into()))
    })?;
    issuance::validate_recipient(&arg.recipient, &issuer)?;
    // ---- awaits: nothing is persisted yet ----
    let asset = IssuedAsset::icusd(&issuer).map_err(|e| XrplIcusdError::Internal(e.to_string()))?;
    let proof = fetch_trust_line(arg.network, &arg.recipient, &asset)
        .await
        .map_err(|e| XrplIcusdError::Unavailable(e.to_string()))?;
    settlement::check_trust_line(proof.line.as_ref(), arg.amount_e8s)?;
    let tip = ledger::ledger_tip()
        .await
        .map_err(|e| XrplIcusdError::Unavailable(format!("ledger tip: {e}")))?;
    // ---- one synchronous commit: re-validate, then persist op + job together ----
    let backend = ic_cdk::id();
    let (id, created) = mutate_state(|s| {
        let op_nonce = s.next_op_nonce();
        issuance::create_import(
            &mut s.xrpl_icusd,
            &issuance::ImportRequest {
                owner: caller,
                recipient: arg.recipient.clone(),
                amount_e8s: arg.amount_e8s,
                network: arg.network,
                request_nonce: nonce,
            },
            op_nonce,
            tip,
            backend,
            now(),
        )
    })?;
    ensure_xrpl_tick_scheduled();
    if created {
        let _ = run_op_step(id).await;
    }
    view_for(&caller, id)
}

// ----------------------------------------------------------------------------------
// Owner claim resolution
// ----------------------------------------------------------------------------------

pub async fn resolve_claim(
    caller: Principal,
    arg: ResolveClaimArg,
) -> Result<OperationView, XrplIcusdError> {
    if caller == Principal::anonymous() {
        return Err(XrplIcusdError::AnonymousCaller);
    }
    let plan = read_state(|s| issuance::plan_resolution(&s.xrpl_icusd, &caller, &arg))?;
    match plan {
        issuance::ResolutionPlan::Reissue => {
            mutate_state(|s| issuance::apply_reissue(&mut s.xrpl_icusd, arg.operation_id, now()))?;
            ensure_xrpl_tick_scheduled();
        }
        issuance::ResolutionPlan::ReceiveCanonical { .. } => {
            let ledger_id = read_state(|s| s.icusd_ledger_principal);
            let fee = crate::management::get_or_refresh_fee(ledger_id)
                .await
                .map_err(XrplIcusdError::Unavailable)?;
            let balance = ledger::escrow_balance_e8s()
                .await
                .map_err(XrplIcusdError::Unavailable)?;
            let tip = ledger::ledger_tip()
                .await
                .map_err(|e| XrplIcusdError::Unavailable(format!("ledger tip: {e}")))?;
            let idx = mutate_state(|s| {
                // Re-plan after the awaits: the claim may have changed meanwhile.
                issuance::plan_resolution(&s.xrpl_icusd, &caller, &arg)?;
                let op_nonce = s.next_op_nonce();
                issuance::begin_receive_canonical(
                    &mut s.xrpl_icusd,
                    arg.operation_id,
                    fee,
                    balance,
                    op_nonce,
                    tip,
                    now(),
                )
            })?;
            let _ = ledger::execute_job(arg.operation_id, idx).await;
            let _ = ledger::verify_done_job(arg.operation_id, idx).await;
            mutate_state(|s| {
                let _ = issuance::advance_resolution(&mut s.xrpl_icusd, arg.operation_id, now());
            });
            ensure_xrpl_tick_scheduled();
        }
    }
    view_for(&caller, arg.operation_id)
}

// ----------------------------------------------------------------------------------
// Background tick (idle-free)
// ----------------------------------------------------------------------------------

pub const TICK_ACTIVE_SECS: u64 = 30;
pub const TICK_MAX_SECS: u64 = 300;
pub const TICK_ITEMS_PER_PASS: usize = 6;

/// Delay until the next pass, or `None` when idle (the timer then stops entirely).
pub fn next_tick_delay(has_work: bool, progressed: bool, current_secs: u64) -> Option<u64> {
    if !has_work {
        return None;
    }
    if progressed {
        return Some(TICK_ACTIVE_SECS);
    }
    Some(
        current_secs
            .saturating_mul(2)
            .clamp(TICK_ACTIVE_SECS, TICK_MAX_SECS),
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TickItem {
    Op(OperationId),
    Setup,
}

/// Which operations (and the setup attempt) the next pass should touch: at most
/// `max`, in id order starting after `after` and wrapping.
pub fn plan_tick(st: &XrplIcusdState, max: usize, after: OperationId) -> Vec<TickItem> {
    let mut items = Vec::new();
    if st
        .setup_attempt
        .as_ref()
        .is_some_and(|a| a.status.holds_lease())
    {
        items.push(TickItem::Setup);
    }
    let later = st.operations.range(after.saturating_add(1)..);
    let earlier = st.operations.range(..=after);
    for (id, op) in later.chain(earlier) {
        if items.len() >= max {
            break;
        }
        if op.needs_work() {
            items.push(TickItem::Op(*id));
        }
    }
    items
}

fn progress_fingerprint(st: &XrplIcusdState) -> (u64, u64, bool) {
    (
        st.operations
            .values()
            .map(|o| o.updated_at_ns)
            .max()
            .unwrap_or(0),
        st.config.config_version,
        st.issuer_lease.is_some(),
    )
}

/// Arm the one-shot tick if work exists and none is pending. Called on operation
/// creation/resume and from `init`/`post_upgrade`.
pub fn ensure_xrpl_tick_scheduled() {
    if !read_state(|s| s.xrpl_icusd.has_work()) {
        return;
    }
    schedule_tick(TICK_ACTIVE_SECS);
}

fn schedule_tick(secs: u64) {
    if TICK_SCHEDULED.with(|c| c.replace(true)) {
        return;
    }
    #[cfg(target_arch = "wasm32")]
    {
        ic_cdk_timers::set_timer(std::time::Duration::from_secs(secs), || {
            TICK_SCHEDULED.with(|c| c.set(false));
            ic_cdk::spawn(xrpl_icusd_tick());
        });
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // No timers off-canister: tests drive `xrpl_icusd_tick` directly.
        let _ = secs;
        TICK_SCHEDULED.with(|c| c.set(false));
    }
}

#[must_use]
struct TickGuard;

impl TickGuard {
    fn acquire() -> Option<Self> {
        if TICK_RUNNING.with(|c| c.replace(true)) {
            None
        } else {
            Some(TickGuard)
        }
    }
}

impl Drop for TickGuard {
    fn drop(&mut self) {
        TICK_RUNNING.with(|c| c.set(false));
    }
}

/// One background pass. Reschedules itself only while work remains: 30 s while it
/// makes progress, backing off to 5 minutes, and stopping when idle. Reconciliation
/// of existing operations runs even while issuance/settlement are paused.
pub async fn xrpl_icusd_tick() {
    let Some(_guard) = TickGuard::acquire() else {
        return;
    };
    let before = read_state(|s| progress_fingerprint(&s.xrpl_icusd));
    let plan = read_state(|s| {
        plan_tick(
            &s.xrpl_icusd,
            TICK_ITEMS_PER_PASS,
            TICK_CURSOR.with(Cell::get),
        )
    });
    for item in plan {
        match item {
            TickItem::Setup => {
                let _ = settlement::drive_attempt(LeaseHolder::Setup).await;
            }
            TickItem::Op(id) => {
                TICK_CURSOR.with(|c| c.set(id));
                let _ = run_op_step(id).await;
            }
        }
    }
    let after = read_state(|s| progress_fingerprint(&s.xrpl_icusd));
    let has_work = read_state(|s| s.xrpl_icusd.has_work());
    let delay = next_tick_delay(has_work, before != after, TICK_DELAY_SECS.with(Cell::get));
    match delay {
        Some(d) => {
            TICK_DELAY_SECS.with(|c| c.set(d));
            drop(_guard);
            schedule_tick(d);
        }
        None => TICK_DELAY_SECS.with(|c| c.set(TICK_ACTIVE_SECS)),
    }
}

/// Upgrade hook: normalise in-flight jobs and re-arm the tick if work remains.
pub fn on_post_upgrade() {
    mutate_state(|s| s.xrpl_icusd.recover_after_upgrade());
    ensure_xrpl_tick_scheduled();
}
