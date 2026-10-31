//! The return observer (53c): Sui `bridge_out` burns become ICP releases, and only through the Gate's
//! durable record.
//!
//! # What may pay
//!
//! Only [`release::open_release`] opens a release, and it takes a [`VerifiedReturn`]. This module builds
//! one only from the Gate's own `returns[transfer_id]` record read through the provider quorum at a
//! finalized checkpoint, after re-deriving `transfer_id` from the network, the Gate, the sender and the
//! nonce and checking every immutable field. **Events and hints only DISCOVER**: they name a
//! `(sender, nonce, transaction)` candidate, and the claim an event carries (amount, sequence,
//! destination) must equal the record or the candidate is dropped with a recorded problem. A lookalike
//! event of another package, module or struct is never even a candidate; an event with no durable record
//! never pays.
//!
//! # Completeness
//!
//! Events of every checkpoint after the observed deployment are listed in bounded windows (a window whose
//! page is full is bisected until it fits; one checkpoint with more than a page is a hold, since cursors
//! are opaque and never compared across providers). A window is committed only when the Gate's own return
//! counter at its end is accounted for by returns already verified or held as candidates: an event index
//! that lags, or a pruned event, leaves a visible gap and commits nothing. Releases are opened STRICTLY in
//! Gate sequence order, so a gap holds every later return rather than letting it skip ahead.
//!
//! # Cost
//!
//! Idle, the observer wakes every five minutes and reads the head and the Gate; with work pending it works
//! every pass. A pass makes a bounded number of quorum reads (a scan budget, a hint limit, a verify limit).

use crate::bind::{exclusion_text, lane_identity, LaneIdentity};
use crate::env;
use crate::flows::release::{self, OpenError, OpenOutcome, VerifiedReturn};
use crate::gate::{ConfiguredGate, GateObject, ReturnRecord};
use crate::graphql::CheckpointFact;
use crate::icp::IcpLedger;
use crate::ids::{self, Hash32};
use crate::mirror::{settle, Mirror, SuiTransport};
use crate::monitor;
use crate::reads::{interpret_bridge_out_event, EventFact, ExecStatus};
use crate::state::{
    self, AuditCtx, Candidate, CandidateSource, ChainObservation, Claim, MonitorReport, ObserverProblem, OwnBooks, ReadyReturn, MAX_CANDIDATES, MAX_PROBLEMS,
};
use std::collections::BTreeSet;

/// Wake interval when nothing is pending.
pub const IDLE_INTERVAL_NS: u64 = 300 * 1_000_000_000;
/// Wake interval when there is work, a gap or a wait.
pub const BUSY_INTERVAL_NS: u64 = 30 * 1_000_000_000;
/// After a failed read the observer waits this long.
pub const WAIT_INTERVAL_NS: u64 = 60 * 1_000_000_000;
/// Checkpoints per event window.
pub const MAX_WINDOW: u64 = 20_000;
/// Event queries one pass may make.
pub const SCAN_QUERY_BUDGET: u32 = 10;
pub const MAX_HINTS_PER_PASS: usize = 3;
pub const MAX_VERIFY_PER_PASS: usize = 8;
/// A candidate whose transaction or record cannot be read this many passes in a row is dropped with a problem.
pub const MAX_TRIES: u32 = 4;

fn obs<R>(f: impl FnOnce(&mut state::ObserverState) -> R) -> R {
    state::mutate_runtime(|c| f(&mut c.observer))
}

fn record_problem(note: impl Into<String>, tx: Option<Hash32>) {
    let note = note.into();
    // The same problem, pass after pass, is recorded once.
    if state::read_runtime(|c| c.observer.problems.last().is_some_and(|p| p.note == note && p.tx_digest == tx)) {
        return;
    }
    let at = env::now_ns();
    obs(|o| {
        o.problems.push(ObserverProblem { at_ns: at, note: note.clone(), tx_digest: tx });
        if o.problems.len() > MAX_PROBLEMS {
            o.problems.remove(0);
        }
    });
    state::append_audit(None, AuditCtx::note(format!("return observer: {note}")));
}

