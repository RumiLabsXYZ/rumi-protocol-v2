//! Solana -> ICP releases (49b): opening a release from a final burn, driving the ICP transfer, and
//! the bounded return hints.
//!
//! **No payout before a final burn.** `open_release` is the ONLY function that creates a release
//! record, and it requires the burn as a stored lane operation that is `Delivered` (finalized,
//! `err == null`, and exactly the stored `BurnChecked` message: that is what the lane's
//! `Delivered` means), with the same amount, and a pending-release bucket that covers every
//! release already owed plus this one. The accounting has no transition from `O`, `RC` or `BU`
//! straight to `PR`, so a release cannot be funded before `BurnFinalized`; and a release record
//! cannot exist without a delivered burn, so the two together make a payout before a final burn
//! unreachable. One burn makes at most one release.
//!
//! The burn lane (49d) calls `open_release` in the same synchronous step as `BurnFinalized`.

use super::ledger_ops::{self, OpRef};
use crate::accounting::Transition;
use crate::admin;
use crate::env;
use crate::icp::IcpLedger;
use crate::ids::{self, Hash32};
use crate::records::{OpKind, OpState, Sig};
use crate::state::{
    self, AuditCtx, Hint, LedgerOp, LedgerOpKind, LedgerOpStatus, ReleaseRecord, ReleaseState,
};
use crate::tx::{signature_from_base58, Pubkey};
use crate::types::*;
use icrc_ledger_types::icrc1::account::Account;

pub const MAX_RELEASES_PER_TICK: usize = 10;
/// Hints older than this are dropped.
pub const HINT_TTL_NS: u64 = 2 * 3_600 * 1_000_000_000;
/// Pending hints kept per caller.
pub const MAX_HINTS_PER_CALLER: usize = 5;

