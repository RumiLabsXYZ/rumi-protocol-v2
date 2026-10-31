//! ICP -> Sui deposits (53a): admission, the ICRC-2 pull into escrow, the ICP refund and fee
//! sweeps. The mint itself is built and sent by the mint lane (53b); this module owns everything
//! before and around it. Ported from the frozen Solana instance (`flows/deposit.rs` at 4394b472).
//!
//! Guarantees carried from the Base and Solana instances, each pinned by a test:
//!
//! * **caller binding.** The pull's `from` account is always `(caller, from_subaccount)` and the
//!   spender is this canister's default account: a deposit can only ever move the caller's own
//!   funds, and only up to an allowance the caller granted to this canister.
//! * **allowance and balance hints** are checked before anything is stored, so a caller without
//!   funds creates no record and causes no background work. They are hints; the ICRC-2 pull stays
//!   the authority.
//! * **request-key idempotency.** `(caller, client_request_id)` names one deposit forever. A repeat
//!   with identical arguments returns it; different arguments are `RequestIdConflict`.
//! * **the tuple is stored before the first send** (see `ledger_ops`), including after an upgrade.
//! * **fees stay disjoint.** The deposit fee is carved out of the minted amount at delivery
//!   (`MintDelivered { amount, fee }`), never out of the pull; the ledger fee is paid by the
//!   depositor on the pull and by the escrow on refunds and sweeps (reserved in `unknown_outflow`
//!   before the send); the release fee is carved out of a release. Operating gas on Sui is paid by
//!   the `gas` role from its own balance and never touches these buckets.
//! * **a refund needs proof that no mint can exist.** Only a deposit whose mint operation was never
//!   built can be refunded (53b extends this to operations the lane's own records prove can never
//!   land). An operation in flight, held with a signed attempt that is not proven undelivered, or
//!   delivered never can: an admin's reason, a timeout, a passing epoch or a missing digest is not
//!   proof.
//! * **the recipient is checked before the pull**, against the configured gate and the role
//!   addresses, so no deposit is admitted that the mint could never deliver.

use super::ledger_ops::{self, OpRef};
use crate::accounting::Transition;
use crate::admin;
use crate::env;
use crate::icp::IcpLedger;
use crate::ids::{self, Hash32};
use crate::state::{
    self, AuditCtx, DepositKind, DepositRecord, DepositState, LedgerOp, LedgerOpKind, LedgerOpStatus, SweepRecord,
};
use crate::sui::{parse_address_strict, SuiAddress};
use crate::types::*;
use crate::views;
use candid::Principal;
use icrc_ledger_types::icrc1::account::Account;

pub const MAX_PULL_RECONCILES_PER_TICK: usize = 10;
pub const MAX_REFUNDS_PER_TICK: usize = 5;
/// Automatic retry delay for holds caused by preflight conditions (pause, cap, lane halt).
pub const PREFLIGHT_HOLD_RETRY_NS: u64 = 10 * 60 * 1_000_000_000;

// ---------------------------------------------------------------------------
// Admission
// ---------------------------------------------------------------------------

fn parse_subaccount(s: &Option<serde_bytes::ByteBuf>) -> BridgeResult<Option<[u8; 32]>> {
    match s {
        None => Ok(None),
        Some(b) if b.len() == 32 => {
            let mut a = [0u8; 32];
            a.copy_from_slice(b);
            Ok(Some(a))
        }
        Some(_) => Err(BridgeError::InvalidArgument("from_subaccount must be 32 bytes".into())),
    }
}

/// Addresses below this bound are the Sui system range (`0x1` the Move stdlib, `0x2` the framework,
/// `0x6` the clock, `0xc` the coin registry, and so on). No wallet lives there; a coin sent there
/// is a loss, so a deposit to one is refused. A policy bound, not a Sui rule.
const RESERVED_ADDRESS_LIMIT: u64 = 0x1_0000;

