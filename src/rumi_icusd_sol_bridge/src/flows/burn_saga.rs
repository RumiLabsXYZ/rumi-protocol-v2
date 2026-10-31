//! The burn lane and the stray refund lane (49d, `chain-solana.md` sections 2.3, 2.4, 3 and 13).
//!
//! **Returns: `RC -> BU -> PR`.** A custodied return gets ONE burn operation in the burn lane, keyed
//! by `burn_id([return_id])` (v1 burns one return per transaction). The operation's exact message is
//! persisted before it is signed; the books move `RC -> BU` once it exists. The release is payable
//! only after the burn is final:
//!
//! * the lane reports the operation `Delivered`: finalized, `err == null` and exactly the stored
//!   `BurnChecked` message (the lane's own proof, never the global supply, which a holder burn can
//!   also lower);
//! * this module then reads that transaction's metadata and requires the return account's balance
//!   effect: it fell by exactly the amount and no other token account of the mint changed;
//! * only then do `BurnFinalized` (`BU -> PR`) and `release::open_release` happen, in one synchronous
//!   step, with the hand-over built from the stored verified return. `open_release` binds the burn to
//!   that return by `burn_id([return_id])`, so an unrelated burn of an equal amount can never
//!   authorize another return or recipient.
//!
//! A burn the lane re-issues stays in `BU` (the tokens are still in custody and the same operation
//! is still the burn of that return); there is no automatic `BU -> RC`, because a burn id names one
//! operation forever.
//!
//! **Strays: refunds.** Each attributable incoming transfer of a stray transaction is a claim bound to
//! its observed source account and amount, with a refund id that carries the transfer's position
//! (`ids::refund_id`, never the amount or the recipient). A claim with a verified sender and at least
//! the minimum gets one refund operation (burn lane, `rumi1|f|<refund_id>`): to the original source
//! account if it still exists as that sender's account of this mint, else to the sender's associated
//! account created in the same transaction. Its delivery is verified like a burn (the return account
//! fell by the amount, the destination rose by it, nothing else changed) before `StrayRefunded`
//! reduces `stray`. Everything else stays in `stray` for manual review.

use crate::accounting::Transition;
use crate::env;
use crate::flows::release::{self, BurnedReturn, OpenError};
use crate::ids;
use crate::lane::{self, SubmitError, SubmitRequest, TxSigner};
use crate::mint::{parse_token_account, EXT_IMMUTABLE_OWNER};
use crate::observer::{balance_at, mint_indexes};
use crate::records::{OpKind, OpPayload, OpState};
use crate::sol_rpc::{LandedTx, SolRpc};
use crate::state::{self, AuditCtx, ClaimState, Queue, ReturnState};
use crate::tx::builders::RefundDestination;
use crate::tx::{decode_message, derive_ata, split_wire_transaction, Pubkey};

/// Burn operations built per pass.
pub const MAX_BURN_SUBMITS_PER_PASS: usize = 8;
/// Delivered burns verified and applied per pass.
pub const MAX_BURN_APPLIES_PER_PASS: usize = 8;
/// Refund claims handled per pass (submitted or applied).
pub const MAX_CLAIMS_PER_PASS: usize = 8;

/// What a final transaction did to the token accounts of the mint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Exactly the expected accounts changed by exactly the expected amounts.
    Verified,
    /// The RPC did not report what is needed to tell (try again).
    Unavailable(String),
    /// The effect is not the expected one: an anomaly.
    Mismatch(String),
}

