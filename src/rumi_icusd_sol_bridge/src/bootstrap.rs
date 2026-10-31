//! The bootstrap driver (49a): derive the keys, create and verify the nonce pool accounts, create
//! the escrow mint, create the return account, and bind (`mint.rs`). It runs from the timer
//! (`timer::run_once`) until everything is bound and verified, and then makes no calls at all.
//!
//! The creation transactions use a REAL recent blockhash (there is no nonce yet; the README route
//! `getSlot` then `getBlock`, never `getLatestBlockhash`). They are idempotent in effect: the
//! target account either exists or not, and the driver READS the account before every send, so it
//! never sends a creation for an account that exists. Each one still follows the lane's
//! discipline: the exact message bytes are persisted BEFORE the first sign call, every signature
//! is recorded as it arrives, the transaction id is recorded before the send, and a lost sign
//! reply re-signs the SAME bytes. A transaction older than [`BOOTSTRAP_TX_TTL_SECS`] (a blockhash
//! lives about a minute) can no longer land and is replaced by a fresh one with a fresh blockhash.
//!
//! The fee payer needs SOL before any of this can land (blocker `solana-fee-payer-funding`): until
//! it is funded the sends fail at preflight, the failure is recorded as a note, and the driver
//! tries again after the TTL.

use crate::lane::{ensure_keys, role_of, SignOutcome, TxSigner};
use crate::mint::{self, BindOutcome};
use crate::nonce::{self, PoolError, VerifyOutcome};
use crate::records::*;
use crate::sol_rpc::{bootstrap_blockhash, RpcFailure, SolRpc};
use crate::state;
use crate::tx::builders::*;
use crate::tx::{assemble_wire_tx, MINT_RENT_LAMPORTS, NONCE_ACCOUNT_RENT_LAMPORTS};

/// A bootstrap transaction older than this can no longer land (its blockhash expired), so a fresh
/// one replaces it. 150 blocks at about 400 ms is a minute; the margin is generous.
pub const BOOTSTRAP_TX_TTL_SECS: u64 = 150;
/// Slots to walk back from the finalized tip when looking for a block (skipped slots have none).
const BLOCKHASH_WALK_BACK: u64 = 16;
const NS_PER_SEC: u64 = 1_000_000_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BootstrapEvent {
    KeysDerived,
    SlotRegistered { lane: Lane, index: u8 },
    SlotVerified { lane: Lane, index: u8 },
    SlotProblem { lane: Lane, index: u8, problem: String },
    /// A creation transaction was signed (fully) and sent (every send outcome is maybe-sent).
    Sent { target: BootstrapTarget, fresh: bool },
    MintBound,
    MintProblem(String),
    Waiting { why: String },
}

/// Whether anything is left to bootstrap. When it is not, `bootstrap_step` makes no calls.
pub fn needs_bootstrap() -> bool {
    state::read_core(|c| {
        let pool = c.config.lane.nonce_pool_size as usize;
        let keys_ready = c.keys.as_ref().is_some_and(|k| k.mint_account.is_some());
        let slots = Lane::ALL.iter().map(|l| state::slots_in_lane(*l)).collect::<Vec<_>>();
        let slots_ready = slots.iter().all(|s| s.len() >= pool && s.iter().all(|x| x.state != SlotState::Unverified));
        !(keys_ready && slots_ready && c.mint_state == MintState::Bound)
    })
}

/// Sign every missing signature of a bootstrap transaction with the SAME stored bytes.
async fn sign_bootstrap<S: TxSigner>(signer: &S, key: u32, now: u64) -> bool {
    loop {
        let Some(tx) = state::get_bootstrap_tx(key) else { return false };
        let Some(next) = tx.signers.iter().find(|s| !tx.signatures.iter().any(|r| &r.signer == *s)).copied() else {
            return true;
        };
        if state::is_halted() {
            return false;
        }
        let role = state::read_core(|c| c.keys.as_ref().and_then(|k| role_of(k, &next)));
        let Some(role) = role else {
            record_sign_failure(key, "signer key is not one of the role keys".into());
            return false;
        };
        match signer.sign(role, &tx.message).await {
            SignOutcome::Signed(sig) => {
                if let Some(mut t) = state::get_bootstrap_tx(key) {
                    t.signatures.push(SignatureRecord { signer: next, signature: Sig(sig), at_ns: now });
                    state::put_bootstrap_tx(&t);
                }
            }
            SignOutcome::Failed(e) | SignOutcome::Unknown(e) => {
                record_sign_failure(key, e);
                return false;
            }
        }
    }
}