/// A `0x` plus 64 lowercase hex Sui address that can receive the coin: not zero, not in the system
/// range, and none of the bridge's own objects or role addresses.
pub fn parse_recipient(text: &str) -> BridgeResult<SuiAddress> {
    let a = parse_address_strict(text).map_err(|_| BridgeError::InvalidRecipient("a Sui address is 0x followed by 64 lowercase hex digits".into()))?;
    if a[..24] == [0u8; 24] && u64::from_be_bytes(a[24..].try_into().unwrap_or([0; 8])) < RESERVED_ADDRESS_LIMIT {
        return Err(BridgeError::InvalidRecipient("the zero address and the Sui system range cannot receive the coin".into()));
    }
    let (binding, roles) = crate::state::read_core(|c| (c.binding.clone(), c.roles.clone()));
    if let Some(b) = binding {
        let ours = [("package", b.package), ("gate", b.gate), ("mint cap", b.mint_cap), ("admin cap", b.admin_cap)];
        if let Some((name, _)) = ours.iter().find(|(_, id)| *id == a) {
            return Err(BridgeError::InvalidRecipient(format!("the recipient is the bridge's own {name}")));
        }
    }
    if roles.iter().any(|k| crate::sui::address_from_ed25519(&k.public_key) == a) {
        return Err(BridgeError::InvalidRecipient("the recipient is one of the bridge's own role addresses".into()));
    }
    Ok(a)
}

fn same_request(d: &DepositRecord, args: &BridgeToSuiArgs) -> BridgeResult<bool> {
    let sub = parse_subaccount(&args.from_subaccount)?;
    let recipient = parse_address_strict(&args.sui_recipient).map_err(|_| BridgeError::InvalidRecipient("not a Sui address".into()))?;
    Ok(d.amount_e8s == args.amount_e8s
        && d.sui_recipient == recipient
        && d.from.map(|a| a.subaccount) == Some(sub)
        && d.deposit_fee_e8s == args.expected_deposit_fee_e8s)
}

/// The mint id of a user deposit once its pull block is known. `None` for a re-mint (its id comes
/// from the claim) or while the gate binding is not configured.
pub fn mint_id_for(d: &DepositRecord, pull_block: u64) -> Option<Hash32> {
    if !matches!(d.kind, DepositKind::UserDeposit) {
        return d.mint_id;
    }
    let (chain, binding, salt) = state::read_core(|c| (c.chain_identifier, c.binding.clone(), c.instance_salt));
    let b = binding?;
    Some(ids::mint_id(&chain, &b.package, &b.gate, &salt, pull_block, &d.sui_recipient, d.mint_amount_e8s))
}

/// Why new deposits cannot be admitted yet, if they cannot: the exact gate identities and the three
/// role keys must be configured so the mint id and the mint transaction can be built, and the
/// providers must have agreed (`observe_deployment`) that the configured deployment is what it
/// claims to be. A deposit is never pulled into escrow for a deployment nobody has observed.
pub fn deployment_not_ready() -> Option<String> {
    let (binding, roles, observed) = state::read_core(|c| (c.binding.is_some(), c.roles.len(), state::read_runtime(|r| r.observed.is_some())));
    if !binding {
        return Some("the gate binding is not configured".into());
    }
    if roles < crate::ids::SuiRole::ALL.len() {
        return Some("the role keys are not derived".into());
    }
    if !observed {
        return Some("the deployment has not been observed by the providers".into());
    }
    None
}

/// Validate and persist a new deposit in `PullPending`. Pure state, no awaits, so it is atomic.
/// Returns the stored record.
pub fn admit(caller: Principal, args: &BridgeToSuiArgs) -> BridgeResult<DepositRecord> {
    admit_inner(caller, args, false).and_then(|d| d.ok_or_else(|| BridgeError::Internal("admit persisted no record".into())))
}

/// Run every admission check without persisting anything.
pub fn validate_admission(caller: Principal, args: &BridgeToSuiArgs) -> BridgeResult<()> {
    admit_inner(caller, args, true).map(|_| ())
}

