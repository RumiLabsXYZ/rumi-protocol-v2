//! The mint saga (49c): from an escrowed deposit to a delivered Solana mint, and from a held release
//! claim to its re-mint.
//!
//! * **Submission.** An `Escrowed` deposit (its pull is proven, backing in `pending_mint`) gets ONE
//!   mint operation in the nonce lane, keyed by its mint id: `lane::submit_op` is idempotent on that
//!   id, so a crash between creating the operation and linking it to the deposit just finds the same
//!   operation next pass. The operation's exact message is persisted before any signature (the lane's
//!   discipline); this module never builds, signs or sends anything itself.
//! * **Delivery.** The accounting moves only when the lane reports the operation `Delivered`
//!   (finalized, `err == null`, and exactly the stored message): `PM -> O` with the deposit fee carved
//!   into `F` (`MintDelivered`), or `PM -> O` for a re-mint (`RemintDelivered`). Applied once, marked
//!   on the operation (`outcome_applied`) in the same synchronous step.
//! * **Re-mints.** A release the ICP side cannot pay (`ReleaseState::Held`) is re-minted to the
//!   Solana sender it came from (`RemintStarted`, `H -> PM`); the sender is fixed by the verified
//!   return, so no claimant can redirect the funds. At most [`MAX_INFLIGHT_REMINTS`] are active, so
//!   they never crowd out users.
//!
//! A held or abandoned operation is not touched here: its resolution is `flows::held`, and a
//! deposit with a live operation is never minted twice.

use crate::accounting::Transition;
use crate::env;
use crate::flows::deposit;
use crate::ids;
use crate::lane::{self, SubmitError, SubmitRequest, TxSigner};
use crate::records::{OpKind, OpPayload, OpState};
use crate::sol_rpc::SolRpc;
use crate::state::{self, AuditCtx, DepositKind, DepositRecord, DepositState, MintDelivery, Queue, ReleaseState};
use crate::tx::builders::validate_recipient;

/// Escrowed deposits turned into mint operations per pass.
pub const MAX_MINT_SUBMITS_PER_PASS: usize = 8;
/// Delivered mints applied to the books per pass.
pub const MAX_MINT_DELIVERIES_PER_PASS: usize = 16;
/// Re-mint deposits created per pass.
pub const MAX_REMINTS_PER_PASS: usize = 4;
/// Re-mints that may be active at once. They bypass admission (they return a held burn to its
/// sender), so without this cap a holder recycling a small amount through unpayable destinations
/// could queue enough of them to delay every user deposit.
pub const MAX_INFLIGHT_REMINTS: u64 = 2;

/// Apply every delivered mint to the books. Synchronous; a pass may run it any number of times.
pub fn apply_deliveries() -> usize {
    let mut applied = 0;
    for d in state::next_deposits(Queue::MintDeliveries, MAX_MINT_DELIVERIES_PER_PASS, |d| {
        matches!(d.state, DepositState::MintInFlight) && d.mint_op.is_some()
    }) {
        let Some(op_id) = d.mint_op else { continue };
        let Some(mut op) = state::get_op(op_id) else { continue };
        let OpState::Delivered { signature, slot, .. } = op.state.clone() else { continue };
        if op.outcome_applied {
            continue;
        }
        // The operation must be this deposit's mint, exactly.
        if Some(op.external_id) != d.mint_id || op.kind != OpKind::Mint || op.amount != d.mint_amount_e8s {
            state::halt(format!("deposit {}: delivered operation {op_id} is not its mint (id, kind or amount differ)", d.id));
            return applied;
        }
        let t = match d.kind {
            DepositKind::UserDeposit => Transition::MintDelivered { amount: d.amount_e8s, fee: d.deposit_fee_e8s },
            DepositKind::Remint { .. } => Transition::RemintDelivered { amount: d.amount_e8s },
        };
        let ctx = AuditCtx { note: format!("mint delivered at slot {slot}"), deposit_id: Some(d.id), op_id: Some(op_id), ..AuditCtx::default() };
        if let Err(e) = state::apply_transition(t, ctx) {
            state::halt(format!("deposit {}: delivery accounting failed: {e}", d.id));
            return applied;
        }
        let now = env::now_ns();
        op.outcome_applied = true;
        state::put_op(&op);
        let mut d = d;
        d.state = DepositState::Delivered;
        d.delivered = Some(MintDelivery { signature, slot });
        d.updated_at_ns = now;
        state::put_deposit(&d);
        if let DepositKind::Remint { return_id } = d.kind {
            // The release this re-mint settles (found by its return id).
            if let Some(mut r) = state::get_release(&return_id) {
                r.state = ReleaseState::Reminted { deposit_id: d.id };
                r.updated_at_ns = now;
                state::put_release(&r);
            }
        }
        applied += 1;
    }
    applied
}

/// Whether new mint operations may be built now.
fn mints_allowed() -> bool {
    state::read_core(|c| c.halted.is_none() && !c.config.pauses.mints && c.tx_profile().is_ok()) && state::lane_halt(crate::records::Lane::Mint).is_none()
}