fn set_wait(why: Option<String>) {
    obs(|o| o.last_wait = why);
}

// ---------------------------------------------------------------------------
// Verification (pure)
// ---------------------------------------------------------------------------

/// Verify the Gate's durable record for one candidate and build the return that may be opened. Every check
/// is on the record or on values this canister derived itself; the candidate's claim (from an event) only
/// has to AGREE.
///
/// * the transfer id is recomputed from the network, the Gate, the record's sender and the record's nonce
///   and must equal the key the record was read under and the candidate's;
/// * the record's sender and nonce are the candidate's;
/// * the sequence is within the Gate's own counter at the checkpoint, and the amount within what it burned;
/// * the claim (transfer id, amount, sequence, destination) equals the record;
/// * the destination obeys the Gate's own bounds (re-applied by `decode_return_record`).
pub fn verify_return(
    cfg: &ConfiguredGate,
    chain: &Hash32,
    gate: &GateObject,
    checkpoint: u64,
    cand: &Candidate,
    key: &Hash32,
    rec: &ReturnRecord,
) -> Result<ReadyReturn, String> {
    let derived = ids::transfer_id(chain, &cfg.gate, &rec.sender, rec.client_nonce);
    if &derived != key {
        return Err("the record's sender and nonce do not produce the transfer id it was stored under".into());
    }
    if rec.sender != cand.sender || rec.client_nonce != cand.nonce {
        return Err("the record belongs to another sender or nonce than the candidate".into());
    }
    if cand.claim.transfer_id != derived {
        return Err("the event's transfer id is not the one its sender and nonce produce".into());
    }
    if rec.sequence == 0 || rec.sequence > gate.bridge_out_count {
        return Err(format!("the record's sequence {} is outside the Gate's {} returns at checkpoint {checkpoint}", rec.sequence, gate.bridge_out_count));
    }
    if rec.amount == 0 || rec.amount as u128 > gate.burned_total {
        return Err("the record's amount is zero or more than the Gate has burned".into());
    }
    if cand.claim.amount_e8s != rec.amount || cand.claim.sequence != rec.sequence || cand.claim.icp_principal != rec.icp_principal || cand.claim.icp_subaccount != rec.icp_subaccount {
        return Err("the event and the durable record disagree about the amount, sequence or destination".into());
    }
    Ok(ReadyReturn {
        transfer_id: derived,
        gate_sequence: rec.sequence,
        sui_sender: rec.sender,
        client_nonce: rec.client_nonce,
        amount_e8s: rec.amount,
        icp_principal: rec.icp_principal.clone(),
        icp_subaccount: rec.icp_subaccount.clone(),
        tx_digest: cand.tx_digest,
        checkpoint,
    })
}

/// The first Gate sequence in `(verified_through, count]` that no candidate accounts for, if any.
pub fn first_unaccounted(verified_through: u64, candidates: &[Candidate], count: u64) -> Option<u64> {
    let known: BTreeSet<u64> = candidates.iter().map(|c| c.claim.sequence).collect();
    ((verified_through + 1)..=count).find(|s| !known.contains(s))
}

// ---------------------------------------------------------------------------
// Candidates
// ---------------------------------------------------------------------------

enum Added {
    New,
    Known,
}