fn admit_inner(caller: Principal, args: &BridgeToSuiArgs, dry_run: bool) -> BridgeResult<Option<DepositRecord>> {
    let rkey = state::request_key(&caller, &args.client_request_id).map_err(BridgeError::InvalidArgument)?;
    if state::lookup_request(&rkey).is_some() {
        return Err(BridgeError::RequestIdConflict);
    }
    let subaccount = parse_subaccount(&args.from_subaccount)?;
    let now = env::now_ns();
    let canister = env::canister_id();
    let a = args.amount_e8s;

    let core = state::read_runtime(|c| c.clone());
    if !core.initialized {
        return Err(BridgeError::NotConfigured("not initialized".into()));
    }
    if let Some(h) = &core.halted {
        return Err(BridgeError::Halted(h.reason.clone()));
    }
    if core.config.pauses.deposits {
        return Err(BridgeError::Paused("deposits are paused".into()));
    }
    // A deposit is admitted only once the gate binding and the role keys exist: its recipient can be
    // checked against them, and its mint id and mint transaction can be built.
    if let Some(why) = deployment_not_ready() {
        return Err(BridgeError::NotConfigured(format!("the Sui deployment is not ready: {why}")));
    }
    let recipient = parse_recipient(&args.sui_recipient)?;
    let limits = &core.config.limits;
    if a < limits.min_deposit_e8s {
        return Err(BridgeError::AmountBelowMinimum { min_e8s: limits.min_deposit_e8s });
    }
    if a > limits.max_deposit_e8s {
        return Err(BridgeError::AmountAboveMaximum { max_e8s: limits.max_deposit_e8s });
    }
    let fee = core.config.fees.deposit_fee_e8s;
    if args.expected_deposit_fee_e8s != fee {
        return Err(BridgeError::FeeChanged { current_fee_e8s: fee });
    }
    let mint_amount = a
        .checked_sub(fee)
        .filter(|m| *m > 0)
        .ok_or_else(|| BridgeError::InvalidArgument("amount does not exceed the deposit fee".into()))?;

    // Capacity: everything that is, or may become, Sui supply, including held claims that will
    // be re-minted without admission and pulls whose outcome is not proven yet.
    let exposure = core
        .buckets
        .destination_exposure()
        .ok()
        .and_then(|e| e.checked_add(core.buckets.held))
        .and_then(|e| e.checked_add(core.buckets.reserved_pulls))
        .ok_or_else(|| BridgeError::Internal("capacity overflow".into()))?;
    let cap = limits.max_outstanding_e8s as u128;
    if exposure.saturating_add(a as u128) > cap {
        return Err(BridgeError::CapacityExceeded { available_e8s: cap.saturating_sub(exposure).min(u64::MAX as u128) as u64 });
    }

    // Window.
    let mut windows_core = core.clone();
    windows_core.roll_window(now);
    let used = windows_core.windows.admitted_in_window;
    let wlimit = limits.mint_window_limit_e8s as u128;
    if used.saturating_add(a as u128) > wlimit {
        return Err(BridgeError::RateLimited {
            available_e8s: wlimit.saturating_sub(used).min(u64::MAX as u128) as u64,
            window_resets_at_ns: windows_core.window_resets_at_ns(),
        });
    }

    // Active operation limits. Re-mints of held claims are not user admissions and must not crowd
    // them out.
    if state::active_user_deposit_count() >= limits.max_active_global as u64 {
        return Err(BridgeError::TooManyActiveOperations);
    }
    if state::active_deposits_owned_by(&caller) >= limits.max_active_per_caller as usize {
        return Err(BridgeError::TooManyActiveOperations);
    }

    if dry_run {
        return Ok(None);
    }

    // Persist.
    let id = state::mutate_runtime(|c| {
        c.roll_window(now);
        c.windows.admitted_in_window = c.windows.admitted_in_window.saturating_add(a as u128);
        let id = c.next_deposit_id;
        c.next_deposit_id += 1;
        id
    });
    // The pull moves the CALLER's funds: `from` is the caller's own account, never anything the
    // arguments name, and the spender is this canister's default account.
    let from = Account { owner: caller, subaccount };
    let escrow = core.escrow_account(canister);
    let pull = LedgerOp {
        kind: LedgerOpKind::Pull,
        from,
        to: escrow,
        spender: Some(Account { owner: canister, subaccount: None }),
        amount_e8s: a,
        fee_e8s: None,
        memo: ids::pull_memo(id),
        created_at_time: now,
        attempts: 0,
        ambiguous_seen: false,
        status: LedgerOpStatus::Pending,
        search_from_block: core.ledger_tip_lower_bound,
        search_next_block: None,
        last_attempt_ns: None,
        outflow_reserved: false,
    };
    let d = DepositRecord {
        id,
        kind: DepositKind::UserDeposit,
        owner: caller,
        from: Some(from),
        client_request_id: args.client_request_id.to_vec(),
        sui_recipient: recipient,
        amount_e8s: a,
        deposit_fee_e8s: fee,
        mint_amount_e8s: mint_amount,
        mint_id: None,
        state: DepositState::PullPending,
        pull: Some(pull),
        refund: None,
        mint_op: None,
        abandoned_mint_ops: vec![],
        delivered: None,
        hold_retry_at_ns: None,
        created_at_ns: now,
        updated_at_ns: now,
    };
    state::apply_transition(Transition::AdmitDeposit { amount: a }, AuditCtx::deposit(id, "admitted"))
        .map_err(|e| BridgeError::Internal(e.to_string()))?;
    state::put_deposit(&d);
    state::index_owner_deposit(&caller, id);
    state::insert_request(rkey, id);
    Ok(Some(d))
}

