//! The return observer (49c, `chain-solana.md` sections 2.2 and 4): it finds the finalized
//! transactions that moved tokens into the return account, decides STRICTLY which of them are valid
//! returns, and moves exactly those to the custodied bucket (`O -> RC`). Everything else that
//! increased the return account's balance is a stray: it stays in `O`, tracked, never credited.
//!
//! **Discovery.** `getSignaturesForAddress(return account, until = cursor)` pages walk from the
//! newest signature back to the cursor; the signatures found go into a stable queue keyed by
//! `(slot, signature)`. Only when the walk is complete (the oldest page was short) is the queue
//! processed, oldest first, and only then does the cursor move to the newest signature the walk saw.
//! A crash, an upgrade or an RPC failure at any point repeats work and loses none: processing is
//! idempotent (a return id or a stray signature is recorded once) and the cursor moves last.
//!
//! **Verification** (all must hold, on the transaction as `getTransaction` returns it at
//! `finalized`): `meta.err == null`; the message decodes strictly (no address lookups) and has the
//! exact shape of a return (`tx::shape`: one checked transfer to the canonical return account, one
//! return memo, nothing else but ComputeBudget); `innerInstructions` is present and empty;
//! `pre/postTokenBalances` are present; across every entry of this mint exactly two accounts changed,
//! the source down by the amount and the return account up by the amount; and the pre-balance
//! `owner` of the source account is reported and equals the transaction's signing authority (so a
//! delegate, or an omitted owner, is not a return). The source's owner, not the memo, is the
//! verified sender. A memo alone, or a balance delta alone, never credits.
//!
//! **Cost.** An idle observer lists the return account once per [`OBSERVER_IDLE_INTERVAL_NS`]; it
//! runs every pass while a walk or a queue is open, and at once for a user hint
//! (`notify_solana_return`). The idle interval is a cost/latency choice, not a measurement
//! (blocker `solana-rpc-consensus-unmeasured`).

use crate::accounting::Transition;
use crate::env;
use crate::flows::release;
use crate::ids::{self, Hash32};
use crate::records::Sig;
use crate::sol_rpc::{LandedTx, SolRpc, TokenBalanceEntry};
use crate::state::{self, AuditCtx, ReturnRecord, ReturnState, StrayRecord, StrayState};
use crate::tx::builders::TxProfile;
use crate::tx::shape::{check_return_shape, ReturnShape, ReturnShapeContext};
use crate::tx::{decode_message, split_wire_transaction, Message, Pubkey};
use candid::Principal;
use icrc_ledger_types::icrc1::account::Account;

