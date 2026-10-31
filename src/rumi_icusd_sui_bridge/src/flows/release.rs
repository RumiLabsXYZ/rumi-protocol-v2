//! Sui -> ICP releases (53a): opening a release from a verified final return, driving the ICP
//! transfer, and the bounded return hints. Ported from the frozen Solana instance
//! (`flows/release.rs` at 4394b472) with the burn-lane binding replaced by the Gate's own durable
//! record: on Sui the coin is burned by the `bridge_out` transaction itself, so a return is final
//! the moment it is verified.
//!
//! **No payout before a final return.** `open_release` is the ONLY function that creates a release
//! record, and it takes a [`VerifiedReturn`]: the fields of the Gate's `ReturnRecord`
//! (`returns[transfer_id]`, read at a finalized checkpoint by the provider quorum and decoded by
//! `gate::decode_return_record`, 53c). In one synchronous step it applies `ReturnFinalized`
//! (`O -> PR`), creates the release keyed by `transfer_id`, and indexes it, so:
//!
//! * a release record cannot exist without a verified return, and the accounting has no transition
//!   from `O`, `PM` or `H` straight to a payout;
//! * one return makes at most one release: a second call with the same `transfer_id` is
//!   `AlreadyOpen` and changes nothing; a call that disagrees with the stored record, or a second
//!   `transfer_id` for the same Gate sequence, halts the bridge;
//! * the destination is the ICP account the Gate saved on chain, copied verbatim and immutable. A
//!   saved account that cannot receive a payment (the anonymous principal, this canister, the
//!   ledger's minting account) or an amount that does not cover the fees opens the release in
//!   `Held`, never dropped: the coin is already burned, so the value is re-minted to the Sui
//!   sender (53b), not lost.

use super::ledger_ops::{self, OpRef};
use crate::accounting::Transition;
use crate::admin;
use crate::env;
use crate::icp::IcpLedger;
use crate::ids::{self, Hash32};
use crate::state::{self, AuditCtx, Hint, LedgerOp, LedgerOpKind, LedgerOpStatus, ReleaseRecord, ReleaseState};
use crate::sui::parse_digest_base58;
use crate::types::*;
use candid::Principal;
use icrc_ledger_types::icrc1::account::Account;

pub const MAX_RELEASES_PER_TICK: usize = 10;
/// Hints older than this are dropped.
pub const HINT_TTL_NS: u64 = 2 * 3_600 * 1_000_000_000;
/// Pending hints kept per caller.
pub const MAX_HINTS_PER_CALLER: usize = 5;

/// The longest principal the Gate accepts (`PRINCIPAL_MAX_LEN` in the Move module).
const MAX_PRINCIPAL_LEN: usize = 29;
const SUBACCOUNT_LEN: usize = 32;

