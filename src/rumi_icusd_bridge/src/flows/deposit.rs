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

    // Capacity: everything that is, or may become, Base supply.
    let exposure = core
        .buckets
        .base_exposure()
        .and_then(|e| e.checked_add(core.buckets.reserved_pulls).ok_or(crate::accounting::AccountingError::Overflow { bucket: "exposure".into() }))
        .map_err(|e| BridgeError::Internal(e.to_string()))?;
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
    if core.active_deposits.len() >= limits.max_active_global as usize {
        return Err(BridgeError::TooManyActiveOperations);
    }
    let mine = core
        .active_deposits
        .iter()
        .filter_map(|id| state::get_deposit(*id))
        .filter(|d| d.owner == caller)
        .count();
    if mine >= limits.max_active_per_caller as usize {
        return Err(BridgeError::TooManyActiveOperations);
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
    Ok(d)
}

pub async fn bridge_to_base(args: BridgeToBaseArgs) -> BridgeResult<DepositView> {
    let caller = admin::require_authenticated()?;
    let rkey = state::request_key(&caller, &args.client_request_id).map_err(BridgeError::InvalidArgument)?;
    if let Some(existing) = state::lookup_request(&rkey) {
        let d = state::get_deposit(existing).ok_or_else(|| BridgeError::Internal("request index points to a missing deposit".into()))?;
        return if same_request(&d, &args)? { Ok(views::deposit_view(&d)) } else { Err(BridgeError::RequestIdConflict) };
    }
    let d = admit(caller, &args)?;
    let r = OpRef::Pull(d.id);
    if let Some(res) = ledger_ops::attempt(&r).await {
        ledger_ops::apply_resolution(&r, res);
    }
    let d = state::get_deposit(d.id).ok_or_else(|| BridgeError::Internal("deposit vanished".into()))?;
    if matches!(d.state, DepositState::PullPending) {
        // Retryable no-effect outcome: mark visibly so the timer resends.
        let mut d2 = d.clone();
        d2.state = DepositState::PullUnknown;
        state::put_deposit(&d2);
    }
    let d = state::get_deposit(d.id).unwrap_or(d);
    crate::timer::schedule_soon();
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
pub fn next_mintable_deposit() -> Option<DepositRecord> {
    let ok = state::read_core(|c| c.halted.is_none() && !c.config.pauses.mints && c.config.contract.is_some());
    if !ok {
        return None;
    }
    state::deposits_in_state(|d| matches!(d.state, DepositState::Escrowed) && d.active_tx.is_none(), 1).into_iter().next()
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
            state::mutate_core(|c| c.observer.max_settled_block = c.observer.max_settled_block.max(receipt.block_number));
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
    Ok(views::deposit_view(&d))
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

    let r = OpRef::Refund(id);
    if let Some(res) = ledger_ops::attempt(&r).await {
        ledger_ops::apply_resolution(&r, res);
    }
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
    let r = OpRef::Sweep(id);
    if let Some(res) = ledger_ops::attempt(&r).await {
        ledger_ops::apply_resolution(&r, res);
    }
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
        assert_eq!(pull.to.subaccount, Some(ids::escrow_subaccount()));
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
        assert!(!state::read_core(|c| c.active_deposits.contains(&d.id)));
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
        assert!(on_mint_finalized(d.id, 7, MintTxOutcome::Executed(receipt)).is_err());
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
        retry_held(d.id).unwrap();
        assert_eq!(state::get_deposit(d.id).unwrap().state, DepositState::Escrowed);
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
}