/// Check a final own transaction's balance effect: the return account `ata` fell by `amount`, the
/// optional `destination` rose by it, and no other token account of `mint` changed.
pub fn verify_effect(tx: &LandedTx, mint: &Pubkey, ata: &Pubkey, destination: Option<&Pubkey>, amount: u64) -> Effect {
    if tx.failed {
        return Effect::Mismatch("the transaction failed".into());
    }
    let (Some(pre), Some(post)) = (tx.meta.pre_token_balances.as_deref(), tx.meta.post_token_balances.as_deref()) else {
        return Effect::Unavailable("token balances are not reported".into());
    };
    let Ok((_, message_bytes)) = split_wire_transaction(&tx.wire) else { return Effect::Mismatch("the transaction bytes do not parse".into()) };
    let Ok(msg) = decode_message(message_bytes) else { return Effect::Mismatch("the message does not decode strictly".into()) };
    let index_of = |k: &Pubkey| msg.account_keys.iter().position(|x| x == k).map(|i| i as u8);
    let Some(ata_index) = index_of(ata) else { return Effect::Mismatch("the return account is not in the transaction".into()) };
    let dest_index = match destination {
        Some(d) => match index_of(d) {
            Some(i) => Some(i),
            None => return Effect::Mismatch("the destination account is not in the transaction".into()),
        },
        None => None,
    };
    let changed: Vec<u8> = mint_indexes(pre, post, mint).into_iter().filter(|i| balance_at(pre, mint, *i) != balance_at(post, mint, *i)).collect();
    let mut expected: Vec<u8> = std::iter::once(ata_index).chain(dest_index).collect();
    expected.sort_unstable();
    if changed != expected {
        return Effect::Mismatch(format!("token accounts {changed:?} changed, expected exactly {expected:?}"));
    }
    if balance_at(pre, mint, ata_index).checked_sub(balance_at(post, mint, ata_index)) != Some(amount) {
        return Effect::Mismatch(format!("the return account did not fall by exactly {amount}"));
    }
    if let Some(d) = dest_index {
        if balance_at(post, mint, d).checked_sub(balance_at(pre, mint, d)) != Some(amount) {
            return Effect::Mismatch(format!("the destination did not rise by exactly {amount}"));
        }
    }
    Effect::Verified
}

fn burns_allowed() -> bool {
    state::read_core(|c| c.halted.is_none() && c.tx_profile().is_ok()) && state::lane_halt(crate::records::Lane::Burn).is_none()
}

// ---------------------------------------------------------------------------
// Returns: RC -> BU
// ---------------------------------------------------------------------------

/// Build the burn operation of every custodied return that has none. Idempotent per burn id.
pub async fn submit_burns<R: SolRpc, S: TxSigner>(rpc: &R, signer: &S, now: u64) -> usize {
    if !burns_allowed() {
        return 0;
    }
    let (genesis, mint, salt) = state::read_core(|c| (c.config.genesis_hash, c.mint, c.instance_salt));
    let Some(mint) = mint else { return 0 };
    let mut submitted = 0;
    for r in state::next_returns(Queue::Returns, MAX_BURN_SUBMITS_PER_PASS, |r| r.state == ReturnState::Custodied) {
        let Ok(burn_id) = ids::burn_id(&genesis, &mint, &salt, &[r.return_id]) else { continue };
        let req = SubmitRequest { kind: OpKind::Burn, payload: OpPayload::Burn, external_id: burn_id, amount: r.amount };
        match lane::submit_op(rpc, signer, req, now).await {
            Ok(op_id) => {
                // The operation exists and its exact message is persisted. Link it and move RC -> BU
                // in one synchronous step, only if the return still waits for it.
                let Some(mut cur) = state::get_return(&r.return_id) else { continue };
                if cur.state != ReturnState::Custodied {
                    continue;
                }
                let Some(op) = state::get_op(op_id) else { continue };
                if op.kind != OpKind::Burn || op.amount != cur.amount || op.external_id != burn_id {
                    state::halt(format!("return {}: operation {op_id} is not its burn (kind, amount or id differ)", hex::encode(cur.return_id)));
                    return submitted;
                }
                let ctx = AuditCtx { note: "burn operation created".into(), return_id: Some(cur.return_id), op_id: Some(op_id), ..AuditCtx::default() };
                if let Err(e) = state::apply_transition(Transition::BurnSigned { amount: cur.amount }, ctx) {
                    state::halt(format!("return {}: custody accounting failed: {e}", hex::encode(cur.return_id)));
                    return submitted;
                }
                cur.state = ReturnState::Burning { burn_op: op_id };
                cur.updated_at_ns = env::now_ns();
                state::put_return(&cur);
                submitted += 1;
            }
            // Conditions that clear on their own: the return simply stays custodied.
            Err(SubmitError::LaneHalted(_) | SubmitError::NotConfigured(_) | SubmitError::NoFreeSlot | SubmitError::Busy | SubmitError::Rpc(_) | SubmitError::NotAdmitted(_)) => {}
            Err(SubmitError::InvalidRequest(why)) => {
                state::halt(format!("return {}: the burn cannot be built: {why}", hex::encode(r.return_id)));
                return submitted;
            }
        }
    }
    submitted
}