/// Signatures requested per `getSignaturesForAddress` page.
pub const OBSERVER_PAGE: u32 = 50;
/// Pages one pass may fetch.
pub const MAX_PAGES_PER_PASS: usize = 2;
/// Transactions one pass may fetch and classify.
pub const MAX_TX_PER_PASS: usize = 10;
/// Hints one pass may follow.
pub const MAX_HINTS_PER_PASS: usize = 5;
/// How often an idle observer lists the return account.
pub const OBSERVER_IDLE_INTERVAL_NS: u64 = 5 * 60 * 1_000_000_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObserverEvent {
    /// A valid return was credited (`O -> RC`).
    Credited { return_id: Hash32, amount: u64 },
    /// A transfer into the return account failed a check: tracked as a stray.
    Stray { signature: Sig, amount: u64, reason: String },
    /// Nothing to record (failed, no balance increase, already recorded, ...).
    Ignored { signature: Sig, why: &'static str },
    /// Could not conclude; the pass stopped and the transaction stays queued.
    Waiting { why: String },
    CaughtUp,
}

/// What a classification decided; applied synchronously after the only await.
enum Verdict {
    Return(Verified),
    Stray { reason: String },
    Ignore(&'static str),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verified {
    pub source: Pubkey,
    pub sender: Pubkey,
    pub amount: u64,
    pub destination: Account,
    pub client_nonce: u64,
}

struct Ctx {
    profile: TxProfile,
    ata: Pubkey,
    bridge: Principal,
    genesis: Hash32,
    cap: u64,
}

fn context() -> Result<Ctx, String> {
    let (profile, genesis, cap) = state::read_core(|c| (c.tx_profile(), c.config.genesis_hash, c.config.limits.max_deposit_e8s));
    let profile = profile?;
    let ata = profile.return_ata().map_err(|e| format!("{e}"))?;
    Ok(Ctx { profile, ata, bridge: env::canister_id(), genesis, cap })
}

// ---------------------------------------------------------------------------
// Pure classification
// ---------------------------------------------------------------------------

/// A token account's balance of this mint in a transaction's pre and post balances. An account with
/// no entry on one side holds zero there (it did not exist, or holds none).
fn balance_at(entries: &[TokenBalanceEntry], mint: &Pubkey, index: u8) -> u64 {
    entries.iter().find(|e| e.account_index == index && &e.mint == mint).map(|e| e.amount).unwrap_or(0)
}

/// Every account index that has an entry of `mint` before or after, ascending.
fn mint_indexes(pre: &[TokenBalanceEntry], post: &[TokenBalanceEntry], mint: &Pubkey) -> Vec<u8> {
    let mut v: Vec<u8> = pre.iter().chain(post.iter()).filter(|e| &e.mint == mint).map(|e| e.account_index).collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// The strict return check (see the module docs). `delta` is the increase of the return account.
fn verify_return(ctx: &Ctx, msg: &Message, tx: &LandedTx, delta: u64) -> Result<Verified, String> {
    let shape_ctx = ReturnShapeContext {
        token_program: ctx.profile.token_program.id(),
        memo_program: ctx.profile.memo_program,
        mint: ctx.profile.mint,
        return_ata: ctx.ata,
        bridge: &ctx.bridge,
    };
    let shape: ReturnShape = check_return_shape(msg, &shape_ctx).map_err(|e| format!("not the exact shape of a return: {e:?}"))?;
    if tx.meta.inner_instruction_groups != Some(0) {
        return Err(format!("inner instructions are not reported empty ({:?})", tx.meta.inner_instruction_groups));
    }
    let (Some(pre), Some(post)) = (tx.meta.pre_token_balances.as_deref(), tx.meta.post_token_balances.as_deref()) else {
        return Err("token balances are not reported".into());
    };
    let mint = ctx.profile.mint;
    let source_index = msg.account_keys.iter().position(|k| *k == shape.source).ok_or("the source account is not a static key")? as u8;
    let ata_index = msg.account_keys.iter().position(|k| *k == ctx.ata).ok_or("the return account is not a static key")? as u8;
    // Exactly two accounts of this mint changed: the source down by the amount, the return account up.
    let changed: Vec<u8> = mint_indexes(pre, post, &mint).into_iter().filter(|i| balance_at(pre, &mint, *i) != balance_at(post, &mint, *i)).collect();
    if changed.len() != 2 || !changed.contains(&source_index) || !changed.contains(&ata_index) {
        return Err(format!("{} token accounts of the mint changed, not exactly the source and the return account", changed.len()));
    }
    let (src_pre, src_post) = (balance_at(pre, &mint, source_index), balance_at(post, &mint, source_index));
    let (ata_pre, ata_post) = (balance_at(pre, &mint, ata_index), balance_at(post, &mint, ata_index));
    if src_pre.checked_sub(src_post) != Some(shape.amount) || ata_post.checked_sub(ata_pre) != Some(shape.amount) || delta != shape.amount {
        return Err("the balance changes are not exactly the transferred amount".into());
    }
    // The pre-balance owner of the source must be reported and be the signing authority: a delegate,
    // or an omitted owner, is not a verified sender.
    let owner = pre
        .iter()
        .find(|e| e.account_index == source_index && e.mint == mint)
        .and_then(|e| e.owner)
        .ok_or("the RPC did not report the owner of the source account")?;
    if owner != shape.authority {
        return Err("the transfer authority is not the owner of the source account (a delegate)".into());
    }
    if shape.amount == 0 || shape.amount > ctx.cap {
        return Err(format!("the amount {} is outside [1, {}]", shape.amount, ctx.cap));
    }
    let destination = Account { owner: shape.memo.destination, subaccount: shape.memo.subaccount };
    Ok(Verified { source: shape.source, sender: shape.authority, amount: shape.amount, destination, client_nonce: shape.memo.client_nonce })
}

/// Classify one finalized transaction found in the return account's signature list.
fn classify(ctx: &Ctx, tx: &LandedTx) -> (Verdict, Option<StrayFacts>) {
    if tx.failed {
        return (Verdict::Ignore("the transaction failed: it moved nothing"), None);
    }
    let Ok((_, message_bytes)) = split_wire_transaction(&tx.wire) else {
        return (Verdict::Stray { reason: "the transaction bytes do not parse".into() }, None);
    };
    let msg = match decode_message(message_bytes) {
        Ok(m) => m,
        Err(e) => return (Verdict::Stray { reason: format!("the message does not decode strictly: {e}") }, None),
    };
    let mint = ctx.profile.mint;
    let Some(ata_index) = msg.account_keys.iter().position(|k| *k == ctx.ata).map(|i| i as u8) else {
        return (Verdict::Ignore("the return account is not a static key of this transaction"), None);
    };
    let (Some(pre), Some(post)) = (tx.meta.pre_token_balances.as_deref(), tx.meta.post_token_balances.as_deref()) else {
        // Whatever it moved, it cannot be measured: tracked at amount 0, and a real change shows up as
        // custody the books cannot explain (custody check, 49d).
        return (Verdict::Stray { reason: "token balances are not reported".into() }, Some(StrayFacts::unmeasured()));
    };
    let (before, after) = (balance_at(pre, &mint, ata_index), balance_at(post, &mint, ata_index));
    if after <= before {
        return (
            Verdict::Ignore(if after < before { "tokens left the return account (the bridge's own burn or refund)" } else { "no balance increase" }),
            None,
        );
    }
    let delta = after - before;
    let facts = StrayFacts::from_balances(&msg, pre, post, &mint, delta);
    match verify_return(ctx, &msg, tx, delta) {
        Ok(v) => (Verdict::Return(v), Some(facts)),
        Err(reason) => (Verdict::Stray { reason }, Some(facts.with_amount(delta))),
    }
}

/// What a stray's record keeps about where the tokens came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrayFacts {
    pub amount: u64,
    pub source_account: Option<Pubkey>,
    pub verified_sender: Option<Pubkey>,
}

impl StrayFacts {
    fn unmeasured() -> Self {
        StrayFacts { amount: 0, source_account: None, verified_sender: None }
    }

    fn with_amount(mut self, amount: u64) -> Self {
        self.amount = amount;
        self
    }

    /// The single account of this mint that fell by exactly `delta`, and its reported owner if that
    /// owner also signed the transaction.
    fn from_balances(msg: &Message, pre: &[TokenBalanceEntry], post: &[TokenBalanceEntry], mint: &Pubkey, delta: u64) -> Self {
        let decreased: Vec<u8> =
            mint_indexes(pre, post, mint).into_iter().filter(|i| balance_at(pre, mint, *i).checked_sub(balance_at(post, mint, *i)) == Some(delta)).collect();
        let [index] = decreased.as_slice() else { return StrayFacts { amount: delta, source_account: None, verified_sender: None } };
        let source_account = msg.account_keys.get(*index as usize).copied();
        let owner = pre.iter().find(|e| e.account_index == *index && &e.mint == mint).and_then(|e| e.owner);
        let verified_sender = owner.filter(|o| msg.required_signers().contains(o));
        StrayFacts { amount: delta, source_account, verified_sender }
    }
}

// ---------------------------------------------------------------------------
// Applying a verdict (synchronous)
// ---------------------------------------------------------------------------

enum Applied {
    Event(ObserverEvent),
    /// The books cannot take it yet (a mint it depends on is not applied): try again next pass.
    Wait(String),
}

/// Decide between "wait for the mint's settlement" and "supply anomaly" when `O` cannot cover a
/// credit: the tokens may belong to a mint that is final on chain but not applied yet.
fn cannot_take(what: &str, amount: u64) -> Applied {
    let (o, inflight) = (state::read_core(|c| c.buckets.outstanding), state::inflight_mint_e8s());
    if (amount as u128) <= o.saturating_add(inflight) {
        Applied::Wait(format!("{what} of {amount} e8s waits for in-flight mint settlement (outstanding {o}, in flight {inflight})"))
    } else {
        state::halt(format!("{what} of {amount} e8s exceeds outstanding supply {o} and everything in flight {inflight}"));
        Applied::Wait("halted: a return exceeds the supply the books know of".into())
    }
}

fn apply_verdict(ctx: &Ctx, signature: Sig, slot: u64, verdict: Verdict, facts: Option<StrayFacts>, now: u64) -> Applied {
    let return_id = ids::return_id(&ctx.genesis, &signature.0);
    // Re-check after the await: another pass may have recorded it.
    if state::get_return(&return_id).is_some() || state::get_stray(&signature).is_some() {
        return Applied::Event(ObserverEvent::Ignored { signature, why: "already recorded" });
    }
    match verdict {
        Verdict::Ignore(why) => Applied::Event(ObserverEvent::Ignored { signature, why }),
        Verdict::Return(v) => {
            if state::is_halted() {
                return Applied::Wait("the bridge is halted: no return is credited".into());
            }
            let t = Transition::ReturnCustodied { amount: v.amount };
            if state::apply_transition(t, AuditCtx::release(return_id, format!("return finalized at slot {slot}"))).is_err() {
                return cannot_take("return", v.amount);
            }
            let seq = state::mutate_core(|c| {
                let s = c.next_return_seq;
                c.next_return_seq += 1;
                s
            });
            state::put_return(&ReturnRecord {
                return_id,
                seq,
                signature,
                slot,
                source_account: v.source,
                sender: v.sender,
                amount: v.amount,
                destination: v.destination,
                client_nonce: v.client_nonce,
                state: ReturnState::Custodied,
                created_at_ns: now,
                updated_at_ns: now,
            });
            // The hint for this transaction has served its purpose.
            state::mutate_core(|c| c.hints.retain(|h| h.signature != signature));
            Applied::Event(ObserverEvent::Credited { return_id, amount: v.amount })
        }
        Verdict::Stray { reason } => {
            let facts = facts.unwrap_or_else(StrayFacts::unmeasured);
            if facts.amount > 0 && state::apply_transition(Transition::StrayObserved { amount: facts.amount }, AuditCtx::note(format!("stray at slot {slot}: {reason}"))).is_err() {
                return cannot_take("stray", facts.amount);
            }
            state::put_stray(&StrayRecord {
                signature,
                slot,
                amount: facts.amount,
                source_account: facts.source_account,
                verified_sender: facts.verified_sender,
                reason: reason.clone(),
                state: StrayState::Held,
                created_at_ns: now,
            });
            state::mutate_core(|c| c.hints.retain(|h| h.signature != signature));
            Applied::Event(ObserverEvent::Stray { signature, amount: facts.amount, reason })
        }
    }
}

/// Fetch, classify and apply one signature. `Err` means: do not advance, try again.
async fn process_signature<R: SolRpc>(rpc: &R, ctx: &Ctx, signature: Sig, now: u64) -> Result<ObserverEvent, String> {
    let tx = match rpc.get_transaction(&signature.0).await {
        Ok(Some(tx)) => tx,
        Ok(None) => return Err(format!("{} is listed but not retrievable yet", crate::tx::signature_to_base58(&signature.0))),
        Err(e) => return Err(format!("getTransaction: {e:?}")),
    };
    let (verdict, facts) = classify(ctx, &tx);
    match apply_verdict(ctx, signature, tx.slot, verdict, facts, now) {
        Applied::Event(e) => Ok(e),
        Applied::Wait(why) => Err(why),
    }
}

// ---------------------------------------------------------------------------
// The pass
// ---------------------------------------------------------------------------

fn due(now: u64) -> bool {
    let o = state::read_core(|c| c.observer.clone());
    o.walk_active
        || state::observer_queue_len() > 0
        || !release::live_hints(now).is_empty()
        || o.last_run_ns.is_none_or(|t| now.saturating_sub(t) >= OBSERVER_IDLE_INTERVAL_NS)
}

fn note(text: impl Into<String>) {
    let text = text.into();
    state::mutate_core(|c| c.observer.last_note = Some(text));
}

/// One observer pass: follow hints, extend the walk, process the queue. Bounded in RPC calls.
pub async fn observe<R: SolRpc>(rpc: &R, now: u64) -> Vec<ObserverEvent> {
    let mut events = vec![];
    if !state::read_core(|c| c.initialized) || state::read_core(|c| c.config.pauses.observer) || state::is_halted() {
        return events;
    }
    let Ok(ctx) = context() else { return events };
    if !due(now) {
        return events;
    }
    state::mutate_core(|c| c.observer.last_run_ns = Some(now));

    // ---- hints: process the named transaction at once; processing is idempotent ----
    for hint in release::live_hints(now).into_iter().take(MAX_HINTS_PER_PASS) {
        match process_signature(rpc, &ctx, hint.signature, now).await {
            Ok(e) => {
                state::mutate_core(|c| c.hints.retain(|h| h.signature != hint.signature));
                state::mutate_core(|c| c.observer.processed += 1);
                events.push(e);
            }
            Err(why) => events.push(ObserverEvent::Waiting { why }),
        }
    }

    // ---- the walk: from the newest signature back to the cursor ----
    let mut pages = 0usize;
    while state::read_core(|c| c.observer.walk_active) || pages == 0 {
        if pages >= MAX_PAGES_PER_PASS {
            break;
        }
        if !state::read_core(|c| c.observer.walk_active) {
            // Start a walk only when no queue is open.
            if state::observer_queue_len() > 0 {
                break;
            }
            state::mutate_core(|c| {
                c.observer.walk_active = true;
                c.observer.walk_before = None;
                c.observer.walk_newest = None;
            });
        }
        pages += 1;
        let (cursor, before) = state::read_core(|c| (c.observer.cursor, c.observer.walk_before));
        let page = match rpc
            .get_signatures_for_address(&ctx.ata, before.as_ref().map(|s| &s.0), cursor.as_ref().map(|s| &s.0), OBSERVER_PAGE)
            .await
        {
            Ok(p) => p,
            Err(e) => {
                note(format!("listing the return account: {e:?}"));
                events.push(ObserverEvent::Waiting { why: format!("getSignaturesForAddress: {e:?}") });
                return events;
            }
        };
        // The cursor or the walk may have been changed by another pass during the await (a reclaimed
        // guard): continue only if the walk is still the one this page belongs to.
        let still = state::read_core(|c| c.observer.walk_active && c.observer.walk_before == before && c.observer.cursor == cursor);
        if !still {
            events.push(ObserverEvent::Waiting { why: "the walk changed while a page was fetched".into() });
            return events;
        }
        let full = page.len() as u32 >= OBSERVER_PAGE;
        for entry in &page {
            if !entry.failed {
                state::observer_queue_insert(entry.slot, &Sig(entry.signature));
            }
        }
        state::mutate_core(|c| {
            if c.observer.walk_newest.is_none() {
                c.observer.walk_newest = page.first().map(|e| Sig(e.signature));
            }
            if full {
                c.observer.walk_before = page.last().map(|e| Sig(e.signature));
            } else {
                c.observer.walk_active = false;
                c.observer.walk_before = None;
            }
        });
        if !full {
            break;
        }
    }

    // ---- the queue: oldest first, only once the walk is complete ----
    if state::read_core(|c| c.observer.walk_active) {
        return events;
    }
    let mut taken = 0usize;
    while taken < MAX_TX_PER_PASS {
        let Some((slot, signature)) = state::observer_queue_first() else { break };
        match process_signature(rpc, &ctx, signature, now).await {
            Ok(e) => {
                state::observer_queue_remove(slot, &signature);
                state::mutate_core(|c| {
                    c.observer.processed += 1;
                    c.observer.last_progress_ns = Some(now);
                });
                events.push(e);
                taken += 1;
            }
            Err(why) => {
                note(why.clone());
                events.push(ObserverEvent::Waiting { why });
                return events;
            }
        }
    }
    if state::observer_queue_len() == 0 {
        // Everything the walk found is processed: the cursor moves, last.
        let moved = state::mutate_core(|c| {
            let newest = c.observer.walk_newest.take();
            let moved = newest.is_some();
            if let Some(n) = newest {
                c.observer.cursor = Some(n);
                c.observer.last_progress_ns = Some(now);
            }
            c.observer.caught_up = true;
            moved
        });
        if moved || events.is_empty() {
            events.push(ObserverEvent::CaughtUp);
        }
    }
    events
}