fn add_candidate(id: &LaneIdentity, chain: &Hash32, sender: Hash32, nonce: u64, tx_digest: Hash32, source: CandidateSource, claim: Claim) -> Result<Added, String> {
    let derived = ids::transfer_id(chain, &id.cfg.gate, &sender, nonce);
    if claim.transfer_id != derived {
        return Err(format!("an event's transfer id is not the one its sender and nonce {nonce} produce (a lookalike or a corrupt event)"));
    }
    if state::get_release(&derived).is_some() {
        return Ok(Added::Known); // already opened: deduplicated
    }
    state::mutate_runtime(|c| {
        let o = &mut c.observer;
        if let Some(existing) = o.candidates.iter().find(|x| x.sender == sender && x.nonce == nonce) {
            return if existing.claim == claim { Ok(Added::Known) } else { Err("two events claim different contents for the same sender and nonce".to_string()) };
        }
        if o.candidates.len() >= MAX_CANDIDATES {
            return Err("the candidate queue is full".to_string());
        }
        o.candidates.push(Candidate { sender, nonce, tx_digest, source, claim, found_at_ns: env::now_ns(), tries: 0, ready: None, waiting: None });
        Ok(Added::New)
    })
}

fn claim_of(ev: &crate::gate::BridgeOutEvent) -> Result<Claim, String> {
    let transfer_id: Hash32 = ev.transfer_id.as_slice().try_into().map_err(|_| "a transfer id that is not 32 bytes".to_string())?;
    Ok(Claim { transfer_id, amount_e8s: ev.amount, sequence: ev.sequence, icp_principal: ev.icp_principal.clone(), icp_subaccount: ev.icp_subaccount.clone() })
}

/// Turn the events of a window (or a hinted transaction) into candidates. Any event that cannot be a
/// candidate stops the caller: an unexplained event is a hold, not something to skip.
fn candidates_from_events(id: &LaneIdentity, chain: &Hash32, events: &[EventFact], source: CandidateSource) -> Result<usize, String> {
    let mut added = 0;
    for e in events {
        let ev = interpret_bridge_out_event(e, &id.cfg).map_err(|x| format!("a BridgeOut event cannot be read: {}", exclusion_text(&x)))?;
        let claim = claim_of(&ev)?;
        if matches!(add_candidate(id, chain, ev.sender, ev.client_nonce, e.tx_digest, source, claim)?, Added::New) {
            added += 1;
        }
    }
    Ok(added)
}

// ---------------------------------------------------------------------------
// Opening, in Gate sequence order (synchronous)
// ---------------------------------------------------------------------------