// ---------------------------------------------------------------------------
// Returns: BU -> PR
// ---------------------------------------------------------------------------

/// Verify every delivered burn's balance effect, then `BU -> PR` and open the release.
pub async fn apply_burns<R: SolRpc>(rpc: &R) -> usize {
    let (profile, ledger_facts) = state::read_core(|c| (c.tx_profile(), c.ledger_fee_e8s.is_some() && c.ledger_minting_account.is_some()));
    let Ok(profile) = profile else { return 0 };
    let Ok(ata) = profile.return_ata() else { return 0 };
    let mut applied = 0;
    for r in state::next_returns(Queue::BurnApplies, MAX_BURN_APPLIES_PER_PASS, |r| matches!(r.state, ReturnState::Burning { .. })) {
        let ReturnState::Burning { burn_op } = r.state else { continue };
        let Some(op) = state::get_op(burn_op) else { continue };
        let OpState::Delivered { signature, .. } = op.state.clone() else { continue };
        if op.outcome_applied {
            continue;
        }
        // The release classifies the destination against the ledger's facts; without them nothing is
        // applied yet (a later pass retries) so no step is left half done.
        if !ledger_facts {
            continue;
        }
        let tx = match rpc.get_transaction(&signature.0).await {
            Ok(Some(tx)) => tx,
            Ok(None) | Err(_) => continue,
        };
        match verify_effect(&tx, &profile.mint, &ata, None, r.amount) {
            Effect::Verified => {}
            Effect::Unavailable(_) => continue,
            Effect::Mismatch(why) => {
                state::halt(format!("return {}: burn operation {burn_op} is final but its balance effect is wrong: {why}", hex::encode(r.return_id)));
                return applied;
            }
        }
        // ---- one synchronous step: BU -> PR, the release, the marks ----
        let Some(mut cur) = state::get_return(&r.return_id) else { continue };
        let Some(mut op) = state::get_op(burn_op) else { continue };
        if cur.state != (ReturnState::Burning { burn_op }) || op.outcome_applied {
            continue;
        }
        let ctx = AuditCtx { note: "burn final".into(), return_id: Some(cur.return_id), op_id: Some(burn_op), ..AuditCtx::default() };
        if let Err(e) = state::apply_transition(Transition::BurnFinalized { amount: cur.amount }, ctx) {
            state::halt(format!("return {}: burn accounting failed: {e}", hex::encode(cur.return_id)));
            return applied;
        }
        match release::open_release(&BurnedReturn::from_return(&cur, burn_op)) {
            Ok(_) => {}
            Err(OpenError::LedgerFactsUnknown) => {
                // Unreachable after the check above; treated as an anomaly rather than left half done.
                state::halt(format!("return {}: the ledger facts vanished while opening the release", hex::encode(cur.return_id)));
                return applied;
            }
            Err(e) => {
                state::halt(format!("return {}: the release could not be opened after its burn: {e:?}", hex::encode(cur.return_id)));
                return applied;
            }
        }
        op.outcome_applied = true;
        state::put_op(&op);
        cur.state = ReturnState::Burned { burn_op };
        cur.updated_at_ns = env::now_ns();
        state::put_return(&cur);
        applied += 1;
    }
    applied
}

// ---------------------------------------------------------------------------
// Strays: refund claims
// ---------------------------------------------------------------------------