fn record_sign_failure(key: u32, error: String) {
    if let Some(mut t) = state::get_bootstrap_tx(key) {
        t.sign_failures += 1;
        t.last_sign_error = Some(error);
        state::put_bootstrap_tx(&t);
    }
}

/// Record the transaction id and the intent, then send. Every outcome is maybe-sent.
async fn broadcast_bootstrap<R: SolRpc>(rpc: &R, key: u32, now: u64) {
    let wire = {
        let Some(mut t) = state::get_bootstrap_tx(key) else { return };
        if state::is_halted() {
            return;
        }
        let Some(sigs) = t.latest_signatures() else { return };
        let Ok(wire) = assemble_wire_tx(&sigs, &t.message) else { return };
        let Some(first) = sigs.first().copied() else { return };
        if !t.tx_ids.contains(&Sig(first)) {
            t.tx_ids.push(Sig(first));
        }
        t.broadcast_count += 1;
        t.last_broadcast_ns = Some(now);
        state::put_bootstrap_tx(&t);
        wire
    };
    let note = match rpc.send_transaction(&wire).await {
        Ok(sig) => format!("sent, rpc returned {}", crate::tx::signature_to_base58(&sig)),
        Err(RpcFailure::Inconsistent) => "send: providers disagreed (maybe sent)".to_string(),
        Err(RpcFailure::Error(e)) => format!("send error (maybe sent): {e}"),
    };
    if let Some(mut t) = state::get_bootstrap_tx(key) {
        t.last_broadcast_note = Some(note);
        state::put_bootstrap_tx(&t);
    }
}

/// Make sure a creation transaction for `target` exists, is signed, and is (re)sent when due. The
/// caller has just read the target account and found it missing.
async fn ensure_tx<R: SolRpc, S: TxSigner>(
    rpc: &R,
    signer: &S,
    target: BootstrapTarget,
    build: impl Fn([u8; 32]) -> Result<UnsignedTx, String>,
    now: u64,
    blockhash_cache: &mut Option<[u8; 32]>,
) -> BootstrapEvent {
    let key = target.key();
    let ttl = BOOTSTRAP_TX_TTL_SECS.saturating_mul(NS_PER_SEC);
    let interval = state::read_core(|c| c.config.lane.rebroadcast_interval_secs).saturating_mul(NS_PER_SEC);
    let mut fresh = false;
    let live = state::get_bootstrap_tx(key).filter(|t| now.saturating_sub(t.created_at_ns) < ttl);
    if live.is_none() {
        // None, or expired: a fresh blockhash and a fresh transaction.
        // One blockhash serves every creation of a pass (one `getSlot` and one `getBlock`).
        let blockhash = match *blockhash_cache {
            Some(h) => h,
            None => match bootstrap_blockhash(rpc, BLOCKHASH_WALK_BACK).await {
                Ok(h) => {
                    *blockhash_cache = Some(h);
                    h
                }
                Err(e) => return BootstrapEvent::Waiting { why: format!("bootstrap blockhash: {e:?}") },
            },
        };
        let unsigned = match build(blockhash) {
            Ok(u) => u,
            Err(e) => return BootstrapEvent::Waiting { why: format!("cannot build the {target:?} transaction: {e}") },
        };
        // ---- persist the exact bytes BEFORE the first sign call ----
        state::put_bootstrap_tx(&BootstrapTx {
            target,
            recent_blockhash: blockhash,
            message: unsigned.message_bytes.clone(),
            signers: unsigned.required_signers().to_vec(),
            signatures: vec![],
            tx_ids: vec![],
            created_at_ns: now,
            sign_failures: 0,
            last_sign_error: None,
            broadcast_count: 0,
            last_broadcast_ns: None,
            last_broadcast_note: None,
        });
        fresh = true;
    }
    if !sign_bootstrap(signer, key, now).await {
        return BootstrapEvent::Waiting { why: format!("{target:?}: waiting for signatures") };
    }
    let due = state::get_bootstrap_tx(key).is_some_and(|t| t.last_broadcast_ns.is_none_or(|l| now.saturating_sub(l) >= interval));
    if due {
        broadcast_bootstrap(rpc, key, now).await;
        return BootstrapEvent::Sent { target, fresh };
    }
    BootstrapEvent::Waiting { why: format!("{target:?}: sent, waiting to land") }
}

