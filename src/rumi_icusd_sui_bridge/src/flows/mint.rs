//! The Sui mint lane (53b): one escrowed deposit becomes one `rumi_icusd_gate::mint` transaction,
//! persisted before it is signed, signed by two threshold roles, broadcast, and settled only by chain
//! evidence read through the strict provider quorum.
//!
//! # The rules this lane keeps
//!
//! 1. **Admission and link in one synchronous step.** [`create_op`] re-validates the deposit, builds
//!    the transaction with the accepted strict builder, stores the operation AND links the deposit
//!    (`mint_op`, `MintInFlight`) with no await between, so no signing call can ever run for an
//!    operation the deposit does not point at. The facts it builds from (owned-object versions, gas
//!    price, epoch) are read BEFORE that step from a quorum at one finalized checkpoint at or after
//!    every earlier transaction of ours, and re-validated by the builder's own decoder.
//! 2. **Same attempt only.** An operation has attempts (generations). Rebroadcast and re-sign work
//!    on the CURRENT attempt's saved bytes only. A new attempt exists only after the previous one is
//!    proven dead: a finalized failed execution (its receipt) with the mint id still unconsumed, or
//!    proven never signed (a role's signature was never produced and none may exist). A passing
//!    epoch, a missing digest or an unchanged version never authorizes replacement, refund or
//!    release. A replacement reuses the SAME mint id, so the Gate aborts a duplicate.
//! 3. **Signing is persisted before the effect.** Before each `sign_with_schnorr` call the role's
//!    "a signature may exist" flag is stored; a definite refusal clears it, a signature is verified
//!    against the role key before it is kept, and a lost reply leaves the flag set. A stale
//!    continuation (the attempt changed while the call was in flight) discards its result.
//! 4. **Delivery is proven, never assumed.** A finalized SUCCESS receipt AND the consumed
//!    `mints[mint_id]` record with this id, recipient and amount; or the consumed record alone with
//!    id, recipient and amount matching (the transaction is then unnamed). A consumed id with another
//!    recipient or amount, a receipt without its record, or a record while our attempt failed halts
//!    the lane; nothing is credited.
//! 5. **Owned inputs are a sequencing token.** The `MintCap` and the gas coin are owned objects; the
//!    pinned source (see `reads.rs`) makes a transaction that executes advance their versions. So
//!    one operation is in flight at a time, and a version that moved with no receipt, no consumed
//!    record and the grace period elapsed halts the lane (an unexplained consumer): it does not
//!    replace the attempt.
//! 6. **Fees, gas, cycles and backing stay separate.** The gas coin pays gas; the mint amount is the
//!    deposit's `M`; the deposit fee moved from `PM` to the fee bucket when the mint landed; cycles
//!    pay signatures and outcalls. Nothing funds the gas coin or the canister automatically.
//!
//! The lane never reads a wallet or a key: signatures come from the threshold signer seam, and the
//! test keys are deterministic seeds.

use crate::accounting::Transition;
use crate::bind::{exclusion_text, lane_identity, LaneIdentity};
use crate::env;
use crate::flows::deposit;
use crate::gate::MintRecord;
use crate::graphql::GateFact;
use crate::ids::{self, Hash32, SuiRole};
use crate::mirror::{settle, Mirror, SuiTransport};
use crate::reads::{interpret_gas_coin, interpret_mint_cap, ObservedRef};
use crate::schnorr::{verify_signature, RoleSigner, SignOutcome};
use crate::state::{
    self, AttemptDisposition, AuditCtx, DeliveryVia, DepositKind, DepositRecord, DepositState, LedgerOpStatus, MintAttempt, MintDelivery, MintOp, MintOpState,
    ReleaseState, StoredRef,
};
use crate::sui::{format_digest_base58, Bytes32};
use crate::tx::{self, MintRequest};
use std::collections::BTreeSet;

/// A signed attempt is rebroadcast at most this often.
pub const RESEND_INTERVAL_NS: u64 = 60 * 1_000_000_000;
/// And at most this many times in all.
pub const MAX_SENDS_PER_ATTEMPT: u32 = 20;
/// After an unavailable or disagreeing answer the lane waits this long before asking for fresh facts.
pub const GATHER_BACKOFF_NS: u64 = 60 * 1_000_000_000;
/// An unresolved attempt is checked every pass for its first five minutes, every five minutes for its
/// first hour, then every half hour: an attempt held for days must not burn cycles every pass.
pub const CHECK_FAST_NS: u64 = 5 * 60 * 1_000_000_000;
pub const CHECK_SLOW_NS: u64 = 60 * 60 * 1_000_000_000;
pub const CHECK_MID_INTERVAL_NS: u64 = 5 * 60 * 1_000_000_000;
pub const CHECK_SLOW_INTERVAL_NS: u64 = 30 * 60 * 1_000_000_000;

/// Whether the providers should be asked about `a` now.
pub fn check_due(a: &MintAttempt, now: u64) -> bool {
    let Some(last) = a.last_check_ns else { return true };
    let age = now.saturating_sub(a.built_at_ns);
    let interval = if age < CHECK_FAST_NS {
        0
    } else if age < CHECK_SLOW_NS {
        CHECK_MID_INTERVAL_NS
    } else {
        CHECK_SLOW_INTERVAL_NS
    };
    now.saturating_sub(last) >= interval
}

fn mark_checked(op_id: u64) {
    if let Some(mut op) = state::get_mint_op(op_id) {
        if let Some(a) = op.attempts.last_mut() {
            a.last_check_ns = Some(env::now_ns());
            state::put_mint_op(&op);
        }
    }
}

/// How long a moved `MintCap` or gas-coin version may stay unexplained before the lane halts. The
/// receipt comes from an indexer that may trail the object store by a little.
pub const FOREIGN_GRACE_NS: u64 = 120 * 1_000_000_000;

// ---------------------------------------------------------------------------
// Readiness
// ---------------------------------------------------------------------------

/// Why a pass would not run the lane right now. Empty means it would.
pub fn blocked_by() -> Vec<String> {
    let mut why = vec![];
    let (halted, lane_halt, paused, gas_coin, observed) = state::read_runtime(|c| {
        (c.halted.clone(), c.mint_lane_halted.clone(), c.config.pauses.mints, c.config.lane.gas_coin, c.observed.is_some())
    });
    if let Some(h) = halted {
        why.push(format!("the bridge is halted: {}", h.reason));
    }
    if let Some(h) = lane_halt {
        why.push(format!("the mint lane is halted: {}", h.reason));
    }
    if paused {
        why.push("mints are paused".into());
    }
    if !state::read_core(|c| c.mirror.enabled) {
        why.push("the Sui provider mirror is disabled".into());
    }
    if let Err(e) = lane_identity() {
        why.push(e);
    }
    if !observed {
        why.push("the deployment has not been observed".into());
    }
    if gas_coin.is_none() {
        why.push("no gas coin is configured".into());
    }
    why
}

/// The synchronous guard every state-changing lane step repeats: the same conditions as a pass, minus
/// configuration that cannot change within one message.
fn lane_guard() -> Result<(), String> {
    state::read_runtime(|c| {
        if let Some(h) = &c.halted {
            return Err(format!("the bridge is halted: {}", h.reason));
        }
        if let Some(h) = &c.mint_lane_halted {
            return Err(format!("the mint lane is halted: {}", h.reason));
        }
        if c.config.pauses.mints {
            return Err("mints are paused".into());
        }
        Ok(())
    })
}

fn note_wait(reason: impl Into<String>) {
    let r = reason.into();
    state::mutate_runtime(|c| c.lane_wait = Some(r));
}

fn clear_wait() {
    state::mutate_runtime(|c| {
        c.lane_wait = None;
        c.gather_retry_at_ns = 0;
    });
}