/// The ledger fee, cached in the core and fetched on first use.
pub async fn ledger_fee<L: IcpLedger>(ledger: &L) -> BridgeResult<u64> {
    if let Some(f) = state::read_runtime(|c| c.ledger_fee_e8s) {
        return Ok(f);
    }
    let f = ledger.fee().await.map_err(BridgeError::Ledger)?;
    state::mutate_runtime(|c| c.ledger_fee_e8s = Some(f));
    Ok(f)
}

/// Check the caller's allowance and balance before anything is persisted. This is a hint only: the
/// ICRC-2 pull remains the authority. An allowance that expires within a minute is treated as
/// missing.
async fn precheck_funds<L: IcpLedger>(ledger: &L, caller: Principal, args: &BridgeToSuiArgs) -> BridgeResult<()> {
    let subaccount = parse_subaccount(&args.from_subaccount)?;
    let canister = env::canister_id();
    let fee = ledger_fee(ledger).await?;
    let required = args.amount_e8s.checked_add(fee).ok_or_else(|| BridgeError::InvalidArgument("amount overflow".into()))?;
    let from = Account { owner: caller, subaccount };
    let spender = Account { owner: canister, subaccount: None };
    let (allowance, expires_at) = ledger.allowance(from, spender).await.map_err(BridgeError::Ledger)?;
    let live = expires_at.map(|e| e > env::now_ns().saturating_add(60 * 1_000_000_000)).unwrap_or(true);
    let usable = if live { allowance } else { 0 };
    if usable < required as u128 {
        return Err(BridgeError::InsufficientAllowance { allowance_e8s: usable.min(u64::MAX as u128) as u64, required_e8s: required });
    }
    let balance = ledger.balance_of(from).await.map_err(BridgeError::Ledger)?;
    if balance < required as u128 {
        return Err(BridgeError::InsufficientFunds { balance_e8s: balance.min(u64::MAX as u128) as u64, required_e8s: required });
    }
    Ok(())
}

fn existing_view(rkey: &state::RequestKey, args: &BridgeToSuiArgs) -> BridgeResult<Option<DepositView>> {
    let Some(existing) = state::lookup_request(rkey) else { return Ok(None) };
    let d = state::get_deposit(existing).ok_or_else(|| BridgeError::Internal("request index points to a missing deposit".into()))?;
    if same_request(&d, args)? {
        Ok(Some(views::deposit_view(&d)))
    } else {
        Err(BridgeError::RequestIdConflict)
    }
}

/// `bridge_to_sui`: admit, pull, and hand the rest to the timer.
pub async fn bridge_to_sui<L: IcpLedger>(ledger: &L, args: BridgeToSuiArgs) -> BridgeResult<DepositView> {
    let caller = admin::require_authenticated()?;
    let rkey = state::request_key(&caller, &args.client_request_id).map_err(BridgeError::InvalidArgument)?;
    if let Some(v) = existing_view(&rkey, &args)? {
        return Ok(v);
    }
    // Cheap checks first, then the funds pre-check, then re-check idempotency (a concurrent
    // identical call may have been admitted during the awaits) and admit.
    validate_admission(caller, &args)?;
    precheck_funds(ledger, caller, &args).await?;
    if let Some(v) = existing_view(&rkey, &args)? {
        return Ok(v);
    }
    let d = admit(caller, &args)?;
    let r = OpRef::Pull(d.id);
    if let Some(sent) = ledger_ops::attempt(ledger, &r).await {
        ledger_ops::apply_outcome(&r, sent);
    }
    let d = state::get_deposit(d.id).ok_or_else(|| BridgeError::Internal("deposit vanished".into()))?;
    if matches!(d.state, DepositState::PullPending) {
        // Retryable no-effect outcome: mark visibly so the timer resends.
        let mut d2 = d.clone();
        d2.state = DepositState::PullUnknown;
        state::put_deposit(&d2);
    }
    let d = state::get_deposit(d.id).unwrap_or(d);
    // Only work that is still in progress needs the background tick.
    if !d.state.is_terminal() {
        crate::timer::schedule_soon();
    }
    Ok(views::deposit_view(&d))
}