/// Where a claim's refund goes, decided from the source account as it is now.
enum Destination {
    Use(RefundDestination),
    /// The source account exists but is not a plain account of this sender: retained for review.
    Retain(String),
}

async fn refund_destination<R: SolRpc>(rpc: &R, profile: &crate::tx::builders::TxProfile, source: &Pubkey, sender: &Pubkey) -> Result<Destination, String> {
    match rpc.get_account(source).await {
        Err(e) => Err(format!("getAccountInfo: {e:?}")),
        // The source account is gone (closed): the sender's associated account is created with the refund.
        Ok(None) => Ok(Destination::Use(RefundDestination::CreateAta { owner: *sender })),
        Ok(Some(snap)) => {
            if snap.owner != profile.token_program.id() {
                return Ok(Destination::Retain("the source account is not owned by the token program".into()));
            }
            match parse_token_account(&snap.data, profile.token_program) {
                Ok(v) if v.mint == profile.mint && v.owner == *sender && v.state == 1 && v.delegate.is_none() && v.close_authority.is_none() && !v.is_native => {
                    if v.extensions.iter().any(|(k, _)| *k != EXT_IMMUTABLE_OWNER) {
                        return Ok(Destination::Retain("the source account has extensions a refund does not take".into()));
                    }
                    Ok(Destination::Use(RefundDestination::ExistingAccount { token_account: *source }))
                }
                Ok(_) => Ok(Destination::Retain("the source account is not an ordinary initialized account of this sender".into())),
                Err(e) => Ok(Destination::Retain(format!("the source account does not parse: {e}"))),
            }
        }
    }
}

/// Build the refund operation of every refundable claim that has none, and apply the delivered ones.
pub async fn refund_claims<R: SolRpc, S: TxSigner>(rpc: &R, signer: &S, now: u64) -> usize {
    let Ok(profile) = state::read_core(|c| c.tx_profile()) else { return 0 };
    let Ok(ata) = profile.return_ata() else { return 0 };
    let mut done = 0;
    for st in state::next_strays(Queue::Strays, MAX_CLAIMS_PER_PASS, |s| s.has_open_claim()) {
        for (i, claim) in st.claims.iter().enumerate() {
            match claim.state {
                ClaimState::Held if claim.refundable() && burns_allowed() => {
                    done += submit_claim(rpc, signer, &profile, st.signature, i, now).await;
                }
                ClaimState::Refunding { op_id } => {
                    done += apply_claim(rpc, &profile, &ata, st.signature, i, op_id).await;
                }
                _ => {}
            }
        }
    }
    done
}

async fn submit_claim<R: SolRpc, S: TxSigner>(
    rpc: &R,
    signer: &S,
    profile: &crate::tx::builders::TxProfile,
    stray_sig: crate::records::Sig,
    index: usize,
    now: u64,
) -> usize {
    let Some(st) = state::get_stray(&stray_sig) else { return 0 };
    let Some(claim) = st.claims.get(index).cloned() else { return 0 };
    let Some(sender) = claim.verified_sender else { return 0 };
    // The tokens must be accounted as stray: a refund never takes more than the books hold.
    if (state::read_core(|c| c.buckets.stray)) < claim.amount as u128 {
        return 0;
    }
    let destination = match refund_destination(rpc, profile, &claim.source_account, &sender).await {
        Ok(Destination::Use(d)) => d,
        Ok(Destination::Retain(why)) => {
            retain_claim(stray_sig, index, &why);
            return 0;
        }
        Err(_) => return 0,
    };
    let req = SubmitRequest {
        kind: OpKind::Refund,
        payload: OpPayload::Refund { destination },
        external_id: claim.refund_id,
        amount: claim.amount,
    };
    match lane::submit_op(rpc, signer, req, now).await {
        Ok(op_id) => {
            // Link, only if the claim is still waiting for it.
            let Some(mut cur) = state::get_stray(&stray_sig) else { return 0 };
            let Some(c) = cur.claims.get_mut(index) else { return 0 };
            if c.state != ClaimState::Held || c.refund_id != claim.refund_id {
                return 0;
            }
            let Some(op) = state::get_op(op_id) else { return 0 };
            if op.kind != OpKind::Refund || op.amount != c.amount || op.external_id != c.refund_id {
                state::halt(format!("stray claim {}: operation {op_id} is not its refund (kind, amount or id differ)", hex::encode(c.refund_id)));
                return 0;
            }
            c.state = ClaimState::Refunding { op_id };
            state::put_stray(&mut cur);
            state::append_audit(None, AuditCtx { note: "stray refund operation created".into(), op_id: Some(op_id), ..AuditCtx::default() });
            1
        }
        Err(SubmitError::InvalidRequest(why)) => {
            // For example a destination that is one of our own keys: retained, not refunded.
            retain_claim(stray_sig, index, &why);
            0
        }
        Err(_) => 0,
    }
}