/// Do not ask for fresh facts again for a minute.
fn back_off() {
    let until = env::now_ns().saturating_add(GATHER_BACKOFF_NS);
    state::mutate_runtime(|c| c.gather_retry_at_ns = until);
}

fn set_hold(op_id: u64, reason: Option<String>) {
    if let Some(mut op) = state::get_mint_op(op_id) {
        if op.hold_reason != reason {
            op.hold_reason = reason;
            op.updated_at_ns = env::now_ns();
            state::put_mint_op(&op);
        }
    }
}

// ---------------------------------------------------------------------------
// What a deposit asks the Gate to mint
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MintAsk {
    mint_id: Hash32,
    recipient: Bytes32,
    amount: u64,
}

/// What the deposit asks for, recomputed from the records it was derived from. A mint id that is not
/// the one the stored pull (or the held release it re-mints) yields is refused: the id is the
/// Gate-level idempotency key, so it is never taken on trust from a stored field.
fn deposit_ask(d: &DepositRecord) -> Result<MintAsk, String> {
    let mint_id = d.mint_id.ok_or("the deposit has no mint id yet")?;
    if d.mint_amount_e8s == 0 {
        return Err("the mint amount is zero".into());
    }
    match &d.kind {
        DepositKind::UserDeposit => {
            let block = match d.pull.as_ref().map(|p| &p.status) {
                Some(LedgerOpStatus::Succeeded { block_index }) => *block_index,
                _ => return Err("the escrow pull is not proven".into()),
            };
            let expected = deposit::mint_id_for(d, block).ok_or("the mint id cannot be derived (no gate binding)")?;
            if expected != mint_id {
                return Err("the stored mint id is not the one the proven pull yields".into());
            }
        }
        DepositKind::Remint { return_id } => {
            let r = state::get_release(return_id).ok_or("the re-minted release does not exist")?;
            if r.state != (ReleaseState::Reminting { deposit_id: d.id }) {
                return Err("the release is not being re-minted by this deposit".into());
            }
            if r.sui_sender != d.sui_recipient || r.amount_e8s != d.mint_amount_e8s || d.amount_e8s != r.amount_e8s || d.deposit_fee_e8s != 0 {
                return Err("the re-mint does not match its release".into());
            }
            let (chain, binding, salt) = state::read_core(|c| (c.chain_identifier, c.binding.clone(), c.instance_salt));
            let b = binding.ok_or("no gate binding")?;
            let expected = ids::remint_id(&chain, &b.package, &b.gate, &salt, return_id, &r.sui_sender, r.amount_e8s);
            if expected != mint_id {
                return Err("the stored mint id is not the re-mint id of its release".into());
            }
        }
    }
    Ok(MintAsk { mint_id, recipient: d.sui_recipient, amount: d.mint_amount_e8s })
}

// ---------------------------------------------------------------------------
// The refund and retry guard over every operation a deposit ever had
// ---------------------------------------------------------------------------