/// The per-operation cap of a mint: the configured maximum deposit; a re-mint may exceed it, because
/// a held claim can predate a lowered limit and must still be honored for exactly its own amount.
fn cap_for(d: &DepositRecord) -> u64 {
    let max = state::read_core(|c| c.config.limits.max_deposit_e8s);
    match d.kind {
        DepositKind::UserDeposit => max,
        DepositKind::Remint { .. } => max.max(d.mint_amount_e8s),
    }
}

/// Build the mint operation of every escrowed deposit that has none. Idempotent per mint id.
pub async fn submit_mints<R: SolRpc, S: TxSigner>(rpc: &R, signer: &S, now: u64) -> usize {
    if !mints_allowed() {
        return 0;
    }
    let mut submitted = 0;
    for d in state::next_deposits(Queue::MintSubmits, MAX_MINT_SUBMITS_PER_PASS, |d| {
        matches!(d.state, DepositState::Escrowed) && d.mint_op.is_none() && d.abandoned_mint_ops.is_empty() && d.mint_id.is_some()
    }) {
        let Some(mint_id) = d.mint_id else { continue };
        let req = SubmitRequest {
            kind: OpKind::Mint,
            payload: OpPayload::Mint { recipient_wallet: d.solana_recipient, per_operation_cap: cap_for(&d) },
            external_id: mint_id,
            amount: d.mint_amount_e8s,
        };
        match lane::submit_op(rpc, signer, req, now).await {
            Ok(op_id) => {
                // Link it. The deposit may have changed during the await; only an escrowed deposit
                // without an operation takes the link.
                let Some(mut cur) = state::get_deposit(d.id) else { continue };
                if !matches!(cur.state, DepositState::Escrowed) || cur.mint_op.is_some() {
                    continue;
                }
                cur.state = DepositState::MintInFlight;
                cur.mint_op = Some(op_id);
                cur.hold_retry_at_ns = None;
                cur.updated_at_ns = env::now_ns();
                state::put_deposit(&cur);
                state::append_audit(None, AuditCtx { note: "mint operation created".into(), deposit_id: Some(d.id), op_id: Some(op_id), ..AuditCtx::default() });
                submitted += 1;
            }
            // Conditions that clear on their own: the deposit simply stays escrowed.
            Err(SubmitError::LaneHalted(_) | SubmitError::NotConfigured(_) | SubmitError::NoFreeSlot | SubmitError::Busy | SubmitError::Rpc(_)) => {}
            // The request itself is unbuildable (for example an amount above the cap): no operation
            // exists, so the deposit is held and may be refunded.
            Err(SubmitError::InvalidRequest(why)) => deposit::on_mint_preflight_hold(d.id, why),
        }
    }
    submitted
}

/// Turn held releases into re-mint deposits to their Solana senders. Synchronous.
pub fn start_remints() -> usize {
    if !mints_allowed() {
        return 0;
    }
    let (profile, genesis, mint, salt) = state::read_core(|c| (c.tx_profile(), c.config.genesis_hash, c.mint, c.instance_salt));
    let (Ok(profile), Some(mint)) = (profile, mint) else { return 0 };
    let canister = env::canister_id();
    let mut started = 0;
    for mut r in state::next_releases(Queue::Remints, MAX_REMINTS_PER_PASS, |r| matches!(r.state, ReleaseState::Held { .. })) {
        if state::active_remint_count() >= MAX_INFLIGHT_REMINTS {
            break;
        }
        // The sender is a wallet the bridge can mint to; if it is not (a program id, one of our own
        // keys), the claim stays held for a human.
        if validate_recipient(&profile, &r.solana_sender).is_err() {
            continue;
        }
        let now = env::now_ns();
        let id = state::mutate_core(|c| {
            let id = c.next_deposit_id;
            c.next_deposit_id += 1;
            id
        });
        let ctx = AuditCtx { note: format!("re-mint as deposit {id}"), return_id: Some(r.return_id), deposit_id: Some(id), ..AuditCtx::default() };
        if let Err(e) = state::apply_transition(Transition::RemintStarted { amount: r.amount_e8s }, ctx) {
            state::halt(format!("re-mint accounting: {e}"));
            return started;
        }
        state::put_deposit(&DepositRecord {
            id,
            kind: DepositKind::Remint { return_id: r.return_id },
            owner: canister,
            from: None,
            client_request_id: r.return_id.to_vec(),
            solana_recipient: r.solana_sender,
            amount_e8s: r.amount_e8s,
            deposit_fee_e8s: 0,
            mint_amount_e8s: r.amount_e8s,
            mint_id: Some(ids::remint_id(&genesis, &mint, &salt, &r.return_id)),
            state: DepositState::Escrowed,
            pull: None,
            refund: None,
            mint_op: None,
            abandoned_mint_ops: vec![],
            delivered: None,
            hold_retry_at_ns: None,
            created_at_ns: now,
            updated_at_ns: now,
        });
        r.state = ReleaseState::Reminting { deposit_id: id };
        r.updated_at_ns = now;
        state::put_release(&r);
        started += 1;
    }
    started
}

/// One pass of the mint saga: apply what was delivered, start re-mints, build new operations.
pub async fn process_mints<R: SolRpc, S: TxSigner>(rpc: &R, signer: &S, now: u64) {
    apply_deliveries();
    start_remints();
    submit_mints(rpc, signer, now).await;
    // Operations may have been delivered by the lane tick that ran just before this pass.
    apply_deliveries();
}