/// One bootstrap pass. Bounded: at most `2 * nonce_pool_size` pool reads, two mint reads and the
/// creation sends of the targets that are still missing.
pub async fn bootstrap_step<R: SolRpc, S: TxSigner>(rpc: &R, signer: &S, now: u64) -> Vec<BootstrapEvent> {
    let mut events = vec![];
    let mut blockhash: Option<[u8; 32]> = None;
    // Keys first: nothing else can be addressed without them.
    let had_keys = state::read_core(|c| c.keys.as_ref().is_some_and(|k| k.mint_account.is_some()));
    if let Err(e) = ensure_keys(signer).await {
        return vec![BootstrapEvent::Waiting { why: format!("role keys: {e}") }];
    }
    if !had_keys {
        events.push(BootstrapEvent::KeysDerived);
    }
    if !needs_bootstrap() {
        return events;
    }
    let (pool_size, profile) = state::read_core(|c| (c.config.lane.nonce_pool_size, c.creation_profile()));
    let profile = match profile {
        Ok(p) => p,
        Err(e) => {
            events.push(BootstrapEvent::Waiting { why: e });
            return events;
        }
    };

    // ---- the nonce pool ----
    for lane in Lane::ALL {
        for index in 0..pool_size {
            match nonce::register_slot(lane, index) {
                Ok(_) => events.push(BootstrapEvent::SlotRegistered { lane, index }),
                Err(PoolError::AlreadyRegistered) => {}
                Err(e) => {
                    events.push(BootstrapEvent::Waiting { why: format!("register {lane:?} {index}: {e:?}") });
                    return events;
                }
            }
        }
    }
    for lane in Lane::ALL {
        for slot in state::slots_in_lane(lane) {
            if slot.state != SlotState::Unverified {
                continue;
            }
            let target = BootstrapTarget::PoolSlot { lane, index: slot.index };
            match nonce::verify_slot(rpc, lane, slot.index).await {
                Ok(VerifyOutcome::Verified(_)) => {
                    state::remove_bootstrap_tx(target.key());
                    events.push(BootstrapEvent::SlotVerified { lane, index: slot.index });
                }
                Ok(VerifyOutcome::NotCreated) => {
                    let build = |hash| {
                        let (tx, address) = build_nonce_pool_account_message(&profile, &slot.seed, NONCE_ACCOUNT_RENT_LAMPORTS, hash).map_err(|e| format!("{e}"))?;
                        if address != slot.address {
                            return Err("the pool account address does not match its seed".to_string());
                        }
                        Ok(tx)
                    };
                    events.push(ensure_tx(rpc, signer, target, build, now, &mut blockhash).await);
                }
                Ok(VerifyOutcome::Problem(p)) => events.push(BootstrapEvent::SlotProblem { lane, index: slot.index, problem: p.to_string() }),
                Ok(VerifyOutcome::Unavailable(e)) => events.push(BootstrapEvent::Waiting { why: format!("pool slot read: {e:?}") }),
                Err(e) => events.push(BootstrapEvent::Waiting { why: format!("pool slot: {e:?}") }),
            }
        }
    }

    // ---- the mint and the return account ----
    let was = state::read_core(|c| c.mint_state);
    match mint::try_bind(rpc).await {
        Ok(BindOutcome::AlreadyBound) => {}
        Ok(BindOutcome::Bound) => {
            state::remove_bootstrap_tx(BootstrapTarget::Mint.key());
            state::remove_bootstrap_tx(BootstrapTarget::ReturnAta.key());
            if was != MintState::Bound {
                events.push(BootstrapEvent::MintBound);
            }
        }
        Ok(BindOutcome::NotCreated) => {
            let build = |hash| build_mint_creation_message(&profile, MINT_RENT_LAMPORTS, hash).map_err(|e| format!("{e}"));
            events.push(ensure_tx(rpc, signer, BootstrapTarget::Mint, build, now, &mut blockhash).await);
        }
        Ok(BindOutcome::ReturnAccountMissing) => {
            state::remove_bootstrap_tx(BootstrapTarget::Mint.key());
            let build = |hash| build_return_ata_message(&profile, hash).map_err(|e| format!("{e}"));
            events.push(ensure_tx(rpc, signer, BootstrapTarget::ReturnAta, build, now, &mut blockhash).await);
        }
        Ok(BindOutcome::Problem(p)) => events.push(BootstrapEvent::MintProblem(p.to_string())),
        Ok(BindOutcome::Unavailable(e)) => events.push(BootstrapEvent::Waiting { why: format!("mint read: {e:?}") }),
        Err(e) => events.push(BootstrapEvent::Waiting { why: format!("mint bind: {e}") }),
    }
    events
}