/// Whether every mint operation `d` ever had is provably unable to deliver, from the lane's OWN
/// stored records, re-derived here rather than trusted:
///
/// * the deposit has no live operation;
/// * the deposit's list of abandoned operations equals the by-deposit index (nothing is missing from
///   either);
/// * every one of them belongs to this deposit with this mint id, recipient and amount, is `Failed`,
///   and has only attempts that are either a finalized FAILED execution (whose stored bytes still
///   hash to the stored transaction digest) or proven never signed (no role's signature exists or may
///   exist).
///
/// An unknown or merely abandoned signed operation never passes. A deposit that never had an
/// operation passes trivially.
pub fn prior_ops_provably_dead(d: &DepositRecord) -> Result<(), String> {
    if d.mint_op.is_some() {
        return Err("the deposit has a live mint operation".into());
    }
    let indexed: BTreeSet<u64> = state::mint_op_ids_for_deposit(d.id).into_iter().collect();
    let listed: BTreeSet<u64> = d.abandoned_mint_ops.iter().copied().collect();
    if indexed != listed {
        return Err("the deposit's mint operations disagree with the operation index; it is not refundable".into());
    }
    for id in indexed {
        let op = state::get_mint_op(id).ok_or_else(|| format!("mint operation {id} is missing"))?;
        if op.deposit_id != d.id || Some(op.mint_id) != d.mint_id || op.recipient != d.sui_recipient || op.amount_e8s != d.mint_amount_e8s {
            return Err(format!("mint operation {id} does not describe this deposit"));
        }
        if !matches!(op.state, MintOpState::Failed { .. }) {
            return Err(format!("mint operation {id} is not a failed operation"));
        }
        if op.attempts.is_empty() {
            return Err(format!("mint operation {id} has no attempts to judge"));
        }
        for a in &op.attempts {
            match &a.disposition {
                AttemptDisposition::NeverSigned { .. } if !a.may_be_complete() => {}
                AttemptDisposition::NeverSigned { .. } => {
                    return Err(format!("attempt {} of operation {id} may have a complete signature; it is not proven dead", a.generation));
                }
                AttemptDisposition::FailedFinal { .. } if tx::transaction_digest(&a.tx_bcs) == a.tx_digest => {}
                AttemptDisposition::FailedFinal { .. } => {
                    return Err(format!("attempt {} of operation {id} has a receipt for other bytes than it stores", a.generation));
                }
                AttemptDisposition::Open | AttemptDisposition::LandedFinal { .. } => {
                    return Err(format!("attempt {} of operation {id} is not proven dead", a.generation));
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Facts
// ---------------------------------------------------------------------------

/// What a build needs, read from a quorum as of one finalized checkpoint.
#[derive(Clone, Debug)]
struct Facts {
    checkpoint: u64,
    epoch: u64,
    gas_price: u64,
    gate: GateFact,
    mint_cap: ObservedRef,
    gas: ObservedRef,
    /// The gas budget the coin's balance was checked against when the facts were read.
    gas_budget: u64,
}

enum Gather {
    /// Providers unavailable or disagreeing, or behind our own last transaction: retry.
    Wait(String),
    /// The mint id is already consumed on chain.
    Consumed(MintRecord),
    /// What the providers agree on is not the configured deployment.
    Mismatch(String),
}

async fn head_checkpoint<T: SuiTransport>(mirror: &Mirror<'_, T>) -> Result<u64, String> {
    settle("head", mirror.head().await)?.ok_or_else(|| "the providers have no checkpoint yet".to_string())
}

async fn gather<T: SuiTransport>(mirror: &Mirror<'_, T>, id: &LaneIdentity, mint_id: &Hash32) -> Result<Facts, Gather> {
    let (gas_coin, gas_budget, floor) = state::read_runtime(|c| (c.config.lane.gas_coin, c.config.lane.gas_budget, c.last_final_checkpoint));
    let gas_coin = gas_coin.ok_or_else(|| Gather::Wait("no gas coin is configured".into()))?;
    let head = head_checkpoint(mirror).await.map_err(Gather::Wait)?;
    if head < floor {
        return Err(Gather::Wait(format!("the providers' head {head} is behind our last finalized transaction (checkpoint {floor})")));
    }
    let cp = settle("checkpoint", mirror.checkpoint_gas(head).await)
        .map_err(Gather::Wait)?
        .ok_or_else(|| Gather::Wait(format!("the providers do not yet serve checkpoint {head}")))?;
    let gate = settle("gate", mirror.gate(&id.cfg, head).await)
        .map_err(Gather::Wait)?
        .ok_or_else(|| Gather::Mismatch(format!("the configured gate does not exist as of checkpoint {head}")))?;
    if let Some(rec) = settle("mint record", mirror.mint_entry(&id.cfg, &gate.gate.mints_table, head, mint_id).await).map_err(Gather::Wait)? {
        return Err(Gather::Consumed(rec));
    }
    let cap_obj = settle("mint cap", mirror.owned_object(&id.cfg.mint_cap, head).await)
        .map_err(Gather::Wait)?
        .ok_or_else(|| Gather::Mismatch("the configured mint cap does not exist".into()))?;
    let mint_cap = interpret_mint_cap(&cap_obj, &id.cfg, &id.roles.treasury).map_err(|e| Gather::Mismatch(exclusion_text(&e)))?;
    let gas_obj = settle("gas coin", mirror.owned_object(&gas_coin, head).await)
        .map_err(Gather::Wait)?
        .ok_or_else(|| Gather::Wait("the configured gas coin does not exist".into()))?;
    let (gas, _balance) = interpret_gas_coin(&gas_obj, &gas_coin, &id.roles.gas, gas_budget).map_err(|e| Gather::Wait(exclusion_text(&e)))?;
    Ok(Facts { checkpoint: head, epoch: cp.epoch, gas_price: cp.reference_gas_price, gate, mint_cap, gas, gas_budget })
}

// ---------------------------------------------------------------------------
// Building (synchronous)
// ---------------------------------------------------------------------------

enum Refused {
    /// The deposit may be held and retried (a preflight condition).
    Hold(String),
    /// Something is inconsistent or the lane is not allowed to proceed; nothing changed.
    Stop(String),
}

fn stop<T>(m: impl Into<String>) -> Result<T, Refused> {
    Err(Refused::Stop(m.into()))
}

/// The attempt for `ask` from `facts`: built with the accepted strict builder, then decoded again by
/// the shape guard and compared field by field with the ask and the facts. Pure.
fn build_attempt(generation: u32, ask: &MintAsk, facts: &Facts, id: &LaneIdentity, now: u64) -> Result<MintAttempt, Refused> {
    let gas_budget = state::read_runtime(|c| c.config.lane.gas_budget);
    let req = MintRequest {
        gate_initial_shared_version: facts.gate.initial_shared_version,
        mint_cap: facts.mint_cap.into(),
        mint_id: ask.mint_id,
        recipient: ask.recipient,
        amount: ask.amount,
        gas_payment: facts.gas.into(),
        gas_price: facts.gas_price,
        gas_budget,
        epoch: facts.epoch,
    };
    let built = tx::build_mint_transaction(&id.cfg, &id.roles, &req).map_err(|e| Refused::Stop(format!("the mint transaction cannot be built: {e}")))?;
    let parsed = tx::check_mint_transaction(&built.bcs, &id.cfg, &id.roles).map_err(|e| Refused::Stop(format!("the built bytes fail the shape guard: {e}")))?;
    if parsed.mint_id != ask.mint_id || parsed.recipient != ask.recipient || parsed.amount != ask.amount || parsed.epoch != facts.epoch {
        return stop("the built transaction does not carry the request");
    }
    Ok(MintAttempt {
        generation,
        tx_bcs: built.bcs,
        signing_digest: built.signing_digest,
        tx_digest: built.transaction_digest,
        mint_cap: StoredRef::from(facts.mint_cap),
        gas_coin: StoredRef::from(facts.gas),
        gate_initial_shared_version: facts.gate.initial_shared_version,
        gas_price: facts.gas_price,
        gas_budget,
        epoch: facts.epoch,
        built_checkpoint: facts.checkpoint,
        built_at_ns: now,
        treasury_sig: None,
        gas_sig: None,
        treasury_sig_unknown: false,
        gas_sig_unknown: false,
        treasury_sign_calls: 0,
        gas_sign_calls: 0,
        sends: 0,
        last_send_ns: None,
        last_send_note: None,
        versions_changed_since_ns: None,
        last_check_ns: None,
        disposition: AttemptDisposition::Open,
    })
}

/// Preflight against the observed Gate: the supply cap, and the per-operation cap for a user deposit.
fn preflight(d: &DepositRecord, ask: &MintAsk, facts: &Facts) -> Result<(), Refused> {
    let g = &facts.gate.gate;
    if (g.total_supply as u128) + (ask.amount as u128) > g.supply_cap as u128 {
        return Err(Refused::Hold(format!("the Gate's supply cap ({}) would be exceeded: supply {} plus {}", g.supply_cap, g.total_supply, ask.amount)));
    }
    if matches!(d.kind, DepositKind::UserDeposit) && ask.amount > state::read_runtime(|c| c.config.limits.max_deposit_e8s) {
        return Err(Refused::Hold("the mint amount is above the per-operation cap".into()));
    }
    Ok(())
}

/// The facts were read across awaits, and a slower older pass can finish after a newer one advanced our
/// state. Everything they were read against is re-checked against the CURRENT state, synchronously, at
/// admission: they must not be behind our last finalized transaction (their owned references could be
/// spent), and the gas coin and budget must still be the ones they were validated for. Otherwise the
/// lane waits and re-gathers; nothing is admitted from a pre-await snapshot.
fn facts_current(facts: &Facts) -> Result<(), Refused> {
    let (floor, coin, budget) = state::read_runtime(|c| (c.last_final_checkpoint, c.config.lane.gas_coin, c.config.lane.gas_budget));
    if facts.checkpoint < floor {
        return stop(format!(
            "the facts were read at checkpoint {} behind our last finalized transaction (checkpoint {floor}); they will be read again",
            facts.checkpoint
        ));
    }
    if coin != Some(facts.gas.id) {
        return stop("the gas coin was reconfigured while the facts were being read; they will be read again");
    }
    if budget > facts.gas_budget {
        return stop("the gas budget was raised after the coin's balance was checked; the facts will be read again");
    }
    Ok(())
}

/// Create the operation for `deposit_id` AND link the deposit to it, in one synchronous step.
fn create_op(deposit_id: u64, facts: &Facts, id: &LaneIdentity) -> Result<u64, Refused> {
    lane_guard().map_err(Refused::Stop)?;
    facts_current(facts)?;
    if state::read_runtime(|c| c.active_mint_op).is_some() {
        return stop("another mint operation is in flight");
    }
    let mut d = state::get_deposit(deposit_id).ok_or_else(|| Refused::Stop("the deposit does not exist".into()))?;
    if !matches!(d.state, DepositState::Escrowed) || d.mint_op.is_some() {
        return stop("the deposit is not waiting for a mint operation");
    }
    prior_ops_provably_dead(&d).map_err(Refused::Stop)?;
    let ask = deposit_ask(&d).map_err(Refused::Stop)?;
    preflight(&d, &ask, facts)?;
    let now = env::now_ns();
    let attempt = build_attempt(1, &ask, facts, id, now)?;
    let op_id = state::mutate_runtime(|c| {
        let n = c.next_mint_op_id;
        c.next_mint_op_id += 1;
        c.active_mint_op = Some(n);
        c.lane_wait = None;
        c.gather_retry_at_ns = 0;
        n
    });
    let op = MintOp {
        id: op_id,
        deposit_id,
        mint_id: ask.mint_id,
        recipient: ask.recipient,
        amount_e8s: ask.amount,
        attempts: vec![attempt],
        state: MintOpState::Active,
        hold_reason: None,
        created_at_ns: now,
        updated_at_ns: now,
    };
    state::put_mint_op(&op);
    d.mint_op = Some(op_id);
    d.state = DepositState::MintInFlight;
    d.hold_retry_at_ns = None;
    d.updated_at_ns = now;
    state::put_deposit(&d);
    state::append_audit(
        None,
        AuditCtx { note: format!("mint operation {op_id} created at checkpoint {} (tx {})", facts.checkpoint, format_digest_base58(&op.attempts[0].tx_digest)), deposit_id: Some(deposit_id), return_id: None, op_id: Some(op_id) },
    );
    Ok(op_id)
}

/// Append the next attempt to an operation whose last attempt is proven dead. Synchronous.
fn append_attempt(op_id: u64, facts: &Facts, id: &LaneIdentity) -> Result<(), Refused> {
    lane_guard().map_err(Refused::Stop)?;
    facts_current(facts)?;
    let mut op = state::get_mint_op(op_id).ok_or_else(|| Refused::Stop("the operation does not exist".into()))?;
    if !matches!(op.state, MintOpState::Active) || state::read_runtime(|c| c.active_mint_op) != Some(op_id) {
        return stop("the operation is not the active one");
    }
    let last = op.current().ok_or_else(|| Refused::Stop("the operation has no attempt".into()))?;
    if !matches!(last.disposition, AttemptDisposition::FailedFinal { .. } | AttemptDisposition::NeverSigned { .. }) {
        return stop("the previous attempt is not proven dead");
    }
    let max = state::read_runtime(|c| c.config.lane.max_attempts);
    if op.attempts.len() as u32 >= max {
        return stop("the operation has used its attempts");
    }
    let d = state::get_deposit(op.deposit_id).ok_or_else(|| Refused::Stop("the deposit does not exist".into()))?;
    if d.mint_op != Some(op_id) || !matches!(d.state, DepositState::MintInFlight) {
        return stop("the deposit is not linked to this operation");
    }
    let ask = deposit_ask(&d).map_err(Refused::Stop)?;
    if ask.mint_id != op.mint_id || ask.recipient != op.recipient || ask.amount != op.amount_e8s {
        return stop("the deposit no longer matches the operation");
    }
    preflight(&d, &ask, facts)?;
    let now = env::now_ns();
    let generation = op.attempts.len() as u32 + 1;
    let attempt = build_attempt(generation, &ask, facts, id, now)?;
    op.attempts.push(attempt);
    op.hold_reason = None;
    op.updated_at_ns = now;
    state::put_mint_op(&op);
    state::append_audit(None, AuditCtx::op(op_id, format!("attempt {generation} built at checkpoint {}", facts.checkpoint)));
    Ok(())
}

// ---------------------------------------------------------------------------
// Settling (synchronous)
// ---------------------------------------------------------------------------

/// The mint landed. Applies the books, finishes the deposit (and the release a re-mint serves) and
/// frees the lane, all in one step.
fn finish_delivered(op_id: u64, generation: u32, tx_digest: Option<Hash32>, checkpoint: u64, sequence: u64, via: DeliveryVia) -> Result<(), String> {
    let mut op = state::get_mint_op(op_id).ok_or("the operation does not exist")?;
    if !matches!(op.state, MintOpState::Active) {
        return Ok(());
    }
    let mut d = state::get_deposit(op.deposit_id).ok_or("the deposit does not exist")?;
    if d.mint_op != Some(op_id) || !matches!(d.state, DepositState::MintInFlight) {
        return Err("the deposit is not linked to this operation".into());
    }
    let t = match d.kind {
        DepositKind::UserDeposit => Transition::MintDelivered { amount: d.amount_e8s, fee: d.deposit_fee_e8s },
        DepositKind::Remint { .. } => Transition::RemintDelivered { amount: d.amount_e8s },
    };
    if let Err(e) = state::apply_transition(t, AuditCtx { note: format!("mint operation {op_id} delivered ({via:?})"), deposit_id: Some(d.id), return_id: None, op_id: Some(op_id) }) {
        state::halt(format!("the books refused the delivered mint of operation {op_id}: {e}"));
        return Err(e.to_string());
    }
    let now = env::now_ns();
    if via == DeliveryVia::Receipt {
        if let Some(a) = op.attempts.iter_mut().find(|a| a.generation == generation) {
            a.disposition = AttemptDisposition::LandedFinal { checkpoint };
        }
    }
    op.state = MintOpState::Delivered { tx_digest, checkpoint, sequence, via };
    op.hold_reason = None;
    op.updated_at_ns = now;
    state::put_mint_op(&op);
    d.state = DepositState::Delivered;
    d.delivered = Some(MintDelivery { tx_digest, checkpoint, sequence });
    d.updated_at_ns = now;
    state::put_deposit(&d);
    if let DepositKind::Remint { return_id } = d.kind {
        if let Some(mut r) = state::get_release(&return_id) {
            if r.state == (ReleaseState::Reminting { deposit_id: d.id }) {
                r.state = ReleaseState::Reminted { deposit_id: d.id };
                r.updated_at_ns = now;
                state::put_release(&r);
            }
        }
    }
    state::mutate_runtime(|c| {
        c.active_mint_op = None;
        c.last_final_checkpoint = c.last_final_checkpoint.max(checkpoint);
        c.lane_wait = None;
    });
    Ok(())
}

/// Every attempt is dead and none is left: the operation fails and the deposit returns to
/// `MintHeld`, where its owner may refund it (a user deposit) or an admin retry it.
fn fail_op(op_id: u64, reason: &str) {
    let Some(mut op) = state::get_mint_op(op_id) else { return };
    if !matches!(op.state, MintOpState::Active) {
        return;
    }
    let now = env::now_ns();
    op.state = MintOpState::Failed { reason: reason.to_string(), at_ns: now };
    op.hold_reason = None;
    op.updated_at_ns = now;
    state::put_mint_op(&op);
    if let Some(mut d) = state::get_deposit(op.deposit_id) {
        if d.mint_op == Some(op_id) {
            d.mint_op = None;
            if !d.abandoned_mint_ops.contains(&op_id) {
                d.abandoned_mint_ops.push(op_id);
            }
            d.state = DepositState::MintHeld { reason: format!("mint operation {op_id} failed: {reason}") };
            d.hold_retry_at_ns = None;
            d.updated_at_ns = now;
            state::put_deposit(&d);
        }
    }
    state::mutate_runtime(|c| {
        if c.active_mint_op == Some(op_id) {
            c.active_mint_op = None;
        }
    });
    state::append_audit(None, AuditCtx::op(op_id, format!("mint operation failed: {reason}")));
}

/// Close the current attempt with `disposition`, only if it is still the attempt the caller looked at.
fn close_attempt(op_id: u64, generation: u32, tx_digest: &Hash32, disposition: AttemptDisposition, final_checkpoint: Option<u64>) -> bool {
    let Some(mut op) = state::get_mint_op(op_id) else { return false };
    if !matches!(op.state, MintOpState::Active) {
        return false;
    }
    let Some(a) = op.attempts.last_mut() else { return false };
    if a.generation != generation || &a.tx_digest != tx_digest || !a.is_open() {
        return false;
    }
    a.disposition = disposition.clone();
    op.hold_reason = None;
    op.updated_at_ns = env::now_ns();
    state::put_mint_op(&op);
    if let Some(cp) = final_checkpoint {
        state::mutate_runtime(|c| c.last_final_checkpoint = c.last_final_checkpoint.max(cp));
    }
    state::append_audit(None, AuditCtx::op(op_id, format!("attempt {generation} closed: {disposition:?}")));
    true
}

// ---------------------------------------------------------------------------
// Signing
// ---------------------------------------------------------------------------

/// The attempt a signing step looked at, and everything the signing call needs.
pub(crate) struct SignJob {
    generation: u32,
    tx_digest: Hash32,
    signing_digest: Bytes32,
    /// This call's number among the role's calls on the attempt.
    call_no: u32,
    /// Nothing earlier could have signed when this call began (the role's flag was clear). Only such a
    /// call, while it is still the latest, may clear the flag with a definite refusal.
    clean_start: bool,
}

/// Re-check the stored request, amount, recipient and gas limits and decide whether `role` still needs
/// a signature on the current attempt; and, before any await, mark that a signature for the role may
/// come to exist. Synchronous.
pub(crate) fn begin_sign(op_id: u64, role: SuiRole, id: &LaneIdentity) -> Result<Option<SignJob>, String> {
    lane_guard()?;
    let mut op = state::get_mint_op(op_id).ok_or("the operation does not exist")?;
    if !matches!(op.state, MintOpState::Active) {
        return Err("the operation is not active".into());
    }
    let d = state::get_deposit(op.deposit_id).ok_or("the deposit does not exist")?;
    if d.mint_op != Some(op_id) || !matches!(d.state, DepositState::MintInFlight) {
        return Err("the deposit is not linked to this operation".into());
    }
    let ask = deposit_ask(&d)?;
    if ask.mint_id != op.mint_id || ask.recipient != op.recipient || ask.amount != op.amount_e8s {
        return Err("the deposit no longer matches the operation".into());
    }
    let (lane, max_deposit) = state::read_runtime(|c| (c.config.lane.clone(), c.config.limits.max_deposit_e8s));
    let a = op.attempts.last_mut().ok_or("the operation has no attempt")?;
    if !a.is_open() {
        return Err("the current attempt is closed".into());
    }
    let parsed = tx::check_mint_transaction(&a.tx_bcs, &id.cfg, &id.roles).map_err(|e| format!("the stored transaction fails the shape guard: {e}"))?;
    if tx::signing_digest(&a.tx_bcs) != a.signing_digest || tx::transaction_digest(&a.tx_bcs) != a.tx_digest {
        return Err("the stored digests do not match the stored bytes".into());
    }
    if parsed.mint_id != op.mint_id || parsed.recipient != op.recipient || parsed.amount != op.amount_e8s {
        return Err("the stored transaction does not carry the operation's request".into());
    }
    if matches!(d.kind, DepositKind::UserDeposit) && parsed.amount > max_deposit {
        return Err("the stored amount is above the per-operation cap".into());
    }
    if Some(parsed.gas_payment[0].id) != lane.gas_coin || parsed.gas_budget > lane.gas_budget {
        return Err("the stored gas coin or budget is no longer the configured one".into());
    }
    let (sig, unknown, calls) = match role {
        SuiRole::Treasury => (a.treasury_sig.is_some(), &mut a.treasury_sig_unknown, &mut a.treasury_sign_calls),
        SuiRole::Gas => (a.gas_sig.is_some(), &mut a.gas_sig_unknown, &mut a.gas_sign_calls),
        SuiRole::UpgradeCustody => return Err("the upgrade-custody role never signs a mint".into()),
    };
    if sig {
        return Ok(None);
    }
    // Remember whether an earlier call already left this role uncertain, and number this call, BEFORE
    // marking: a signature for this role may exist from the moment the call is made.
    let clean_start = !*unknown;
    *calls += 1;
    let call_no = *calls;
    *unknown = true;
    let job = SignJob { generation: a.generation, tx_digest: a.tx_digest, signing_digest: a.signing_digest, call_no, clean_start };
    op.updated_at_ns = env::now_ns();
    state::put_mint_op(&op);
    Ok(Some(job))
}

/// Apply the outcome of a signing call to the attempt it was made for. A result for an attempt that
/// has since changed or closed is discarded. Synchronous.
fn finish_sign(op_id: u64, role: SuiRole, job: &SignJob, outcome: SignOutcome, id: &LaneIdentity) {
    let Some(mut op) = state::get_mint_op(op_id) else { return };
    if !matches!(op.state, MintOpState::Active) {
        return;
    }
    let Some(a) = op.attempts.last_mut() else { return };
    if a.generation != job.generation || a.tx_digest != job.tx_digest || !a.is_open() {
        // Stale continuation: the attempt it signed for is not the current open one. The flag stays
        // set on that attempt, so it can never be judged "never signed".
        return;
    }
    let key = match role {
        SuiRole::Treasury => id.treasury_key,
        SuiRole::Gas => id.gas_key,
        SuiRole::UpgradeCustody => return,
    };
    let (slot, unknown, calls) = match role {
        SuiRole::Treasury => (&mut a.treasury_sig, &mut a.treasury_sig_unknown, a.treasury_sign_calls),
        SuiRole::Gas => (&mut a.gas_sig, &mut a.gas_sig_unknown, a.gas_sign_calls),
        SuiRole::UpgradeCustody => return,
    };
    let note = match outcome {
        SignOutcome::Signed(sig) => {
            if verify_signature(&key, &job.signing_digest, &sig) {
                *slot = Some(sig.to_vec());
                *unknown = false;
                None
            } else {
                // Garbage behind a reply: a valid signature may exist elsewhere, so the flag stays.
                Some(format!("the {role:?} signature does not verify against the role key"))
            }
        }
        SignOutcome::Failed(why) => {
            // A definite refusal proves only that THIS call signed nothing. It resolves the role's
            // uncertainty only when nothing earlier could have signed (the flag was clear when the call
            // began) and no later call has begun since; otherwise an older unknown outcome, or a newer
            // call in flight, still stands and only a verified stored signature will clear it.
            if job.clean_start && calls == job.call_no {
                *unknown = false;
                Some(format!("{role:?} signing refused: {why}"))
            } else {
                Some(format!("{role:?} signing refused: {why} (an earlier or overlapping call is still unresolved)"))
            }
        }
        SignOutcome::Unknown(why) => Some(format!("{role:?} signing outcome unknown: {why}")),
    };
    // A signing note stays visible until the role signs; a signature clears only its own role's note.
    match note {
        Some(n) => op.hold_reason = Some(n),
        None => {
            if op.hold_reason.as_deref().is_some_and(|h| h.starts_with(&format!("{role:?} signing"))) {
                op.hold_reason = None;
            }
        }
    }
    op.updated_at_ns = env::now_ns();
    state::put_mint_op(&op);
}

/// Sign what is missing on the current attempt: treasury first, then gas. The same bytes every time.
async fn sign_attempt<S: RoleSigner>(signer: &S, op_id: u64, id: &LaneIdentity) {
    for role in [SuiRole::Treasury, SuiRole::Gas] {
        let job = match begin_sign(op_id, role, id) {
            Ok(Some(job)) => job,
            Ok(None) => continue,
            Err(why) => {
                set_hold(op_id, Some(why));
                return;
            }
        };
        let outcome = signer.sign(role, &job.signing_digest).await;
        finish_sign(op_id, role, &job, outcome, id);
    }
}

// ---------------------------------------------------------------------------
// Broadcast and reconciliation
// ---------------------------------------------------------------------------

fn signatures_of(a: &MintAttempt, id: &LaneIdentity) -> Option<[String; 2]> {
    let treasury: [u8; 64] = a.treasury_sig.as_deref()?.try_into().ok()?;
    let gas: [u8; 64] = a.gas_sig.as_deref()?.try_into().ok()?;
    Some([tx::wire_signature_base64(&treasury, &id.treasury_key), tx::wire_signature_base64(&gas, &id.gas_key)])
}

/// Broadcast the current attempt if it is due. Returns false when the lane may no longer act (a pause
/// or a halt arrived while a signature was in flight): the signed attempt stays as it is, held with its
/// exact bytes, with no send persisted or made.
async fn maybe_send<T: SuiTransport>(mirror: &Mirror<'_, T>, op_id: u64, id: &LaneIdentity) -> bool {
    // The same synchronous guard as every other state-changing lane step, immediately before the send is
    // persisted: the pass checked it before the signing awaits, and those awaits may have outlived it.
    if let Err(why) = lane_guard() {
        set_hold(op_id, Some(format!("not sent: {why}")));
        return false;
    }
    let now = env::now_ns();
    let Some(op) = state::get_mint_op(op_id) else { return true };
    let Some(a) = op.current() else { return true };
    if !a.is_open() || a.sends >= MAX_SENDS_PER_ATTEMPT {
        return true;
    }
    if a.last_send_ns.is_some_and(|t| now.saturating_sub(t) < RESEND_INTERVAL_NS) {
        return true;
    }
    let Some(sigs) = signatures_of(a, id) else { return true };
    let (bytes, generation, digest) = (a.tx_bcs.clone(), a.generation, a.tx_digest);
    // Persist the send BEFORE the call: a trap or a lost reply must read as "maybe sent".
    {
        let mut op = op;
        if let Some(a) = op.attempts.last_mut() {
            a.sends += 1;
            a.last_send_ns = Some(now);
        }
        op.updated_at_ns = now;
        state::put_mint_op(&op);
    }
    let report = mirror.send(&bytes, &sigs).await;
    // Record the note only if it is still the same attempt.
    if let Some(mut op) = state::get_mint_op(op_id) {
        if let Some(a) = op.attempts.last_mut() {
            if a.generation == generation && a.tx_digest == digest {
                a.last_send_note = Some(format!("{} provider(s) HTTP-acknowledged (maybe sent), {} failed{}", report.accepted, report.failed, report.last_error.map(|e| format!(" ({e})")).unwrap_or_default()));
                state::put_mint_op(&op);
            }
        }
    }
    true
}

fn record_matches(rec: &MintRecord, op: &MintOp) -> bool {
    rec.recipient == op.recipient && rec.amount == op.amount_e8s
}

/// Settle the current attempt from chain evidence. Every branch is either a proven end (delivered,
/// failed with a receipt), a halt of the mint lane, or a wait; none replaces the attempt on an absence.
///
/// With a receipt, the Gate and the consumed-record table are read as of the receipt's OWN checkpoint,
/// so the answer does not depend on how far the providers' head has moved. Without one, they are read
/// as of the head: a consumed record there still proves delivery (when it matches), an absent one
/// proves nothing.
async fn reconcile<T: SuiTransport>(mirror: &Mirror<'_, T>, id: &LaneIdentity, op_id: u64) {
    let Some(op) = state::get_mint_op(op_id) else { return };
    let Some(att) = op.current().cloned() else { return };
    if !att.is_open() || signatures_of(&att, id).is_none() {
        return;
    }
    let generation = att.generation;
    let waiting = |why: String| {
        note_wait(why.clone());
        set_hold(op_id, Some(why));
    };

    let receipt = match settle("transaction", mirror.transaction(&att.tx_digest).await) {
        Ok(r) => r,
        Err(w) => return waiting(w),
    };
    let at = match &receipt {
        Some(r) => r.checkpoint,
        None => match head_checkpoint(mirror).await {
            Ok(h) => h,
            Err(w) => return waiting(w),
        },
    };
    let gate = match settle("gate", mirror.gate(&id.cfg, at).await) {
        Ok(Some(g)) => g,
        Ok(None) => return waiting(format!("the configured gate does not exist as of checkpoint {at}")),
        Err(w) => return waiting(w),
    };
    let entry = match settle("mint record", mirror.mint_entry(&id.cfg, &gate.gate.mints_table, at, &op.mint_id).await) {
        Ok(e) => e,
        Err(w) => return waiting(w),
    };
    use crate::reads::ExecStatus;
    match (receipt, entry) {
        (Some(r), Some(rec)) if matches!(r.status, ExecStatus::Success) => {
            if record_matches(&rec, &op) {
                if let Err(e) = finish_delivered(op_id, generation, Some(att.tx_digest), r.checkpoint, rec.sequence, DeliveryVia::Receipt) {
                    state::halt_mint_lane(format!("delivery of operation {op_id} could not be applied: {e}"));
                }
            } else {
                state::halt_mint_lane(format!("operation {op_id}: the receipt succeeded but the consumed mint record names another recipient or amount"));
            }
        }
        (Some(r), None) if matches!(r.status, ExecStatus::Success) => {
            state::halt_mint_lane(format!("operation {op_id}: the transaction succeeded at checkpoint {} but the mint id is not consumed", r.checkpoint));
        }
        (Some(r), None) => {
            // Executed and failed, and the mint id is unconsumed as of that very checkpoint: this attempt
            // is dead and cannot have consumed the id.
            let error = match &r.status {
                ExecStatus::Failure { abort_code, message } => format!("{}{}", abort_code.as_ref().map(|c| format!("abort {c}: ")).unwrap_or_default(), message),
                ExecStatus::Success => String::new(),
            };
            close_attempt(op_id, generation, &att.tx_digest, AttemptDisposition::FailedFinal { checkpoint: r.checkpoint, error }, Some(r.checkpoint));
        }
        (Some(_), Some(_)) => {
            state::halt_mint_lane(format!("operation {op_id}: the mint id is consumed although our transaction failed"));
        }
        (None, Some(rec)) => {
            if record_matches(&rec, &op) {
                if let Err(e) = finish_delivered(op_id, generation, None, at, rec.sequence, DeliveryVia::ConsumedRecord) {
                    state::halt_mint_lane(format!("delivery of operation {op_id} could not be applied: {e}"));
                }
            } else {
                state::halt_mint_lane(format!("operation {op_id}: the mint id is consumed with another recipient or amount (a foreign mint)"));
            }
        }
        (None, None) => unexplained_or_waiting(mirror, op_id, &att, at).await,
    }
}

/// No receipt and no consumed record. Compare the owned inputs with the attempt's references: unchanged
/// means the attempt may simply not have run (or be lost): wait, never replace. Changed with no
/// explanation, past the grace period, halts the lane.
async fn unexplained_or_waiting<T: SuiTransport>(mirror: &Mirror<'_, T>, op_id: u64, att: &MintAttempt, head: u64) {
    let waiting = |why: String| {
        note_wait(why.clone());
        set_hold(op_id, Some(why));
    };
    let cp = match settle("checkpoint", mirror.checkpoint_gas(head).await) {
        Ok(Some(c)) => c,
        Ok(None) => return waiting(format!("the providers do not yet serve checkpoint {head}")),
        Err(w) => return waiting(w),
    };
    let mut changed: Vec<(&str, StoredRef)> = vec![];
    for (name, reference) in [("mint cap", att.mint_cap), ("gas coin", att.gas_coin)] {
        let now_obj = match settle(name, mirror.owned_object(&reference.id, head).await) {
            Ok(Some(o)) => o,
            Ok(None) => return waiting(format!("the {name} is not served as of checkpoint {head}")),
            Err(w) => return waiting(w),
        };
        if now_obj.version != reference.version || now_obj.digest != reference.digest {
            changed.push((name, reference));
        }
    }
    let now = env::now_ns();
    if changed.is_empty() {
        let mut note = String::from("no receipt and no consumed record; the attempt's inputs are unchanged, so it may not have run");
        if cp.epoch > att.epoch {
            note.push_str(&format!("; its epoch {} has passed (current {}), which does not prove it cannot execute", att.epoch, cp.epoch));
        }
        // Clear any earlier suspicion: the inputs are as the attempt left them.
        if let Some(mut op) = state::get_mint_op(op_id) {
            if let Some(a) = op.attempts.last_mut() {
                a.versions_changed_since_ns = None;
                state::put_mint_op(&op);
            }
        }
        return waiting(note);
    }
    // Confirm the references we stored were genuine before suspecting anyone: the old version must
    // exist with the stored digest (an exact-version read, parsed before the tally).
    for (name, reference) in &changed {
        match settle(name, mirror.object_at_version(&reference.id, reference.version).await) {
            Ok(Some(o)) if o.digest == reference.digest => {}
            Ok(Some(_)) => return waiting(format!("the {name} reference stored for this attempt does not match the chain; not judged")),
            Ok(None) => return waiting(format!("the old {name} version is not served; not judged")),
            Err(w) => return waiting(w),
        }
    }
    let names: Vec<&str> = changed.iter().map(|(n, _)| *n).collect();
    let since = {
        let Some(mut op) = state::get_mint_op(op_id) else { return };
        let Some(a) = op.attempts.last_mut() else { return };
        let since = *a.versions_changed_since_ns.get_or_insert(now);
        state::put_mint_op(&op);
        since
    };
    if now.saturating_sub(since) >= FOREIGN_GRACE_NS {
        state::halt_mint_lane(format!(
            "operation {op_id}: the {} moved with no receipt for our transaction and no consumed mint record: an unexplained consumer. Nothing is replaced, refunded or released.",
            names.join(" and ")
        ));
    } else {
        waiting(format!("the {} moved; waiting for the receipt before judging (grace period)", names.join(" and ")));
    }
}

// ---------------------------------------------------------------------------
// Driving
// ---------------------------------------------------------------------------

/// Epoch check for an attempt that is stuck unsigned: if its epoch has passed and no complete
/// signature may exist, nobody can ever execute it, so it is closed as never signed.
async fn check_unsigned_expiry<T: SuiTransport>(mirror: &Mirror<'_, T>, op_id: u64) {
    let Some(op) = state::get_mint_op(op_id) else { return };
    let Some(a) = op.current() else { return };
    if !a.is_open() || a.may_be_complete() {
        return;
    }
    let (generation, digest, epoch) = (a.generation, a.tx_digest, a.epoch);
    let head = match head_checkpoint(mirror).await {
        Ok(h) => h,
        Err(w) => return note_wait(w),
    };
    let cp = match settle("checkpoint", mirror.checkpoint_gas(head).await) {
        Ok(Some(c)) => c,
        Ok(None) => return note_wait(format!("the providers do not yet serve checkpoint {head}")),
        Err(w) => return note_wait(w),
    };
    if cp.epoch > epoch {
        // Re-read: the attempt may have gained a signature while we asked.
        let Some(op) = state::get_mint_op(op_id) else { return };
        if op.current().is_some_and(|a| a.generation == generation && a.tx_digest == digest && !a.may_be_complete()) {
            close_attempt(
                op_id,
                generation,
                &digest,
                AttemptDisposition::NeverSigned { reason: format!("its epoch {epoch} passed (current {}) while a role's signature was never produced", cp.epoch), at_ns: env::now_ns() },
                None,
            );
        }
    }
}

async fn advance_after_failure<T: SuiTransport>(mirror: &Mirror<'_, T>, id: &LaneIdentity, op_id: u64) {
    let Some(op) = state::get_mint_op(op_id) else { return };
    if env::now_ns() < state::read_runtime(|c| c.gather_retry_at_ns) {
        return;
    }
    let max = state::read_runtime(|c| c.config.lane.max_attempts);
    if op.attempts.len() as u32 >= max {
        fail_op(op_id, "attempts exhausted: every attempt is a finalized failure or proven never signed");
        return;
    }
    match gather(mirror, id, &op.mint_id).await {
        Ok(f) => match append_attempt(op_id, &f, id) {
            Ok(()) => clear_wait(),
            Err(Refused::Hold(why)) | Err(Refused::Stop(why)) => {
                note_wait(why.clone());
                set_hold(op_id, Some(why));
            }
        },
        Err(Gather::Wait(w)) => {
            back_off();
            note_wait(w.clone());
            set_hold(op_id, Some(w));
        }
        Err(Gather::Consumed(_)) => state::halt_mint_lane(format!("operation {op_id}: the mint id is consumed after the previous attempt failed")),
        Err(Gather::Mismatch(m)) => state::halt_mint_lane(format!("operation {op_id}: {m}")),
    }
}

async fn drive_op<T: SuiTransport, S: RoleSigner>(mirror: &Mirror<'_, T>, signer: &S, id: &LaneIdentity, op_id: u64) {
    let Some(op) = state::get_mint_op(op_id) else {
        state::halt_mint_lane(format!("the active mint operation {op_id} does not exist"));
        return;
    };
    if !matches!(op.state, MintOpState::Active) {
        state::mutate_runtime(|c| {
            if c.active_mint_op == Some(op_id) {
                c.active_mint_op = None;
            }
        });
        return;
    }
    let Some(att) = op.current().cloned() else {
        state::halt_mint_lane(format!("mint operation {op_id} has no attempt"));
        return;
    };
    match att.disposition {
        AttemptDisposition::FailedFinal { .. } | AttemptDisposition::NeverSigned { .. } => {
            return advance_after_failure(mirror, id, op_id).await;
        }
        AttemptDisposition::LandedFinal { .. } => {
            // Delivery applies in the same step that records it; an operation that still shows a landed
            // attempt while active is inconsistent.
            state::halt_mint_lane(format!("mint operation {op_id} shows a landed attempt but is still active"));
            return;
        }
        AttemptDisposition::Open => {}
    }
    if att.treasury_sig.is_none() || att.gas_sig.is_none() {
        sign_attempt(signer, op_id, id).await;
        let complete = state::get_mint_op(op_id).and_then(|o| o.current().map(|a| a.treasury_sig.is_some() && a.gas_sig.is_some())).unwrap_or(false);
        if !complete {
            let due = state::get_mint_op(op_id).and_then(|o| o.current().map(|a| check_due(a, env::now_ns()))).unwrap_or(false);
            if due {
                mark_checked(op_id);
                check_unsigned_expiry(mirror, op_id).await;
            }
            return;
        }
    }
    if !maybe_send(mirror, op_id, id).await {
        return;
    }
    let due = state::get_mint_op(op_id).and_then(|o| o.current().map(|a| check_due(a, env::now_ns()))).unwrap_or(false);
    if due {
        mark_checked(op_id);
        reconcile(mirror, id, op_id).await;
    }
}

/// The next escrowed deposit that has a proven mint id and no operation.
fn next_candidate() -> Option<u64> {
    state::next_deposits(state::Queue::MintSubmits, 1, |d| {
        matches!(d.state, DepositState::Escrowed) && d.mint_op.is_none() && d.mint_id.is_some() && prior_ops_provably_dead(d).is_ok()
    })
    .first()
    .map(|d| d.id)
}

async fn start_next<T: SuiTransport>(mirror: &Mirror<'_, T>, id: &LaneIdentity) {
    let Some(deposit_id) = next_candidate() else {
        clear_wait();
        return;
    };
    if env::now_ns() < state::read_runtime(|c| c.gather_retry_at_ns) {
        return;
    }
    let Some(d) = state::get_deposit(deposit_id) else { return };
    let ask = match deposit_ask(&d) {
        Ok(a) => a,
        Err(why) => {
            // A deposit whose id does not match its records is not minted; hold it for an operator.
            deposit::on_mint_preflight_hold(deposit_id, format!("not minted: {why}"));
            return;
        }
    };
    match gather(mirror, id, &ask.mint_id).await {
        Ok(f) => match create_op(deposit_id, &f, id) {
            Ok(_) => {}
            Err(Refused::Hold(why)) => deposit::on_mint_preflight_hold(deposit_id, why),
            Err(Refused::Stop(why)) => note_wait(why),
        },
        Err(Gather::Wait(w)) => {
            back_off();
            note_wait(w)
        }
        Err(Gather::Consumed(rec)) => state::halt_mint_lane(format!(
            "deposit {deposit_id}: its mint id is already consumed on chain (recipient {}, amount {}) with no operation of ours",
            crate::sui::format_address(&rec.recipient),
            rec.amount
        )),
        Err(Gather::Mismatch(m)) => state::halt_mint_lane(format!("deposit {deposit_id}: {m}")),
    }
}

/// One lane step: drive the active operation, or start the next. At most one operation is in flight.
pub async fn mint_pass<T: SuiTransport, S: RoleSigner>(transport: &T, signer: &S) {
    if !blocked_by().is_empty() {
        return;
    }
    let Ok(id) = lane_identity() else { return };
    let Ok(mirror) = Mirror::from_state(transport) else { return };
    match state::read_runtime(|c| c.active_mint_op) {
        Some(op_id) => drive_op(&mirror, signer, &id, op_id).await,
        None => start_next(&mirror, &id).await,
    }
}

// ---------------------------------------------------------------------------
// Admin recovery
// ---------------------------------------------------------------------------

/// Abandon an operation whose current attempt is PROVEN never signed (no role's signature exists or
/// may exist). Anything that may be a complete signed transaction is refused: it needs a receipt or a
/// consumed record, which no human statement replaces.
pub fn abandon_never_signed(op_id: u64, note: &str) -> Result<(), String> {
    let note = note.trim();
    if note.is_empty() || note.len() > 512 {
        return Err("a note of 1 to 512 bytes is required".into());
    }
    let op = state::get_mint_op(op_id).ok_or("no such mint operation")?;
    if !matches!(op.state, MintOpState::Active) {
        return Err("the operation is not active".into());
    }
    let a = op.current().ok_or("the operation has no attempt")?;
    if !a.is_open() {
        return Err("the current attempt is already closed".into());
    }
    if a.may_be_complete() {
        return Err("a complete signed transaction may exist (a signature was produced or its reply was lost); only a finalized receipt or a consumed mint record can settle it".into());
    }
    let (generation, digest) = (a.generation, a.tx_digest);
    if !close_attempt(op_id, generation, &digest, AttemptDisposition::NeverSigned { reason: format!("abandoned by an admin: {note}"), at_ns: env::now_ns() }, None) {
        return Err("the attempt changed; retry".into());
    }
    fail_op(op_id, &format!("abandoned while never signed: {note}"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lane_tests::lane;
    use crate::saga_tests::{deposit_of, escrowed, E8};
    use crate::testkit::block_on;

    #[test]
    fn a_signature_is_exactly_64_bytes_in_the_wire_form() {
        let key = [7u8; 32];
        let sig = [1u8; 64];
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        let wire = STANDARD.decode(tx::wire_signature_base64(&sig, &key)).unwrap();
        assert_eq!(wire.len(), tx::WIRE_SIGNATURE_LEN);
        assert_eq!(wire[0], crate::sui::ED25519_FLAG);
    }

    fn facts_for(l: &crate::lane_tests::Lane, id: u64) -> (Facts, LaneIdentity) {
        let identity = lane_identity().unwrap();
        let net = l.net();
        let mirror = Mirror::from_state(&net).unwrap();
        let ask = deposit_ask(&deposit_of(id)).unwrap();
        match block_on(gather(&mirror, &identity, &ask.mint_id)) {
            Ok(f) => (f, identity),
            Err(_) => panic!("the facts could not be gathered"),
        }
    }

    #[test]
    fn the_synchronous_admission_step_rechecks_what_changed_while_the_facts_were_being_read() {
        let l = lane();
        let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
        let (facts, identity) = facts_for(&l, id);
        let stop = |r: Result<u64, Refused>| match r {
            Err(Refused::Stop(m)) => m,
            Err(Refused::Hold(m)) => panic!("a hold, not a stop: {m}"),
            Ok(_) => panic!("an operation was created"),
        };
        // The deposit was held while the providers were asked.
        deposit::on_mint_preflight_hold(id, "held meanwhile".into());
        assert!(stop(create_op(id, &facts, &identity)).contains("not waiting"));
        let mut d = deposit_of(id);
        d.state = DepositState::Escrowed;
        d.hold_retry_at_ns = None;
        state::put_deposit(&d);
        // Mints were paused, or the lane halted, or the bridge halted.
        state::mutate_runtime(|r| r.config.pauses.mints = true);
        assert!(stop(create_op(id, &facts, &identity)).contains("paused"));
        state::mutate_runtime(|r| r.config.pauses.mints = false);
        state::halt_mint_lane("test");
        assert!(stop(create_op(id, &facts, &identity)).contains("halted"));
        state::mutate_runtime(|r| r.mint_lane_halted = None);
        // The deposit's amount changed: its mint id no longer follows from its records.
        let mut d = deposit_of(id);
        d.mint_amount_e8s -= 1;
        state::put_deposit(&d);
        assert!(stop(create_op(id, &facts, &identity)).contains("mint id"));
        d.mint_amount_e8s += 1;
        state::put_deposit(&d);
        assert_eq!((state::mint_op_count(), state::read_runtime(|r| r.active_mint_op)), (0, None), "every refusal left nothing behind");
        // Admitted: the op exists and the deposit is linked in the same step.
        let op_id = match create_op(id, &facts, &identity) {
            Ok(n) => n,
            Err(_) => panic!("admission failed"),
        };
        let d = deposit_of(id);
        assert_eq!((d.mint_op, d.state.clone()), (Some(op_id), DepositState::MintInFlight));
        assert_eq!(state::read_runtime(|r| r.active_mint_op), Some(op_id));
        // A second operation cannot start while this one is in flight, even from fresh facts.
        let second = escrowed(&l.fx, 0x22, 3 * E8, 1);
        let (facts2, _) = facts_for(&l, second);
        assert!(stop(create_op(second, &facts2, &identity)).contains("in flight"));
        assert_eq!(deposit_of(second).state, DepositState::Escrowed);
    }

    #[test]
    fn a_new_attempt_is_appended_only_after_the_previous_one_is_proven_dead_and_within_the_limit() {
        let l = lane();
        let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
        let (facts, identity) = facts_for(&l, id);
        let op_id = match create_op(id, &facts, &identity) {
            Ok(n) => n,
            Err(_) => panic!("admission failed"),
        };
        let refused = |r: Result<(), Refused>| match r {
            Err(Refused::Stop(m)) | Err(Refused::Hold(m)) => m,
            Ok(()) => panic!("an attempt was appended"),
        };
        assert!(refused(append_attempt(op_id, &facts, &identity)).contains("not proven dead"), "an open attempt is never replaced");
        let a = state::get_mint_op(op_id).unwrap().attempts[0].clone();
        assert!(close_attempt(op_id, 1, &a.tx_digest, AttemptDisposition::FailedFinal { checkpoint: 1, error: "x".into() }, Some(1)));
        // A close for a stale attempt (wrong digest or generation) changes nothing.
        assert!(!close_attempt(op_id, 1, &[7; 32], AttemptDisposition::NeverSigned { reason: "r".into(), at_ns: 1 }, None));
        assert!(!close_attempt(op_id, 2, &a.tx_digest, AttemptDisposition::NeverSigned { reason: "r".into(), at_ns: 1 }, None));
        assert!(append_attempt(op_id, &facts, &identity).is_ok());
        let op = state::get_mint_op(op_id).unwrap();
        assert_eq!((op.attempts.len(), op.attempts[1].generation), (2, 2));
        // The second attempt is open again; the limit (3 by default) stops a fourth.
        let b = op.attempts[1].clone();
        assert!(close_attempt(op_id, 2, &b.tx_digest, AttemptDisposition::NeverSigned { reason: "r".into(), at_ns: 1 }, None));
        assert!(append_attempt(op_id, &facts, &identity).is_ok());
        let c = state::get_mint_op(op_id).unwrap().attempts[2].clone();
        assert!(close_attempt(op_id, 3, &c.tx_digest, AttemptDisposition::NeverSigned { reason: "r".into(), at_ns: 1 }, None));
        assert!(refused(append_attempt(op_id, &facts, &identity)).contains("used its attempts"));
    }
}
