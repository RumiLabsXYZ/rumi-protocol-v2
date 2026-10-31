//! ICP -> Base deposits, held-claim re-mints, refunds and fee sweeps.

use super::ledger_ops::{self, OpRef};
use crate::accounting::Transition;
use crate::admin;
use crate::env;
use crate::evm::abi::{self, ViewCall};
use crate::evm::rpc::{self, BlockTag, Receipt};
use crate::ids;
use crate::ledger;
use crate::state::{
    self, AuditCtx, DepositKind, DepositRecord, DepositState, Inclusion, LedgerOp, LedgerOpKind, LedgerOpStatus,
    ReleaseState, SweepRecord, TxStatus,
};
use crate::types::*;
use crate::views;
use icrc_ledger_types::icrc1::account::Account;

pub const MAX_PULL_RECONCILES_PER_TICK: usize = 10;
pub const MAX_REFUNDS_PER_TICK: usize = 5;
/// Automatic retry delay for holds caused by preflight conditions (pause, cap, gas).
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

fn same_request(d: &DepositRecord, args: &BridgeToBaseArgs) -> BridgeResult<bool> {
    let sub = parse_subaccount(&args.from_subaccount)?;
    let recipient = ids::parse_address(&args.base_recipient).map_err(BridgeError::InvalidRecipient)?;
    Ok(d.amount_e8s == args.amount_e8s
        && d.base_recipient == recipient
        && d.from.map(|a| a.subaccount) == Some(sub)
        && d.deposit_fee_e8s == args.expected_deposit_fee_e8s)
}

/// Validate and persist a new deposit in `PullPending`. Pure state, no awaits, so it is
/// atomic. Returns the stored record.
pub fn admit(caller: candid::Principal, args: &BridgeToBaseArgs) -> BridgeResult<DepositRecord> {
    admit_inner(caller, args, false).map(|d| d.expect("admit persists a record"))
}

/// Run every admission check without persisting anything.
pub fn validate_admission(caller: candid::Principal, args: &BridgeToBaseArgs) -> BridgeResult<()> {
    admit_inner(caller, args, true).map(|_| ())
}