/// Keep a claim in `stray` for manual review: the refund was considered and refused.
fn retain_claim(stray_sig: crate::records::Sig, index: usize, why: &str) {
    let Some(mut st) = state::get_stray(&stray_sig) else { return };
    let Some(c) = st.claims.get_mut(index) else { return };
    if c.state != ClaimState::Held {
        return;
    }
    c.state = ClaimState::Retained;
    let id = hex::encode(c.refund_id);
    state::put_stray(&mut st);
    state::append_audit(None, AuditCtx::note(format!("stray claim {id} is retained for manual review: {why}")));
}

async fn apply_claim<R: SolRpc>(
    rpc: &R,
    profile: &crate::tx::builders::TxProfile,
    ata: &Pubkey,
    stray_sig: crate::records::Sig,
    index: usize,
    op_id: u64,
) -> usize {
    let Some(op) = state::get_op(op_id) else { return 0 };
    let OpState::Delivered { signature, .. } = op.state.clone() else { return 0 };
    if op.outcome_applied {
        return 0;
    }
    let OpPayload::Refund { destination } = &op.payload else { return 0 };
    let dest_account = match destination {
        RefundDestination::ExistingAccount { token_account } => *token_account,
        RefundDestination::CreateAta { owner } => match derive_ata(owner, &profile.token_program.id(), &profile.mint) {
            Ok(a) => a,
            Err(_) => return 0,
        },
    };
    let tx = match rpc.get_transaction(&signature.0).await {
        Ok(Some(tx)) => tx,
        Ok(None) | Err(_) => return 0,
    };
    match verify_effect(&tx, &profile.mint, ata, Some(&dest_account), op.amount) {
        Effect::Verified => {}
        Effect::Unavailable(_) => return 0,
        Effect::Mismatch(why) => {
            state::halt(format!("stray refund operation {op_id} is final but its balance effect is wrong: {why}"));
            return 0;
        }
    }
    // ---- one synchronous step: StrayRefunded, the marks ----
    let (Some(mut st), Some(mut op)) = (state::get_stray(&stray_sig), state::get_op(op_id)) else { return 0 };
    let Some(c) = st.claims.get_mut(index) else { return 0 };
    if c.state != (ClaimState::Refunding { op_id }) || op.outcome_applied || op.amount != c.amount {
        return 0;
    }
    let ctx = AuditCtx { note: "stray refunded".into(), op_id: Some(op_id), ..AuditCtx::default() };
    if let Err(e) = state::apply_transition(Transition::StrayRefunded { amount: c.amount }, ctx) {
        state::halt(format!("stray refund operation {op_id}: accounting failed: {e}"));
        return 0;
    }
    c.state = ClaimState::Refunded { op_id };
    op.outcome_applied = true;
    state::put_op(&op);
    state::put_stray(&mut st);
    1
}

/// One pass of the burn lane: apply what was delivered, build new burns, then the stray refunds.
pub async fn process_burns<R: SolRpc, S: TxSigner>(rpc: &R, signer: &S, now: u64) {
    apply_burns(rpc).await;
    submit_burns(rpc, signer, now).await;
    refund_claims(rpc, signer, now).await;
}
