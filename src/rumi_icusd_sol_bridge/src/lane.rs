//! The durable-nonce lane: persist before sign, sign through a seam, record every signature
//! before any send, rebroadcast the SAME bytes, and reconcile every tick as the SAME attempt
//! (`chain-solana.md` section 4 and section 12 item 1).
//!
//! # Rules this module enforces
//!
//! 1. **Persist before sign.** An attempt `{op id, external id, recipient, amount, nonce account,
//!    nonce value, exact unsigned message bytes, signer list}` is written to stable state, with
//!    the slot reservation, in one synchronous block BEFORE the first call to the signer.
//! 2. **Sign through a seam** ([`TxSigner`]). Every signature received is recorded as it arrives,
//!    before the next signer is called and before any send. A lost or ambiguous sign reply is not
//!    an error to recover from by rebuilding: the next tick re-signs the SAME stored bytes.
//! 3. **Broadcast the same bytes.** The wire transaction is assembled from the recorded
//!    signatures; its transaction id is recorded BEFORE the send; every send outcome (including
//!    "already processed") is "maybe sent" and changes nothing but a note.
//! 4. **Reconcile as the same attempt**, each tick, per operation:
//!    * the nonce account is read at `finalized` and checked (owner System, authority fee payer);
//!      a failed check halts the lane;
//!    * **value unchanged**: no attempt landed as of `finalized`. Rebroadcast the same bytes,
//!      never build a new nonce. After `cancel_after` persist and sign an advance-only message on
//!      the same nonce as another attempt;
//!    * **value advanced**: look up every recorded transaction id (`getTransaction`, finalized).
//!      Landed with `err == null` and message bytes equal to the recorded bytes: delivered (the
//!      original) or cancelled (the cancel). Landed with `err != null`: not delivered, the same id
//!      may be re-signed on the new value. Not found: list the nonce account's signatures,
//!      verify each candidate singly, and adopt only a finalized, successful transaction whose
//!      message bytes equal a stored unsigned message. A memo is discovery only (it merely orders
//!      the candidates). A different or unverifiable consumer holds the operation and halts the
//!      lane: no replacement and no refund;
//!    * `null` and `Inconsistent` wait. Absence never proves non-delivery once the nonce advanced.
//!      The only proofs of non-delivery are our own cancel landing, and one of our attempts
//!      landing with an error; both consumed the nonce, which excludes every other transaction
//!      bound to that value.
//! 5. **Halts**: a foreign or unverifiable consumer, the nonce advanced with no consumer found
//!    within `history_horizon` (UNMEASURED, blocker `solana-history-availability-unmeasured`), a
//!    changed nonce owner or authority. A halted lane signs, sends and re-issues nothing; every
//!    signed attempt stays recorded. The halt is a fence checked AFTER every await that can lag
//!    behind another operation's halt: before each signer call, before each send, after the nonce
//!    read of a tick, and before a cancel. A signature that comes back after a halt is still
//!    recorded (it is evidence); it is just not followed by another sign or a send.
//! 6. **Slot identity.** A preparation reads a `Free` slot's account across an await. Reservation
//!    is committed only if the slot is still `Free` at the generation seen when the preparation
//!    began (`nonce::reserve_slot_checked`), so a slot another operation reserved, consumed and
//!    released in between is never reused on the value that was just consumed.
//! 7. **Fair ticks.** A tick processes one bounded page of the non-terminal operations after a
//!    round-robin cursor, so held or blocked operations cannot starve newer ones.
//!
//! The lane produces [`OpEvent`]s; applying a delivery to the accounting buckets is the mint
//! saga's job (BB-49), keyed on `SolOp::outcome_applied`.

use crate::ids::{Hash32, KeyRole};
use crate::nonce;
use crate::records::*;
use crate::sol_rpc::{LandedTx, RpcFailure, SignatureEntry, SolRpc};
use crate::state;
use crate::tx::builders::*;
use crate::tx::{
    assemble_wire_tx, decode_message, leading_advance_nonce, peek_required_signers, split_wire_transaction, Pubkey, Signature,
};
use std::cell::RefCell;
use std::collections::BTreeSet;

const NS_PER_SEC: u64 = 1_000_000_000;
/// Times one admission prepares on a free slot before giving up with `Busy`.
const MAX_PREPARE_ATTEMPTS: usize = 3;
/// Operations one tick processes at most (one page of the round-robin walk).
pub const MAX_OPS_PER_TICK: usize = 32;

// ---------------------------------------------------------------------------
// The signer seam
// ---------------------------------------------------------------------------

/// What a sign call returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignOutcome {
    Signed(Signature),
    /// Nothing was signed.
    Failed(String),
    /// The reply was lost or ambiguous (`SYS_UNKNOWN`, `CANISTER_ERROR`): a signature may exist
    /// that we never saw. It cannot be broadcast by anyone but us, so the handling is the same:
    /// sign the same bytes again.
    Unknown(String),
}

/// Threshold-Ed25519 signing. The wasm implementation (BB-49) calls `sign_with_schnorr` with the
/// derivation path of the role; unit tests use a mock. Nothing here signs for real.
#[allow(async_fn_in_trait)]
pub trait TxSigner {
    /// The 32-byte public key (the address) of a role key.
    async fn public_key(&self, role: KeyRole) -> Result<Pubkey, String>;
    /// Sign the full serialized message bytes with the role key.
    async fn sign(&self, role: KeyRole, message: &[u8]) -> SignOutcome;
}