fn admit_inner(caller: candid::Principal, args: &BridgeToBaseArgs, dry_run: bool) -> BridgeResult<Option<DepositRecord>> {
    let rkey = state::request_key(&caller, &args.client_request_id).map_err(BridgeError::InvalidArgument)?;
    if state::lookup_request(&rkey).is_some() {
        return Err(BridgeError::RequestIdConflict);
    }
    let subaccount = parse_subaccount(&args.from_subaccount)?;
    let recipient = ids::parse_address(&args.base_recipient).map_err(BridgeError::InvalidRecipient)?;
    if recipient == [0u8; 20] {
        return Err(BridgeError::InvalidRecipient("zero address".into()));
    }
    let now = env::now_ns();
    let canister = env::canister_id();
    let a = args.amount_e8s;

    let core = state::read_core(|c| c.clone());
    if !core.initialized {
        return Err(BridgeError::NotConfigured("not initialized".into()));
    }
    if let Some(h) = &core.halted {
        return Err(BridgeError::Halted(h.reason.clone()));
    }
    if core.config.pauses.deposits {
        return Err(BridgeError::Paused("deposits are paused".into()));
    }
    let Some(binding) = core.config.contract.clone() else {
        return Err(BridgeError::NotConfigured("Base contract not bound".into()));
    };
    if recipient == binding.address {
        return Err(BridgeError::InvalidRecipient("recipient is the bridge contract".into()));
    }
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
    let mint_amount = a.checked_sub(fee).filter(|m| *m > 0).ok_or_else(|| {
        BridgeError::InvalidArgument("amount does not exceed the deposit fee".into())
    })?;

    // Capacity: everything that is, or may become, Base supply, including held claims
    // that will be re-minted without admission.
    let exposure = core
        .buckets
        .base_exposure()
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

    // Active operation limits.
    // Re-mints of held claims are not user admissions and must not crowd them out.
    let active_user = state::active_user_deposit_count();
    if active_user >= limits.max_active_global as u64 {
        return Err(BridgeError::TooManyActiveOperations);
    }
    let mine = state::active_deposits_owned_by(&caller);
    if mine >= limits.max_active_per_caller as usize {
        return Err(BridgeError::TooManyActiveOperations);
    }

    if dry_run {
        return Ok(None);
    }

    // Persist.
    let id = state::mutate_core(|c| {
        c.roll_window(now);
        c.windows.admitted_in_window = c.windows.admitted_in_window.saturating_add(a as u128);
        let id = c.next_deposit_id;
        c.next_deposit_id += 1;
        id
    });
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
        base_recipient: recipient,
        amount_e8s: a,
        deposit_fee_e8s: fee,
        mint_amount_e8s: mint_amount,
        mint_id: ids::mint_id(core.config.chain_id, &binding.address, &core.instance_salt, id),
        state: DepositState::PullPending,
        pull: Some(pull),
        refund: None,
        tx_ids: vec![],
        active_tx: None,
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

/// Check the caller's allowance and balance before anything is persisted, so a caller
/// without funds cannot create records or cause paid background work. This is a hint
/// only: the ICRC-2 pull remains the authority.
async fn precheck_funds(caller: candid::Principal, args: &BridgeToBaseArgs) -> BridgeResult<()> {
    let subaccount = parse_subaccount(&args.from_subaccount)?;
    let canister = env::canister_id();
    let (ledger_id, cached_fee) = state::read_core(|c| (c.config.icusd_ledger, c.ledger_fee_e8s));
    let fee = match cached_fee {
        Some(f) => f,
        None => {
            let f = ledger::fee(ledger_id).await.map_err(BridgeError::Ledger)?;
            state::mutate_core(|c| c.ledger_fee_e8s = Some(f));
            f
        }
    };
    let required = args.amount_e8s.checked_add(fee).ok_or_else(|| BridgeError::InvalidArgument("amount overflow".into()))?;
    let from = Account { owner: caller, subaccount };
    let spender = Account { owner: canister, subaccount: None };
    let (allowance, expires_at) = ledger::allowance(ledger_id, from, spender).await.map_err(BridgeError::Ledger)?;
    // An allowance that expires within a minute is treated as missing.
    let live = expires_at.map(|e| e > env::now_ns().saturating_add(60 * 1_000_000_000)).unwrap_or(true);
    let usable = if live { allowance } else { 0 };
    if usable < required as u128 {
        return Err(BridgeError::InsufficientAllowance { allowance_e8s: usable.min(u64::MAX as u128) as u64, required_e8s: required });
    }
    let balance = ledger::balance_of(ledger_id, from).await.map_err(BridgeError::Ledger)?;
    if balance < required as u128 {
        return Err(BridgeError::InsufficientFunds { balance_e8s: balance.min(u64::MAX as u128) as u64, required_e8s: required });
    }
    Ok(())
}

pub async fn bridge_to_base(args: BridgeToBaseArgs) -> BridgeResult<DepositView> {
    let caller = admin::require_authenticated()?;
    let rkey = state::request_key(&caller, &args.client_request_id).map_err(BridgeError::InvalidArgument)?;
    if let Some(existing) = state::lookup_request(&rkey) {
        let d = state::get_deposit(existing).ok_or_else(|| BridgeError::Internal("request index points to a missing deposit".into()))?;
        return if same_request(&d, &args)? { Ok(views::deposit_view(&d)) } else { Err(BridgeError::RequestIdConflict) };
    }
    // Cheap checks first, then the funds pre-check, then re-check idempotency (a
    // concurrent identical call may have been admitted during the awaits) and admit.
    validate_admission(caller, &args)?;
    precheck_funds(caller, &args).await?;
    if let Some(existing) = state::lookup_request(&rkey) {
        let d = state::get_deposit(existing).ok_or_else(|| BridgeError::Internal("request index points to a missing deposit".into()))?;
        return if same_request(&d, &args)? { Ok(views::deposit_view(&d)) } else { Err(BridgeError::RequestIdConflict) };
    }
    let d = admit(caller, &args)?;
    let r = OpRef::Pull(d.id);
    if let Some(sent) = ledger_ops::attempt(&r).await {
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

/// Timer: drive pulls that are unknown or whose attempt went stale.
pub async fn reconcile_pulls() {
    let now = env::now_ns();
    let candidates = state::deposits_in_state(
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
        MAX_PULL_RECONCILES_PER_TICK,
    );
    for d in candidates {
        let r = OpRef::Pull(d.id);
        ledger_ops::reconcile(&r).await;
        // A retryable no-effect on reconcile leaves the op Pending; show it as unknown
        // so the user sees it is not final.
        if let Some(mut cur) = state::get_deposit(d.id) {
            if matches!(cur.state, DepositState::PullPending)
                && cur.pull.as_ref().map(|p| p.attempts > 0).unwrap_or(false)
            {
                cur.state = DepositState::PullUnknown;
                state::put_deposit(&cur);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Hooks used by the minter lane
// ---------------------------------------------------------------------------

/// Oldest deposit waiting for a Base mint, if minting is allowed now.
/// Re-mints that may be on the mint lane at once. Re-mints bypass admission (they return
/// a held Base burn to its sender), so without this cap a holder recycling a small amount
/// through invalid ICP destinations could queue enough of them to delay every user deposit.
pub const MAX_INFLIGHT_REMINTS: usize = 2;

/// The next deposit to mint. User deposits always go first (oldest first). A re-mint is
/// picked only when no user deposit is waiting and fewer than [`MAX_INFLIGHT_REMINTS`]
/// re-mints are already in flight. User deposits are bounded by admission (amounts,
/// windows, capacity), so re-mints are delayed by at most that bounded flow.
pub fn next_mintable_deposit() -> Option<DepositRecord> {
    let ok = state::read_core(|c| c.halted.is_none() && !c.config.pauses.mints && c.config.contract.is_some());
    if !ok {
        return None;
    }
    let waiting = |d: &DepositRecord| matches!(d.state, DepositState::Escrowed) && d.active_tx.is_none();
    let user = state::deposits_in_state(|d| waiting(d) && matches!(d.kind, DepositKind::UserDeposit), 1);
    if let Some(d) = user.into_iter().next() {
        return Some(d);
    }
    let remints_in_flight = state::deposits_in_state(
        |d| matches!(d.kind, DepositKind::Remint { .. }) && matches!(d.state, DepositState::MintInFlight),
        MAX_INFLIGHT_REMINTS,
    )
    .len();
    if remints_in_flight >= MAX_INFLIGHT_REMINTS {
        return None;
    }
    state::deposits_in_state(|d| waiting(d) && matches!(d.kind, DepositKind::Remint { .. }), 1).into_iter().next()
}

pub fn on_mint_tx_created(deposit_id: u64, tx_id: u64) -> Result<(), String> {
    let mut d = state::get_deposit(deposit_id).ok_or("deposit not found")?;
    if !matches!(d.state, DepositState::Escrowed) || d.active_tx.is_some() {
        return Err(format!("deposit {deposit_id} is not waiting for a mint"));
    }
    d.state = DepositState::MintInFlight;
    d.active_tx = Some(tx_id);
    d.tx_ids.push(tx_id);
    d.updated_at_ns = env::now_ns();
    state::put_deposit(&d);
    state::append_audit(None, AuditCtx { deposit_id: Some(deposit_id), tx_id: Some(tx_id), note: "mint tx created".into(), ..Default::default() });
    Ok(())
}

/// A preflight check (local pause/cap, contract simulation, gas) blocked signing. No tx
/// exists. Holds retry automatically after a delay.
pub fn on_mint_preflight_hold(deposit_id: u64, reason: String) {
    let Some(mut d) = state::get_deposit(deposit_id) else { return };
    if !matches!(d.state, DepositState::Escrowed) || d.active_tx.is_some() {
        return;
    }
    let now = env::now_ns();
    d.state = DepositState::MintHeld { reason: reason.clone() };
    d.hold_retry_at_ns = Some(now.saturating_add(PREFLIGHT_HOLD_RETRY_NS));
    d.updated_at_ns = now;
    state::put_deposit(&d);
    state::append_audit(None, AuditCtx::deposit(deposit_id, format!("mint held before signing: {reason}")));
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MintTxOutcome {
    /// The mint attempt executed successfully at a finalized, canonical block.
    Executed(Receipt),
    /// The mint attempt was finalized with a failed status (nonce consumed, no mint).
    Reverted(Receipt),
    /// A cancel attempt won the nonce; the mint never executed.
    Cancelled(Receipt),
}

/// Apply a finalized mint transaction outcome. Called once per tx by the lane.
pub fn on_mint_finalized(deposit_id: u64, tx_id: u64, outcome: MintTxOutcome) -> Result<(), String> {
    let mut d = state::get_deposit(deposit_id).ok_or("deposit not found")?;
    if d.active_tx != Some(tx_id) || !matches!(d.state, DepositState::MintInFlight) {
        return Err(format!("deposit {deposit_id} is not waiting on tx {tx_id}"));
    }
    let contract = state::read_core(|c| c.config.contract.as_ref().map(|b| b.address)).ok_or("contract not bound")?;
    let now = env::now_ns();
    match outcome {
        MintTxOutcome::Executed(receipt) => {
            let mints: Vec<abi::BridgeMintEvent> = receipt
                .logs
                .iter()
                .filter(|l| l.address == contract && l.topics.first() == Some(&abi::BRIDGE_MINT_TOPIC0))
                .map(|l| abi::decode_bridge_mint(l, &contract))
                .collect::<Result<_, _>>()
                .map_err(|e| {
                    state::halt(format!("deposit {deposit_id}: undecodable BridgeMint log: {e:?}"));
                    "undecodable BridgeMint log".to_string()
                })?;
            let ok = mints.len() == 1
                && mints[0].mint_id == d.mint_id
                && mints[0].to == d.base_recipient
                && mints[0].amount_e8s == d.mint_amount_e8s;
            if !ok {
                state::halt(format!(
                    "deposit {deposit_id}: finalized mint receipt does not contain exactly the expected BridgeMint (found {})",
                    mints.len()
                ));
                return Err("mint receipt mismatch".into());
            }
            let t = match d.kind {
                DepositKind::UserDeposit => Transition::MintDelivered { amount: d.amount_e8s, fee: d.deposit_fee_e8s },
                DepositKind::Remint { .. } => Transition::RemintDelivered { amount: d.amount_e8s },
            };
            if let Err(e) = state::apply_transition(t, AuditCtx { deposit_id: Some(deposit_id), tx_id: Some(tx_id), note: "mint finalized".into(), ..Default::default() }) {
                state::halt(format!("deposit {deposit_id}: delivery accounting failed: {e}"));
                return Err(e.to_string());
            }
            state::mutate_core(|c| {
                c.observer.max_settled_block = c.observer.max_settled_block.max(receipt.block_number);
                c.cumulative_minted_e8s = c.cumulative_minted_e8s.saturating_add(d.mint_amount_e8s as u128);
            });
            d.state = DepositState::Delivered;
            d.active_tx = None;
            d.delivered = Some(Inclusion { tx_hash: receipt.tx_hash, block_number: receipt.block_number, block_hash: receipt.block_hash });
            d.updated_at_ns = now;
            state::put_deposit(&d);
            if let DepositKind::Remint { transfer_id } = d.kind {
                if let Some(mut x) = state::get_release(&transfer_id) {
                    x.state = ReleaseState::Reminted { deposit_id };
                    x.updated_at_ns = now;
                    state::put_release(&x);
                }
            }
        }
        MintTxOutcome::Reverted(_) | MintTxOutcome::Cancelled(_) => {
            let reason = if matches!(outcome, MintTxOutcome::Reverted(_)) {
                "Base mint transaction reverted"
            } else {
                "Base mint attempt was cancelled"
            };
            d.state = DepositState::MintHeld { reason: reason.into() };
            d.active_tx = None;
            // A finalized revert does not auto-retry (it would spend gas again).
            d.hold_retry_at_ns = None;
            d.updated_at_ns = now;
            state::put_deposit(&d);
            state::append_audit(None, AuditCtx { deposit_id: Some(deposit_id), tx_id: Some(tx_id), note: reason.into(), ..Default::default() });
        }
    }
    Ok(())
}

/// Timer: return preflight holds whose retry time passed to `Escrowed`.
pub fn retry_due_holds() {
    let now = env::now_ns();
    for mut d in state::deposits_in_state(
        |d| matches!(d.state, DepositState::MintHeld { .. }) && d.hold_retry_at_ns.map(|t| now >= t).unwrap_or(false),
        50,
    ) {
        d.state = DepositState::Escrowed;
        d.hold_retry_at_ns = None;
        d.updated_at_ns = now;
        state::put_deposit(&d);
    }
}

/// Admin: put a held deposit back in the mint queue (same mint id, new transaction).
pub fn retry_held(id: u64) -> BridgeResult<DepositView> {
    let mut d = state::get_deposit(id).ok_or(BridgeError::NotFound)?;
    if !matches!(d.state, DepositState::MintHeld { .. }) || d.active_tx.is_some() {
        return Err(BridgeError::InvalidState("deposit is not held".into()));
    }
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
///
/// The durable transfer tuple, the outflow reservation and the ambiguity bookkeeping all
/// stay in `ledger_ops::attempt` and `ledger_ops::apply_outcome`; this only adds the
/// follow-up decision of [`rearm_if_unresolved`].
async fn send_inline_attempt(r: &OpRef) {
    if let Some(sent) = ledger_ops::attempt(r).await {
        ledger_ops::apply_outcome(r, sent);
    }
    // Runs whether or not an attempt could be made: `attempt` returns `None` when it could
    // not reserve the outflow (the bridge halts), and the persisted operation still needs
    // its follow-up once the halt is cleared.
    rearm_if_unresolved(r);
}

/// Post-attempt decision. When the operation is not terminal (reply lost, ambiguous
/// outcome, `Unknown` status, a retryable no-effect reply, or no attempt could be made),
/// arm a tick within about a second instead of waiting out the idle interval, which an
/// admin may set as high as a day. This is the same rule `bridge_to_base` applies.
///
/// Synchronous, and it only arms the timer: it never allocates an operation, rebuilds or
/// resends a tuple, retries, or touches the buckets. What happens to the operation next is
/// decided by the tick (`process_refunds`, `process_sweeps` and `ledger_ops::reconcile`).
fn rearm_if_unresolved(r: &OpRef) {
    if ledger_ops::load_op(r).is_some_and(|op| !ledger_ops::is_terminal(&op)) {
        crate::timer::schedule_soon();
    }
}

// ---------------------------------------------------------------------------
// Refunds
// ---------------------------------------------------------------------------

fn refund_preconditions(d: &DepositRecord, caller: candid::Principal) -> BridgeResult<()> {
    if d.owner != caller {
        return Err(BridgeError::NotOwner);
    }
    if !matches!(d.kind, DepositKind::UserDeposit) {
        return Err(BridgeError::InvalidState("re-mints cannot be refunded to ICP".into()));
    }
    if !matches!(d.state, DepositState::MintHeld { .. }) || d.active_tx.is_some() {
        return Err(BridgeError::InvalidState("only a held deposit with no Base transaction in flight can be refunded".into()));
    }
    for tx_id in &d.tx_ids {
        let finalized = state::get_tx(*tx_id).map(|t| matches!(t.status, TxStatus::Finalized { .. })).unwrap_or(false);
        if !finalized {
            return Err(BridgeError::InvalidState(format!("Base transaction {tx_id} for this deposit is not finalized")));
        }
    }
    Ok(())
}

pub async fn request_refund(id: u64) -> BridgeResult<DepositView> {
    let caller = admin::require_authenticated()?;
    let d = state::get_deposit(id).ok_or(BridgeError::NotFound)?;
    refund_preconditions(&d, caller)?;
    if state::is_halted() {
        return Err(BridgeError::Halted("bridge is halted".into()));
    }
    let contract = state::read_core(|c| c.config.contract.as_ref().map(|b| b.address))
        .ok_or_else(|| BridgeError::NotConfigured("contract not bound".into()))?;

    // Destination truth: the mint id must be unconsumed at the finalized head.
    let ret = rpc::eth_call(&contract, &abi::encode_view(&ViewCall::MintConsumed(d.mint_id)), BlockTag::Finalized)
        .await
        .map_err(|e| BridgeError::Rpc(e.to_string()))?;
    let consumed = abi::decode_bool(&ret).map_err(|e| BridgeError::Rpc(format!("{e:?}")))?;
    if consumed {
        state::halt(format!("deposit {id}: mint id consumed on Base while the deposit is held"));
        return Err(BridgeError::InvalidState("the Base mint for this deposit exists; refund refused".into()));
    }
    let ledger_id = state::read_core(|c| c.config.icusd_ledger);
    let fee = match state::read_core(|c| c.ledger_fee_e8s) {
        Some(f) => f,
        None => {
            let f = ledger::fee(ledger_id).await.map_err(BridgeError::Ledger)?;
            state::mutate_core(|c| c.ledger_fee_e8s = Some(f));
            f
        }
    };

    // Re-check after the awaits.
    let mut d = state::get_deposit(id).ok_or(BridgeError::NotFound)?;
    refund_preconditions(&d, caller)?;
    if d.amount_e8s <= fee {
        return Err(BridgeError::InvalidState("deposit does not cover the ledger fee".into()));
    }
    let now = env::now_ns();
    let canister = env::canister_id();
    let escrow = state::read_core(|c| c.escrow_account(canister));
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
        memo: ids::refund_memo(id),
        created_at_time: now,
        attempts: 0,
        ambiguous_seen: false,
        status: LedgerOpStatus::Pending,
        search_from_block: state::read_core(|c| c.ledger_tip_lower_bound),
        search_next_block: None,
        last_attempt_ns: None,
        outflow_reserved: false,
    });
    d.state = DepositState::Refunding;
    d.updated_at_ns = now;
    state::put_deposit(&d);

    send_inline_attempt(&OpRef::Refund(id)).await;
    let d = state::get_deposit(id).ok_or(BridgeError::NotFound)?;
    Ok(views::deposit_view(&d))
}

/// Timer: finish refunds that are pending or unknown.
pub async fn process_refunds() {
    if state::is_halted() {
        return;
    }
    for d in state::deposits_in_state(|d| matches!(d.state, DepositState::Refunding), MAX_REFUNDS_PER_TICK) {
        ledger_ops::reconcile(&OpRef::Refund(d.id)).await;
    }
}

// ---------------------------------------------------------------------------
// Fee sweeps
// ---------------------------------------------------------------------------

pub async fn sweep_fees(amount_e8s: u64) -> BridgeResult<u64> {
    if state::is_halted() {
        return Err(BridgeError::Halted("bridge is halted".into()));
    }
    let ledger_id = state::read_core(|c| c.config.icusd_ledger);
    let fee = ledger::fee(ledger_id).await.map_err(BridgeError::Ledger)?;
    state::mutate_core(|c| c.ledger_fee_e8s = Some(fee));
    let canister = env::canister_id();
    let now = env::now_ns();
    let id = state::mutate_core(|c| -> BridgeResult<u64> {
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
    send_inline_attempt(&OpRef::Sweep(id)).await;
    Ok(id)
}

pub async fn process_sweeps() {
    if state::is_halted() {
        return;
    }
    let ids: Vec<u64> = state::read_core(|c| c.sweeps.iter().filter(|s| !ledger_ops::is_terminal(&s.op)).map(|s| s.id).collect());
    for id in ids {
        ledger_ops::reconcile(&OpRef::Sweep(id)).await;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::env::test_env;
    use crate::ledger::{CallOutcome, LedgerRejection};
    use candid::Principal;

    const E8: u64 = 100_000_000;

    fn user() -> Principal {
        Principal::from_slice(&[0x11; 29])
    }

    pub(crate) fn setup_bound() {
        state::reset_for_tests();
        state::mutate_core(|c| {
            c.initialized = true;
            c.config.chain_id = state::CHAIN_BASE_SEPOLIA;
            c.config.admins = vec![Principal::from_slice(&[3; 10])];
            c.config.pauses.deposits = false;
            c.config.contract = Some(state::ContractBinding {
                address: [0xcc; 20],
                bound_at_ns: 0,
                start_block: 1,
                supply_cap: u128::MAX,
                mint_window_limit: u128::MAX,
                mint_window_seconds: 1,
                min_bridge_out: 1,
            });
            c.instance_salt = [9; 32];
            c.ledger_fee_e8s = Some(10_000);
            c.ledger_tip_lower_bound = 77;
        });
    }

    fn args(amount: u64, rid: u8) -> BridgeToBaseArgs {
        BridgeToBaseArgs {
            amount_e8s: amount,
            base_recipient: "0x00000000000000000000000000000000000000aa".into(),
            from_subaccount: None,
            client_request_id: serde_bytes::ByteBuf::from(vec![rid; 16]),
            expected_deposit_fee_e8s: 0,
        }
    }

    fn resolve_pull(id: u64, outcome: CallOutcome) {
        let r = OpRef::Pull(id);
        let op = ledger_ops::load_op(&r).unwrap();
        ledger_ops::apply_resolution(&r, ledger_ops::resolution_for(&op, &outcome));
    }

    /// Simulate the persisted-before-send part of `attempt` for host tests.
    fn mark_sent(id: u64) {
        let r = OpRef::Pull(id);
        let mut op = ledger_ops::load_op(&r).unwrap();
        op.attempts += 1;
        op.last_attempt_ns = Some(env::now_ns());
        ledger_ops::store_op(&r, op);
    }

    #[test]
    fn admission_persists_tuple_and_reserves_capacity() {
        setup_bound();
        let d = admit(user(), &args(100 * E8, 1)).unwrap();
        assert_eq!(d.state, DepositState::PullPending);
        let pull = d.pull.unwrap();
        assert_eq!(pull.amount_e8s, 100 * E8);
        assert_eq!(pull.memo, ids::pull_memo(d.id));
        assert_eq!(pull.search_from_block, 77);
        assert_eq!(pull.spender.unwrap().owner, env::canister_id());
        assert_eq!(pull.to.subaccount, Some(ids::escrow_subaccount(state::CHAIN_BASE_SEPOLIA)));
        assert_eq!(state::read_core(|c| c.buckets.reserved_pulls), (100 * E8) as u128);
        assert_eq!(d.mint_id, ids::mint_id(state::CHAIN_BASE_SEPOLIA, &[0xcc; 20], &[9; 32], d.id));
    }

    #[test]
    fn admission_rejections() {
        setup_bound();
        assert!(matches!(admit(user(), &args(E8 - 1, 1)), Err(BridgeError::AmountBelowMinimum { .. })));
        assert!(matches!(admit(user(), &args(1_001 * E8, 1)), Err(BridgeError::AmountAboveMaximum { .. })));
        let mut a = args(10 * E8, 1);
        a.expected_deposit_fee_e8s = 1;
        assert!(matches!(admit(user(), &a), Err(BridgeError::FeeChanged { current_fee_e8s: 0 })));
        let mut a = args(10 * E8, 1);
        a.base_recipient = "0x0000000000000000000000000000000000000000".into();
        assert!(matches!(admit(user(), &a), Err(BridgeError::InvalidRecipient(_))));
        let mut a = args(10 * E8, 1);
        a.base_recipient = format!("0x{}", hex::encode([0xcc; 20]));
        assert!(matches!(admit(user(), &a), Err(BridgeError::InvalidRecipient(_))));
        let mut a = args(10 * E8, 1);
        a.client_request_id = serde_bytes::ByteBuf::from(vec![1; 8]);
        assert!(matches!(admit(user(), &a), Err(BridgeError::InvalidArgument(_))));
        state::mutate_core(|c| c.config.pauses.deposits = true);
        assert!(matches!(admit(user(), &args(10 * E8, 2)), Err(BridgeError::Paused(_))));
        state::mutate_core(|c| c.config.pauses.deposits = false);
        state::halt("x");
        assert!(matches!(admit(user(), &args(10 * E8, 3)), Err(BridgeError::Halted(_))));
    }

    #[test]
    fn duplicate_request_id_is_rejected_by_admit() {
        setup_bound();
        admit(user(), &args(10 * E8, 1)).unwrap();
        assert!(matches!(admit(user(), &args(10 * E8, 1)), Err(BridgeError::RequestIdConflict)));
        // Different caller, same id is a different request.
        assert!(admit(Principal::from_slice(&[0x22; 29]), &args(10 * E8, 1)).is_ok());
    }

    #[test]
    fn capacity_window_and_active_limits() {
        setup_bound();
        state::mutate_core(|c| {
            c.config.limits.max_outstanding_e8s = 25 * E8;
            c.config.limits.mint_window_limit_e8s = 1_000 * E8;
        });
        admit(user(), &args(10 * E8, 1)).unwrap();
        admit(user(), &args(10 * E8, 2)).unwrap();
        match admit(user(), &args(10 * E8, 3)) {
            Err(BridgeError::CapacityExceeded { available_e8s }) => assert_eq!(available_e8s, 5 * E8),
            other => panic!("{other:?}"),
        }
        // Held claims count toward capacity (they re-mint without admission).
        setup_bound();
        state::mutate_core(|c| {
            c.config.limits.max_outstanding_e8s = 25 * E8;
            c.buckets.held = (20 * E8) as u128;
        });
        assert!(matches!(admit(user(), &args(10 * E8, 1)), Err(BridgeError::CapacityExceeded { available_e8s }) if available_e8s == 5 * E8));
        assert!(validate_admission(user(), &args(5 * E8, 1)).is_ok());
        assert!(state::lookup_request(&state::request_key(&user(), &[1; 16]).unwrap()).is_none(), "dry run persists nothing");
        setup_bound();
        state::mutate_core(|c| c.config.limits.mint_window_limit_e8s = 15 * E8);
        admit(user(), &args(10 * E8, 1)).unwrap();
        assert!(matches!(admit(user(), &args(10 * E8, 2)), Err(BridgeError::RateLimited { available_e8s, .. }) if available_e8s == 5 * E8));
        test_env::advance_secs(86_401);
        assert!(admit(user(), &args(10 * E8, 3)).is_ok(), "window rolled");
        setup_bound();
        state::mutate_core(|c| c.config.limits.max_active_per_caller = 2);
        admit(user(), &args(E8, 1)).unwrap();
        admit(user(), &args(E8, 2)).unwrap();
        assert!(matches!(admit(user(), &args(E8, 3)), Err(BridgeError::TooManyActiveOperations)));
    }

    #[test]
    fn remints_never_count_against_the_global_active_limit() {
        setup_bound();
        state::mutate_core(|c| c.config.limits.max_active_global = 2);
        // A backlog of active re-mints far above the global limit.
        for id in 1_000..1_050u64 {
            let mut r = state::tests::sample_deposit(id, env::canister_id());
            r.kind = DepositKind::Remint { transfer_id: [(id % 251) as u8; 32] };
            r.mint_id = [(id % 253) as u8; 32];
            state::put_deposit(&r);
        }
        assert_eq!(state::active_remint_count(), 50);
        assert_eq!(state::active_user_deposit_count(), 0);
        let other = Principal::from_slice(&[8; 10]);
        admit(user(), &args(E8, 1)).unwrap();
        admit(other, &args(E8, 2)).unwrap();
        assert_eq!(state::active_user_deposit_count(), 2);
        // The third user deposit hits the limit: only user deposits count.
        assert!(matches!(admit(Principal::from_slice(&[9; 10]), &args(E8, 3)), Err(BridgeError::TooManyActiveOperations)));
        // A re-mint finishing frees nothing for users; a user deposit finishing does.
        let mut r = state::get_deposit(1_000).unwrap();
        r.state = DepositState::Delivered;
        state::put_deposit(&r);
        assert_eq!((state::active_remint_count(), state::active_user_deposit_count()), (49, 2));
    }

    #[test]
    fn pull_success_moves_reservation_to_pending_mint() {
        setup_bound();
        let d = admit(user(), &args(100 * E8, 1)).unwrap();
        mark_sent(d.id);
        resolve_pull(d.id, CallOutcome::Accepted { block_index: 500 });
        let d = state::get_deposit(d.id).unwrap();
        assert_eq!(d.state, DepositState::Escrowed);
        let b = state::read_core(|c| c.buckets);
        assert_eq!(b.reserved_pulls, 0);
        assert_eq!(b.pending_mint, (100 * E8) as u128);
        // A duplicate success report changes nothing.
        resolve_pull(d.id, CallOutcome::Accepted { block_index: 500 });
        assert_eq!(state::read_core(|c| c.buckets), b);
    }

    #[test]
    fn first_attempt_insufficient_allowance_fails_cleanly() {
        setup_bound();
        let d = admit(user(), &args(100 * E8, 1)).unwrap();
        mark_sent(d.id);
        resolve_pull(d.id, CallOutcome::Rejected(LedgerRejection::InsufficientAllowance { allowance: 0 }));
        let d = state::get_deposit(d.id).unwrap();
        assert!(matches!(d.state, DepositState::PullFailed { .. }));
        assert_eq!(state::read_core(|c| c.buckets), crate::accounting::Buckets::default());
        assert_eq!(state::read_core(|c| c.windows.admitted_in_window), 0);
        assert!(!state::is_deposit_active(d.id));
    }

    #[test]
    fn ambiguous_then_no_effect_reply_stays_unknown() {
        setup_bound();
        let d = admit(user(), &args(100 * E8, 1)).unwrap();
        mark_sent(d.id);
        resolve_pull(d.id, CallOutcome::Ambiguous("reply lost".into()));
        let rec = state::get_deposit(d.id).unwrap();
        assert!(rec.pull.as_ref().unwrap().ambiguous_seen);
        // The retry reports a typed no-effect error: that is NOT proof the first attempt
        // failed, so nothing is released.
        resolve_pull(d.id, CallOutcome::Rejected(LedgerRejection::InsufficientAllowance { allowance: 0 }));
        let rec = state::get_deposit(d.id).unwrap();
        assert!(matches!(rec.pull.as_ref().unwrap().status, LedgerOpStatus::Unknown { .. }));
        assert_eq!(state::read_core(|c| c.buckets.reserved_pulls), (100 * E8) as u128);
        // Duplicate later proves the first attempt executed.
        resolve_pull(d.id, CallOutcome::Accepted { block_index: 9 });
        assert_eq!(state::get_deposit(d.id).unwrap().state, DepositState::Escrowed);
    }

    #[test]
    fn user_deposits_mint_before_remints_and_remints_in_flight_are_capped() {
        setup_bound();
        let owner = user();
        let remint = |id: u64, state: DepositState| {
            let mut d = state::tests::sample_deposit(id, owner);
            d.kind = DepositKind::Remint { transfer_id: [id as u8; 32] };
            d.mint_id = [id as u8 + 100; 32];
            d.state = state;
            state::put_deposit(&d);
        };
        // Older re-mints are waiting, then a newer user deposit arrives: the user goes first.
        remint(10, DepositState::Escrowed);
        remint(11, DepositState::Escrowed);
        let mut user_dep = state::tests::sample_deposit(12, owner);
        user_dep.mint_id = [99; 32];
        state::put_deposit(&user_dep);
        assert_eq!(next_mintable_deposit().map(|x| x.id), Some(12));
        // With no user deposit waiting, the oldest re-mint is next.
        user_dep.state = DepositState::MintInFlight;
        user_dep.active_tx = Some(1);
        state::put_deposit(&user_dep);
        assert_eq!(next_mintable_deposit().map(|x| x.id), Some(10));
        // Once MAX_INFLIGHT_REMINTS re-mints are in flight, no further re-mint starts...
        for id in 20..20 + MAX_INFLIGHT_REMINTS as u64 {
            remint(id, DepositState::MintInFlight);
        }
        assert!(next_mintable_deposit().is_none());
        // ...but a user deposit still does.
        let mut late_user = state::tests::sample_deposit(30, owner);
        late_user.mint_id = [98; 32];
        state::put_deposit(&late_user);
        assert_eq!(next_mintable_deposit().map(|x| x.id), Some(30));
    }

    #[test]
    fn mint_hooks_deliver_exactly_once() {
        setup_bound();
        let d = admit(user(), &args(100 * E8, 1)).unwrap();
        mark_sent(d.id);
        resolve_pull(d.id, CallOutcome::Accepted { block_index: 1 });
        assert_eq!(next_mintable_deposit().map(|x| x.id), Some(d.id));
        on_mint_tx_created(d.id, 7).unwrap();
        assert!(next_mintable_deposit().is_none());
        assert!(on_mint_tx_created(d.id, 8).is_err());
        let rec = state::get_deposit(d.id).unwrap();
        let log = crate::evm::abi::RawLog {
            address: [0xcc; 20],
            topics: vec![abi::BRIDGE_MINT_TOPIC0, rec.mint_id, {
                let mut t = [0u8; 32];
                t[12..].copy_from_slice(&rec.base_recipient);
                t
            }],
            data: ids::word_u64(rec.mint_amount_e8s).to_vec(),
            block_number: 10,
            block_hash: [1; 32],
            tx_hash: [2; 32],
            log_index: 1,
            removed: false,
        };
        let receipt = Receipt { tx_hash: [2; 32], success: true, block_number: 10, block_hash: [1; 32], from: [0; 20], to: Some([0xcc; 20]), logs: vec![log] };
        on_mint_finalized(d.id, 7, MintTxOutcome::Executed(receipt.clone())).unwrap();
        let rec = state::get_deposit(d.id).unwrap();
        assert_eq!(rec.state, DepositState::Delivered);
        let b = state::read_core(|c| c.buckets);
        assert_eq!(b.outstanding_base, (100 * E8) as u128);
        assert_eq!(b.pending_mint, 0);
        assert_eq!(state::read_core(|c| c.cumulative_minted_e8s), (100 * E8) as u128);
        assert!(on_mint_finalized(d.id, 7, MintTxOutcome::Executed(receipt)).is_err());
        assert_eq!(state::read_core(|c| c.cumulative_minted_e8s), (100 * E8) as u128, "counted once");
        assert_eq!(state::read_core(|c| c.buckets), b);
    }

    #[test]
    fn mismatched_mint_receipt_halts_without_crediting() {
        setup_bound();
        let d = admit(user(), &args(100 * E8, 1)).unwrap();
        mark_sent(d.id);
        resolve_pull(d.id, CallOutcome::Accepted { block_index: 1 });
        on_mint_tx_created(d.id, 7).unwrap();
        let receipt = Receipt { tx_hash: [2; 32], success: true, block_number: 10, block_hash: [1; 32], from: [0; 20], to: Some([0xcc; 20]), logs: vec![] };
        assert!(on_mint_finalized(d.id, 7, MintTxOutcome::Executed(receipt)).is_err());
        assert!(state::is_halted());
        assert_eq!(state::read_core(|c| c.buckets.outstanding_base), 0);
        assert_eq!(state::get_deposit(d.id).unwrap().state, DepositState::MintInFlight);
    }

    #[test]
    fn reverted_mint_holds_and_admin_retry_requeues() {
        setup_bound();
        let d = admit(user(), &args(100 * E8, 1)).unwrap();
        mark_sent(d.id);
        resolve_pull(d.id, CallOutcome::Accepted { block_index: 1 });
        on_mint_tx_created(d.id, 7).unwrap();
        let receipt = Receipt { tx_hash: [2; 32], success: false, block_number: 10, block_hash: [1; 32], from: [0; 20], to: Some([0xcc; 20]), logs: vec![] };
        on_mint_finalized(d.id, 7, MintTxOutcome::Reverted(receipt)).unwrap();
        let rec = state::get_deposit(d.id).unwrap();
        assert!(matches!(rec.state, DepositState::MintHeld { .. }));
        assert_eq!(rec.hold_retry_at_ns, None, "finalized reverts do not auto-retry");
        assert_eq!(state::read_core(|c| c.buckets.pending_mint), (100 * E8) as u128, "backing stays reserved");
        retry_due_holds();
        assert!(matches!(state::get_deposit(d.id).unwrap().state, DepositState::MintHeld { .. }));
        let now = env::now_ns();
        crate::timer::set_pending_due_for_tests(Some(now + 300 * 1_000_000_000));
        retry_held(d.id).unwrap();
        assert_eq!(state::get_deposit(d.id).unwrap().state, DepositState::Escrowed);
        assert_eq!(crate::timer::pending_due_for_tests(), Some(now + 1_000_000_000), "an admin retry re-arms the timer");
    }

    #[test]
    fn preflight_hold_auto_retries_after_delay() {
        setup_bound();
        let d = admit(user(), &args(100 * E8, 1)).unwrap();
        mark_sent(d.id);
        resolve_pull(d.id, CallOutcome::Accepted { block_index: 1 });
        on_mint_preflight_hold(d.id, "contract mint paused".into());
        retry_due_holds();
        assert!(matches!(state::get_deposit(d.id).unwrap().state, DepositState::MintHeld { .. }));
        test_env::advance_secs(601);
        retry_due_holds();
        assert_eq!(state::get_deposit(d.id).unwrap().state, DepositState::Escrowed);
    }

    #[test]
    fn refund_preconditions_guard_ownership_and_unfinalized_txs() {
        setup_bound();
        let d = admit(user(), &args(100 * E8, 1)).unwrap();
        mark_sent(d.id);
        resolve_pull(d.id, CallOutcome::Accepted { block_index: 1 });
        let rec = state::get_deposit(d.id).unwrap();
        assert!(matches!(refund_preconditions(&rec, user()), Err(BridgeError::InvalidState(_))), "escrowed, not held");
        on_mint_preflight_hold(d.id, "paused".into());
        let rec = state::get_deposit(d.id).unwrap();
        assert!(refund_preconditions(&rec, user()).is_ok());
        assert_eq!(refund_preconditions(&rec, Principal::from_slice(&[0x22; 29])), Err(BridgeError::NotOwner));
        // A deposit with an unfinalized tx in its history cannot be refunded.
        let mut rec2 = rec.clone();
        rec2.tx_ids.push(99);
        state::put_tx(&state::EvmTxRecord {
            id: 99,
            lane: ids::LaneKind::Minter,
            nonce: 0,
            purpose: state::TxPurpose::Mint { deposit_id: d.id },
            to: [0xcc; 20],
            value_wei: 0,
            data: vec![],
            gas_limit: 100_000,
            attempts: vec![],
            status: TxStatus::Pending,
            cancel_requested: false,
            outcome_applied: false,
            created_at_ns: 0,
            updated_at_ns: 0,
            last_checked_ns: None,
            note: None,
        });
        assert!(matches!(refund_preconditions(&rec2, user()), Err(BridgeError::InvalidState(_))));
    }

    // -----------------------------------------------------------------------
    // Refund and sweep: an unresolved inline attempt re-arms the tick
    // -----------------------------------------------------------------------
    //
    // On the host every ledger call is an `Ambiguous` outcome (see `ledger::call_candid`),
    // which is exactly a lost reply, so `ledger_ops::attempt` runs for real here.
    // `request_refund` runs end to end. `sweep_fees` cannot: it always awaits the ledger
    // fee first, and that read is an error off-chain. The sweep tests therefore start from
    // the state `sweep_fees` persists and run the shared `send_inline_attempt`, which is all
    // `sweep_fees` does after persisting the operation and writing its audit note.

    use crate::evm::rpc::testutil;
    use crate::flows::ledger_ops::SentAttempt;

    const IDLE_NS: u64 = 300 * 1_000_000_000;
    const SOON_NS: u64 = 1_000_000_000;

    /// Pretend the idle tick is armed, as it is when nothing was pending. Returns `now`.
    fn arm_idle_tick() -> u64 {
        let now = env::now_ns();
        crate::timer::set_pending_due_for_tests(Some(now + IDLE_NS));
        now
    }

    /// A held user deposit that may be refunded, with every provider answering "the mint
    /// id is not consumed". The caller is its owner.
    fn held_refundable_deposit() -> u64 {
        setup_bound();
        state::mutate_core(|c| {
            c.config.rpc_services = testutil::services(3);
            c.config.min_quorum = 2;
            c.config.evm_rpc = Principal::from_slice(&[7; 10]);
        });
        let not_consumed = format!("0x{}", "00".repeat(32));
        crate::evm::rpc::set_mock_transport(move |_, _| Ok(testutil::ok_body(serde_json::json!(not_consumed))));
        let d = admit(user(), &args(100 * E8, 1)).unwrap();
        mark_sent(d.id);
        resolve_pull(d.id, CallOutcome::Accepted { block_index: 1 });
        on_mint_preflight_hold(d.id, "paused".into());
        test_env::set_caller(user());
        d.id
    }

    /// Builds an operation whose inline attempt got no reply; returns it and the time.
    type Fixture = fn() -> (OpRef, u64);

    /// A refund requested while the ledger reply is lost: `Refunding`, op `Unknown`.
    fn refund_with_lost_reply() -> (OpRef, u64) {
        let id = held_refundable_deposit();
        let now = arm_idle_tick();
        testutil::block_on(request_refund(id)).unwrap();
        (OpRef::Refund(id), now)
    }

    /// A just-persisted fee sweep, in the shape `sweep_fees` stores it (10_000 e8s ledger
    /// fee, 5 icUSD of accrued fees).
    fn persisted_sweep(amount: u64) -> OpRef {
        setup_bound();
        let recipient = Account { owner: Principal::from_slice(&[0x44; 29]), subaccount: None };
        let fee = 10_000;
        let now = env::now_ns();
        let canister = env::canister_id();
        let id = state::mutate_core(|c| {
            c.config.fee_recipient = Some(recipient);
            c.buckets.fees = (5 * E8) as u128;
            let id = c.next_sweep_id;
            c.next_sweep_id += 1;
            let op = LedgerOp {
                kind: LedgerOpKind::Sweep,
                from: c.escrow_account(canister),
                to: recipient,
                spender: None,
                amount_e8s: amount - fee,
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
            c.sweeps.push(SweepRecord { id, amount_e8s: amount, op, created_at_ns: now });
            id
        });
        OpRef::Sweep(id)
    }

    /// A sweep whose inline attempt got no reply: `Unknown`.
    fn sweep_with_lost_reply() -> (OpRef, u64) {
        let r = persisted_sweep(E8);
        let now = arm_idle_tick();
        testutil::block_on(send_inline_attempt(&r));
        (r, now)
    }

    /// The op with its send history blanked: what is left is the durable transfer tuple
    /// (accounts, amount, fee, memo, `created_at_time`) the ledger deduplicates on.
    fn tuple_of(op: &LedgerOp) -> LedgerOp {
        LedgerOp {
            attempts: 0,
            ambiguous_seen: false,
            status: LedgerOpStatus::Pending,
            search_next_block: None,
            last_attempt_ns: None,
            outflow_reserved: false,
            ..op.clone()
        }
    }

    /// Everything a re-arm must leave alone: the core (buckets, sweeps, halt, counters),
    /// the deposit if any, and the audit log.
    fn snapshot(deposit: Option<u64>) -> (state::BridgeCore, Option<DepositRecord>, u64) {
        (state::read_core(|c| c.clone()), deposit.and_then(state::get_deposit), state::audit_len())
    }

    fn deposit_id_of(r: &OpRef) -> Option<u64> {
        match r {
            OpRef::Refund(id) | OpRef::Pull(id) => Some(*id),
            _ => None,
        }
    }

    #[test]
    fn refund_with_a_lost_reply_rearms_the_tick_and_keeps_the_original_tuple() {
        let id = held_refundable_deposit();
        let pending_mint_before = state::read_core(|c| c.buckets.pending_mint);
        assert_eq!(pending_mint_before, (100 * E8) as u128);
        let now = arm_idle_tick();

        testutil::block_on(request_refund(id)).unwrap();

        let d = state::get_deposit(id).unwrap();
        assert_eq!(d.state, DepositState::Refunding, "an unresolved refund stays Refunding");
        let op = d.refund.clone().unwrap();
        assert!(matches!(op.status, LedgerOpStatus::Unknown { .. }), "{:?}", op.status);
        assert!(op.ambiguous_seen);
        assert!(!ledger_ops::is_terminal(&op));
        // The persisted tuple, sent exactly once.
        assert_eq!(op.attempts, 1, "the re-arm sent nothing");
        assert_eq!(op.created_at_time, now);
        assert_eq!(op.memo, ids::refund_memo(id));
        assert_eq!((op.amount_e8s, op.fee_e8s), (100 * E8 - 10_000, Some(10_000)));
        assert_eq!(op.to, d.from.unwrap());
        assert!(op.outflow_reserved, "the outflow stays reserved while the outcome is unknown");
        // One refund, no other operation.
        assert!(d.pull.as_ref().map(ledger_ops::is_terminal).unwrap_or(false), "the pull is untouched");
        assert!(state::read_core(|c| c.sweeps.is_empty()));
        // Buckets are exactly what RefundStarted and the outflow reservation produce.
        let b = state::read_core(|c| c.buckets);
        assert_eq!(
            (b.pending_mint, b.pending_refund, b.unknown_outflow, b.reserved_pulls, b.outstanding_base, b.fees),
            (0, (100 * E8) as u128, (100 * E8) as u128, 0, 0, 0)
        );
        // The tick is due within a second, not at the idle interval.
        assert_eq!(crate::timer::pending_due_for_tests(), Some(now + SOON_NS));
    }

    #[test]
    fn sweep_with_a_lost_reply_rearms_the_tick_and_keeps_the_original_tuple() {
        let r = persisted_sweep(E8);
        let before = ledger_ops::load_op(&r).unwrap();
        let now = arm_idle_tick();

        testutil::block_on(send_inline_attempt(&r));

        let op = ledger_ops::load_op(&r).unwrap();
        assert!(matches!(op.status, LedgerOpStatus::Unknown { .. }), "{:?}", op.status);
        assert!(op.ambiguous_seen);
        assert!(!ledger_ops::is_terminal(&op));
        assert_eq!(tuple_of(&op), tuple_of(&before), "the durable tuple is unchanged");
        assert_eq!(op.attempts, 1, "the re-arm sent nothing");
        assert!(op.outflow_reserved);
        // One sweep, no other operation.
        assert_eq!(state::read_core(|c| (c.sweeps.len(), c.next_sweep_id)), (1, 1));
        let b = state::read_core(|c| c.buckets);
        assert_eq!((b.fees, b.unknown_outflow), ((5 * E8) as u128, E8 as u128), "fees are only reserved, not spent");
        assert_eq!(crate::timer::pending_due_for_tests(), Some(now + SOON_NS));
    }

    #[test]
    fn the_rearm_changes_nothing_but_the_timer() {
        // Every non-terminal shape an inline attempt can leave behind, for both kinds.
        let kinds: [(&str, Fixture); 2] = [("refund", refund_with_lost_reply), ("sweep", sweep_with_lost_reply)];
        for (label, make) in kinds {
            for shape in ["unknown after a lost reply", "sent, outcome never recorded", "definite no-effect reply"] {
                let (r, _) = make();
                let mut op = ledger_ops::load_op(&r).unwrap();
                match shape {
                    "unknown after a lost reply" => {}
                    "sent, outcome never recorded" => op.status = LedgerOpStatus::Pending,
                    _ => {
                        op.status = LedgerOpStatus::Pending;
                        op.attempts = 0;
                        op.ambiguous_seen = false;
                        op.last_attempt_ns = None;
                    }
                }
                ledger_ops::store_op(&r, op);
                let now = arm_idle_tick();
                let before = snapshot(deposit_id_of(&r));

                rearm_if_unresolved(&r);

                assert_eq!(snapshot(deposit_id_of(&r)), before, "{label} / {shape}: no record, bucket or audit change");
                assert_eq!(crate::timer::pending_due_for_tests(), Some(now + SOON_NS), "{label} / {shape}");
            }
        }
    }

    #[test]
    fn a_retryable_no_effect_reply_is_rearmed_without_a_new_tuple() {
        // A typed `TemporarilyUnavailable` before any ambiguous attempt leaves the op
        // Pending and unsent: the tick must resend the same tuple soon.
        for kind in ["refund", "sweep"] {
            let (r, tuple_before) = if kind == "refund" {
                let id = held_refundable_deposit();
                testutil::block_on(request_refund(id)).unwrap();
                let r = OpRef::Refund(id);
                // Rewind to the state just before the send: nothing ambiguous happened.
                let mut op = ledger_ops::load_op(&r).unwrap();
                op.status = LedgerOpStatus::Pending;
                op.ambiguous_seen = false;
                op.attempts = 1;
                ledger_ops::store_op(&r, op.clone());
                (r, tuple_of(&op))
            } else {
                let r = persisted_sweep(E8);
                // The attempt is recorded as sent; its raw outcome is discarded and replaced
                // by the typed reply below.
                let sent = testutil::block_on(ledger_ops::attempt(&r)).unwrap();
                assert_eq!(sent.attempt, 1);
                let op = ledger_ops::load_op(&r).unwrap();
                assert_eq!((op.status.clone(), op.ambiguous_seen), (LedgerOpStatus::Pending, false));
                (r, tuple_of(&op))
            };
            let now = arm_idle_tick();
            ledger_ops::apply_outcome(
                &r,
                SentAttempt { attempt: 1, outcome: CallOutcome::Rejected(LedgerRejection::TemporarilyUnavailable) },
            );
            let op = ledger_ops::load_op(&r).unwrap();
            assert_eq!((op.status.clone(), op.attempts), (LedgerOpStatus::Pending, 0), "{kind}: unsent, same tuple");
            let before = snapshot(deposit_id_of(&r));
            rearm_if_unresolved(&r);
            assert_eq!(snapshot(deposit_id_of(&r)), before, "{kind}");
            assert_eq!(tuple_of(&ledger_ops::load_op(&r).unwrap()), tuple_before, "{kind}: same tuple");
            assert_eq!(crate::timer::pending_due_for_tests(), Some(now + SOON_NS), "{kind}");
        }
    }

    #[test]
    fn a_succeeded_operation_needs_no_fast_rearm() {
        // Refund: the ledger answers `Accepted` for the one attempt.
        let (r, now) = refund_with_lost_reply();
        ledger_ops::apply_outcome(&r, SentAttempt { attempt: 1, outcome: CallOutcome::Accepted { block_index: 9 } });
        assert_eq!(state::get_deposit(deposit_id_of(&r).unwrap()).unwrap().state, DepositState::Refunded);
        assert!(ledger_ops::is_terminal(&ledger_ops::load_op(&r).unwrap()));
        crate::timer::set_pending_due_for_tests(Some(now + IDLE_NS));
        rearm_if_unresolved(&r);
        assert_eq!(crate::timer::pending_due_for_tests(), Some(now + IDLE_NS), "a finished refund leaves the idle tick alone");
        assert_eq!(state::read_core(|c| (c.buckets.pending_refund, c.buckets.unknown_outflow)), (0, 0));

        // Sweep: same.
        let (r, now) = sweep_with_lost_reply();
        ledger_ops::apply_outcome(&r, SentAttempt { attempt: 1, outcome: CallOutcome::Accepted { block_index: 10 } });
        assert!(ledger_ops::is_terminal(&ledger_ops::load_op(&r).unwrap()));
        crate::timer::set_pending_due_for_tests(Some(now + IDLE_NS));
        rearm_if_unresolved(&r);
        assert_eq!(crate::timer::pending_due_for_tests(), Some(now + IDLE_NS), "a finished sweep leaves the idle tick alone");
        assert_eq!(state::read_core(|c| (c.buckets.fees, c.buckets.unknown_outflow)), (((5 * E8) - E8) as u128, 0));
        // The rule is only "arm the timer", so re-arming a finished operation would also
        // be harmless; it is skipped to keep an idle bridge idle.
    }

    #[test]
    fn a_missing_operation_is_not_rearmed() {
        setup_bound();
        let now = arm_idle_tick();
        rearm_if_unresolved(&OpRef::Sweep(42));
        rearm_if_unresolved(&OpRef::Refund(42));
        assert_eq!(crate::timer::pending_due_for_tests(), Some(now + IDLE_NS));
    }

    #[test]
    fn a_sweep_that_could_not_be_attempted_is_still_rearmed() {
        // The outflow reservation overflows, so `attempt` halts the bridge and returns
        // `None` without sending or storing anything.
        let r = persisted_sweep(E8);
        state::mutate_core(|c| c.buckets.unknown_outflow = u128::MAX);
        let before_op = ledger_ops::load_op(&r).unwrap();
        let now = arm_idle_tick();

        testutil::block_on(send_inline_attempt(&r));

        assert!(state::is_halted());
        assert_eq!(ledger_ops::load_op(&r).unwrap(), before_op, "no attempt was recorded, the op is as persisted");
        assert_eq!(before_op.attempts, 0);
        assert_eq!(state::read_core(|c| c.buckets.unknown_outflow), u128::MAX, "no partial reservation");
        assert_eq!(crate::timer::pending_due_for_tests(), Some(now + SOON_NS));
    }

    #[test]
    fn a_refund_that_could_not_be_attempted_is_still_rearmed() {
        let id = held_refundable_deposit();
        state::mutate_core(|c| c.buckets.unknown_outflow = u128::MAX);
        let now = arm_idle_tick();

        testutil::block_on(request_refund(id)).unwrap();

        assert!(state::is_halted());
        let d = state::get_deposit(id).unwrap();
        assert_eq!(d.state, DepositState::Refunding);
        let op = d.refund.unwrap();
        assert_eq!((op.attempts, op.status.clone(), op.outflow_reserved, op.last_attempt_ns), (0, LedgerOpStatus::Pending, false, None));
        assert_eq!(op.created_at_time, now);
        assert_eq!(crate::timer::pending_due_for_tests(), Some(now + SOON_NS));
    }

    #[test]
    fn the_tick_resends_the_identical_tuple_after_the_rearm() {
        // The re-arm only schedules; the existing reconciliation path decides. It resends
        // the same tuple (same created_at_time and memo), so the ledger can deduplicate.
        let (r, now) = refund_with_lost_reply();
        let first = ledger_ops::load_op(&r).unwrap();
        test_env::advance_secs(1);
        testutil::block_on(process_refunds());
        let second = ledger_ops::load_op(&r).unwrap();
        assert_eq!(second.attempts, 2);
        assert_eq!(tuple_of(&second), tuple_of(&first));
        assert_eq!(second.created_at_time, now);
        assert_eq!(state::read_core(|c| c.buckets.unknown_outflow), (100 * E8) as u128, "reserved once, not twice");

        let (r, now) = sweep_with_lost_reply();
        let first = ledger_ops::load_op(&r).unwrap();
        test_env::advance_secs(1);
        testutil::block_on(process_sweeps());
        let second = ledger_ops::load_op(&r).unwrap();
        assert_eq!(second.attempts, 2);
        assert_eq!(tuple_of(&second), tuple_of(&first));
        assert_eq!(second.created_at_time, now);
        assert_eq!(state::read_core(|c| (c.sweeps.len(), c.buckets.unknown_outflow)), (1, E8 as u128));
    }
}