/// What the burn lane hands over once a custodied return's burn is final.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BurnedReturn {
    /// The return id (note section 2.2).
    pub return_id: Hash32,
    /// The delivered burn operation.
    pub burn_op_id: u64,
    /// The verified sender (owner of the source token account).
    pub solana_sender: Pubkey,
    /// The ICP account named by the return memo.
    pub destination: Account,
    pub return_signature: Sig,
    pub return_slot: u64,
    /// `B`: the burned amount.
    pub amount_e8s: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenOutcome {
    Opened(ReleaseState),
    AlreadyOpen,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenError {
    /// No such operation, not a burn, or not delivered: there is no final burn to pay out for.
    NoFinalBurn(String),
    /// The burn amount differs from the return amount.
    AmountMismatch { burn: u64, release: u64 },
    /// The pending-release bucket does not cover this release on top of those already owed.
    NotFunded { pending_release: u128, owed: u128, amount: u64 },
    /// The ledger fee or the minting account is not known yet: nothing was changed; retry.
    LedgerFactsUnknown,
    /// A different release already exists for this return id or this burn (the bridge halts).
    Conflict(String),
    Accounting(String),
}

fn same_account(a: &Account, b: &Account) -> bool {
    a.owner == b.owner && a.subaccount.unwrap_or([0u8; 32]) == b.subaccount.unwrap_or([0u8; 32])
}

/// Open the release of a return whose burn is final. Idempotent on the return id. See the module
/// docs for what it checks.
pub fn open_release(b: &BurnedReturn) -> Result<OpenOutcome, OpenError> {
    if let Some(existing) = state::get_release(&b.return_id) {
        if existing.burn_op_id != b.burn_op_id || existing.amount_e8s != b.amount_e8s || existing.solana_sender != b.solana_sender {
            let why = format!("return {} was opened twice with different contents", hex::encode(b.return_id));
            state::halt(why.clone());
            return Err(OpenError::Conflict(why));
        }
        return Ok(OpenOutcome::AlreadyOpen);
    }
    if let Some(other) = state::release_by_burn_op(b.burn_op_id) {
        let why = format!("burn operation {} already made release {} payable", b.burn_op_id, hex::encode(other));
        state::halt(why.clone());
        return Err(OpenError::Conflict(why));
    }
    // The burn: a stored lane operation, delivered, of exactly this amount.
    let op = state::get_op(b.burn_op_id).ok_or_else(|| OpenError::NoFinalBurn(format!("no operation {}", b.burn_op_id)))?;
    if op.kind != OpKind::Burn {
        return Err(OpenError::NoFinalBurn(format!("operation {} is not a burn", b.burn_op_id)));
    }
    let OpState::Delivered { signature: burn_signature, slot: burn_slot, .. } = op.state.clone() else {
        return Err(OpenError::NoFinalBurn(format!("burn operation {} is not delivered", b.burn_op_id)));
    };
    if op.amount != b.amount_e8s {
        return Err(OpenError::AmountMismatch { burn: op.amount, release: b.amount_e8s });
    }

    let now = env::now_ns();
    let (release_fee, ledger_fee, minting) =
        state::read_core(|c| (c.config.fees.release_fee_e8s, c.ledger_fee_e8s, c.ledger_minting_account));
    let (Some(ledger_fee), Some(minting)) = (ledger_fee, minting) else {
        return Err(OpenError::LedgerFactsUnknown);
    };
    let hold: Option<String> = if same_account(&b.destination, &minting) {
        Some("destination is the icUSD minting account (a transfer there would burn the funds)".into())
    } else if (b.amount_e8s as u128) <= release_fee as u128 + ledger_fee as u128 {
        Some(format!(
            "amount {} e8s does not cover the bridge fee ({release_fee}) and ledger fee ({ledger_fee})",
            b.amount_e8s
        ))
    } else {
        None
    };

    // Funding: `PR` must cover every pending release already owed plus this one. `BurnFinalized`
    // (applied by the burn lane in the same step) is the only way into `PR`.
    let (pr, owed) = (state::read_core(|c| c.buckets.pending_release), state::pending_release_total());
    if pr < owed.saturating_add(b.amount_e8s as u128) {
        return Err(OpenError::NotFunded { pending_release: pr, owed, amount: b.amount_e8s });
    }
    let release_state = match hold {
        None => ReleaseState::Pending,
        Some(reason) => {
            state::apply_transition(
                Transition::ReleaseHeld { amount: b.amount_e8s },
                AuditCtx::release(b.return_id, format!("held at opening: {reason}")),
            )
            .map_err(|e| OpenError::Accounting(e.to_string()))?;
            ReleaseState::Held { reason }
        }
    };
    let seq = state::mutate_core(|c| {
        let s = c.next_release_seq;
        c.next_release_seq += 1;
        s
    });
    let r = ReleaseRecord {
        return_id: b.return_id,
        seq,
        burn_op_id: b.burn_op_id,
        solana_sender: b.solana_sender,
        amount_e8s: b.amount_e8s,
        destination: b.destination,
        return_signature: b.return_signature,
        return_slot: b.return_slot,
        burn_signature,
        burn_slot,
        release_fee_e8s: release_fee,
        state: release_state.clone(),
        transfer: None,
        created_at_ns: now,
        updated_at_ns: now,
    };
    state::put_release(&r);
    state::index_release(&r);
    state::append_audit(None, AuditCtx::release(b.return_id, format!("release {seq} opened from burn operation {}", b.burn_op_id)));
    // The hint for this return, if any, has served its purpose.
    state::mutate_core(|c| c.hints.retain(|h| h.signature != b.return_signature));
    Ok(OpenOutcome::Opened(release_state))
}

// ---------------------------------------------------------------------------
// Hints
// ---------------------------------------------------------------------------

/// User hint: "check this Solana transaction sooner". Never settles anything: it is stored,
/// bounded per caller and in all, and expires.
pub fn record_hint(signature_text: String) -> BridgeResult<()> {
    let caller = admin::require_authenticated()?;
    let sig = signature_from_base58(&signature_text).map_err(|e| BridgeError::InvalidArgument(format!("signature: {e:?}")))?;
    let sig = Sig(sig);
    let now = env::now_ns();
    state::mutate_core(|c| {
        c.hints.retain(|x| now.saturating_sub(x.submitted_at_ns) < HINT_TTL_NS);
        if c.hints.iter().any(|x| x.signature == sig) {
            return Ok(());
        }
        if c.hints.iter().filter(|x| x.submitted_by == caller).count() >= MAX_HINTS_PER_CALLER {
            return Err(BridgeError::TooManyActiveOperations);
        }
        if c.hints.len() >= state::MAX_HINTS {
            c.hints.remove(0);
        }
        c.hints.push(Hint { signature: sig, submitted_by: caller, submitted_at_ns: now });
        Ok(())
    })
}

/// The unexpired hints, oldest first (what the return observer reads).
pub fn live_hints(now: u64) -> Vec<Hint> {
    state::read_core(|c| c.hints.iter().filter(|h| now.saturating_sub(h.submitted_at_ns) < HINT_TTL_NS).cloned().collect())
}

// ---------------------------------------------------------------------------
// Driving the ICP transfer
// ---------------------------------------------------------------------------

/// Create (if needed) and drive release transfers.
pub async fn process_releases<L: IcpLedger>(ledger: &L) {
    let paused = state::read_core(|c| c.config.pauses.releases || c.halted.is_some());
    if paused {
        return;
    }
    // The next window of the active index (`state::next_releases`): a release whose transfer is still
    // unresolved, or that waits for the window, delays the ones behind it by one rotation at most.
    let pending: Vec<Hash32> = state::next_releases(state::Queue::Releases, MAX_RELEASES_PER_TICK, |r| matches!(r.state, ReleaseState::Pending))
        .into_iter()
        .map(|r| r.return_id)
        .collect();
    for rid in pending {
        // Re-read: an earlier iteration's await may have let another tick change it.
        let Some(r) = state::get_release(&rid) else { continue };
        if !matches!(r.state, ReleaseState::Pending) {
            continue;
        }
        if r.transfer.is_none() && !prepare_release_op(&rid) {
            continue;
        }
        let op_ref = OpRef::Release(rid);
        let Some(op) = ledger_ops::load_op(&op_ref) else { continue };
        if matches!(op.status, LedgerOpStatus::Pending) && op.attempts == 0 {
            if let Some(sent) = ledger_ops::attempt(ledger, &op_ref).await {
                ledger_ops::apply_outcome(&op_ref, sent);
            }
        } else {
            ledger_ops::reconcile(ledger, &op_ref).await;
        }
    }
}

/// Build the transfer tuple for a pending release, applying the release window. Returns false if
/// the release must wait.
fn prepare_release_op(rid: &Hash32) -> bool {
    let Some(mut r) = state::get_release(rid) else { return false };
    if !matches!(r.state, ReleaseState::Pending) {
        return false;
    }
    // Never replace a tuple that may already have been sent: a second distinct tuple with the same
    // memo would be a second, independent transfer.
    if r.transfer.is_some() {
        return true;
    }
    let now = env::now_ns();
    let canister = env::canister_id();
    let (fee, escrow, tip) = state::read_core(|c| (c.ledger_fee_e8s, c.escrow_account(canister), c.ledger_tip_lower_bound));
    let Some(fee) = fee else { return false };
    let outflow = r.amount_e8s.saturating_sub(r.release_fee_e8s);
    if outflow <= fee {
        // The ledger fee rose after the release was opened: hold for re-mint instead of releasing
        // dust.
        if state::apply_transition(Transition::ReleaseHeld { amount: r.amount_e8s }, AuditCtx::release(*rid, "fees exceed release")).is_ok() {
            r.state = ReleaseState::Held { reason: "fees exceed the released amount".into() };
            r.updated_at_ns = now;
            state::put_release(&r);
        }
        return false;
    }
    // Window: a release that alone exceeds the limit may go out at the start of a fresh window, so
    // no valid release waits forever.
    let window_ok = state::mutate_core(|c| {
        c.roll_window(now);
        let used = c.windows.released_in_window;
        let ok = used == 0 || used.saturating_add(r.amount_e8s as u128) <= c.config.limits.release_window_limit_e8s as u128;
        if ok {
            c.windows.released_in_window = used.saturating_add(r.amount_e8s as u128);
        }
        ok
    });
    if !window_ok {
        return false;
    }
    r.transfer = Some(LedgerOp {
        kind: LedgerOpKind::Release,
        from: escrow,
        to: r.destination,
        spender: None,
        amount_e8s: outflow - fee,
        fee_e8s: Some(fee),
        memo: ids::release_memo(rid),
        created_at_time: now,
        attempts: 0,
        ambiguous_seen: false,
        status: LedgerOpStatus::Pending,
        search_from_block: tip,
        search_next_block: None,
        last_attempt_ns: None,
        outflow_reserved: false,
    });
    r.updated_at_ns = now;
    state::put_release(&r);
    true
}