/// Derive and cache the four role public keys (idempotent once cached). A BB-48 snapshot holds
/// three; the missing `mint-account` key is derived and added.
pub async fn ensure_keys<S: TxSigner>(signer: &S) -> Result<state::RoleKeys, String> {
    if let Some(k) = state::read_core(|c| c.keys.clone()) {
        if k.mint_account.is_some() {
            return Ok(k);
        }
    }
    let keys = state::RoleKeys {
        fee_payer: signer.public_key(KeyRole::FeePayer).await?,
        mint_authority: signer.public_key(KeyRole::MintAuthority).await?,
        return_owner: signer.public_key(KeyRole::ReturnOwner).await?,
        mint_account: Some(signer.public_key(KeyRole::MintAccount).await?),
    };
    state::mutate_core(|c| {
        match &c.keys {
            // Never replace keys that are already in use by stored attempts: a mismatch is an error
            // the caller must see, not something to overwrite.
            Some(existing) if existing.fee_payer != keys.fee_payer || existing.mint_authority != keys.mint_authority || existing.return_owner != keys.return_owner => {}
            _ => c.keys = Some(keys.clone()),
        }
    });
    let stored = state::read_core(|c| c.keys.clone()).ok_or("keys were not stored")?;
    if stored != keys {
        return Err("the derived role keys differ from the stored ones".into());
    }
    Ok(stored)
}