/// Open every verified return whose turn has come. A return waits (nothing is skipped) while the mint it
/// returns is not yet applied to the books or the ledger facts are unknown.
pub fn open_ready_returns() {
    loop {
        let next = state::read_runtime(|c| c.observer.verified_through) + 1;
        let found = state::read_runtime(|c| c.observer.candidates.iter().position(|x| x.ready.as_ref().is_some_and(|r| r.gate_sequence == next)));
        let Some(idx) = found else {
            // No candidate for the next sequence. If a release for it already exists (opened earlier),
            // the cursor simply catches up.
            if state::release_by_sequence(next).is_some() {
                obs(|o| o.verified_through = next);
                continue;
            }
            return;
        };
        let cand = state::read_runtime(|c| c.observer.candidates[idx].clone());
        let r = cand.ready.clone().expect("position() found a ready candidate");
        let v = VerifiedReturn {
            transfer_id: r.transfer_id,
            gate_sequence: r.gate_sequence,
            sui_sender: r.sui_sender,
            client_nonce: r.client_nonce,
            amount_e8s: r.amount_e8s,
            icp_principal: r.icp_principal.clone(),
            icp_subaccount: r.icp_subaccount.clone(),
            tx_digest: r.tx_digest,
            checkpoint: r.checkpoint,
        };
        match release::open_release(&v) {
            Ok(outcome) => {
                let opened = matches!(outcome, OpenOutcome::Opened(_));
                obs(|o| {
                    o.verified_through = next;
                    o.candidates.retain(|x| !(x.sender == cand.sender && x.nonce == cand.nonce));
                });
                if opened {
                    state::mutate_runtime(|c| c.returned_e8s += r.amount_e8s as u128);
                }
            }
            Err(OpenError::NotYetOutstanding { outstanding, amount }) => {
                let why = format!("waiting: the books hold {outstanding} e8s outstanding, the return burned {amount}; the mint it returns is not applied yet");
                obs(|o| o.candidates[idx].waiting = Some(why));
                return;
            }
            Err(OpenError::LedgerFactsUnknown) => {
                obs(|o| o.candidates[idx].waiting = Some("waiting: the ledger fee or minting account is not known yet".into()));
                return;
            }
            Err(OpenError::Malformed(m)) => {
                // The Gate cannot produce this; it is a failed verification, not a return.
                obs(|o| o.candidates.retain(|x| !(x.sender == cand.sender && x.nonce == cand.nonce)));
                record_problem(format!("a verified record failed the release bounds: {m}"), Some(cand.tx_digest));
                return;
            }
            // `open_release` already halted the bridge for a conflict or an unfunded release.
            Err(OpenError::Conflict(_)) | Err(OpenError::NotFunded { .. }) | Err(OpenError::Accounting(_)) => return,
        }
    }
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

enum ScanError {
    Budget,
    Wait(String),
    Hold(String),
}

/// All `BridgeOut` events with checkpoint in `(a, b]`, bisecting until every query fits a page.
async fn scan_window<T: SuiTransport>(mirror: &Mirror<'_, T>, cfg: &ConfiguredGate, a: u64, b: u64, budget: &mut u32) -> Result<Vec<EventFact>, ScanError> {
    let mut stack = vec![(a, b)];
    let mut out = vec![];
    while let Some((lo, hi)) = stack.pop() {
        if *budget == 0 {
            return Err(ScanError::Budget);
        }
        *budget -= 1;
        match settle("events", mirror.events(cfg, "BridgeOut", lo, hi + 1).await) {
            Err(w) => return Err(ScanError::Wait(w)),
            Ok(None) => return Err(ScanError::Hold(format!("the providers do not serve events for checkpoints {}..={hi}", lo + 1))),
            Ok(Some(page)) if page.has_next => {
                if hi - lo <= 1 {
                    return Err(ScanError::Hold(format!("checkpoint {hi} has more BridgeOut events than one page: cursors are not compared across providers, so it cannot be listed")));
                }
                let mid = lo + (hi - lo) / 2;
                stack.push((lo, mid));
                stack.push((mid, hi));
            }
            Ok(Some(page)) => out.extend(page.events),
        }
    }
    Ok(out)
}

/// Hints name transactions; their events name candidates. A hint is consumed once its transaction is final
/// and read, whatever it turns out to contain.
async fn process_hints<T: SuiTransport>(mirror: &Mirror<'_, T>, id: &LaneIdentity, chain: &Hash32, head: u64) {
    let hints = release::live_hints(env::now_ns());
    for h in hints.into_iter().take(MAX_HINTS_PER_PASS) {
        let t = match settle("hinted transaction", mirror.tx_events(&h.digest).await) {
            Ok(Some(t)) => t,
            Ok(None) => continue, // not indexed or not final yet: kept until it expires
            Err(w) => {
                set_wait(Some(w));
                continue;
            }
        };
        if t.checkpoint > head {
            continue; // final, but beyond the checkpoint this pass works at
        }
        state::mutate_runtime(|c| c.hints.retain(|x| x.digest != h.digest));
        if !t.success {
            record_problem("a hinted transaction failed; it burned nothing", Some(h.digest));
            continue;
        }
        if t.page.has_next {
            record_problem("a hinted transaction emitted more events than one page; it cannot be read", Some(h.digest));
            continue;
        }
        let ours: Vec<EventFact> = t.page.events.into_iter().filter(|e| id.cfg.type_repr_matches(&e.type_repr, "BridgeOut")).collect();
        if ours.is_empty() {
            record_problem("a hinted transaction has no BridgeOut event of the configured Gate package", Some(h.digest));
            continue;
        }
        if let Err(why) = candidates_from_events(id, chain, &ours, CandidateSource::Hint) {
            record_problem(why, Some(h.digest));
        }
    }
}

/// Fetch and verify the durable record of every candidate not yet verified (bounded per pass).
async fn verify_candidates<T: SuiTransport>(mirror: &Mirror<'_, T>, id: &LaneIdentity, chain: &Hash32, gate: &GateObject, head: u64) {
    let pending: Vec<Candidate> = state::read_runtime(|c| c.observer.candidates.iter().filter(|x| x.ready.is_none()).take(MAX_VERIFY_PER_PASS).cloned().collect());
    for cand in pending {
        let tid = ids::transfer_id(chain, &id.cfg.gate, &cand.sender, cand.nonce);
        let drop_it = |why: String| {
            obs(|o| o.candidates.retain(|x| !(x.sender == cand.sender && x.nonce == cand.nonce)));
            record_problem(why, Some(cand.tx_digest));
        };
        let retry = |why: String| {
            let dropped = state::mutate_runtime(|c| {
                if let Some(x) = c.observer.candidates.iter_mut().find(|x| x.sender == cand.sender && x.nonce == cand.nonce) {
                    x.tries += 1;
                    x.waiting = Some(why.clone());
                    x.tries >= MAX_TRIES
                } else {
                    false
                }
            });
            if dropped {
                drop_it(format!("unreadable after {MAX_TRIES} passes: {why}"));
            }
        };
        // The transaction: executed successfully at a finalized checkpoint we have reached.
        match settle("transaction", mirror.transaction(&cand.tx_digest).await) {
            Err(w) => {
                set_wait(Some(w));
                continue;
            }
            Ok(None) => {
                retry("the transaction is not found at a finalized checkpoint".into());
                continue;
            }
            Ok(Some(t)) if !matches!(t.status, ExecStatus::Success) => {
                drop_it("the transaction that emitted the event did not succeed".into());
                continue;
            }
            Ok(Some(t)) if t.checkpoint > head => {
                retry(format!("the transaction is at checkpoint {}, past the {head} this pass works at", t.checkpoint));
                continue;
            }
            Ok(Some(_)) => {}
        }
        // The durable record: the proof.
        match settle("return record", mirror.return_entry(&id.cfg, &gate.returns_table, head, &tid).await) {
            Err(w) => set_wait(Some(w)),
            Ok(None) => retry(format!("no durable record for transfer id {} at checkpoint {head}", hex::encode(tid))),
            Ok(Some(rec)) => match verify_return(&id.cfg, chain, gate, head, &cand, &tid, &rec) {
                Ok(ready) => obs(|o| {
                    if let Some(x) = o.candidates.iter_mut().find(|x| x.sender == cand.sender && x.nonce == cand.nonce) {
                        x.ready = Some(ready);
                        x.waiting = None;
                    }
                }),
                Err(why) => drop_it(format!("not verified: {why}")),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// The pass
// ---------------------------------------------------------------------------

fn monitor_run(head: u64, cp: &CheckpointFact, gate: &GateObject, escrow: Option<u128>) {
    let (buckets, o, minted_count, minted_e8s, returned_e8s, last_final, active_op) = state::read_runtime(|c| {
        (c.buckets, c.observer.clone(), c.minted_count, c.minted_e8s, c.returned_e8s, c.last_final_checkpoint, c.active_mint_op.is_some())
    });
    let caught_up = o.scanned_through >= head && o.verified_through == gate.bridge_out_count && o.candidates.is_empty();
    let books = OwnBooks {
        buckets,
        minted_count,
        minted_e8s,
        verified_through: o.verified_through,
        returned_e8s,
        inflight_mint_e8s: state::inflight_mint_e8s(),
        inflight_ops: u64::from(active_op),
    };
    let chain = ChainObservation {
        total_supply: gate.total_supply,
        supply_cap: gate.supply_cap,
        minted_total: gate.minted_total,
        burned_total: gate.burned_total,
        mint_count: gate.mint_count,
        bridge_out_count: gate.bridge_out_count,
        gate_version: gate.version,
    };
    let checks = monitor::evaluate(&monitor::Inputs {
        chain: &chain,
        checkpoint: head,
        escrow_balance: escrow,
        books: &books,
        last_final_checkpoint: last_final,
        caught_up,
        gap: o.gap.as_deref(),
        active_op,
    });
    let report = MonitorReport { at_ns: env::now_ns(), checkpoint: head, checkpoint_digest: cp.digest, epoch: cp.epoch, chain, escrow_balance: escrow, books, caught_up, checks };
    obs(|ob| ob.caught_up = caught_up);
    state::mutate_runtime(|c| c.monitor = Some(report.clone()));
    monitor::apply(&report);
}

/// One observer pass: open what is ready; if due, read the head and the Gate, scan, take hints, verify
/// candidates, open, and run the monitors. Reads only; the only effects are bookkeeping and releases opened
/// through [`release::open_release`].
pub async fn observer_pass<T: SuiTransport, L: IcpLedger>(transport: &T, ledger: &L) {
    // Releases that are ready need no outcall: a mint delivered since the last pass may have unblocked one.
    open_ready_returns();
    let now = env::now_ns();
    let (paused, due, started, observed) =
        state::read_runtime(|c| (c.config.pauses.observer, now >= c.observer.next_run_ns, c.observer.started, c.observed.clone()));
    let hinted = !release::live_hints(now).is_empty();
    if paused || observed.is_none() || (!due && !hinted) {
        return;
    }
    let Ok(id) = lane_identity() else { return };
    let Ok(mirror) = Mirror::from_state(transport) else { return };
    let chain = mirror.chain_identifier();
    let schedule = |busy: bool, waited: bool| {
        let step = if waited { WAIT_INTERVAL_NS } else if busy { BUSY_INTERVAL_NS } else { IDLE_INTERVAL_NS };
        obs(|o| o.next_run_ns = env::now_ns().saturating_add(step));
    };

    // The finalized checkpoint this pass works at.
    let head = match settle("head", mirror.head().await) {
        Ok(Some(h)) => h,
        Ok(None) => {
            set_wait(Some("the providers have no checkpoint yet".into()));
            return schedule(true, true);
        }
        Err(w) => {
            set_wait(Some(w));
            return schedule(true, true);
        }
    };
    if head < state::read_runtime(|c| c.observer.last_head) {
        set_wait(Some(format!("the providers' head {head} is behind the {} this observer already worked at", state::read_runtime(|c| c.observer.last_head))));
        return schedule(true, true);
    }
    let cp = match settle("checkpoint", mirror.checkpoint(head).await) {
        Ok(Some(c)) => c,
        Ok(None) => {
            set_wait(Some(format!("the providers do not yet serve checkpoint {head}")));
            return schedule(true, true);
        }
        Err(w) => {
            set_wait(Some(w));
            return schedule(true, true);
        }
    };
    let gate = match settle("gate", mirror.gate(&id.cfg, head).await) {
        Ok(Some(g)) => g.gate,
        Ok(None) => {
            record_problem(format!("the configured gate does not exist as of checkpoint {head}"), None);
            return schedule(true, true);
        }
        Err(w) => {
            set_wait(Some(w));
            return schedule(true, true);
        }
    };
    set_wait(None);
    if !started {
        let start = observed.as_ref().map(|o| o.checkpoint).unwrap_or(0);
        obs(|o| {
            o.started = true;
            o.scanned_through = start;
        });
    }
    obs(|o| o.last_head = head);

    // Nothing new on chain: the Gate's own counter says every return up to this checkpoint is already
    // verified, so no event needs listing and the scan cursor can move up to it. (This is what keeps an
    // idle observer to a head, a checkpoint and a Gate read.)
    let idle = state::read_runtime(|c| c.observer.verified_through == gate.bridge_out_count && c.observer.candidates.is_empty()) && release::live_hints(env::now_ns()).is_empty();
    if idle {
        obs(|o| {
            o.scanned_through = o.scanned_through.max(head);
            o.gap = None;
        });
    }

    // Discovery by event windows. A window that cannot be listed (unserved, or a checkpoint with more than a
    // page) is still committed when the Gate's own counter at its end is fully accounted for by returns
    // already verified or held as candidates (hints can supply them): the counter, not the listing, is what
    // proves nothing is missing.
    let mut budget = SCAN_QUERY_BUDGET;
    while state::read_runtime(|c| c.observer.scanned_through) < head && budget > 0 {
        let (a, w) = state::read_runtime(|c| (c.observer.scanned_through, if c.observer.window == 0 { MAX_WINDOW } else { c.observer.window }));
        let b = head.min(a + w);
        let listed = match scan_window(&mirror, &id.cfg, a, b, &mut budget).await {
            Ok(e) => Some(e),
            Err(ScanError::Budget) => {
                // The window needed more queries than a pass may make: look at a smaller one next time.
                obs(|o| o.window = (w / 4).max(1));
                break;
            }
            Err(ScanError::Wait(why)) => {
                set_wait(Some(why));
                break;
            }
            Err(ScanError::Hold(h)) => {
                record_problem(h, None);
                None
            }
        };
        if let Some(events) = &listed {
            if let Err(why) = candidates_from_events(&id, &chain, events, CandidateSource::Event) {
                record_problem(why, None);
                break;
            }
        }
        let at_end = if b == head {
            gate.clone()
        } else {
            match settle("gate", mirror.gate(&id.cfg, b).await) {
                Ok(Some(g)) => g.gate,
                Ok(None) => break,
                Err(why) => {
                    set_wait(Some(why));
                    break;
                }
            }
        };
        let (verified, cands) = state::read_runtime(|c| (c.observer.verified_through, c.observer.candidates.clone()));
        match first_unaccounted(verified, &cands, at_end.bridge_out_count) {
            None => obs(|o| {
                o.scanned_through = b;
                o.gap = None;
                if listed.is_some() {
                    o.window = (w * 2).min(MAX_WINDOW);
                }
            }),
            Some(missing) => {
                let gap = format!(
                    "the Gate counted {} returns at checkpoint {b} but no event or hint accounts for sequence {missing}; nothing after it is opened",
                    at_end.bridge_out_count
                );
                obs(|o| o.gap = Some(gap));
                break;
            }
        }
    }

    // Hints, then verification of everything found, then the opening (strictly in sequence).
    process_hints(&mirror, &id, &chain, head).await;
    verify_candidates(&mirror, &id, &chain, &gate, head).await;
    // A hint may have supplied what a gap lacked.
    let (verified, cands, scanned) = state::read_runtime(|c| (c.observer.verified_through, c.observer.candidates.clone(), c.observer.scanned_through));
    if scanned >= head && first_unaccounted(verified, &cands, gate.bridge_out_count).is_none() {
        obs(|o| o.gap = None);
    }
    open_ready_returns();

    // The monitors, from the independent observations just made and the books as they now stand.
    let escrow_account = state::read_runtime(|c| c.escrow_account(env::canister_id()));
    let escrow = ledger.balance_of(escrow_account).await.ok();
    monitor_run(head, &cp, &gate, escrow);

    let busy = state::read_runtime(|c| !c.observer.candidates.is_empty() || c.observer.gap.is_some() || !c.observer.caught_up) || !release::live_hints(env::now_ns()).is_empty();
    let waited = state::read_runtime(|c| c.observer.last_wait.is_some());
    schedule(busy, waited);
}