/// Timer: drive pulls that are unknown or whose attempt went stale. A pass takes its pulls from the
/// next window of the active index (`state::next_deposits`), so a pull whose outcome cannot be proven
/// yet (an archive that is unavailable) delays the pulls behind it by one rotation at most; it is
/// neither skipped forever nor reset nor marked settled by being looked at.
pub async fn reconcile_pulls<L: IcpLedger>(ledger: &L) {
    let now = env::now_ns();
    let candidates = state::next_deposits(
        state::Queue::Pulls,
        MAX_PULL_RECONCILES_PER_TICK,
        |d| match d.state {
            DepositState::PullUnknown => true,
            DepositState::PullPending => d
                .pull
                .as_ref()
                .and_then(|p| p.last_attempt_ns)
                .map(|t| now.saturating_sub(t) >= ledger_ops::STALE_ATTEMPT_NS)
                .unwrap_or(true),
            _ => false,
        },
    );
    for d in candidates {
        let r = OpRef::Pull(d.id);
        ledger_ops::reconcile(ledger, &r).await;
        // A retryable no-effect on reconcile leaves the op Pending; show it as unknown so the user
        // sees it is not final.
        if let Some(mut cur) = state::get_deposit(d.id) {
            if matches!(cur.state, DepositState::PullPending) && cur.pull.as_ref().map(|p| p.attempts > 0).unwrap_or(false) {
                cur.state = DepositState::PullUnknown;
                state::put_deposit(&cur);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Preflight holds and the admin retry
// ---------------------------------------------------------------------------

/// A preflight check (a pause, the cap, a halted lane, no free gas coin) blocked building the
/// mint. No mint operation exists, so the deposit may be retried or refunded. Holds retry
/// automatically after a delay.
pub fn on_mint_preflight_hold(deposit_id: u64, reason: String) {
    let Some(mut d) = state::get_deposit(deposit_id) else { return };
    if !matches!(d.state, DepositState::Escrowed) || d.mint_op.is_some() {
        return;
    }
    let now = env::now_ns();
    d.state = DepositState::MintHeld { reason: reason.clone() };
    d.hold_retry_at_ns = Some(now.saturating_add(PREFLIGHT_HOLD_RETRY_NS));
    d.updated_at_ns = now;
    state::put_deposit(&d);
    state::append_audit(None, AuditCtx::deposit(deposit_id, format!("mint held before building: {reason}")));
}

/// Timer: return preflight holds whose retry time passed to `Escrowed`. A deposit that has an
/// abandoned mint operation is never returned (it can only be refunded).
pub fn retry_due_holds() {
    let now = env::now_ns();
    for mut d in state::next_deposits(state::Queue::Holds, 50, |d| {
        matches!(d.state, DepositState::MintHeld { .. })
            && d.mint_op.is_none()
            && d.abandoned_mint_ops.is_empty()
            && d.hold_retry_at_ns.map(|t| now >= t).unwrap_or(false)
    }) {
        d.state = DepositState::Escrowed;
        d.hold_retry_at_ns = None;
        d.updated_at_ns = now;
        state::put_deposit(&d);
    }
}

/// Admin: put a held deposit back in the mint queue (same mint id, same single mint operation).
pub fn retry_held(id: u64) -> BridgeResult<DepositView> {
    let mut d = state::get_deposit(id).ok_or(BridgeError::NotFound)?;
    if !matches!(d.state, DepositState::MintHeld { .. }) || d.mint_op.is_some() {
        return Err(BridgeError::InvalidState("deposit is not held without a mint operation".into()));
    }
    // A deposit whose earlier operations are all proven dead (finalized failures or never signed) may be
    // minted again with the SAME mint id; anything less is refused.
    crate::flows::mint::prior_ops_provably_dead(&d).map_err(|why| BridgeError::InvalidState(format!("not retryable: {why}")))?;
    d.state = DepositState::Escrowed;
    d.hold_retry_at_ns = None;
    d.updated_at_ns = env::now_ns();
    state::put_deposit(&d);
    state::append_audit(None, AuditCtx::deposit(id, "admin retry of held mint"));
    crate::timer::schedule_soon();
    Ok(views::deposit_view(&d))
}

// ---------------------------------------------------------------------------
// Inline attempt shared by refunds and fee sweeps
// ---------------------------------------------------------------------------

/// Send the one inline attempt of a refund or fee sweep that was just persisted, apply its
/// outcome, then hand any unresolved operation to the background tick.
async fn send_inline_attempt<L: IcpLedger>(ledger: &L, r: &OpRef) {
    if let Some(sent) = ledger_ops::attempt(ledger, r).await {
        ledger_ops::apply_outcome(r, sent);
    }
    // Runs whether or not an attempt could be made: `attempt` returns `None` when it could not
    // reserve the outflow (the bridge halts), and the persisted operation still needs its
    // follow-up once the halt is cleared.
    rearm_if_unresolved(r);
}

/// Arm a tick within about a second when the operation is not terminal (reply lost, ambiguous
/// outcome, `Unknown` status, a retryable no-effect reply, or no attempt could be made), instead of
/// waiting out the idle interval. Synchronous; it never allocates, rebuilds or resends anything.
fn rearm_if_unresolved(r: &OpRef) {
    if ledger_ops::load_op(r).is_some_and(|op| !ledger_ops::is_terminal(&op)) {
        crate::timer::schedule_soon();
    }
}

// ---------------------------------------------------------------------------
// Refunds
// ---------------------------------------------------------------------------

/// A user deposit is refundable only while it is held with NO live mint operation, and every mint
/// operation it ever had was abandoned by an admin AND is provably unable to deliver. (A deposit
/// whose mint was never built has none.)
fn refund_preconditions(d: &DepositRecord, caller: Principal) -> BridgeResult<()> {
    if d.owner != caller {
        return Err(BridgeError::NotOwner);
    }
    if !matches!(d.kind, DepositKind::UserDeposit) {
        return Err(BridgeError::InvalidState("re-mints cannot be refunded to ICP".into()));
    }
    if !matches!(d.state, DepositState::MintHeld { .. }) || d.mint_op.is_some() {
        return Err(BridgeError::InvalidState("only a held deposit with no Sui mint operation in flight can be refunded".into()));
    }
    mint_operations_allow_refund(d)
}

/// The refund proof over EVERY mint operation the deposit ever had: [`crate::flows::mint::prior_ops_provably_dead`].
/// A deposit that never had an operation passes. An abandoned, unknown or merely expired signed
/// operation never does: only a finalized failed receipt for every signed attempt (with the mint id
/// still unconsumed) or a proof that no complete signature can exist.
fn mint_operations_allow_refund(d: &DepositRecord) -> BridgeResult<()> {
    crate::flows::mint::prior_ops_provably_dead(d).map_err(|why| BridgeError::InvalidState(format!("not refundable: {why}")))
}

pub async fn request_refund<L: IcpLedger>(ledger: &L, id: u64) -> BridgeResult<DepositView> {
    let caller = admin::require_authenticated()?;
    let d = state::get_deposit(id).ok_or(BridgeError::NotFound)?;
    refund_preconditions(&d, caller)?;
    if state::is_halted() {
        return Err(BridgeError::Halted("bridge is halted".into()));
    }
    let fee = ledger_fee(ledger).await?;

    // Re-check after the await.
    let mut d = state::get_deposit(id).ok_or(BridgeError::NotFound)?;
    refund_preconditions(&d, caller)?;
    if state::is_halted() {
        return Err(BridgeError::Halted("bridge is halted".into()));
    }
    if d.amount_e8s <= fee {
        return Err(BridgeError::InvalidState("deposit does not cover the ledger fee".into()));
    }
    let now = env::now_ns();
    let canister = env::canister_id();
    let escrow = state::read_runtime(|c| c.escrow_account(canister));
    let to = d.from.ok_or_else(|| BridgeError::Internal("user deposit without source account".into()))?;
    state::apply_transition(Transition::RefundStarted { amount: d.amount_e8s }, AuditCtx::deposit(id, "refund started"))
        .map_err(|e| BridgeError::Internal(e.to_string()))?;
    d.refund = Some(LedgerOp {
        kind: LedgerOpKind::Refund,
        from: escrow,
        to,
        spender: None,
        amount_e8s: d.amount_e8s - fee,
        fee_e8s: Some(fee),
        memo: ids::refund_ledger_memo(id),
        created_at_time: now,
        attempts: 0,
        ambiguous_seen: false,
        status: LedgerOpStatus::Pending,
        search_from_block: state::read_runtime(|c| c.ledger_tip_lower_bound),
        search_next_block: None,
        last_attempt_ns: None,
        outflow_reserved: false,
    });
    d.state = DepositState::Refunding;
    d.hold_retry_at_ns = None;
    d.updated_at_ns = now;
    state::put_deposit(&d);

    send_inline_attempt(ledger, &OpRef::Refund(id)).await;
    let d = state::get_deposit(id).ok_or(BridgeError::NotFound)?;
    Ok(views::deposit_view(&d))
}

/// Timer: finish refunds that are pending or unknown. Same fair window as the pulls
/// (`state::next_deposits`): a refund whose outcome cannot be proven yet delays the ones behind it by
/// one rotation at most, and its held funds stay exactly as they are (outflow in doubt, tuple intact).
pub async fn process_refunds<L: IcpLedger>(ledger: &L) {
    if state::is_halted() {
        return;
    }
    for d in state::next_deposits(state::Queue::Refunds, MAX_REFUNDS_PER_TICK, |d| matches!(d.state, DepositState::Refunding)) {
        ledger_ops::reconcile(ledger, &OpRef::Refund(d.id)).await;
    }
}

// ---------------------------------------------------------------------------
// Fee sweeps
// ---------------------------------------------------------------------------

pub async fn sweep_fees<L: IcpLedger>(ledger: &L, amount_e8s: u64) -> BridgeResult<u64> {
    if state::is_halted() {
        return Err(BridgeError::Halted("bridge is halted".into()));
    }
    let fee = ledger.fee().await.map_err(BridgeError::Ledger)?;
    state::mutate_runtime(|c| c.ledger_fee_e8s = Some(fee));
    let canister = env::canister_id();
    let now = env::now_ns();
    let id = state::mutate_runtime(|c| -> BridgeResult<u64> {
        let recipient = c.config.fee_recipient.ok_or_else(|| BridgeError::NotConfigured("no fee recipient".into()))?;
        if c.sweeps.iter().any(|s| !ledger_ops::is_terminal(&s.op)) {
            return Err(BridgeError::InvalidState("a fee sweep is already in progress".into()));
        }
        if (amount_e8s as u128) > c.buckets.fees {
            return Err(BridgeError::InvalidArgument(format!("only {} e8s of fees are available", c.buckets.fees)));
        }
        if amount_e8s <= fee {
            return Err(BridgeError::InvalidArgument("sweep amount must exceed the ledger fee".into()));
        }
        let id = c.next_sweep_id;
        c.next_sweep_id += 1;
        let op = LedgerOp {
            kind: LedgerOpKind::Sweep,
            from: c.escrow_account(canister),
            to: recipient,
            spender: None,
            amount_e8s: amount_e8s - fee,
            fee_e8s: Some(fee),
            memo: ids::sweep_memo(id),
            created_at_time: now,
            attempts: 0,
            ambiguous_seen: false,
            status: LedgerOpStatus::Pending,
            search_from_block: c.ledger_tip_lower_bound,
            search_next_block: None,
            last_attempt_ns: None,
            outflow_reserved: false,
        };
        c.sweeps.push(SweepRecord { id, amount_e8s, op, created_at_ns: now });
        if c.sweeps.len() > state::MAX_SWEEPS {
            if let Some(pos) = c.sweeps.iter().position(|s| ledger_ops::is_terminal(&s.op)) {
                c.sweeps.remove(pos);
            }
        }
        Ok(id)
    })?;
    state::append_audit(None, AuditCtx::note(format!("fee sweep {id} of {amount_e8s} e8s started")));
    send_inline_attempt(ledger, &OpRef::Sweep(id)).await;
    Ok(id)
}

pub async fn process_sweeps<L: IcpLedger>(ledger: &L) {
    if state::is_halted() {
        return;
    }
    let ids: Vec<u64> = state::read_runtime(|c| c.sweeps.iter().filter(|s| !ledger_ops::is_terminal(&s.op)).map(|s| s.id).collect());
    for id in ids {
        ledger_ops::reconcile(ledger, &OpRef::Sweep(id)).await;
    }
}