/// The role a key plays, if it is one of the four role keys.
pub fn role_of(keys: &state::RoleKeys, key: &Pubkey) -> Option<KeyRole> {
    if *key == keys.fee_payer {
        Some(KeyRole::FeePayer)
    } else if *key == keys.mint_authority {
        Some(KeyRole::MintAuthority)
    } else if *key == keys.return_owner {
        Some(KeyRole::ReturnOwner)
    } else if Some(*key) == keys.mint_account {
        Some(KeyRole::MintAccount)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Events and errors
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpEvent {
    /// The original message landed with `err == null`. `adopted` means it was found by discovery
    /// against the exact stored bytes rather than by a recorded signature.
    Delivered { op_id: u64, signature: Sig, adopted: bool },
    /// The nonce is proven consumed without delivery; the same id waits for a fresh nonce value.
    NotDelivered { op_id: u64, reason: NotDeliveredReason },
    /// The same id was re-signed on a fresh nonce value.
    Reissued { op_id: u64, generation: u32 },
    CancelIssued { op_id: u64 },
    /// Held for review: no replacement and no refund is invented.
    Held { op_id: u64, reason: String },
    LaneHalted { lane: Lane, reason: String },
    /// Unresolved for `reconcile_alarm`.
    Alarm { op_id: u64, age_secs: u64 },
    /// Nothing could be concluded this tick (null, `Inconsistent`, an error, or no consumer yet).
    Waiting { op_id: u64, why: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubmitError {
    LaneHalted(String),
    NotConfigured(String),
    InvalidRequest(String),
    /// No `Free` slot in the lane; retry later.
    NoFreeSlot,
    /// Another submission took the slot while this one was reading it; retry.
    Busy,
    Rpc(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TickReport {
    pub processed: usize,
    pub events: Vec<OpEvent>,
    /// The tick was skipped because another tick is still running.
    pub skipped: bool,
}

// ---------------------------------------------------------------------------
// Re-entrancy guards (heap only: an upgrade cannot happen mid-call)
// ---------------------------------------------------------------------------

thread_local! {
    static BUSY_OPS: RefCell<BTreeSet<u64>> = const { RefCell::new(BTreeSet::new()) };
    static TICKING: RefCell<bool> = const { RefCell::new(false) };
}

struct OpGuard(u64);

impl OpGuard {
    fn acquire(id: u64) -> Option<OpGuard> {
        BUSY_OPS.with(|b| b.borrow_mut().insert(id)).then_some(OpGuard(id))
    }
}

impl Drop for OpGuard {
    fn drop(&mut self) {
        BUSY_OPS.with(|b| {
            b.borrow_mut().remove(&self.0);
        });
    }
}

pub(crate) struct TickGuard;

impl TickGuard {
    pub(crate) fn acquire() -> Option<TickGuard> {
        TICKING.with(|t| {
            let mut t = t.borrow_mut();
            if *t {
                None
            } else {
                *t = true;
                Some(TickGuard)
            }
        })
    }
}

impl Drop for TickGuard {
    fn drop(&mut self) {
        TICKING.with(|t| *t.borrow_mut() = false);
    }
}

// ---------------------------------------------------------------------------
// Message building and attempt commit (synchronous)
// ---------------------------------------------------------------------------

fn build_original(profile: &TxProfile, op: &SolOp, nonce: NonceRef) -> Result<UnsignedTx, String> {
    let built = match &op.payload {
        OpPayload::Mint { recipient_wallet, per_operation_cap } => build_mint_message(
            profile,
            &MintRequest {
                nonce,
                recipient_wallet: *recipient_wallet,
                amount: op.amount,
                mint_id: op.external_id,
                per_operation_cap: *per_operation_cap,
            },
        ),
        OpPayload::Burn => build_burn_message(profile, &BurnRequest { nonce, amount: op.amount, burn_id: op.external_id }),
        OpPayload::Refund { destination } => build_refund_message(
            profile,
            &RefundRequest { nonce, destination: destination.clone(), amount: op.amount, refund_id: op.external_id },
        ),
    };
    built.map_err(|e| format!("{e}"))
}

fn new_attempt(kind: AttemptKind, generation: u32, nonce: NonceRef, tx: &UnsignedTx, now: u64) -> Attempt {
    Attempt {
        kind,
        generation,
        nonce_account: nonce.account,
        nonce_value: nonce.value,
        message: tx.message_bytes.clone(),
        signers: tx.required_signers().to_vec(),
        signatures: vec![],
        tx_ids: vec![],
        created_at_ns: now,
        sign_failures: 0,
        last_sign_error: None,
        broadcast_count: 0,
        last_broadcast_ns: None,
        last_broadcast_note: None,
    }
}

fn update_op<R>(id: u64, f: impl FnOnce(&mut SolOp) -> R) -> Option<R> {
    let mut op = state::get_op(id)?;
    let r = f(&mut op);
    state::put_op(&op);
    Some(r)
}

/// A free slot of `lane`, read from the chain, checked, with an unsigned message built for it.
struct Prepared {
    slot_index: u8,
    /// The slot's generation when the preparation began (the slot was `Free` then). The commit
    /// reserves only at this generation, so a slot cycled during the await is refused.
    slot_generation: u64,
    nonce: NonceRef,
    tx: UnsignedTx,
}

/// Choose the lowest free slot, read and check its account at `finalized`, and build the message
/// bound to the value just read. Nothing is written. The caller commits (synchronously) right
/// after, re-checking that the slot is still free.
async fn prepare_on_free_slot<R: SolRpc>(
    rpc: &R,
    lane: Lane,
    profile: &TxProfile,
    build: impl Fn(NonceRef) -> Result<UnsignedTx, String>,
) -> Result<Prepared, SubmitError> {
    let slot = nonce::first_free_slot(lane).ok_or(SubmitError::NoFreeSlot)?;
    let snapshot = match rpc.get_account(&slot.address).await {
        Ok(Some(s)) => s,
        Ok(None) => return Err(SubmitError::Rpc("the nonce account does not exist at finalized".into())),
        Err(e) => return Err(SubmitError::Rpc(format!("{e:?}"))),
    };
    let nonce_state = match nonce::check_snapshot(&snapshot, &profile.fee_payer) {
        Ok(n) => n,
        Err(problem) => {
            let reason = problem.to_string();
            state::halt_lane(lane, reason.clone());
            return Err(SubmitError::LaneHalted(reason));
        }
    };
    let nonce_ref = NonceRef { account: slot.address, value: nonce_state.value };
    let tx = build(nonce_ref).map_err(SubmitError::InvalidRequest)?;
    Ok(Prepared { slot_index: slot.index, slot_generation: slot.generation, nonce: nonce_ref, tx })
}

#[derive(Clone, Debug)]
pub struct SubmitRequest {
    pub kind: OpKind,
    pub payload: OpPayload,
    /// `mint_id`, `burn_id` or `refund_id`.
    pub external_id: Hash32,
    pub amount: u64,
}

/// Admit an operation: reserve a nonce slot, persist the attempt, then sign and send.
///
/// Idempotent on `external_id`: a repeat returns the existing operation without touching it. The
/// operation is committed (with its reservation and its exact unsigned bytes) BEFORE the first
/// call to the signer; signing and the first broadcast then run best-effort, and whatever they
/// leave unfinished the next tick completes with the same bytes.
pub async fn submit_op<R: SolRpc, S: TxSigner>(rpc: &R, signer: &S, req: SubmitRequest, now: u64) -> Result<u64, SubmitError> {
    if let Some(id) = state::op_by_external(&req.external_id) {
        return Ok(id);
    }
    let lane = req.kind.lane();
    if let Some(h) = state::lane_halt(lane) {
        return Err(SubmitError::LaneHalted(h.reason));
    }
    let profile = state::read_core(|c| c.tx_profile()).map_err(SubmitError::NotConfigured)?;
    let template = SolOp {
        id: 0,
        kind: req.kind,
        payload: req.payload.clone(),
        external_id: req.external_id,
        amount: req.amount,
        state: OpState::Active,
        lane,
        slot_index: None,
        generation: 0,
        attempts: vec![],
        observed_value: None,
        advance_seen_at_ns: None,
        discovery_cursor: None,
        created_at_ns: now,
        updated_at_ns: now,
        outcome_applied: false,
        alarm_raised_at_ns: None,
        note: None,
    };
    // A preparation that finds its slot cycled while the account read was in flight is refused by
    // the synchronous commit (stale generation) and prepared again on the fresh value.
    let mut committed = None;
    for _ in 0..MAX_PREPARE_ATTEMPTS {
        let prepared = prepare_on_free_slot(rpc, lane, &profile, |n| build_original(&profile, &template, n)).await?;

        // ---- synchronous commit: nothing below awaits until the attempt is stored ----
        if let Some(id) = state::op_by_external(&req.external_id) {
            return Ok(id);
        }
        if let Some(h) = state::lane_halt(lane) {
            return Err(SubmitError::LaneHalted(h.reason));
        }
        let op_id = state::next_op_id();
        if !nonce::reserve_slot_checked(lane, prepared.slot_index, op_id, prepared.slot_generation) {
            continue;
        }
        let mut op = template.clone();
        op.id = op_id;
        op.slot_index = Some(prepared.slot_index);
        op.attempts.push(new_attempt(AttemptKind::Original, 0, prepared.nonce, &prepared.tx, now));
        state::put_op(&op);
        state::append_audit(None, state::AuditCtx::note(format!("op {op_id} admitted ({:?}), slot {} reserved", req.kind, prepared.slot_index)));
        committed = Some((op_id, OpGuard::acquire(op_id)));
        break;
    }
    let Some((op_id, _guard)) = committed else { return Err(SubmitError::Busy) };

    // ---- the first call to the signer happens only after the commit above ----
    drive_attempt(rpc, signer, op_id, 0, now).await;
    Ok(op_id)
}

// ---------------------------------------------------------------------------
// Sign and broadcast
// ---------------------------------------------------------------------------

/// Sign every missing signature of an attempt with the SAME stored bytes, recording each one as
/// it arrives. Returns whether the attempt is now fully signed.
async fn sign_missing<S: TxSigner>(signer: &S, op_id: u64, idx: usize, now: u64) -> bool {
    loop {
        let Some(op) = state::get_op(op_id) else { return false };
        let Some(attempt) = op.attempts.get(idx) else { return false };
        let Some(next) = attempt.signers.iter().find(|s| !attempt.has_signature_from(s)).copied() else {
            return true;
        };
        // Halt fence: a halted lane signs nothing. Checked before EVERY signer call, because a
        // sign reply (or an RPC reply earlier in the tick) may have come back after another
        // operation halted this lane. Signatures already received stay recorded.
        if state::lane_halt(op.lane).is_some() {
            return false;
        }
        let message = attempt.message.clone();
        let role = state::read_core(|c| c.keys.as_ref().and_then(|k| role_of(k, &next)));
        let Some(role) = role else {
            update_op(op_id, |op| {
                if let Some(a) = op.attempts.get_mut(idx) {
                    a.sign_failures += 1;
                    a.last_sign_error = Some("signer key is not one of the role keys".into());
                }
            });
            return false;
        };
        let outcome = signer.sign(role, &message).await;
        match outcome {
            SignOutcome::Signed(sig) => {
                update_op(op_id, |op| {
                    if let Some(a) = op.attempts.get_mut(idx) {
                        a.signatures.push(SignatureRecord { signer: next, signature: Sig(sig), at_ns: now });
                    }
                });
            }
            SignOutcome::Failed(e) | SignOutcome::Unknown(e) => {
                update_op(op_id, |op| {
                    if let Some(a) = op.attempts.get_mut(idx) {
                        a.sign_failures += 1;
                        a.last_sign_error = Some(e);
                    }
                });
                return false;
            }
        }
    }
}

/// Assemble the wire transaction from the recorded signatures, record its id, then send. Every
/// outcome is "maybe sent".
async fn broadcast<R: SolRpc>(rpc: &R, op_id: u64, idx: usize, now: u64) {
    let wire = {
        let Some(op) = state::get_op(op_id) else { return };
        // Halt fence: a halted lane sends nothing. There is no await between this check and the send.
        if state::lane_halt(op.lane).is_some() {
            return;
        }
        let Some(attempt) = op.attempts.get(idx) else { return };
        let Some(sigs) = attempt.latest_signatures() else { return };
        let Ok(wire) = assemble_wire_tx(&sigs, &attempt.message) else { return };
        let Some(first) = sigs.first().copied() else { return };
        // Persist the intent (the tx id and the count) BEFORE the send.
        update_op(op_id, |op| {
            if let Some(a) = op.attempts.get_mut(idx) {
                if !a.tx_ids.contains(&Sig(first)) {
                    a.tx_ids.push(Sig(first));
                }
                a.broadcast_count += 1;
                a.last_broadcast_ns = Some(now);
            }
        });
        wire
    };
    let note = match rpc.send_transaction(&wire).await {
        Ok(sig) => format!("sent, rpc returned {}", crate::tx::signature_to_base58(&sig)),
        Err(RpcFailure::Inconsistent) => "send: providers disagreed (maybe sent)".to_string(),
        Err(RpcFailure::Error(e)) => format!("send error (maybe sent): {e}"),
    };
    update_op(op_id, |op| {
        if let Some(a) = op.attempts.get_mut(idx) {
            a.last_broadcast_note = Some(note);
        }
    });
}

/// Sign what is unsigned and broadcast if due. Safe to call every tick.
async fn drive_attempt<R: SolRpc, S: TxSigner>(rpc: &R, signer: &S, op_id: u64, idx: usize, now: u64) {
    if !sign_missing(signer, op_id, idx, now).await {
        return;
    }
    let interval = state::read_core(|c| c.config.lane.rebroadcast_interval_secs).saturating_mul(NS_PER_SEC);
    let due = state::get_op(op_id)
        .and_then(|op| op.attempts.get(idx).map(|a| a.last_broadcast_ns.is_none_or(|t| now.saturating_sub(t) >= interval)))
        .unwrap_or(false);
    if due {
        broadcast(rpc, op_id, idx, now).await;
    }
}

// ---------------------------------------------------------------------------
// Evidence classification (pure)
// ---------------------------------------------------------------------------

/// What a landed transaction is, relative to one operation's current generation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Landing {
    /// Its message bytes equal the stored unsigned message of attempt `attempt`.
    Exact { attempt: usize, failed: bool },
    /// It consumed our nonce value (advance-first on our account, our value, our authority) but
    /// its bytes equal no stored message.
    DifferentConsumer,
    /// It does not decode strictly and its required signers include the fee payer key: it may be
    /// a consumer and cannot be verified.
    UnverifiableSigned,
    /// It is not a consumer of this nonce value.
    NotConsumer,
}

/// Classify one landed transaction against the current generation of `op`.
pub fn classify_landing(op: &SolOp, nonce_account: &Pubkey, bound_value: &Hash32, fee_payer: &Pubkey, tx: &LandedTx) -> Landing {
    let Ok((_sigs, message_bytes)) = split_wire_transaction(&tx.wire) else {
        return Landing::UnverifiableSigned;
    };
    // Exact bytes first: a stored message is the only thing that may be adopted.
    if let Some((idx, _)) = op.current_attempts().find(|(_, a)| a.message == message_bytes) {
        return Landing::Exact { attempt: idx, failed: tx.failed };
    }
    match decode_message(message_bytes) {
        Ok(msg) => match leading_advance_nonce(&msg) {
            Some((account, authority)) if &account == nonce_account && &authority == fee_payer && &msg.recent_blockhash == bound_value => {
                Landing::DifferentConsumer
            }
            _ => Landing::NotConsumer,
        },
        Err(_) => match peek_required_signers(message_bytes) {
            Some(signers) if signers.contains(fee_payer) => Landing::UnverifiableSigned,
            _ => Landing::NotConsumer,
        },
    }
}

/// The decision a lookup or discovery reached.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Resolution {
    Delivered { signature: Sig, slot: u64, adopted: bool },
    NotDelivered { reason: NotDeliveredReason, consumer: Sig },
    Hold(String),
    /// Nothing concluded (null, `Inconsistent`, error, or a sweep still in progress).
    Wait(String),
    /// The whole range was scanned and no consumer exists in it (yet).
    NoConsumerYet,
}

/// Turn an exact landing into a resolution.
fn exact_resolution(op: &SolOp, attempt: usize, failed: bool, signature: Sig, slot: u64, adopted: bool) -> Resolution {
    let kind = op.attempts.get(attempt).map(|a| a.kind);
    match (failed, kind) {
        (false, Some(AttemptKind::Original)) => Resolution::Delivered { signature, slot, adopted },
        (false, Some(AttemptKind::Cancel)) => {
            Resolution::NotDelivered { reason: NotDeliveredReason::CancelLanded { signature }, consumer: signature }
        }
        (true, Some(_)) => Resolution::NotDelivered { reason: NotDeliveredReason::LandedWithError { signature }, consumer: signature },
        (_, None) => Resolution::Hold("landed attempt index is out of range".into()),
    }
}

// ---------------------------------------------------------------------------
// Reconcile
// ---------------------------------------------------------------------------

/// Step 3a: look up every recorded transaction id of the current generation.
async fn resolve_by_recorded<R: SolRpc>(rpc: &R, op: &SolOp, nonce_account: &Pubkey, bound: &Hash32, fee_payer: &Pubkey) -> Resolution {
    let mut landed: Vec<(usize, Sig, LandedTx)> = vec![];
    let mut unavailable: Option<String> = None;
    for (idx, attempt) in op.current_attempts() {
        for id in &attempt.tx_ids {
            match rpc.get_transaction(&id.0).await {
                Ok(Some(tx)) => landed.push((idx, *id, tx)),
                Ok(None) => {}
                Err(e) => unavailable = Some(format!("getTransaction {id:?}: {e:?}")),
            }
        }
    }
    if landed.len() > 1 {
        return Resolution::Hold(format!("{} recorded transactions are reported landed for one nonce value", landed.len()));
    }
    if let Some((idx, id, tx)) = landed.pop() {
        return match classify_landing(op, nonce_account, bound, fee_payer, &tx) {
            Landing::Exact { attempt, failed } if attempt == idx => exact_resolution(op, attempt, failed, id, tx.slot, false),
            other => Resolution::Hold(format!("recorded transaction {id:?} landed with bytes that are not its stored message ({other:?})")),
        };
    }
    match unavailable {
        Some(why) => Resolution::Wait(why),
        None => Resolution::NoConsumerYet,
    }
}

/// Step 3b: bounded discovery on the nonce account. Candidates are verified singly; a memo only
/// orders them.
async fn resolve_by_discovery<R: SolRpc>(
    rpc: &R,
    op: &SolOp,
    slot: &NonceSlot,
    bound: &Hash32,
    fee_payer: &Pubkey,
    limit: u16,
) -> (Resolution, Option<Sig>) {
    let limit32 = limit as u32;
    let before = op.discovery_cursor;
    let page = match rpc.get_signatures_for_address(&slot.address, before.as_ref().map(|s| &s.0), slot.anchor.as_ref().map(|s| &s.0), limit32).await {
        Ok(p) => p,
        Err(e) => return (Resolution::Wait(format!("getSignaturesForAddress: {e:?}")), before),
    };
    let own: BTreeSet<Sig> = op.attempts.iter().flat_map(|a| a.tx_ids.iter().copied()).collect();
    let id_hex = hex::encode(op.external_id);
    let mut candidates: Vec<&SignatureEntry> = page.iter().filter(|e| !own.contains(&Sig(e.signature))).collect();
    // Discovery only: candidates whose memo names our id are looked at first. Verification below
    // is identical for every candidate.
    candidates.sort_by_key(|e| !e.memo.as_deref().is_some_and(|m| m.contains(&id_hex)));
    for entry in candidates {
        let tx = match rpc.get_transaction(&entry.signature).await {
            Ok(Some(tx)) => tx,
            Ok(None) => return (Resolution::Wait(format!("listed signature {} is not retrievable yet", crate::tx::signature_to_base58(&entry.signature))), before),
            Err(e) => return (Resolution::Wait(format!("getTransaction (candidate): {e:?}")), before),
        };
        match classify_landing(op, &slot.address, bound, fee_payer, &tx) {
            Landing::NotConsumer => continue,
            Landing::Exact { attempt, failed } => {
                return (exact_resolution(op, attempt, failed, Sig(entry.signature), tx.slot, true), None);
            }
            Landing::DifferentConsumer => {
                let memo = entry.memo.as_deref().is_some_and(|m| m.contains(&id_hex));
                return (
                    Resolution::Hold(format!(
                        "transaction {} consumed the nonce with message bytes that are not a stored message{}",
                        crate::tx::signature_to_base58(&entry.signature),
                        if memo { " (it carries our memo, which is discovery only)" } else { "" }
                    )),
                    None,
                );
            }
            Landing::UnverifiableSigned => {
                return (
                    Resolution::Hold(format!(
                        "transaction {} is signed by the fee payer key and cannot be verified",
                        crate::tx::signature_to_base58(&entry.signature)
                    )),
                    None,
                );
            }
        }
    }
    // Page full: more history may exist; continue next tick from the oldest signature seen.
    if page.len() >= limit as usize {
        let next = page.last().map(|e| Sig(e.signature));
        return (Resolution::Wait("discovery sweep continues".into()), next);
    }
    (Resolution::NoConsumerYet, None)
}

/// Apply a resolution to the operation, its slot and the lane. Returns the events.
fn apply_resolution(op_id: u64, resolution: Resolution, observed: &Hash32, now: u64) -> Vec<OpEvent> {
    let Some(op) = state::get_op(op_id) else { return vec![] };
    let mut events = vec![];
    match resolution {
        Resolution::Delivered { signature, slot, adopted } => {
            if let Some(idx) = op.slot_index {
                nonce::release_slot(op.lane, idx, op_id, *observed, signature);
            }
            update_op(op_id, |o| {
                o.state = OpState::Delivered { signature, slot, adopted };
                o.updated_at_ns = now;
                o.discovery_cursor = None;
            });
            state::append_audit(None, state::AuditCtx::note(format!("op {op_id} delivered (adopted={adopted})")));
            events.push(OpEvent::Delivered { op_id, signature, adopted });
        }
        Resolution::NotDelivered { reason, consumer } => {
            if let Some(idx) = op.slot_index {
                nonce::release_slot(op.lane, idx, op_id, *observed, consumer);
            }
            update_op(op_id, |o| {
                o.state = OpState::NeedsReissue { reason: reason.clone() };
                o.updated_at_ns = now;
                o.slot_index = None;
                o.discovery_cursor = None;
                o.observed_value = None;
                o.advance_seen_at_ns = None;
            });
            state::append_audit(None, state::AuditCtx::note(format!("op {op_id} proven not delivered: {reason:?}")));
            events.push(OpEvent::NotDelivered { op_id, reason });
        }
        Resolution::Hold(reason) => {
            update_op(op_id, |o| {
                o.state = OpState::Held { reason: reason.clone() };
                o.updated_at_ns = now;
            });
            state::halt_lane(op.lane, reason.clone());
            events.push(OpEvent::Held { op_id, reason: reason.clone() });
            events.push(OpEvent::LaneHalted { lane: op.lane, reason });
        }
        Resolution::Wait(why) => events.push(OpEvent::Waiting { op_id, why }),
        Resolution::NoConsumerYet => events.push(OpEvent::Waiting { op_id, why: "nonce advanced; no consumer found yet".into() }),
    }
    events
}

/// One reconcile pass over an `Active` operation.
async fn process_active<R: SolRpc, S: TxSigner>(rpc: &R, signer: &S, op_id: u64, now: u64) -> Vec<OpEvent> {
    let mut events = vec![];
    let Some(op) = state::get_op(op_id) else { return events };
    if op.state != OpState::Active {
        return events;
    }
    let cfg = state::read_core(|c| c.config.lane.clone());
    // The alarm is raised even if the lane is halted: an unresolved operation is what it is for.
    let age = now.saturating_sub(op.created_at_ns);
    if op.alarm_raised_at_ns.is_none() && age >= cfg.reconcile_alarm_secs.saturating_mul(NS_PER_SEC) {
        update_op(op_id, |o| o.alarm_raised_at_ns = Some(now));
        events.push(OpEvent::Alarm { op_id, age_secs: age / NS_PER_SEC });
    }
    if state::lane_halt(op.lane).is_some() {
        return events;
    }
    let (Some(slot_index), Some(fee_payer)) = (op.slot_index, state::read_core(|c| c.keys.as_ref().map(|k| k.fee_payer))) else {
        events.push(OpEvent::Waiting { op_id, why: "no reserved slot or no role keys".into() });
        return events;
    };
    let Some(slot) = state::get_slot(op.lane, slot_index) else {
        events.push(OpEvent::Waiting { op_id, why: "the reserved slot record is missing".into() });
        return events;
    };
    let Some(bound) = op.current_attempts().next().map(|(_, a)| a.nonce_value) else {
        events.push(OpEvent::Waiting { op_id, why: "the current generation has no attempt".into() });
        return events;
    };

    // ---- read the nonce account (finalized) and check it on every read ----
    let nonce_state = match rpc.get_account(&slot.address).await {
        Ok(Some(snapshot)) => match nonce::check_snapshot(&snapshot, &fee_payer) {
            Ok(n) => n,
            Err(problem) => {
                let reason = problem.to_string();
                state::halt_lane(op.lane, reason.clone());
                events.push(OpEvent::LaneHalted { lane: op.lane, reason });
                return events;
            }
        },
        Ok(None) => {
            events.push(OpEvent::Waiting { op_id, why: "nonce account not found at finalized".into() });
            return events;
        }
        Err(e) => {
            events.push(OpEvent::Waiting { op_id, why: format!("nonce account read: {e:?}") });
            return events;
        }
    };

    // Halt fence across the await above: another operation may have halted this lane while the
    // read was in flight. Nothing below may sign, send or issue a cancel on a halted lane.
    if state::lane_halt(op.lane).is_some() {
        return events;
    }
    if nonce_state.value == bound {
        events.extend(reconcile_unchanged(rpc, signer, op_id, &cfg, now).await);
    } else {
        events.extend(reconcile_advanced(rpc, op_id, &slot, &bound, &nonce_state.value, &fee_payer, &cfg, now).await);
    }
    events
}

/// Step 1 and 2: the stored value equals the recorded one, so nothing landed as of `finalized`.
async fn reconcile_unchanged<R: SolRpc, S: TxSigner>(
    rpc: &R,
    signer: &S,
    op_id: u64,
    cfg: &crate::types::LaneSettings,
    now: u64,
) -> Vec<OpEvent> {
    let mut events = vec![];
    // The advance marker is only meaningful while the account is advanced.
    update_op(op_id, |o| {
        o.observed_value = None;
        o.advance_seen_at_ns = None;
        o.discovery_cursor = None;
    });
    // Rebroadcast the SAME bytes of every current attempt (and finish signing, with the same bytes).
    let idxs: Vec<usize> = state::get_op(op_id).map(|o| o.current_attempts().map(|(i, _)| i).collect()).unwrap_or_default();
    for idx in idxs {
        drive_attempt(rpc, signer, op_id, idx, now).await;
    }
    // Cancel after `cancel_after`, once the original has been sent at least once.
    let Some(op) = state::get_op(op_id) else { return events };
    if state::lane_halt(op.lane).is_some() {
        return events;
    }
    let original = op.current_attempts().find(|(_, a)| a.kind == AttemptKind::Original).map(|(_, a)| a.clone());
    let has_cancel = op.current_attempts().any(|(_, a)| a.kind == AttemptKind::Cancel);
    if let Some(original) = original {
        let waited = now.saturating_sub(original.created_at_ns) >= cfg.cancel_after_secs.saturating_mul(NS_PER_SEC);
        if !has_cancel && waited && original.fully_signed() && original.broadcast_count > 0 {
            let profile = state::read_core(|c| c.tx_profile());
            let nonce_ref = NonceRef { account: original.nonce_account, value: original.nonce_value };
            match profile.and_then(|p| build_advance_only_message(&p, &nonce_ref).map_err(|e| format!("{e}"))) {
                Ok(tx) => {
                    // Persist the cancel (exact bytes) BEFORE signing it.
                    let idx = update_op(op_id, |o| {
                        o.attempts.push(new_attempt(AttemptKind::Cancel, o.generation, nonce_ref, &tx, now));
                        o.attempts.len().saturating_sub(1)
                    });
                    if let Some(idx) = idx {
                        events.push(OpEvent::CancelIssued { op_id });
                        drive_attempt(rpc, signer, op_id, idx, now).await;
                    }
                }
                Err(e) => events.push(OpEvent::Waiting { op_id, why: format!("cannot build the cancel: {e}") }),
            }
        }
    }
    if events.is_empty() {
        events.push(OpEvent::Waiting { op_id, why: "nonce unchanged; waiting for the attempt to land".into() });
    }
    events
}

/// Step 3 and 4: the stored value moved off the recorded one.
#[allow(clippy::too_many_arguments)]
async fn reconcile_advanced<R: SolRpc>(
    rpc: &R,
    op_id: u64,
    slot: &NonceSlot,
    bound: &Hash32,
    observed: &Hash32,
    fee_payer: &Pubkey,
    cfg: &crate::types::LaneSettings,
    now: u64,
) -> Vec<OpEvent> {
    // Remember when the advance was first seen: the history horizon runs from here.
    let Some(op) = update_op(op_id, |o| {
        if o.advance_seen_at_ns.is_none() {
            o.advance_seen_at_ns = Some(now);
        }
        o.observed_value = Some(*observed);
        o.clone()
    }) else {
        return vec![];
    };

    let mut resolution = resolve_by_recorded(rpc, &op, &slot.address, bound, fee_payer).await;
    if resolution == Resolution::NoConsumerYet {
        let (r, cursor) = resolve_by_discovery(rpc, &op, slot, bound, fee_payer, cfg.discover_limit).await;
        update_op(op_id, |o| o.discovery_cursor = cursor);
        resolution = r;
    }
    // History horizon: the advance is old and nothing explains it. Stays halted; resolved by hand.
    if resolution == Resolution::NoConsumerYet {
        let since = op.advance_seen_at_ns.unwrap_or(now);
        if now.saturating_sub(since) >= cfg.history_horizon_secs.saturating_mul(NS_PER_SEC) {
            resolution = Resolution::Hold(format!(
                "the nonce advanced and no consumer was found within the history horizon of {}s ({})",
                cfg.history_horizon_secs,
                crate::types::HISTORY_HORIZON_BLOCKER
            ));
        }
    }
    apply_resolution(op_id, resolution, observed, now)
}

/// Re-issue the same id on a fresh nonce value after a proven non-delivery.
async fn process_reissue<R: SolRpc, S: TxSigner>(rpc: &R, signer: &S, op_id: u64, now: u64) -> Vec<OpEvent> {
    let Some(op) = state::get_op(op_id) else { return vec![] };
    if !matches!(op.state, OpState::NeedsReissue { .. }) {
        return vec![];
    }
    if state::lane_halt(op.lane).is_some() {
        return vec![];
    }
    let max = state::read_core(|c| c.config.lane.max_reissues) as u32;
    if op.generation >= max {
        let reason = format!("re-issue limit of {max} reached; held for review");
        update_op(op_id, |o| {
            o.state = OpState::Held { reason: reason.clone() };
            o.updated_at_ns = now;
        });
        return vec![OpEvent::Held { op_id, reason }];
    }
    let profile = match state::read_core(|c| c.tx_profile()) {
        Ok(p) => p,
        Err(e) => return vec![OpEvent::Waiting { op_id, why: e }],
    };
    let prepared = match prepare_on_free_slot(rpc, op.lane, &profile, |n| build_original(&profile, &op, n)).await {
        Ok(p) => p,
        Err(SubmitError::LaneHalted(reason)) => return vec![OpEvent::LaneHalted { lane: op.lane, reason }],
        Err(e) => return vec![OpEvent::Waiting { op_id, why: format!("re-issue: {e:?}") }],
    };
    // ---- synchronous commit of the new generation ----
    let Some(op) = state::get_op(op_id) else { return vec![] };
    if !matches!(op.state, OpState::NeedsReissue { .. }) || state::lane_halt(op.lane).is_some() {
        return vec![];
    }
    if !nonce::reserve_slot_checked(op.lane, prepared.slot_index, op_id, prepared.slot_generation) {
        return vec![OpEvent::Waiting { op_id, why: "re-issue: the slot changed while it was read; prepare again next tick".into() }];
    }
    let generation = op.generation + 1;
    update_op(op_id, |o| {
        o.generation = generation;
        o.slot_index = Some(prepared.slot_index);
        o.state = OpState::Active;
        o.updated_at_ns = now;
        o.attempts.push(new_attempt(AttemptKind::Original, generation, prepared.nonce, &prepared.tx, now));
    });
    state::append_audit(None, state::AuditCtx::note(format!("op {op_id} re-issued as generation {generation} on slot {}", prepared.slot_index)));
    let idx = state::get_op(op_id).map(|o| o.attempts.len().saturating_sub(1)).unwrap_or(0);
    drive_attempt(rpc, signer, op_id, idx, now).await;
    vec![OpEvent::Reissued { op_id, generation }]
}

/// Process one operation (whatever its state calls for).
pub async fn process_op<R: SolRpc, S: TxSigner>(rpc: &R, signer: &S, op_id: u64, now: u64) -> Vec<OpEvent> {
    let Some(_guard) = OpGuard::acquire(op_id) else { return vec![] };
    let Some(op) = state::get_op(op_id) else { return vec![] };
    match op.state {
        OpState::Active => process_active(rpc, signer, op_id, now).await,
        OpState::NeedsReissue { .. } => process_reissue(rpc, signer, op_id, now).await,
        OpState::Delivered { .. } | OpState::Held { .. } => vec![],
    }
}

/// One lane tick: process one bounded page of the non-terminal operations, advancing a round-robin
/// cursor so every operation is reached within `ceil(n / MAX_OPS_PER_TICK)` ticks. A tick that
/// finds another tick running does nothing.
pub async fn tick<R: SolRpc, S: TxSigner>(rpc: &R, signer: &S, now: u64) -> TickReport {
    let Some(_guard) = TickGuard::acquire() else {
        return TickReport { skipped: true, ..TickReport::default() };
    };
    let mut report = TickReport::default();
    // Fair paging: a page after the previous page's last id, wrapping. Held operations and
    // operations blocked behind a halted lane stay in the rotation (they cost no calls), so they
    // can delay others by at most ceil(n / MAX_OPS_PER_TICK) ticks and never starve them.
    for id in state::next_tick_page(MAX_OPS_PER_TICK) {
        report.processed += 1;
        report.events.extend(process_op(rpc, signer, id, now).await);
    }
    report
}