/// A return verified from the Gate's durable `ReturnRecord` at a finalized checkpoint. Built by the
/// return observer (53c) from the decoded record and the transaction it names; nothing here is an
/// event field alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedReturn {
    /// The Gate's `transfer_id`, recomputed by the canister from the genesis, gate, sender and nonce.
    pub transfer_id: Hash32,
    /// The Gate's `bridge_out` sequence of this return (first is 1).
    pub gate_sequence: u64,
    /// The `bridge_out` transaction's sender.
    pub sui_sender: [u8; 32],
    pub client_nonce: u64,
    /// `B`: the amount burned.
    pub amount_e8s: u64,
    /// The exact principal bytes and subaccount the Gate saved.
    pub icp_principal: Vec<u8>,
    pub icp_subaccount: Vec<u8>,
    /// The typed transaction digest of the `bridge_out` transaction and the finalized checkpoint the
    /// record was verified at.
    pub tx_digest: Hash32,
    pub checkpoint: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenOutcome {
    Opened(ReleaseState),
    AlreadyOpen,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenError {
    /// The record breaks a rule the Gate enforces on chain (amount, principal length, subaccount
    /// length). A genuine gate cannot produce it, so the caller must treat it as a failed
    /// verification, not as a return.
    Malformed(String),
    /// `O` does not yet cover the amount: the return is of coin whose mint the books have not applied
    /// yet. Nothing was changed; retry after the mint delivery is applied.
    NotYetOutstanding { outstanding: u128, amount: u64 },
    /// The ledger fee or the minting account is not known yet: nothing was changed; retry.
    LedgerFactsUnknown,
    /// A different release already exists for this `transfer_id` or this Gate sequence (the bridge
    /// halts).
    Conflict(String),
    /// The pending-release bucket does not cover this release on top of those already owed.
    NotFunded { pending_release: u128, owed: u128, amount: u64 },
    Accounting(String),
}

fn same_account(a: &Account, b: &Account) -> bool {
    a.owner == b.owner && a.subaccount.unwrap_or([0u8; 32]) == b.subaccount.unwrap_or([0u8; 32])
}

/// The ICP account the saved bytes name, or why the release must be held.
fn destination_of(principal: &[u8], subaccount: &[u8], canister: Principal) -> (Account, Option<String>) {
    let placeholder = Account { owner: Principal::anonymous(), subaccount: None };
    let Ok(owner) = Principal::try_from_slice(principal) else {
        return (placeholder, Some("the saved ICP principal is not a valid principal".into()));
    };
    let sub: Option<[u8; 32]> = <[u8; 32]>::try_from(subaccount).ok().filter(|s| *s != [0u8; 32]);
    let account = Account { owner, subaccount: sub };
    if owner == Principal::anonymous() {
        return (placeholder, Some("the saved ICP destination is the anonymous principal".into()));
    }
    if owner == Principal::management_canister() {
        return (placeholder, Some("the saved ICP destination is the management canister".into()));
    }
    if owner == canister {
        return (account, Some("the saved ICP destination is this bridge canister".into()));
    }
    (account, None)
}

/// Open the release of a verified final return. Idempotent on the `transfer_id`. See the module docs
/// for what it checks and what it guarantees.
pub fn open_release(v: &VerifiedReturn) -> Result<OpenOutcome, OpenError> {
    if let Some(existing) = state::get_release(&v.transfer_id) {
        let same = existing.gate_sequence == v.gate_sequence
            && existing.sui_sender == v.sui_sender
            && existing.client_nonce == v.client_nonce
            && existing.amount_e8s == v.amount_e8s
            && existing.icp_principal == v.icp_principal
            && existing.icp_subaccount == v.icp_subaccount
            && existing.tx_digest == v.tx_digest;
        if !same {
            let why = format!("return {} was opened twice with different contents", hex::encode(v.transfer_id));
            state::halt(why.clone());
            return Err(OpenError::Conflict(why));
        }
        return Ok(OpenOutcome::AlreadyOpen);
    }
    if let Some(other) = state::release_by_sequence(v.gate_sequence) {
        let why = format!("Gate sequence {} already opened release {}", v.gate_sequence, hex::encode(other));
        state::halt(why.clone());
        return Err(OpenError::Conflict(why));
    }
    if v.amount_e8s == 0 || v.gate_sequence == 0 {
        return Err(OpenError::Malformed("a return has a zero amount or sequence".into()));
    }
    if v.icp_principal.is_empty() || v.icp_principal.len() > MAX_PRINCIPAL_LEN || v.icp_principal == [0x04] {
        return Err(OpenError::Malformed("the saved ICP principal breaks the Gate's own bounds".into()));
    }
    if v.icp_subaccount.len() != SUBACCOUNT_LEN {
        return Err(OpenError::Malformed("the saved ICP subaccount is not 32 bytes".into()));
    }

    let now = env::now_ns();
    let canister = env::canister_id();
    let (release_fee, ledger_fee, minting, outstanding) =
        state::read_runtime(|c| (c.config.fees.release_fee_e8s, c.ledger_fee_e8s, c.ledger_minting_account, c.buckets.outstanding));
    let (Some(ledger_fee), Some(minting)) = (ledger_fee, minting) else {
        return Err(OpenError::LedgerFactsUnknown);
    };
    if outstanding < v.amount_e8s as u128 {
        return Err(OpenError::NotYetOutstanding { outstanding, amount: v.amount_e8s });
    }
    let (destination, bad_destination) = destination_of(&v.icp_principal, &v.icp_subaccount, canister);
    let hold: Option<String> = if let Some(why) = bad_destination {
        Some(why)
    } else if same_account(&destination, &minting) {
        Some("destination is the icUSD minting account (a transfer there would burn the funds)".into())
    } else if (v.amount_e8s as u128) <= release_fee as u128 + ledger_fee as u128 {
        Some(format!("amount {} e8s does not cover the bridge fee ({release_fee}) and ledger fee ({ledger_fee})", v.amount_e8s))
    } else {
        None
    };

    // The books: `ReturnFinalized` (O -> PR), then `ReleaseHeld` (PR -> H) when held, all or nothing.
    let mut transitions = vec![Transition::ReturnFinalized { amount: v.amount_e8s }];
    if hold.is_some() {
        transitions.push(Transition::ReleaseHeld { amount: v.amount_e8s });
    }
    state::apply_transitions(&transitions, AuditCtx::release(v.transfer_id, "return verified and final on Sui"))
        .map_err(|e| OpenError::Accounting(e.to_string()))?;
    // `PR` must cover every pending release already owed plus this one.
    let (pr, owed) = (state::read_runtime(|c| c.buckets.pending_release), state::pending_release_total());
    let this = if hold.is_none() { v.amount_e8s as u128 } else { 0 };
    if pr < owed.saturating_add(this) {
        let why = format!("pending release {pr} does not cover the {owed} already owed plus {this}");
        state::halt(why);
        return Err(OpenError::NotFunded { pending_release: pr, owed, amount: v.amount_e8s });
    }
    let release_state = match hold {
        None => ReleaseState::Pending,
        Some(reason) => ReleaseState::Held { reason },
    };
    let seq = state::mutate_runtime(|c| {
        let s = c.next_release_seq;
        c.next_release_seq += 1;
        s
    });
    let r = ReleaseRecord {
        return_id: v.transfer_id,
        seq,
        gate_sequence: v.gate_sequence,
        sui_sender: v.sui_sender,
        client_nonce: v.client_nonce,
        amount_e8s: v.amount_e8s,
        destination,
        icp_principal: v.icp_principal.clone(),
        icp_subaccount: v.icp_subaccount.clone(),
        tx_digest: v.tx_digest,
        checkpoint: v.checkpoint,
        release_fee_e8s: release_fee,
        state: release_state.clone(),
        transfer: None,
        created_at_ns: now,
        updated_at_ns: now,
    };
    state::put_release(&r);
    state::index_release(&r);
    state::append_audit(
        None,
        AuditCtx::release(v.transfer_id, format!("release {seq} opened for Gate sequence {} ({:?})", v.gate_sequence, release_state)),
    );
    // The hint for this return, if any, has served its purpose.
    state::mutate_runtime(|c| c.hints.retain(|h| h.digest != v.tx_digest));
    Ok(OpenOutcome::Opened(release_state))
}

// ---------------------------------------------------------------------------
// Hints
// ---------------------------------------------------------------------------

/// User hint: "check this Sui transaction sooner" (a base58 typed transaction digest). Never settles
/// anything: it is stored, bounded per caller and in all, and expires.
pub fn record_hint(digest_text: String) -> BridgeResult<()> {
    let caller = admin::require_authenticated()?;
    let digest = parse_digest_base58(digest_text.trim()).map_err(|e| BridgeError::InvalidArgument(format!("digest: {e}")))?;
    let now = env::now_ns();
    state::mutate_runtime(|c| {
        c.hints.retain(|x| now.saturating_sub(x.submitted_at_ns) < HINT_TTL_NS);
        if c.hints.iter().any(|x| x.digest == digest) {
            return Ok(());
        }
        if c.hints.iter().filter(|x| x.submitted_by == caller).count() >= MAX_HINTS_PER_CALLER {
            return Err(BridgeError::TooManyActiveOperations);
        }
        if c.hints.len() >= state::MAX_HINTS {
            c.hints.remove(0);
        }
        c.hints.push(Hint { digest, submitted_by: caller, submitted_at_ns: now });
        Ok(())
    })
}

/// The unexpired hints, oldest first (what the return observer reads).
pub fn live_hints(now: u64) -> Vec<Hint> {
    state::read_runtime(|c| c.hints.iter().filter(|h| now.saturating_sub(h.submitted_at_ns) < HINT_TTL_NS).cloned().collect())
}

// ---------------------------------------------------------------------------
// Driving the ICP transfer
// ---------------------------------------------------------------------------

/// Create (if needed) and drive release transfers.
pub async fn process_releases<L: IcpLedger>(ledger: &L) {
    let paused = state::read_runtime(|c| c.config.pauses.releases || c.halted.is_some());
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
    let (fee, escrow, tip) = state::read_runtime(|c| (c.ledger_fee_e8s, c.escrow_account(canister), c.ledger_tip_lower_bound));
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
    let window_ok = state::mutate_runtime(|c| {
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
