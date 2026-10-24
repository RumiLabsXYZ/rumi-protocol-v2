//! Owned by card BB-06. Nonce lanes and the Base transaction lifecycle.
//!
//! Two lanes (minter, admin) each own one signing key and one nonce sequence. A
//! transaction has a fixed lane, nonce, purpose, call and gas limit, plus a growing list
//! of signed attempts. The rules this module enforces (design 7.6):
//!
//! * Every attempt (signed bytes plus the locally computed hash) is persisted with
//!   `state::put_tx` BEFORE it is broadcast. Broadcast replies are hints, never proof:
//!   an error leads to receipt checks and a rebroadcast of the SAME bytes, never to a
//!   fresh nonce or a fresh signature for a different transaction.
//! * A signature whose reply was lost leaves no attempt behind. The transaction is
//!   still `Unsigned`, so the next run signs the same fields at the same nonce.
//! * Receipts are looked up for EVERY attempt hash. A replacement only appears when
//!   no attempt has a receipt, the quorum `latest` nonce still equals the tx nonce and
//!   the newest attempt is older than `replace_after_secs` of wall-clock time.
//! * If the quorum `finalized` nonce moved past the tx nonce while no recorded attempt
//!   has a receipt, an unknown transaction consumed our nonce: the lane halts and the
//!   transaction is parked. Nothing is re-signed.
//! * Settlement needs finality: the included block must be at or below the quorum
//!   finalized head, the receipt must still be there, and the canonical block hash at
//!   that height must equal the receipt's block hash. A receipt that vanishes before
//!   that returns the tx to `Pending` and its newest attempt is rebroadcast.
//!
//! ## Structure
//!
//! The decisions are pure functions (`decide_pending`, `decide_finality`, `start_gate`,
//! `initial_fees`, `bumped_fees`, ...) that take the stored record plus observations
//! and return an action. The async driver (`run_lane_with`) only gathers observations
//! through [`LaneIo`], calls the decision function and applies the action with small
//! synchronous state writers. `LaneIo` is implemented over the real RPC and signer
//! modules by [`ChainIo`]; the unit tests implement it over an in-memory chain.
//!
//! ## Decision table for a transaction without a receipt (`decide_pending`)
//!
//! | Situation | Action |
//! | --- | --- |
//! | no attempt, cancel requested | sign a `Cancel` as the first attempt |
//! | no attempt, lane halted | wait |
//! | no attempt, mint paused / halted / unbound | wait (nothing is minted while paused) |
//! | no attempt | sign the `Original` attempt at the assigned nonce |
//! | exactly one attempt has a receipt | `Included` (any attempt index, not only the newest) |
//! | several attempts report a receipt | wait (inconsistent providers) |
//! | newest attempt younger than `replace_after_secs`, or a receipt lookup failed | rebroadcast if 30 s passed, else wait |
//! | older, nonces unknown | fetch `latest` and `finalized` transaction counts |
//! | `finalized` > tx nonce | HALT the lane and park the tx |
//! | `latest` > tx nonce | wait for the receipt (never replace) |
//! | `latest` < tx nonce | rebroadcast / wait (provider lag or reorg) |
//! | `latest` == tx nonce, lane halted | rebroadcast / wait |
//! | `latest` == tx nonce, fees can rise 12.5% under the caps | `Replacement` (or `Cancel` if requested) |
//! | `latest` == tx nonce, caps reached or 64 attempts | rebroadcast / wait |

use std::cell::RefCell;

use crate::env;
use crate::evm::abi::{self, ViewCall};
use crate::evm::rpc::{self, BlockRef, BlockTag, FeeEstimate, Receipt, RpcError, SimulationResult};
use crate::evm::signer;
use crate::evm::tx::{self, Eip1559Tx};
use crate::flows::deposit::{self, MintTxOutcome};
use crate::ids::{hex0x, LaneKind};
use crate::state::{
    self, AttemptKind, AuditCtx, DepositRecord, DepositState, EvmTxRecord, GasSettings, LaneState, TxAttempt,
    TxPurpose, TxStatus,
};
use crate::types::{BridgeError, BridgeResult, ContractAdminAction};

// ---------------------------------------------------------------------------
// Public API (signatures are fixed by the coordinator)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LaneOutcome {
    pub notes: Vec<String>,
    pub pending_work: bool,
}

/// One timer pass over a lane. Never panics; every problem becomes a note.
pub async fn run_lane(lane: LaneKind) -> LaneOutcome {
    run_lane_with(&ChainIo, lane).await
}

/// Queue a contract admin call on the admin lane. The nonce is assigned only when the
/// call becomes the lane's current transaction.
pub fn enqueue_admin_action(action: ContractAdminAction) -> BridgeResult<u64> {
    let (contract, gas_limit, queued) = state::read_core(|c| {
        (c.config.contract.as_ref().map(|b| b.address), c.config.gas.admin_gas_limit, c.admin_lane.queued_txs.len())
    });
    let Some(contract) = contract else {
        return Err(BridgeError::NotConfigured("the Base contract is not bound".into()));
    };
    if queued >= MAX_ADMIN_QUEUE {
        return Err(BridgeError::InvalidState(format!("the admin lane queue is full ({MAX_ADMIN_QUEUE})")));
    }
    let now = env::now_ns();
    let id = state::next_tx_id();
    let data = abi::encode_admin_action(&action);
    let note = format!("admin action queued: {action:?}");
    let rec = EvmTxRecord {
        id,
        lane: LaneKind::Admin,
        nonce: 0,
        purpose: TxPurpose::Admin { action },
        to: contract,
        value_wei: 0,
        data,
        gas_limit,
        attempts: vec![],
        status: TxStatus::Unsigned,
        cancel_requested: false,
        outcome_applied: false,
        created_at_ns: now,
        updated_at_ns: now,
        last_checked_ns: None,
        note: None,
    };
    state::put_tx(&rec);
    state::mutate_core(|c| c.admin_lane.queued_txs.push(id));
    state::append_audit(None, AuditCtx::tx(id, note));
    Ok(id)
}

/// Ask the lane to replace the current unincluded transaction with a zero-value
/// self-transfer at the same nonce. Whichever attempt finalizes decides the outcome.
pub fn request_cancel(tx_id: u64) -> BridgeResult<()> {
    let mut tx = state::get_tx(tx_id).ok_or(BridgeError::NotFound)?;
    let current = state::read_core(|c| c.lane(tx.lane).unincluded_tx == Some(tx_id));
    if !current {
        return Err(BridgeError::InvalidState(
            "only the current unincluded transaction of a lane can be cancelled".into(),
        ));
    }
    if !matches!(tx.status, TxStatus::Unsigned | TxStatus::Pending) {
        return Err(BridgeError::InvalidState("the transaction is already included or finalized".into()));
    }
    if tx.cancel_requested {
        return Ok(());
    }
    tx.cancel_requested = true;
    tx.updated_at_ns = env::now_ns();
    state::put_tx(&tx);
    state::append_audit(None, AuditCtx::tx(tx_id, "cancel requested"));
    Ok(())
}

/// Admin: clear a lane halt only if the chain agrees with local records. The lane must
/// hold no unincluded or unfinalized transaction, and the quorum `finalized` and
/// `latest` transaction counts must both equal the lane's `next_nonce`.
pub async fn clear_lane_halt(lane: LaneKind) -> BridgeResult<()> {
    clear_lane_halt_with(&ChainIo, lane).await
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Included-but-not-final transactions a lane may hold before it stops starting new ones.
pub const MAX_UNFINALIZED: usize = 8;
/// The same signed bytes are rebroadcast at most this often.
pub const REBROADCAST_INTERVAL_NS: u64 = 30 * NS_PER_SEC;
/// Upper bound on attempts per transaction (bounds storage and receipt lookups).
pub const MAX_ATTEMPTS_PER_TX: usize = 64;
/// Upper bound on queued, not yet started admin actions.
pub const MAX_ADMIN_QUEUE: usize = 32;
/// Gas limit of the zero-value self-transfer used to cancel.
pub const CANCEL_GAS_LIMIT: u64 = 21_000;
/// Marker note of a transaction parked after its nonce was consumed by an unknown tx.
pub const PARKED_NOTE: &str = "parked: nonce consumed by an unknown transaction (lane halted)";

const NS_PER_SEC: u64 = 1_000_000_000;
/// A run that has not finished after this long is presumed trapped; its guard is reclaimed.
const STALE_RUN_NS: u64 = 10 * 60 * NS_PER_SEC;
const MAX_NOTES: usize = 64;
const MAX_TEXT: usize = 160;

// ---------------------------------------------------------------------------
// Pure decision logic
// ---------------------------------------------------------------------------

/// Fee fields of one attempt, in wei per gas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fees {
    pub max_fee_per_gas: u128,
    pub max_priority_fee_per_gas: u128,
}

/// What a receipt lookup for one attempt hash produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReceiptObs {
    Found(Receipt),
    NotFound,
    /// The lookup failed or returned a receipt for a different hash. Never read as "no receipt".
    Unknown,
}

/// Observations the pure decision needs. Nonces are `None` until fetched.
#[derive(Clone, Debug)]
pub struct PendingObs {
    pub now_ns: u64,
    /// One entry per attempt, same order as `tx.attempts`.
    pub receipts: Vec<ReceiptObs>,
    pub latest_nonce: Option<u64>,
    pub finalized_nonce: Option<u64>,
}

/// Local rules that restrict signing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Policy {
    /// The lane is halted: no new attempts of any kind.
    pub lane_halted: bool,
    /// A mint is not allowed to be signed now (paused, halted bridge, contract unbound).
    pub mint_blocked: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PendingAction {
    /// Build, gas-check, sign and persist the first attempt, then broadcast it.
    SignFirst { kind: AttemptKind },
    /// A receipt exists for attempt `attempt`.
    Included { attempt: u32, block_number: u64, block_hash: [u8; 32], success: bool },
    /// Broadcast the newest attempt's persisted bytes again.
    Rebroadcast { reason: &'static str },
    /// The decision needs the quorum `latest` and `finalized` transaction counts.
    NeedNonces,
    /// Sign and persist a new attempt at the same nonce with these fees.
    SignBump { kind: AttemptKind, fees: Fees },
    Wait(&'static str),
    /// Halt the lane and park the transaction.
    Halt(String),
}

fn rebroadcast_due(latest: &TxAttempt, now_ns: u64) -> bool {
    latest.last_broadcast_ns.map(|t| now_ns.saturating_sub(t) >= REBROADCAST_INTERVAL_NS).unwrap_or(true)
}

fn rebroadcast_or_wait(due: bool, why: &'static str) -> PendingAction {
    if due {
        PendingAction::Rebroadcast { reason: why }
    } else {
        PendingAction::Wait(why)
    }
}

/// Decide what to do with a transaction that has no included attempt yet (status
/// `Unsigned` or `Pending`, including a transaction returned to `Pending` by a reorg).
pub fn decide_pending(tx: &EvmTxRecord, obs: &PendingObs, gas: &GasSettings, policy: &Policy) -> PendingAction {
    let Some(latest) = tx.attempts.last() else {
        // No attempt exists, so no signed bytes exist anywhere: sign the same fields at
        // the same nonce again (this also covers a lost signature reply).
        return if policy.lane_halted {
            PendingAction::Wait("lane halted: not signing")
        } else if tx.cancel_requested {
            PendingAction::SignFirst { kind: AttemptKind::Cancel }
        } else if policy.mint_blocked {
            PendingAction::Wait("mint signing is blocked (paused, halted or contract unbound)")
        } else {
            PendingAction::SignFirst { kind: AttemptKind::Original }
        };
    };

    // A receipt for ANY recorded attempt settles which attempt landed.
    let mut found = obs.receipts.iter().enumerate().filter_map(|(i, r)| match r {
        ReceiptObs::Found(rc) => Some((i, rc)),
        _ => None,
    });
    let first = found.next();
    if found.next().is_some() {
        return PendingAction::Wait("more than one attempt reports a receipt; re-checking next run");
    }
    if let Some((i, rc)) = first {
        return PendingAction::Included {
            attempt: i as u32,
            block_number: rc.block_number,
            block_hash: rc.block_hash,
            success: rc.success,
        };
    }

    let due = rebroadcast_due(latest, obs.now_ns);
    let lookups_complete =
        obs.receipts.len() == tx.attempts.len() && obs.receipts.iter().all(|r| matches!(r, ReceiptObs::NotFound));
    let age_ns = obs.now_ns.saturating_sub(latest.created_at_ns);
    let replace_after_ns = gas.replace_after_secs.saturating_mul(NS_PER_SEC);
    if !lookups_complete || age_ns < replace_after_ns {
        return rebroadcast_or_wait(due, "awaiting inclusion");
    }

    let (Some(latest_nonce), Some(finalized_nonce)) = (obs.latest_nonce, obs.finalized_nonce) else {
        return PendingAction::NeedNonces;
    };
    if finalized_nonce > tx.nonce {
        return PendingAction::Halt(format!(
            "{} lane nonce {} was consumed (finalized transaction count {finalized_nonce}) but none of the {} recorded attempts of tx {} has a receipt",
            lane_label(tx.lane),
            tx.nonce,
            tx.attempts.len(),
            tx.id
        ));
    }
    if latest_nonce > tx.nonce {
        // One of our attempts (or something else) used the nonce and receipts lag. Never replace.
        return PendingAction::Wait("latest nonce is past the tx nonce; waiting for the receipt");
    }
    if latest_nonce < tx.nonce {
        return rebroadcast_or_wait(due, "latest nonce is behind the tx nonce (provider lag or reorg)");
    }
    if policy.lane_halted {
        return rebroadcast_or_wait(due, "lane halted: not replacing");
    }
    if tx.attempts.len() >= MAX_ATTEMPTS_PER_TX {
        return rebroadcast_or_wait(due, "attempt limit reached");
    }
    let kind = if tx.cancel_requested { AttemptKind::Cancel } else { AttemptKind::Replacement };
    match highest_fees(tx).and_then(|prev| bumped_fees(prev, gas)) {
        Some(fees) => PendingAction::SignBump { kind, fees },
        None => rebroadcast_or_wait(due, "fee cap reached: cannot raise fees"),
    }
}

/// `x + 12.5%`, rounded up, and at least one wei more (geth refuses a replacement unless
/// both fee fields strictly increase).
pub fn bump_once(x: u128) -> u128 {
    x.saturating_add(x.div_ceil(8).max(1))
}

/// The smallest rise a node accepts for a replacement (10%, rounded up).
fn node_minimum_bump(x: u128) -> u128 {
    x.saturating_add(x.div_ceil(10))
}

/// Fees of the first attempt: priority = min(cap, estimate), max fee = min(cap, 2 * base +
/// priority), and the priority never exceeds the max fee.
pub fn initial_fees(est: &FeeEstimate, caps: &GasSettings) -> Fees {
    let priority = est.priority_fee_per_gas.min(caps.max_priority_fee_per_gas_wei);
    let max_fee =
        est.base_fee_per_gas.saturating_mul(2).saturating_add(priority).min(caps.max_fee_per_gas_wei);
    Fees { max_fee_per_gas: max_fee, max_priority_fee_per_gas: priority.min(max_fee) }
}

/// Fees for a same-nonce replacement: both fields up 12.5% (rounded up), clipped to the
/// configured caps. `None` when the caps leave no room for a rise a node would accept
/// (both fields must strictly increase and by at least 10%), in which case the caller
/// only rebroadcasts.
pub fn bumped_fees(prev: Fees, caps: &GasSettings) -> Option<Fees> {
    let max_fee = bump_once(prev.max_fee_per_gas).min(caps.max_fee_per_gas_wei);
    let priority = bump_once(prev.max_priority_fee_per_gas).min(caps.max_priority_fee_per_gas_wei).min(max_fee);
    let rises = |new: u128, old: u128| new > old && new >= node_minimum_bump(old);
    (rises(max_fee, prev.max_fee_per_gas) && rises(priority, prev.max_priority_fee_per_gas))
        .then_some(Fees { max_fee_per_gas: max_fee, max_priority_fee_per_gas: priority })
}

/// Per-field maximum over every attempt (fees only ever rise, but a replacement or
/// cancel must beat whatever a node may still hold).
fn highest_fees(tx: &EvmTxRecord) -> Option<Fees> {
    tx.attempts.iter().fold(None, |acc, a| {
        Some(match acc {
            None => Fees { max_fee_per_gas: a.max_fee_per_gas, max_priority_fee_per_gas: a.max_priority_fee_per_gas },
            Some(f) => Fees {
                max_fee_per_gas: f.max_fee_per_gas.max(a.max_fee_per_gas),
                max_priority_fee_per_gas: f.max_priority_fee_per_gas.max(a.max_priority_fee_per_gas),
            },
        })
    })
}

/// Wei the lane address must hold before an attempt is signed.
pub fn gas_required(value_wei: u128, gas_limit: u64, max_fee_per_gas: u128, reserve_wei: u128) -> u128 {
    (gas_limit as u128).saturating_mul(max_fee_per_gas).saturating_add(value_wei).saturating_add(reserve_wei)
}

pub fn gas_sufficient(balance: u128, value_wei: u128, gas_limit: u64, max_fee_per_gas: u128, reserve_wei: u128) -> bool {
    balance >= gas_required(value_wei, gas_limit, max_fee_per_gas, reserve_wei)
}

/// `(to, value, data, gas_limit)` of an attempt. A cancel is a zero-value self-transfer
/// with empty data; every other attempt repeats the transaction's call.
pub fn attempt_fields(tx: &EvmTxRecord, kind: AttemptKind, own_address: [u8; 20]) -> ([u8; 20], u128, Vec<u8>, u64) {
    match kind {
        AttemptKind::Cancel => (own_address, 0, Vec::new(), CANCEL_GAS_LIMIT),
        AttemptKind::Original | AttemptKind::Replacement => (tx.to, tx.value_wei, tx.data.clone(), tx.gas_limit),
    }
}

/// First 4 bytes of revert data as `0x` hex, or a marker when there are none.
pub fn revert_selector_hex(data: &[u8]) -> String {
    if data.len() >= 4 {
        hex0x(&data[..4])
    } else {
        "no revert selector".to_string()
    }
}

/// Lane state relevant to starting a new transaction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StartGate {
    pub lane_halted: bool,
    pub awaiting_gas: bool,
    pub has_unincluded: bool,
    pub unfinalized: usize,
    /// An unfinalized tx fell back to `Pending` (reorg).
    pub pending_in_unfinalized: bool,
}

/// A new transaction (with a new nonce) may start only when this returns `Ok`.
pub fn start_gate(g: &StartGate) -> Result<(), &'static str> {
    if g.lane_halted {
        Err("lane halted")
    } else if g.has_unincluded {
        Err("an unincluded transaction is outstanding")
    } else if g.pending_in_unfinalized {
        Err("a transaction fell back to pending after a reorg")
    } else if g.awaiting_gas {
        Err("awaiting gas")
    } else if g.unfinalized >= MAX_UNFINALIZED {
        Err("too many unfinalized transactions")
    } else {
        Ok(())
    }
}

/// Whether the lane can hand out another nonce.
pub fn can_allocate(lane: &LaneState) -> bool {
    matches!(lane.next_nonce, Some(n) if n < u64::MAX)
}

/// Assign the next nonce to `tx_id` and make it the lane's current unincluded tx.
/// `None` (and no change) when no nonce is available.
pub fn begin_tx_on_lane(lane: &mut LaneState, tx_id: u64) -> Option<u64> {
    let nonce = lane.next_nonce?;
    let next = nonce.checked_add(1)?;
    lane.next_nonce = Some(next);
    lane.unincluded_tx = Some(tx_id);
    Some(nonce)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FinalityPrecheck {
    /// Not included, or the included block is above the finalized head.
    NotYet,
    /// The included block is at or below the finalized head: confirm it.
    Confirm,
}

pub fn finality_precheck(tx: &EvmTxRecord, finalized_number: u64) -> FinalityPrecheck {
    match tx.status {
        TxStatus::Included { block_number, .. } if block_number <= finalized_number => FinalityPrecheck::Confirm,
        _ => FinalityPrecheck::NotYet,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FinalityAction {
    /// The receipt still matches; fetch the canonical block hash at its height.
    NeedCanonical,
    Finalize { attempt: u32, block_number: u64, block_hash: [u8; 32], success: bool },
    /// The receipt is gone or sits in a different block: back to `Pending`.
    Reorged(&'static str),
    Halt(String),
    Skip(&'static str),
}

/// Decide about an `Included` transaction whose block is at or below the finalized head.
/// `receipt` is a fresh lookup of the included attempt's hash; `canonical_hash` is the
/// quorum block hash at the included height (`None` until fetched).
pub fn decide_finality(tx: &EvmTxRecord, receipt: Option<&Receipt>, canonical_hash: Option<[u8; 32]>) -> FinalityAction {
    let TxStatus::Included { attempt, block_number, block_hash, success } = tx.status else {
        return FinalityAction::Skip("not in the included state");
    };
    let Some(att) = tx.attempts.get(attempt as usize) else {
        return FinalityAction::Halt(format!("tx {} is included with attempt {attempt} but has only {} attempts", tx.id, tx.attempts.len()));
    };
    let Some(rc) = receipt else {
        return FinalityAction::Reorged("receipt disappeared before finality");
    };
    if rc.tx_hash != att.tx_hash || rc.block_number != block_number || rc.block_hash != block_hash {
        return FinalityAction::Reorged("receipt moved to a different block before finality");
    }
    let Some(canonical) = canonical_hash else {
        return FinalityAction::NeedCanonical;
    };
    if canonical != block_hash {
        return FinalityAction::Reorged("included block is not canonical");
    }
    if rc.success != success {
        return FinalityAction::Halt(format!(
            "tx {}: receipt status changed from {success} to {} for the same block hash",
            tx.id, rc.success
        ));
    }
    FinalityAction::Finalize { attempt, block_number, block_hash, success }
}

/// How a finalized transaction maps to a mint outcome.
pub fn mint_outcome(kind: Option<AttemptKind>, success: bool, receipt: Receipt) -> MintTxOutcome {
    if kind == Some(AttemptKind::Cancel) {
        MintTxOutcome::Cancelled(receipt)
    } else if success {
        MintTxOutcome::Executed(receipt)
    } else {
        MintTxOutcome::Reverted(receipt)
    }
}

fn lane_label(lane: LaneKind) -> &'static str {
    match lane {
        LaneKind::Minter => "minter",
        LaneKind::Admin => "admin",
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push_str("...");
        out
    }
}

fn summarize_broadcast(results: &[Result<String, String>]) -> String {
    if results.is_empty() {
        return "no provider responded".to_string();
    }
    let accepted = results.iter().filter(|r| r.is_ok()).count();
    let errors: Vec<&String> = results.iter().filter_map(|r| r.as_ref().err()).collect();
    let mut s = format!("{accepted} accepted, {} rejected", errors.len());
    if let Some(first) = errors.first() {
        s.push_str(": ");
        s.push_str(&truncate(first, MAX_TEXT));
    }
    s
}

// ---------------------------------------------------------------------------
// I/O seam
// ---------------------------------------------------------------------------

/// Everything the driver needs from the outside world. The production implementation
/// forwards to `evm::rpc` and `evm::signer`; tests use an in-memory chain.
pub(crate) trait LaneIo {
    async fn lane_address(&self, lane: LaneKind) -> Result<[u8; 20], String>;
    async fn transaction_count(&self, address: &[u8; 20], tag: BlockTag) -> Result<u64, RpcError>;
    async fn balance(&self, address: &[u8; 20], tag: BlockTag) -> Result<u128, RpcError>;
    async fn fee_estimate(&self) -> Result<FeeEstimate, RpcError>;
    async fn get_receipt(&self, tx_hash: &[u8; 32]) -> Result<Option<Receipt>, RpcError>;
    async fn finalized_head(&self) -> Result<BlockRef, RpcError>;
    async fn block_by_number(&self, number: u64) -> Result<BlockRef, RpcError>;
    async fn send_raw(&self, raw: &[u8]) -> Vec<Result<String, String>>;
    async fn eth_call(&self, to: &[u8; 20], data: &[u8], tag: BlockTag) -> Result<Vec<u8>, RpcError>;
    async fn simulate_call(
        &self,
        from: &[u8; 20],
        to: &[u8; 20],
        data: &[u8],
        gas: u64,
        tag: BlockTag,
    ) -> Result<SimulationResult, RpcError>;
    async fn sign(&self, lane: LaneKind, tx: &Eip1559Tx) -> Result<(Vec<u8>, [u8; 32]), String>;
}

pub(crate) struct ChainIo;

impl LaneIo for ChainIo {
    async fn lane_address(&self, lane: LaneKind) -> Result<[u8; 20], String> {
        signer::lane_address(lane).await
    }
    async fn transaction_count(&self, address: &[u8; 20], tag: BlockTag) -> Result<u64, RpcError> {
        rpc::transaction_count(address, tag).await
    }
    async fn balance(&self, address: &[u8; 20], tag: BlockTag) -> Result<u128, RpcError> {
        rpc::balance(address, tag).await
    }
    async fn fee_estimate(&self) -> Result<FeeEstimate, RpcError> {
        rpc::fee_estimate().await
    }
    async fn get_receipt(&self, tx_hash: &[u8; 32]) -> Result<Option<Receipt>, RpcError> {
        rpc::get_receipt(tx_hash).await
    }
    async fn finalized_head(&self) -> Result<BlockRef, RpcError> {
        rpc::finalized_head().await
    }
    async fn block_by_number(&self, number: u64) -> Result<BlockRef, RpcError> {
        rpc::block_by_number(number).await
    }
    async fn send_raw(&self, raw: &[u8]) -> Vec<Result<String, String>> {
        rpc::send_raw(raw).await
    }
    async fn eth_call(&self, to: &[u8; 20], data: &[u8], tag: BlockTag) -> Result<Vec<u8>, RpcError> {
        rpc::eth_call(to, data, tag).await
    }
    async fn simulate_call(
        &self,
        from: &[u8; 20],
        to: &[u8; 20],
        data: &[u8],
        gas: u64,
        tag: BlockTag,
    ) -> Result<SimulationResult, RpcError> {
        rpc::simulate_call(from, to, data, gas, tag).await
    }
    async fn sign(&self, lane: LaneKind, tx: &Eip1559Tx) -> Result<(Vec<u8>, [u8; 32]), String> {
        signer::sign_tx(lane, tx).await
    }
}

// ---------------------------------------------------------------------------
// Run guard and bookkeeping
// ---------------------------------------------------------------------------

thread_local! {
    static RUNNING: RefCell<[Option<u64>; 2]> = const { RefCell::new([None, None]) };
}

fn lane_slot(lane: LaneKind) -> usize {
    match lane {
        LaneKind::Minter => 0,
        LaneKind::Admin => 1,
    }
}

/// At most one run per lane at a time. A guard left behind by a trap after an await is
/// reclaimed after ten minutes. Each guard carries the claim time it set as an owner
/// token, so a slow run whose guard was reclaimed never clears the newer run's claim.
struct RunGuard {
    lane: LaneKind,
    token: u64,
}

impl RunGuard {
    fn acquire(lane: LaneKind) -> Option<RunGuard> {
        let now = env::now_ns();
        RUNNING.with(|r| {
            let mut r = r.borrow_mut();
            let slot = &mut r[lane_slot(lane)];
            match *slot {
                Some(t) if now.saturating_sub(t) < STALE_RUN_NS => None,
                _ => {
                    *slot = Some(now);
                    Some(RunGuard { lane, token: now })
                }
            }
        })
    }
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        RUNNING.with(|r| {
            let mut r = r.borrow_mut();
            let slot = &mut r[lane_slot(self.lane)];
            if *slot == Some(self.token) {
                *slot = None;
            }
        });
    }
}

struct RunState {
    lane: LaneKind,
    notes: Vec<String>,
    /// Txs sent back to `Pending` by this run: their replacement logic waits for the next run.
    reorged: Vec<u64>,
}

impl RunState {
    fn new(lane: LaneKind) -> Self {
        RunState { lane, notes: Vec::new(), reorged: Vec::new() }
    }

    fn note(&mut self, msg: impl AsRef<str>) {
        if self.notes.len() < MAX_NOTES {
            self.notes.push(format!("{}: {}", lane_label(self.lane), msg.as_ref()));
        }
    }
}

fn lane_has_pending_work(lane: LaneKind) -> bool {
    let parked = |id: &u64| state::get_tx(*id).map(|t| t.note.as_deref() == Some(PARKED_NOTE)).unwrap_or(false);
    state::read_core(|c| {
        let l = c.lane(lane);
        l.unincluded_tx.iter().chain(l.unfinalized_txs.iter()).any(|id| !parked(id)) || !l.queued_txs.is_empty()
    })
}

/// Halt the lane (first reason wins) and audit it. Returns whether this call set it.
fn halt_lane(lane: LaneKind, reason: &str) -> bool {
    let newly = state::mutate_core(|c| {
        let l = c.lane_mut(lane);
        if l.halted.is_none() {
            l.halted = Some(reason.to_string());
            true
        } else {
            false
        }
    });
    if newly {
        state::append_audit(None, AuditCtx::note(format!("{} lane HALT: {reason}", lane_label(lane))));
    }
    newly
}

fn park_and_halt(lane: LaneKind, tx_id: u64, reason: &str) {
    halt_lane(lane, reason);
    if let Some(mut tx) = state::get_tx(tx_id) {
        tx.note = Some(PARKED_NOTE.to_string());
        tx.updated_at_ns = env::now_ns();
        state::put_tx(&tx);
    }
}

fn policy_for(tx: &EvmTxRecord) -> Policy {
    state::read_core(|c| Policy {
        lane_halted: c.lane(tx.lane).halted.is_some(),
        mint_blocked: matches!(tx.purpose, TxPurpose::Mint { .. })
            && (c.halted.is_some() || c.config.pauses.mints || c.config.contract.is_none()),
    })
}

fn start_gate_for(lane: LaneKind) -> StartGate {
    let (halted, awaiting_gas, unincluded, unfinalized) = state::read_core(|c| {
        let l = c.lane(lane);
        (l.halted.is_some(), l.awaiting_gas, l.unincluded_tx.is_some(), l.unfinalized_txs.clone())
    });
    let pending_in_unfinalized = unfinalized.iter().any(|id| {
        state::get_tx(*id).map(|t| matches!(t.status, TxStatus::Unsigned | TxStatus::Pending)).unwrap_or(true)
    });
    StartGate {
        lane_halted: halted,
        awaiting_gas,
        has_unincluded: unincluded,
        unfinalized: unfinalized.len(),
        pending_in_unfinalized,
    }
}

// ---------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------

pub(crate) async fn run_lane_with<I: LaneIo>(io: &I, lane: LaneKind) -> LaneOutcome {
    let mut st = RunState::new(lane);
    match RunGuard::acquire(lane) {
        None => st.note("a run is already in progress"),
        Some(_guard) => {
            step_finality(io, &mut st).await;
            step_pending(io, &mut st).await;
            step_new_work(io, &mut st).await;
        }
    }
    LaneOutcome { notes: st.notes, pending_work: lane_has_pending_work(lane) }
}

// ----- finality ----------------------------------------------------------------

async fn step_finality<I: LaneIo>(io: &I, st: &mut RunState) {
    let lane = st.lane;
    let ids: Vec<u64> = state::read_core(|c| c.lane(lane).unfinalized_txs.clone());
    let mut included = Vec::new();
    for id in ids {
        match state::get_tx(id) {
            None => {
                let reason = format!("tx record {id} is missing");
                halt_lane(lane, &reason);
                st.note(reason);
            }
            Some(tx) => match tx.status {
                TxStatus::Included { .. } => included.push(tx),
                TxStatus::Finalized { .. } => {
                    // Already settled; just drop it from the list.
                    state::mutate_core(|c| c.lane_mut(lane).unfinalized_txs.retain(|x| *x != id));
                }
                TxStatus::Unsigned | TxStatus::Pending => {}
            },
        }
    }
    if included.is_empty() {
        return;
    }
    let head = match io.finalized_head().await {
        Ok(h) => h,
        Err(e) => {
            st.note(format!("finalized head unavailable: {e}"));
            return;
        }
    };
    for tx in included {
        if finality_precheck(&tx, head.number) == FinalityPrecheck::Confirm {
            confirm_and_apply(io, tx, st).await;
        }
    }
}

async fn confirm_and_apply<I: LaneIo>(io: &I, tx: EvmTxRecord, st: &mut RunState) {
    let lane = st.lane;
    let TxStatus::Included { attempt, block_number, .. } = tx.status else { return };
    let Some(att) = tx.attempts.get(attempt as usize) else {
        let reason = format!("tx {} is included with attempt {attempt} but has only {} attempts", tx.id, tx.attempts.len());
        halt_lane(lane, &reason);
        st.note(reason);
        return;
    };
    let hash = att.tx_hash;
    let receipt = match io.get_receipt(&hash).await {
        Ok(r) => r,
        Err(e) => {
            st.note(format!("tx {}: receipt lookup failed: {e}", tx.id));
            return;
        }
    };
    let mut action = decide_finality(&tx, receipt.as_ref(), None);
    if action == FinalityAction::NeedCanonical {
        let canonical = match io.block_by_number(block_number).await {
            Ok(b) if b.number == block_number => b.hash,
            Ok(b) => {
                st.note(format!("tx {}: provider returned block {} for height {block_number}", tx.id, b.number));
                return;
            }
            Err(e) => {
                st.note(format!("tx {}: canonical block lookup failed: {e}", tx.id));
                return;
            }
        };
        action = decide_finality(&tx, receipt.as_ref(), Some(canonical));
    }
    match action {
        FinalityAction::Finalize { .. } => {
            if let Some(rc) = receipt {
                apply_finalized(tx.id, &tx.status, rc, st);
            }
        }
        FinalityAction::Reorged(why) => {
            apply_reorged(tx.id, &tx.status, why, st);
            // Same bytes again, right away; the next run decides about replacement.
            if let Some(last) = state::get_tx(tx.id).and_then(|t| t.attempts.len().checked_sub(1)) {
                broadcast_attempt(io, tx.id, last, st).await;
            }
        }
        FinalityAction::Halt(reason) => {
            halt_lane(lane, &reason);
            st.note(reason);
        }
        FinalityAction::Skip(why) => st.note(format!("tx {}: {why}", tx.id)),
        FinalityAction::NeedCanonical => st.note(format!("tx {}: canonical block still unknown", tx.id)),
    }
}

/// Mark `Finalized` and apply the purpose outcome exactly once.
fn apply_finalized(tx_id: u64, expected: &TxStatus, receipt: Receipt, st: &mut RunState) {
    let lane = st.lane;
    let Some(mut tx) = state::get_tx(tx_id) else { return };
    if tx.status != *expected {
        st.note(format!("tx {tx_id}: changed while confirming finality; re-checking next run"));
        return;
    }
    let TxStatus::Included { attempt, block_number, block_hash, success } = tx.status else { return };
    let kind = tx.attempts.get(attempt as usize).map(|a| a.kind);
    let first_application = !tx.outcome_applied;
    tx.status = TxStatus::Finalized { attempt, block_number, block_hash, success };
    // Set before the hook runs: from here on the outcome can never be applied again.
    tx.outcome_applied = true;
    tx.updated_at_ns = env::now_ns();
    state::put_tx(&tx);
    state::mutate_core(|c| {
        let l = c.lane_mut(lane);
        l.unfinalized_txs.retain(|x| *x != tx_id);
        if l.unincluded_tx == Some(tx_id) {
            l.unincluded_tx = None;
        }
    });
    state::append_audit(
        None,
        AuditCtx::tx(tx_id, format!("finalized in block {block_number} (attempt {attempt}, success={success})")),
    );
    st.note(format!("tx {tx_id}: finalized in block {block_number}"));
    if !first_application {
        return;
    }
    if let Err(e) = apply_purpose(&tx, kind, success, receipt) {
        if let Some(mut t) = state::get_tx(tx_id) {
            t.outcome_applied = false;
            t.note = Some(truncate(&e, MAX_TEXT));
            state::put_tx(&t);
        }
        state::append_audit(None, AuditCtx::tx(tx_id, format!("outcome not applied: {}", truncate(&e, MAX_TEXT))));
        st.note(format!("tx {tx_id}: outcome not applied: {}", truncate(&e, MAX_TEXT)));
    }
}

fn apply_purpose(tx: &EvmTxRecord, kind: Option<AttemptKind>, success: bool, receipt: Receipt) -> Result<(), String> {
    match &tx.purpose {
        TxPurpose::Mint { deposit_id } => {
            deposit::on_mint_finalized(*deposit_id, tx.id, mint_outcome(kind, success, receipt))
        }
        TxPurpose::Admin { action } => {
            let how = if kind == Some(AttemptKind::Cancel) {
                "cancelled by a self-transfer"
            } else if success {
                "succeeded"
            } else {
                "reverted"
            };
            state::append_audit(None, AuditCtx::tx(tx.id, format!("admin action {action:?} {how}")));
            crate::admin::on_contract_action_finalized(action, success && kind != Some(AttemptKind::Cancel));
            Ok(())
        }
    }
}

/// The receipt vanished or moved: the tx goes back to `Pending` (it stays in
/// `unfinalized_txs`, keeps every attempt and its nonce).
fn apply_reorged(tx_id: u64, expected: &TxStatus, why: &str, st: &mut RunState) {
    let Some(mut tx) = state::get_tx(tx_id) else { return };
    if tx.status != *expected {
        return;
    }
    tx.status = TxStatus::Pending;
    tx.updated_at_ns = env::now_ns();
    state::put_tx(&tx);
    st.reorged.push(tx_id);
    state::append_audit(None, AuditCtx::tx(tx_id, format!("reorg before finality: {why}; returned to pending")));
    st.note(format!("tx {tx_id}: {why}; returned to pending"));
}

// ----- transactions without a receipt ------------------------------------------

async fn step_pending<I: LaneIo>(io: &I, st: &mut RunState) {
    let lane = st.lane;
    let ids: Vec<u64> = state::read_core(|c| {
        let l = c.lane(lane);
        l.unincluded_tx.iter().chain(l.unfinalized_txs.iter()).copied().collect()
    });
    for id in ids {
        if st.reorged.contains(&id) {
            continue;
        }
        let waiting = state::get_tx(id).map(|t| matches!(t.status, TxStatus::Unsigned | TxStatus::Pending)).unwrap_or(false);
        if waiting {
            drive_pending_tx(io, id, st).await;
        }
    }
    // The gas flag only describes an unsigned transaction.
    let has_unsigned = state::read_core(|c| c.lane(lane).unincluded_tx)
        .and_then(state::get_tx)
        .map(|t| t.attempts.is_empty())
        .unwrap_or(false);
    if !has_unsigned {
        state::mutate_core(|c| c.lane_mut(lane).awaiting_gas = false);
    }
}

async fn gather_receipts<I: LaneIo>(io: &I, tx: &EvmTxRecord, st: &mut RunState) -> Vec<ReceiptObs> {
    let mut out = Vec::with_capacity(tx.attempts.len());
    let mut reported = false;
    for (i, a) in tx.attempts.iter().enumerate() {
        match io.get_receipt(&a.tx_hash).await {
            Ok(Some(r)) if r.tx_hash == a.tx_hash => out.push(ReceiptObs::Found(r)),
            Ok(Some(_)) => {
                st.note(format!("tx {}: provider returned a receipt for a different hash (attempt {i})", tx.id));
                out.push(ReceiptObs::Unknown);
            }
            Ok(None) => out.push(ReceiptObs::NotFound),
            Err(e) => {
                if !reported {
                    st.note(format!("tx {}: receipt lookup failed: {e}", tx.id));
                    reported = true;
                }
                out.push(ReceiptObs::Unknown);
            }
        }
    }
    out
}

async fn fetch_nonces<I: LaneIo>(io: &I, lane: LaneKind) -> Result<(u64, u64), String> {
    let addr = io.lane_address(lane).await?;
    let latest = io.transaction_count(&addr, BlockTag::Latest).await.map_err(|e| format!("latest nonce unavailable: {e}"))?;
    let finalized =
        io.transaction_count(&addr, BlockTag::Finalized).await.map_err(|e| format!("finalized nonce unavailable: {e}"))?;
    Ok((latest, finalized))
}

async fn drive_pending_tx<I: LaneIo>(io: &I, tx_id: u64, st: &mut RunState) {
    let Some(tx) = state::get_tx(tx_id) else {
        st.note(format!("tx {tx_id}: record missing"));
        return;
    };
    let lane = tx.lane;
    if !matches!(tx.status, TxStatus::Unsigned | TxStatus::Pending) {
        return;
    }
    if tx.note.as_deref() == Some(PARKED_NOTE) {
        st.note(format!("tx {tx_id}: {PARKED_NOTE}"));
        return;
    }
    let gas = state::read_core(|c| c.config.gas.clone());
    let policy = policy_for(&tx);
    let receipts = gather_receipts(io, &tx, st).await;
    let mut obs = PendingObs { now_ns: env::now_ns(), receipts, latest_nonce: None, finalized_nonce: None };
    let mut action = decide_pending(&tx, &obs, &gas, &policy);
    if action == PendingAction::NeedNonces {
        match fetch_nonces(io, lane).await {
            Ok((latest, finalized)) => {
                obs.latest_nonce = Some(latest);
                obs.finalized_nonce = Some(finalized);
            }
            Err(e) => {
                st.note(format!("tx {tx_id}: {e}"));
                return;
            }
        }
        action = decide_pending(&tx, &obs, &gas, &policy);
    }
    match action {
        PendingAction::NeedNonces => st.note(format!("tx {tx_id}: nonces still required")),
        PendingAction::Wait(why) => st.note(format!("tx {tx_id}: {why}")),
        PendingAction::Included { attempt, block_number, block_hash, success } => {
            apply_included(tx_id, attempt, block_number, block_hash, success, st)
        }
        PendingAction::Rebroadcast { reason } => {
            if let Some(last) = tx.attempts.len().checked_sub(1) {
                st.note(format!("tx {tx_id}: {reason}"));
                broadcast_attempt(io, tx_id, last, st).await;
            }
        }
        PendingAction::Halt(reason) => {
            park_and_halt(lane, tx_id, &reason);
            st.note(reason);
        }
        PendingAction::SignFirst { kind } => sign_first(io, &tx, kind, &gas, st).await,
        PendingAction::SignBump { kind, fees } => sign_bump(io, &tx, kind, fees, &gas, st).await,
    }
}

fn apply_included(tx_id: u64, attempt: u32, block_number: u64, block_hash: [u8; 32], success: bool, st: &mut RunState) {
    let lane = st.lane;
    let Some(mut tx) = state::get_tx(tx_id) else { return };
    if !matches!(tx.status, TxStatus::Unsigned | TxStatus::Pending) {
        return;
    }
    tx.status = TxStatus::Included { attempt, block_number, block_hash, success };
    tx.updated_at_ns = env::now_ns();
    state::put_tx(&tx);
    state::mutate_core(|c| {
        let l = c.lane_mut(lane);
        if l.unincluded_tx == Some(tx_id) {
            l.unincluded_tx = None;
        }
        if !l.unfinalized_txs.contains(&tx_id) {
            l.unfinalized_txs.push(tx_id);
        }
    });
    state::append_audit(
        None,
        AuditCtx::tx(tx_id, format!("included in block {block_number} via attempt {attempt} (success={success})")),
    );
    st.note(format!("tx {tx_id}: included in block {block_number} via attempt {attempt}"));
}

// ----- signing and broadcasting ---------------------------------------------------

async fn sign_first<I: LaneIo>(io: &I, tx: &EvmTxRecord, kind: AttemptKind, gas: &GasSettings, st: &mut RunState) {
    let lane = tx.lane;
    let addr = match io.lane_address(lane).await {
        Ok(a) => a,
        Err(e) => {
            st.note(format!("tx {}: lane address unavailable: {e}", tx.id));
            return;
        }
    };
    let est = match io.fee_estimate().await {
        Ok(e) => e,
        Err(e) => {
            st.note(format!("tx {}: fee estimate unavailable: {e}", tx.id));
            return;
        }
    };
    let fees = initial_fees(&est, gas);
    let (_, value, _, gas_limit) = attempt_fields(tx, kind, addr);
    let balance = match io.balance(&addr, BlockTag::Latest).await {
        Ok(b) => b,
        Err(e) => {
            st.note(format!("tx {}: balance unavailable: {e}", tx.id));
            return;
        }
    };
    let sufficient = gas_sufficient(balance, value, gas_limit, fees.max_fee_per_gas, gas.min_gas_reserve_wei);
    let newly_waiting = state::mutate_core(|c| {
        let l = c.lane_mut(lane);
        l.last_gas_balance_wei = Some(balance);
        let was = l.awaiting_gas;
        l.awaiting_gas = !sufficient;
        !sufficient && !was
    });
    if !sufficient {
        if newly_waiting {
            state::append_audit(
                None,
                AuditCtx::tx(tx.id, format!("awaiting gas: balance {balance} wei, need {}", gas_required(value, gas_limit, fees.max_fee_per_gas, gas.min_gas_reserve_wei))),
            );
        }
        st.note(format!("tx {}: awaiting gas (balance {balance} wei)", tx.id));
        return;
    }
    sign_and_record(io, tx, kind, fees, addr, 0, st).await;
}

async fn sign_bump<I: LaneIo>(
    io: &I,
    tx: &EvmTxRecord,
    kind: AttemptKind,
    fees: Fees,
    gas: &GasSettings,
    st: &mut RunState,
) {
    let lane = tx.lane;
    let addr = match io.lane_address(lane).await {
        Ok(a) => a,
        Err(e) => {
            st.note(format!("tx {}: lane address unavailable: {e}", tx.id));
            return;
        }
    };
    let (_, value, _, gas_limit) = attempt_fields(tx, kind, addr);
    let balance = match io.balance(&addr, BlockTag::Latest).await {
        Ok(b) => b,
        Err(e) => {
            st.note(format!("tx {}: balance unavailable: {e}", tx.id));
            return;
        }
    };
    state::mutate_core(|c| c.lane_mut(lane).last_gas_balance_wei = Some(balance));
    if !gas_sufficient(balance, value, gas_limit, fees.max_fee_per_gas, gas.min_gas_reserve_wei) {
        st.note(format!("tx {}: balance too low for a replacement; rebroadcasting the existing attempt", tx.id));
        if let Some(last) = tx.attempts.len().checked_sub(1) {
            broadcast_attempt(io, tx.id, last, st).await;
        }
        return;
    }
    sign_and_record(io, tx, kind, fees, addr, tx.attempts.len(), st).await;
}

/// Sign, persist the attempt, then broadcast it. The attempt is stored before any
/// network send. A failed or lost signature leaves the record unchanged.
async fn sign_and_record<I: LaneIo>(
    io: &I,
    tx: &EvmTxRecord,
    kind: AttemptKind,
    fees: Fees,
    own_address: [u8; 20],
    expected_attempts: usize,
    st: &mut RunState,
) {
    let lane = tx.lane;
    let chain_id = state::read_core(|c| c.config.chain_id);
    let (to, value, data, gas_limit) = attempt_fields(tx, kind, own_address);
    let unsigned = Eip1559Tx {
        chain_id,
        nonce: tx.nonce,
        max_priority_fee_per_gas: fees.max_priority_fee_per_gas,
        max_fee_per_gas: fees.max_fee_per_gas,
        gas_limit,
        to,
        value,
        data: data.clone(),
    };
    let (raw, reported_hash) = match io.sign(lane, &unsigned).await {
        Ok(x) => x,
        Err(e) => {
            // No attempt exists, so nothing is lost: the next run signs the same fields at the same nonce.
            st.note(format!("tx {}: signing failed ({}); will sign the same nonce again", tx.id, truncate(&e, MAX_TEXT)));
            return;
        }
    };
    let tx_hash = tx::tx_hash(&raw);
    if tx_hash != reported_hash {
        st.note(format!("tx {}: signer returned a hash that does not match the signed bytes; discarded", tx.id));
        return;
    }
    // Re-read: the record may have moved while the signature was in flight.
    let Some(mut cur) = state::get_tx(tx.id) else { return };
    let lane_halted = state::read_core(|c| c.lane(lane).halted.is_some());
    if cur.attempts.len() != expected_attempts
        || !matches!(cur.status, TxStatus::Unsigned | TxStatus::Pending)
        || lane_halted
    {
        st.note(format!("tx {}: changed while signing; signature discarded", tx.id));
        return;
    }
    let now = env::now_ns();
    cur.attempts.push(TxAttempt {
        kind,
        to,
        value_wei: value,
        data,
        max_fee_per_gas: fees.max_fee_per_gas,
        max_priority_fee_per_gas: fees.max_priority_fee_per_gas,
        gas_limit,
        raw,
        tx_hash,
        created_at_ns: now,
        broadcast_count: 0,
        last_broadcast_ns: None,
        last_broadcast_note: None,
    });
    cur.status = TxStatus::Pending;
    cur.updated_at_ns = now;
    let attempt_index = cur.attempts.len() - 1;
    // Persist BEFORE broadcasting.
    state::put_tx(&cur);
    if expected_attempts == 0 {
        state::mutate_core(|c| c.lane_mut(lane).awaiting_gas = false);
    }
    state::append_audit(
        None,
        AuditCtx::tx(
            tx.id,
            format!(
                "attempt {attempt_index} ({kind:?}) signed: nonce {}, max fee {} wei, priority {} wei, hash {}",
                tx.nonce,
                fees.max_fee_per_gas,
                fees.max_priority_fee_per_gas,
                hex0x(&tx_hash)
            ),
        ),
    );
    broadcast_attempt(io, tx.id, attempt_index, st).await;
}

/// Send the persisted bytes of one attempt to every provider. Replies are hints only.
async fn broadcast_attempt<I: LaneIo>(io: &I, tx_id: u64, attempt_index: usize, st: &mut RunState) {
    let Some(raw) = state::get_tx(tx_id).and_then(|t| t.attempts.get(attempt_index).map(|a| a.raw.clone())) else {
        return;
    };
    let results = io.send_raw(&raw).await;
    let summary = summarize_broadcast(&results);
    let now = env::now_ns();
    if let Some(mut t) = state::get_tx(tx_id) {
        if let Some(a) = t.attempts.get_mut(attempt_index) {
            a.broadcast_count = a.broadcast_count.saturating_add(1);
            a.last_broadcast_ns = Some(now);
            a.last_broadcast_note = Some(summary.clone());
            t.updated_at_ns = now;
            state::put_tx(&t);
        }
    }
    st.note(format!("tx {tx_id}: attempt {attempt_index} broadcast: {summary}"));
}

// ----- starting new work ----------------------------------------------------------

async fn step_new_work<I: LaneIo>(io: &I, st: &mut RunState) {
    match st.lane {
        LaneKind::Minter => start_mint(io, st).await,
        LaneKind::Admin => start_admin(io, st).await,
    }
}

/// Sync the lane nonce once from the chain: quorum `latest` and `pending` must agree.
async fn ensure_nonce<I: LaneIo>(io: &I, st: &mut RunState) -> Option<u64> {
    let lane = st.lane;
    let (known, in_flight) = state::read_core(|c| {
        let l = c.lane(lane);
        (l.next_nonce, l.unincluded_tx.is_some() || !l.unfinalized_txs.is_empty())
    });
    if known.is_some() {
        return known;
    }
    if in_flight {
        let reason = "next_nonce is unset although transactions are in flight";
        halt_lane(lane, reason);
        st.note(reason);
        return None;
    }
    let addr = match io.lane_address(lane).await {
        Ok(a) => a,
        Err(e) => {
            st.note(format!("lane address unavailable: {e}"));
            return None;
        }
    };
    let latest = match io.transaction_count(&addr, BlockTag::Latest).await {
        Ok(n) => n,
        Err(e) => {
            st.note(format!("nonce sync: latest count unavailable: {e}"));
            return None;
        }
    };
    let pending = match io.transaction_count(&addr, BlockTag::Pending).await {
        Ok(n) => n,
        Err(e) => {
            st.note(format!("nonce sync: pending count unavailable: {e}"));
            return None;
        }
    };
    if latest != pending {
        st.note(format!("nonce sync: latest ({latest}) and pending ({pending}) transaction counts disagree; waiting"));
        return None;
    }
    state::mutate_core(|c| {
        let l = c.lane_mut(lane);
        if l.next_nonce.is_none() {
            l.next_nonce = Some(latest);
        }
        l.next_nonce
    })
}

async fn start_mint<I: LaneIo>(io: &I, st: &mut RunState) {
    let lane = LaneKind::Minter;
    if let Err(why) = start_gate(&start_gate_for(lane)) {
        st.note(format!("no new mint: {why}"));
        return;
    }
    let Some(dep) = deposit::next_mintable_deposit() else { return };
    if ensure_nonce(io, st).await.is_none() {
        return;
    }
    let Some(simulated_gas) = preflight_mint(io, &dep, st).await else {
        return;
    };
    // No await from here on. The state may have changed during the preflight calls, so
    // `create_mint_tx` re-validates the deposit, the pauses, the lane and the gas limit
    // the simulation used in the same synchronous block that assigns the nonce.
    match create_mint_tx_with(dep.id, Some(simulated_gas), deposit::on_mint_tx_created) {
        Ok(tx_id) => {
            st.note(format!("tx {tx_id}: created for deposit {}", dep.id));
            drive_pending_tx(io, tx_id, st).await;
        }
        Err(e) => st.note(format!("mint for deposit {} abandoned without using a nonce: {e}", dep.id)),
    }
}

/// Local and on-chain checks before a mint gets a nonce. `Some(gas)` means go ahead with
/// a transaction carrying exactly that gas limit (the one the simulation used).
async fn preflight_mint<I: LaneIo>(io: &I, dep: &DepositRecord, st: &mut RunState) -> Option<u64> {
    let lane = LaneKind::Minter;
    let (contract, paused, halted, gas_limit) = state::read_core(|c| {
        (c.config.contract.as_ref().map(|b| b.address), c.config.pauses.mints, c.halted.is_some(), c.config.gas.mint_gas_limit)
    });
    let Some(contract) = contract else {
        st.note("preflight: contract is not bound");
        return None;
    };
    if paused || halted {
        st.note("preflight: mints are paused or the bridge is halted");
        return None;
    }
    let from = match io.lane_address(lane).await {
        Ok(a) => a,
        Err(e) => {
            st.note(format!("preflight: minter address unavailable: {e}"));
            return None;
        }
    };
    let consumed_call = abi::encode_view(&ViewCall::MintConsumed(dep.mint_id));
    let consumed = match io.eth_call(&contract, &consumed_call, BlockTag::Latest).await {
        Ok(ret) => match abi::decode_bool(&ret) {
            Ok(b) => b,
            Err(e) => {
                st.note(format!("preflight: undecodable mintConsumed reply: {e:?}"));
                return None;
            }
        },
        Err(e) => {
            st.note(format!("preflight: mintConsumed lookup failed: {e}"));
            return None;
        }
    };
    if consumed {
        // A mint with this id exists on Base but this canister never recorded delivering it.
        state::halt(format!(
            "deposit {}: mintConsumed({}) is true on Base but no mint is recorded",
            dep.id,
            hex0x(&dep.mint_id)
        ));
        st.note(format!("preflight: mint id of deposit {} is already consumed on Base; bridge halted", dep.id));
        return None;
    }
    let calldata = abi::encode_mint(&dep.base_recipient, dep.mint_amount_e8s, &dep.mint_id);
    // Simulated with the exact gas limit the transaction will carry, so a limit that is
    // too low is caught here (a hold that retries) instead of on chain (a paid revert).
    match io.simulate_call(&from, &contract, &calldata, gas_limit, BlockTag::Latest).await {
        Ok(SimulationResult::Ok(_)) => Some(gas_limit),
        Ok(SimulationResult::Reverted(data)) => {
            let reason = format!("mint simulation reverted (selector {})", revert_selector_hex(&data));
            deposit::on_mint_preflight_hold(dep.id, reason.clone());
            st.note(format!("deposit {}: {reason}", dep.id));
            None
        }
        Ok(SimulationResult::OutOfGas) => {
            let reason = format!("mint simulation ran out of gas at the configured limit of {gas_limit}");
            deposit::on_mint_preflight_hold(dep.id, reason.clone());
            state::append_audit(None, AuditCtx::deposit(dep.id, format!("{reason}; raise mint_gas_limit with admin_set_gas")));
            st.note(format!("deposit {}: {reason}", dep.id));
            None
        }
        Err(e) => {
            st.note(format!("preflight: mint simulation failed: {e}"));
            None
        }
    }
}

/// Create the unsigned mint transaction. Synchronous on purpose: the re-check of the
/// deposit, the pauses and the lane, the nonce assignment and the link to the deposit all
/// happen in one block, so a nonce is never consumed by a mint that cannot proceed.
#[cfg(test)]
fn create_mint_tx(deposit_id: u64) -> Result<u64, String> {
    create_mint_tx_with(deposit_id, None, deposit::on_mint_tx_created)
}

/// [`create_mint_tx`] with the deposit link injectable (tests exercise the rollback).
fn create_mint_tx_with(
    deposit_id: u64,
    simulated_gas_limit: Option<u64>,
    link: impl FnOnce(u64, u64) -> Result<(), String>,
) -> Result<u64, String> {
    // (1) Re-validate. Nothing is touched when any check fails (not even the tx id counter).
    let dep = state::get_deposit(deposit_id).ok_or("deposit not found")?;
    if !matches!(dep.state, DepositState::Escrowed) || dep.active_tx.is_some() {
        return Err("the deposit is no longer waiting for a mint".into());
    }
    let (contract, gas_limit, blocked) = state::read_core(|c| {
        (c.config.contract.as_ref().map(|b| b.address), c.config.gas.mint_gas_limit, c.halted.is_some() || c.config.pauses.mints)
    });
    let contract = contract.ok_or("the contract is not bound")?;
    if blocked {
        return Err("mints are paused or the bridge is halted".into());
    }
    if let Some(simulated) = simulated_gas_limit {
        if simulated != gas_limit {
            return Err(format!("the mint gas limit changed from {simulated} to {gas_limit} after the simulation"));
        }
    }
    start_gate(&start_gate_for(LaneKind::Minter)).map_err(|why| format!("the lane is not free: {why}"))?;
    if !state::read_core(|c| can_allocate(&c.minter_lane)) {
        return Err("no nonce is available (lane nonce not synced)".into());
    }
    // (2) Assign the nonce and make the tx current.
    let id = state::next_tx_id();
    let Some(nonce) = state::mutate_core(|c| begin_tx_on_lane(&mut c.minter_lane, id)) else {
        return Err("could not allocate a nonce".into());
    };
    // (3) Link the deposit. If that refuses, give the nonce back: nothing was signed and no
    // record was written, so no orphan transaction can ever be signed.
    if let Err(e) = link(deposit_id, id) {
        state::mutate_core(|c| {
            let l = &mut c.minter_lane;
            if l.unincluded_tx == Some(id) && l.next_nonce == Some(nonce + 1) {
                l.unincluded_tx = None;
                l.next_nonce = Some(nonce);
            }
        });
        state::append_audit(None, AuditCtx::deposit(deposit_id, format!("mint tx {id} rolled back, nonce {nonce} released: {e}")));
        return Err(format!("linking the deposit failed ({e}); nonce {nonce} released"));
    }
    let now = env::now_ns();
    state::put_tx(&EvmTxRecord {
        id,
        lane: LaneKind::Minter,
        nonce,
        purpose: TxPurpose::Mint { deposit_id },
        to: contract,
        value_wei: 0,
        data: abi::encode_mint(&dep.base_recipient, dep.mint_amount_e8s, &dep.mint_id),
        gas_limit,
        attempts: vec![],
        status: TxStatus::Unsigned,
        cancel_requested: false,
        outcome_applied: false,
        created_at_ns: now,
        updated_at_ns: now,
        last_checked_ns: None,
        note: None,
    });
    Ok(id)
}

async fn start_admin<I: LaneIo>(io: &I, st: &mut RunState) {
    let lane = LaneKind::Admin;
    if state::read_core(|c| c.admin_lane.queued_txs.is_empty()) {
        return;
    }
    if let Err(why) = start_gate(&start_gate_for(lane)) {
        st.note(format!("admin action waits: {why}"));
        return;
    }
    if ensure_nonce(io, st).await.is_none() {
        return;
    }
    // No await from here on: `begin_queued_admin_tx` re-checks the lane and assigns the
    // nonce in the same synchronous block.
    match begin_queued_admin_tx() {
        Ok(id) => {
            st.note(format!("tx {id}: started"));
            drive_pending_tx(io, id, st).await;
        }
        Err(e) => st.note(format!("admin action waits: {e}")),
    }
}

/// Pop the oldest queued admin tx, give it the lane's next nonce and make it current. The
/// nonce is assigned only here, in one synchronous block that also re-checks the lane,
/// so a halted or busy lane never consumes one.
fn begin_queued_admin_tx() -> Result<u64, String> {
    start_gate(&start_gate_for(LaneKind::Admin)).map_err(|why| format!("lane is not free: {why}"))?;
    let Some(id) = state::read_core(|c| c.admin_lane.queued_txs.first().copied()) else {
        return Err("the queue is empty".into());
    };
    let Some(mut tx) = state::get_tx(id) else {
        state::mutate_core(|c| c.admin_lane.queued_txs.retain(|x| *x != id));
        state::append_audit(None, AuditCtx::tx(id, "queued admin tx has no record; dropped from the queue"));
        return Err(format!("tx {id}: record missing; dropped from the queue"));
    };
    if !tx.attempts.is_empty() || tx.status != TxStatus::Unsigned {
        state::mutate_core(|c| c.admin_lane.queued_txs.retain(|x| *x != id));
        return Err(format!("tx {id}: already started; dropped from the queue"));
    }
    if !state::read_core(|c| can_allocate(&c.admin_lane)) {
        return Err("no nonce is available (lane nonce not synced)".into());
    }
    let nonce = state::mutate_core(|c| {
        c.admin_lane.queued_txs.retain(|x| *x != id);
        begin_tx_on_lane(&mut c.admin_lane, id)
    })
    .ok_or("could not allocate a nonce")?;
    tx.nonce = nonce;
    tx.updated_at_ns = env::now_ns();
    state::put_tx(&tx);
    state::append_audit(None, AuditCtx::tx(id, format!("admin tx assigned nonce {nonce}")));
    Ok(id)
}

// ----- clear_lane_halt ------------------------------------------------------------

pub(crate) async fn clear_lane_halt_with<I: LaneIo>(io: &I, lane: LaneKind) -> BridgeResult<()> {
    let (halted, unincluded, unfinalized, next_nonce) = state::read_core(|c| {
        let l = c.lane(lane);
        (l.halted.clone(), l.unincluded_tx, l.unfinalized_txs.len(), l.next_nonce)
    });
    if halted.is_none() {
        return Err(BridgeError::InvalidState(format!("the {} lane is not halted", lane_label(lane))));
    }
    if unincluded.is_some() || unfinalized > 0 {
        return Err(BridgeError::InvalidState(format!(
            "the {} lane still has transactions in flight (unincluded: {}, unfinalized: {unfinalized})",
            lane_label(lane),
            unincluded.is_some()
        )));
    }
    let Some(next) = next_nonce else {
        return Err(BridgeError::InvalidState("the lane nonce has never been synced".into()));
    };
    let addr = io.lane_address(lane).await.map_err(BridgeError::Internal)?;
    let finalized = io
        .transaction_count(&addr, BlockTag::Finalized)
        .await
        .map_err(|e| BridgeError::Rpc(format!("finalized transaction count: {e}")))?;
    let latest = io
        .transaction_count(&addr, BlockTag::Latest)
        .await
        .map_err(|e| BridgeError::Rpc(format!("latest transaction count: {e}")))?;
    if finalized != next || latest != next {
        return Err(BridgeError::InvalidState(format!(
            "chain disagrees with local records: finalized count {finalized}, latest count {latest}, local next nonce {next}"
        )));
    }
    let cleared = state::mutate_core(|c| {
        let l = c.lane_mut(lane);
        if l.halted.is_some() && l.unincluded_tx.is_none() && l.unfinalized_txs.is_empty() && l.next_nonce == Some(next) {
            l.halted = None;
            true
        } else {
            false
        }
    });
    if !cleared {
        return Err(BridgeError::InvalidState("the lane changed during the check; try again".into()));
    }
    state::append_audit(
        None,
        AuditCtx::note(format!(
            "{} lane halt cleared (nonce {next} agrees with the chain): {}",
            lane_label(lane),
            halted.unwrap_or_default()
        )),
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::test_env;
    use crate::ids::{keccak256, word_address, word_u64};
    use crate::state::DepositKind;
    use candid::Principal;
    use std::collections::BTreeMap;
    use std::future::Future;
    use std::task::{Context, Poll, Waker};

    const E8: u64 = 100_000_000;
    const MINTER: [u8; 20] = [0x11; 20];
    const ADMIN: [u8; 20] = [0x22; 20];
    const CONTRACT: [u8; 20] = [0xcc; 20];
    const NOW: u64 = 1_700_000_000_000_000_000;

    fn secs(s: u64) -> u64 {
        s * NS_PER_SEC
    }

    fn block_on<F: Future>(f: F) -> F::Output {
        let mut f = std::pin::pin!(f);
        let mut cx = Context::from_waker(Waker::noop());
        for _ in 0..10_000 {
            if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                return v;
            }
        }
        panic!("future did not complete (mock I/O must never pend)");
    }

    // ----- builders for the pure tests --------------------------------------------

    fn gas_cfg() -> GasSettings {
        GasSettings {
            max_fee_per_gas_wei: 2_000_000_000,
            max_priority_fee_per_gas_wei: 100_000_000,
            mint_gas_limit: 150_000,
            admin_gas_limit: 100_000,
            min_gas_reserve_wei: 1_000,
            replace_after_secs: 90,
        }
    }

    fn attempt(kind: AttemptKind, tag: u8, max_fee: u128, prio: u128, created: u64, last_broadcast: Option<u64>) -> TxAttempt {
        TxAttempt {
            kind,
            to: CONTRACT,
            value_wei: 0,
            data: vec![1, 2, 3],
            max_fee_per_gas: max_fee,
            max_priority_fee_per_gas: prio,
            gas_limit: 150_000,
            raw: vec![0x02, tag],
            tx_hash: [tag; 32],
            created_at_ns: created,
            broadcast_count: u32::from(last_broadcast.is_some()),
            last_broadcast_ns: last_broadcast,
            last_broadcast_note: None,
        }
    }

    fn base_tx(nonce: u64, attempts: Vec<TxAttempt>) -> EvmTxRecord {
        EvmTxRecord {
            id: 5,
            lane: LaneKind::Minter,
            nonce,
            purpose: TxPurpose::Mint { deposit_id: 0 },
            to: CONTRACT,
            value_wei: 0,
            data: vec![1, 2, 3],
            gas_limit: 150_000,
            status: if attempts.is_empty() { TxStatus::Unsigned } else { TxStatus::Pending },
            attempts,
            cancel_requested: false,
            outcome_applied: false,
            created_at_ns: NOW,
            updated_at_ns: NOW,
            last_checked_ns: None,
            note: None,
        }
    }

    fn receipt_for(tag: u8, block: u64, bhash: u8, success: bool) -> Receipt {
        Receipt {
            tx_hash: [tag; 32],
            success,
            block_number: block,
            block_hash: [bhash; 32],
            from: MINTER,
            to: Some(CONTRACT),
            logs: vec![],
        }
    }

    fn obs(now: u64, receipts: Vec<ReceiptObs>, latest: Option<u64>, finalized: Option<u64>) -> PendingObs {
        PendingObs { now_ns: now, receipts, latest_nonce: latest, finalized_nonce: finalized }
    }

    fn open() -> Policy {
        Policy::default()
    }

    // ----- fee math ------------------------------------------------------------------

    #[test]
    fn bump_is_twelve_and_a_half_percent_rounded_up_and_at_least_one_wei() {
        assert_eq!(bump_once(0), 1);
        assert_eq!(bump_once(1), 2);
        assert_eq!(bump_once(7), 8);
        assert_eq!(bump_once(8), 9);
        assert_eq!(bump_once(9), 11); // 9 + ceil(9/8)
        assert_eq!(bump_once(1_000), 1_125);
        assert_eq!(bump_once(1_001), 1_127); // 1001 + ceil(125.125)
        assert_eq!(bump_once(u128::MAX), u128::MAX);
        for x in [1u128, 2, 3, 99, 1_000_003, 12_345_678_901] {
            // At least 12.5% and at least the 10% a node demands.
            assert!(bump_once(x) * 8 >= x * 9);
            assert!(bump_once(x) >= node_minimum_bump(x));
            assert!(bump_once(x) > x);
        }
    }

    #[test]
    fn initial_fees_follow_the_estimate_under_the_caps() {
        let caps = gas_cfg();
        let f = initial_fees(&FeeEstimate { base_fee_per_gas: 1_000, priority_fee_per_gas: 50 }, &caps);
        assert_eq!(f, Fees { max_fee_per_gas: 2_050, max_priority_fee_per_gas: 50 });
        // Priority capped.
        let f = initial_fees(&FeeEstimate { base_fee_per_gas: 1_000, priority_fee_per_gas: 999_999_999_999 }, &caps);
        assert_eq!(f.max_priority_fee_per_gas, 100_000_000);
        assert_eq!(f.max_fee_per_gas, 100_002_000);
        // Max fee capped.
        let f = initial_fees(&FeeEstimate { base_fee_per_gas: 5_000_000_000, priority_fee_per_gas: 10 }, &caps);
        assert_eq!(f.max_fee_per_gas, 2_000_000_000);
        // A cap below the priority fee pulls the priority down so max_fee >= priority.
        let mut weird = gas_cfg();
        weird.max_fee_per_gas_wei = 40;
        let f = initial_fees(&FeeEstimate { base_fee_per_gas: 1_000, priority_fee_per_gas: 90 }, &weird);
        assert_eq!(f, Fees { max_fee_per_gas: 40, max_priority_fee_per_gas: 40 });
        // Saturating arithmetic.
        let f = initial_fees(&FeeEstimate { base_fee_per_gas: u128::MAX, priority_fee_per_gas: u128::MAX }, &caps);
        assert_eq!(f.max_fee_per_gas, caps.max_fee_per_gas_wei);
        assert!(f.max_fee_per_gas >= f.max_priority_fee_per_gas);
    }

    #[test]
    fn bumped_fees_raise_both_fields_under_the_caps() {
        let caps = gas_cfg();
        let f = bumped_fees(Fees { max_fee_per_gas: 1_000, max_priority_fee_per_gas: 80 }, &caps).unwrap();
        assert_eq!(f, Fees { max_fee_per_gas: 1_125, max_priority_fee_per_gas: 90 });
        // Zero priority still rises (geth needs a strict increase).
        let f = bumped_fees(Fees { max_fee_per_gas: 1_000, max_priority_fee_per_gas: 0 }, &caps).unwrap();
        assert_eq!(f.max_priority_fee_per_gas, 1);
    }

    #[test]
    fn bumped_fees_clip_to_the_caps_and_stop_at_them() {
        let caps = gas_cfg();
        // Max fee needs 12.5% but the cap leaves less than the 10% a node accepts: no replacement.
        let at_cap = Fees { max_fee_per_gas: caps.max_fee_per_gas_wei, max_priority_fee_per_gas: 10 };
        assert_eq!(bumped_fees(at_cap, &caps), None);
        // Priority already at its cap while the max fee has room: still no valid replacement.
        let prio_cap = Fees { max_fee_per_gas: 1_000_000, max_priority_fee_per_gas: caps.max_priority_fee_per_gas_wei };
        assert_eq!(bumped_fees(prio_cap, &caps), None);
        // Clipped but still at least 10% above: allowed, and never above the cap.
        let near = Fees { max_fee_per_gas: 1_900_000_000, max_priority_fee_per_gas: 1_000 };
        // 12.5% would be 2.1375 gwei; the cap clips it to 2 gwei, which is only +5.2%: below the 10% a node needs.
        assert_eq!(bumped_fees(near, &caps), None);
        let near = Fees { max_fee_per_gas: 1_800_000_000, max_priority_fee_per_gas: 1_000 };
        // 12.5% would be 2.025 gwei; clipped to 2 gwei, which is +11.1%: fine.
        let f = bumped_fees(near, &caps).unwrap();
        assert_eq!(f.max_fee_per_gas, caps.max_fee_per_gas_wei);
        assert_eq!(f.max_priority_fee_per_gas, 1_125);
    }

    #[test]
    fn bumped_fees_never_exceed_caps_when_previous_is_above_them() {
        let caps = gas_cfg();
        let above = Fees { max_fee_per_gas: caps.max_fee_per_gas_wei + 5, max_priority_fee_per_gas: 10 };
        assert_eq!(bumped_fees(above, &caps), None);
    }

    #[test]
    fn gas_check_includes_reserve_and_value() {
        assert_eq!(gas_required(0, 150_000, 2_000, 1_000), 300_001_000);
        assert!(gas_sufficient(300_001_000, 0, 150_000, 2_000, 1_000));
        assert!(!gas_sufficient(300_000_999, 0, 150_000, 2_000, 1_000));
        assert!(!gas_sufficient(300_001_000, 1, 150_000, 2_000, 1_000));
        assert_eq!(gas_required(u128::MAX, u64::MAX, u128::MAX, u128::MAX), u128::MAX);
    }

    #[test]
    fn cancel_attempt_is_a_zero_value_self_transfer() {
        let tx = base_tx(3, vec![]);
        let own = [0x77; 20];
        let (to, value, data, gas) = attempt_fields(&tx, AttemptKind::Cancel, own);
        assert_eq!((to, value, data.len(), gas), (own, 0, 0, 21_000));
        let (to, value, data, gas) = attempt_fields(&tx, AttemptKind::Original, own);
        assert_eq!((to, value, data, gas), (CONTRACT, 0, vec![1, 2, 3], 150_000));
        assert_eq!(attempt_fields(&tx, AttemptKind::Replacement, own).0, CONTRACT);
    }

    #[test]
    fn revert_selector_is_the_first_four_bytes() {
        assert_eq!(revert_selector_hex(&[0x08, 0xc3, 0x79, 0xa0, 0, 0]), "0x08c379a0");
        assert_eq!(revert_selector_hex(&[1, 2, 3]), "no revert selector");
        assert_eq!(revert_selector_hex(&[]), "no revert selector");
    }

    // ----- decide_pending -------------------------------------------------------------

    #[test]
    fn no_attempt_means_sign_the_same_nonce_again() {
        // Covers the lost signature reply: nothing was persisted, so the decision is identical every time.
        let tx = base_tx(9, vec![]);
        for _ in 0..3 {
            assert_eq!(
                decide_pending(&tx, &obs(NOW, vec![], None, None), &gas_cfg(), &open()),
                PendingAction::SignFirst { kind: AttemptKind::Original }
            );
        }
    }

    #[test]
    fn unsigned_tx_waits_while_mints_are_blocked_or_lane_halted_but_cancel_is_still_signed() {
        let mut tx = base_tx(0, vec![]);
        let blocked = Policy { mint_blocked: true, lane_halted: false };
        assert!(matches!(decide_pending(&tx, &obs(NOW, vec![], None, None), &gas_cfg(), &blocked), PendingAction::Wait(_)));
        tx.cancel_requested = true;
        assert_eq!(
            decide_pending(&tx, &obs(NOW, vec![], None, None), &gas_cfg(), &blocked),
            PendingAction::SignFirst { kind: AttemptKind::Cancel }
        );
        let halted = Policy { mint_blocked: false, lane_halted: true };
        assert!(matches!(decide_pending(&tx, &obs(NOW, vec![], None, None), &gas_cfg(), &halted), PendingAction::Wait(_)));
    }

    #[test]
    fn broadcast_error_keeps_the_attempt_and_rebroadcasts_the_same_bytes_without_a_new_nonce() {
        // Attempt persisted, broadcast never recorded (error or trap): rebroadcast right away.
        let tx = base_tx(4, vec![attempt(AttemptKind::Original, 1, 1_000, 10, NOW, None)]);
        let o = obs(NOW + secs(1), vec![ReceiptObs::NotFound], None, None);
        assert_eq!(decide_pending(&tx, &o, &gas_cfg(), &open()), PendingAction::Rebroadcast { reason: "awaiting inclusion" });
        // Recently broadcast: wait.
        let tx = base_tx(4, vec![attempt(AttemptKind::Original, 1, 1_000, 10, NOW, Some(NOW))]);
        let o = obs(NOW + secs(29), vec![ReceiptObs::NotFound], None, None);
        assert_eq!(decide_pending(&tx, &o, &gas_cfg(), &open()), PendingAction::Wait("awaiting inclusion"));
        // 30 s later: rebroadcast.
        let o = obs(NOW + secs(30), vec![ReceiptObs::NotFound], None, None);
        assert!(matches!(decide_pending(&tx, &o, &gas_cfg(), &open()), PendingAction::Rebroadcast { .. }));
    }

    #[test]
    fn receipt_for_the_first_attempt_wins_after_a_replacement_exists() {
        let tx = base_tx(
            4,
            vec![
                attempt(AttemptKind::Original, 1, 1_000, 10, NOW, Some(NOW)),
                attempt(AttemptKind::Replacement, 2, 1_125, 12, NOW + secs(100), Some(NOW + secs(100))),
            ],
        );
        let o = obs(
            NOW + secs(110),
            vec![ReceiptObs::Found(receipt_for(1, 55, 0xb1, true)), ReceiptObs::NotFound],
            None,
            None,
        );
        assert_eq!(
            decide_pending(&tx, &o, &gas_cfg(), &open()),
            PendingAction::Included { attempt: 0, block_number: 55, block_hash: [0xb1; 32], success: true }
        );
        // And the newest attempt can win as well.
        let o = obs(
            NOW + secs(110),
            vec![ReceiptObs::NotFound, ReceiptObs::Found(receipt_for(2, 56, 0xb2, false))],
            None,
            None,
        );
        assert_eq!(
            decide_pending(&tx, &o, &gas_cfg(), &open()),
            PendingAction::Included { attempt: 1, block_number: 56, block_hash: [0xb2; 32], success: false }
        );
    }

    #[test]
    fn two_receipts_for_one_nonce_are_an_inconsistency_not_a_decision() {
        let tx = base_tx(
            4,
            vec![
                attempt(AttemptKind::Original, 1, 1_000, 10, NOW, Some(NOW)),
                attempt(AttemptKind::Replacement, 2, 1_125, 12, NOW + secs(100), Some(NOW)),
            ],
        );
        let o = obs(
            NOW + secs(200),
            vec![ReceiptObs::Found(receipt_for(1, 55, 0xb1, true)), ReceiptObs::Found(receipt_for(2, 56, 0xb2, true))],
            Some(4),
            Some(0),
        );
        assert!(matches!(decide_pending(&tx, &o, &gas_cfg(), &open()), PendingAction::Wait(_)));
    }

    #[test]
    fn no_replacement_before_replace_after_secs_even_if_latest_equals_the_nonce() {
        let tx = base_tx(4, vec![attempt(AttemptKind::Original, 1, 1_000, 10, NOW, Some(NOW + secs(60)))]);
        let o = obs(NOW + secs(89), vec![ReceiptObs::NotFound], Some(4), Some(4));
        // Not even a nonce lookup is requested yet.
        assert!(!matches!(
            decide_pending(&tx, &o, &gas_cfg(), &open()),
            PendingAction::NeedNonces | PendingAction::SignBump { .. } | PendingAction::Halt(_)
        ));
    }

    #[test]
    fn old_attempt_asks_for_nonces_then_replaces_only_when_latest_equals_the_nonce() {
        let tx = base_tx(4, vec![attempt(AttemptKind::Original, 1, 1_000, 10, NOW, Some(NOW + secs(60)))]);
        let now = NOW + secs(90);
        assert_eq!(
            decide_pending(&tx, &obs(now, vec![ReceiptObs::NotFound], None, None), &gas_cfg(), &open()),
            PendingAction::NeedNonces
        );
        let action = decide_pending(&tx, &obs(now, vec![ReceiptObs::NotFound], Some(4), Some(4)), &gas_cfg(), &open());
        assert_eq!(
            action,
            PendingAction::SignBump {
                kind: AttemptKind::Replacement,
                fees: Fees { max_fee_per_gas: 1_125, max_priority_fee_per_gas: 12 }
            }
        );
    }

    #[test]
    fn latest_nonce_past_the_tx_nonce_blocks_replacement() {
        // One of our attempts was included but receipts lag: do NOT replace.
        let tx = base_tx(4, vec![attempt(AttemptKind::Original, 1, 1_000, 10, NOW, Some(NOW))]);
        let o = obs(NOW + secs(500), vec![ReceiptObs::NotFound], Some(5), Some(4));
        assert!(matches!(decide_pending(&tx, &o, &gas_cfg(), &open()), PendingAction::Wait(_)));
    }

    #[test]
    fn latest_nonce_behind_the_tx_nonce_never_replaces() {
        let tx = base_tx(4, vec![attempt(AttemptKind::Original, 1, 1_000, 10, NOW, Some(NOW))]);
        let o = obs(NOW + secs(500), vec![ReceiptObs::NotFound], Some(3), Some(3));
        let a = decide_pending(&tx, &o, &gas_cfg(), &open());
        assert!(matches!(a, PendingAction::Rebroadcast { .. }), "{a:?}");
    }

    #[test]
    fn nonce_finalized_with_no_receipt_halts_and_never_resigns() {
        let tx = base_tx(4, vec![attempt(AttemptKind::Original, 1, 1_000, 10, NOW, Some(NOW))]);
        let o = obs(NOW + secs(500), vec![ReceiptObs::NotFound], Some(5), Some(5));
        match decide_pending(&tx, &o, &gas_cfg(), &open()) {
            PendingAction::Halt(reason) => {
                assert!(reason.contains("nonce 4"), "{reason}");
                assert!(reason.contains("tx 5"), "{reason}");
            }
            other => panic!("expected halt, got {other:?}"),
        }
    }

    #[test]
    fn failed_receipt_lookup_blocks_halt_and_replacement() {
        let tx = base_tx(4, vec![attempt(AttemptKind::Original, 1, 1_000, 10, NOW, Some(NOW))]);
        let o = obs(NOW + secs(500), vec![ReceiptObs::Unknown], Some(5), Some(5));
        let a = decide_pending(&tx, &o, &gas_cfg(), &open());
        assert!(matches!(a, PendingAction::Rebroadcast { .. } | PendingAction::Wait(_)), "{a:?}");
        let o = obs(NOW + secs(500), vec![ReceiptObs::Unknown], Some(4), Some(4));
        let a = decide_pending(&tx, &o, &gas_cfg(), &open());
        assert!(matches!(a, PendingAction::Rebroadcast { .. } | PendingAction::Wait(_)), "{a:?}");
        // A missing lookup entry is treated like a failed one.
        let o = obs(NOW + secs(500), vec![], Some(4), Some(4));
        let a = decide_pending(&tx, &o, &gas_cfg(), &open());
        assert!(matches!(a, PendingAction::Rebroadcast { .. } | PendingAction::Wait(_)), "{a:?}");
    }

    #[test]
    fn at_the_fee_cap_only_rebroadcasts() {
        let caps = gas_cfg();
        let tx = base_tx(
            4,
            vec![attempt(AttemptKind::Replacement, 1, caps.max_fee_per_gas_wei, caps.max_priority_fee_per_gas_wei, NOW, Some(NOW))],
        );
        let late = NOW + secs(500);
        let o = obs(late, vec![ReceiptObs::NotFound], Some(4), Some(4));
        let a = decide_pending(&tx, &o, &caps, &open());
        assert!(matches!(a, PendingAction::Rebroadcast { reason } if reason.contains("cap")), "{a:?}");
        // When the last broadcast was recent it just waits.
        let tx = base_tx(
            4,
            vec![attempt(AttemptKind::Replacement, 1, caps.max_fee_per_gas_wei, caps.max_priority_fee_per_gas_wei, NOW, Some(late))],
        );
        let a = decide_pending(&tx, &o, &caps, &open());
        assert!(matches!(a, PendingAction::Wait(reason) if reason.contains("cap")), "{a:?}");
    }

    #[test]
    fn attempt_limit_stops_replacements() {
        let attempts: Vec<TxAttempt> =
            (0..MAX_ATTEMPTS_PER_TX).map(|i| attempt(AttemptKind::Replacement, i as u8, 1_000, 10, NOW, Some(NOW))).collect();
        let tx = base_tx(4, attempts);
        let o = obs(NOW + secs(500), vec![ReceiptObs::NotFound; MAX_ATTEMPTS_PER_TX], Some(4), Some(4));
        let a = decide_pending(&tx, &o, &gas_cfg(), &open());
        assert!(matches!(a, PendingAction::Rebroadcast { .. }), "{a:?}");
    }

    #[test]
    fn halted_lane_does_not_replace() {
        let tx = base_tx(4, vec![attempt(AttemptKind::Original, 1, 1_000, 10, NOW, Some(NOW))]);
        let o = obs(NOW + secs(500), vec![ReceiptObs::NotFound], Some(4), Some(4));
        let halted = Policy { lane_halted: true, mint_blocked: false };
        let a = decide_pending(&tx, &o, &gas_cfg(), &halted);
        assert!(matches!(a, PendingAction::Rebroadcast { .. }), "{a:?}");
    }

    #[test]
    fn cancel_request_turns_the_next_attempt_into_a_cancel_bumped_over_the_highest_fees() {
        let mut tx = base_tx(
            4,
            vec![
                // Fields are not in the same attempt: priority is highest in attempt 0, max fee in attempt 1.
                attempt(AttemptKind::Original, 1, 1_000, 200, NOW, Some(NOW)),
                attempt(AttemptKind::Replacement, 2, 1_800, 100, NOW + secs(100), Some(NOW + secs(100))),
            ],
        );
        tx.cancel_requested = true;
        let o = obs(NOW + secs(300), vec![ReceiptObs::NotFound, ReceiptObs::NotFound], Some(4), Some(4));
        assert_eq!(
            decide_pending(&tx, &o, &gas_cfg(), &open()),
            PendingAction::SignBump {
                kind: AttemptKind::Cancel,
                fees: Fees { max_fee_per_gas: 2_025, max_priority_fee_per_gas: 225 }
            }
        );
    }

    #[test]
    fn cancel_on_a_pending_tx_still_waits_for_the_age_gate_and_the_nonce_check() {
        let mut tx = base_tx(4, vec![attempt(AttemptKind::Original, 1, 1_000, 10, NOW, Some(NOW))]);
        tx.cancel_requested = true;
        let young = obs(NOW + secs(10), vec![ReceiptObs::NotFound], Some(4), Some(4));
        assert!(!matches!(decide_pending(&tx, &young, &gas_cfg(), &open()), PendingAction::SignBump { .. }));
        let consumed = obs(NOW + secs(300), vec![ReceiptObs::NotFound], Some(5), Some(4));
        assert!(!matches!(decide_pending(&tx, &consumed, &gas_cfg(), &open()), PendingAction::SignBump { .. }));
    }

    // ----- finality (pure) ------------------------------------------------------------

    fn included_tx(attempts: Vec<TxAttempt>, attempt: u32, block: u64, bhash: u8, success: bool) -> EvmTxRecord {
        let mut tx = base_tx(4, attempts);
        tx.status = TxStatus::Included { attempt, block_number: block, block_hash: [bhash; 32], success };
        tx
    }

    #[test]
    fn finality_precheck_compares_with_the_finalized_head() {
        let tx = included_tx(vec![attempt(AttemptKind::Original, 1, 1, 1, NOW, None)], 0, 100, 0xb1, true);
        assert_eq!(finality_precheck(&tx, 99), FinalityPrecheck::NotYet);
        assert_eq!(finality_precheck(&tx, 100), FinalityPrecheck::Confirm);
        assert_eq!(finality_precheck(&tx, 101), FinalityPrecheck::Confirm);
        let pending = base_tx(4, vec![attempt(AttemptKind::Original, 1, 1, 1, NOW, None)]);
        assert_eq!(finality_precheck(&pending, u64::MAX), FinalityPrecheck::NotYet);
    }

    #[test]
    fn finalizes_only_when_receipt_and_canonical_hash_match() {
        let tx = included_tx(vec![attempt(AttemptKind::Original, 1, 1, 1, NOW, None)], 0, 100, 0xb1, true);
        let rc = receipt_for(1, 100, 0xb1, true);
        assert_eq!(decide_finality(&tx, Some(&rc), None), FinalityAction::NeedCanonical);
        assert_eq!(
            decide_finality(&tx, Some(&rc), Some([0xb1; 32])),
            FinalityAction::Finalize { attempt: 0, block_number: 100, block_hash: [0xb1; 32], success: true }
        );
    }

    #[test]
    fn reorg_before_finality_is_detected_three_ways() {
        let tx = included_tx(vec![attempt(AttemptKind::Original, 1, 1, 1, NOW, None)], 0, 100, 0xb1, true);
        // Receipt gone.
        assert!(matches!(decide_finality(&tx, None, None), FinalityAction::Reorged(_)));
        // Receipt in another block (hash changed, or number changed).
        let moved = receipt_for(1, 100, 0xb9, true);
        assert!(matches!(decide_finality(&tx, Some(&moved), Some([0xb9; 32])), FinalityAction::Reorged(_)));
        let moved = receipt_for(1, 101, 0xb1, true);
        assert!(matches!(decide_finality(&tx, Some(&moved), Some([0xb1; 32])), FinalityAction::Reorged(_)));
        // Receipt unchanged but the block at that height is no longer canonical.
        let same = receipt_for(1, 100, 0xb1, true);
        assert!(matches!(decide_finality(&tx, Some(&same), Some([0xee; 32])), FinalityAction::Reorged(_)));
        // A receipt for another hash is no confirmation.
        let wrong = receipt_for(7, 100, 0xb1, true);
        assert!(matches!(decide_finality(&tx, Some(&wrong), Some([0xb1; 32])), FinalityAction::Reorged(_)));
    }

    #[test]
    fn finality_uses_the_hash_of_the_included_attempt_not_the_newest() {
        let tx = included_tx(
            vec![attempt(AttemptKind::Original, 1, 1, 1, NOW, None), attempt(AttemptKind::Replacement, 2, 2, 2, NOW, None)],
            0,
            100,
            0xb1,
            true,
        );
        assert!(matches!(
            decide_finality(&tx, Some(&receipt_for(1, 100, 0xb1, true)), Some([0xb1; 32])),
            FinalityAction::Finalize { attempt: 0, .. }
        ));
        assert!(matches!(
            decide_finality(&tx, Some(&receipt_for(2, 100, 0xb1, true)), Some([0xb1; 32])),
            FinalityAction::Reorged(_)
        ));
    }

    #[test]
    fn changed_status_for_the_same_block_hash_halts() {
        let tx = included_tx(vec![attempt(AttemptKind::Original, 1, 1, 1, NOW, None)], 0, 100, 0xb1, true);
        let flipped = receipt_for(1, 100, 0xb1, false);
        assert!(matches!(decide_finality(&tx, Some(&flipped), Some([0xb1; 32])), FinalityAction::Halt(_)));
    }

    #[test]
    fn mint_outcome_maps_cancel_success_and_revert() {
        let rc = receipt_for(1, 1, 1, true);
        assert!(matches!(mint_outcome(Some(AttemptKind::Cancel), true, rc.clone()), MintTxOutcome::Cancelled(_)));
        assert!(matches!(mint_outcome(Some(AttemptKind::Original), true, rc.clone()), MintTxOutcome::Executed(_)));
        assert!(matches!(mint_outcome(Some(AttemptKind::Replacement), true, rc.clone()), MintTxOutcome::Executed(_)));
        assert!(matches!(mint_outcome(Some(AttemptKind::Original), false, rc.clone()), MintTxOutcome::Reverted(_)));
        assert!(matches!(mint_outcome(Some(AttemptKind::Replacement), false, rc.clone()), MintTxOutcome::Reverted(_)));
        // A cancel is a cancel even if the receipt flag were false.
        assert!(matches!(mint_outcome(Some(AttemptKind::Cancel), false, rc), MintTxOutcome::Cancelled(_)));
    }

    // ----- gates and nonce allocation -------------------------------------------------

    #[test]
    fn start_gate_enforces_every_back_pressure_rule() {
        let ok = StartGate::default();
        assert_eq!(start_gate(&ok), Ok(()));
        assert!(start_gate(&StartGate { lane_halted: true, ..ok }).is_err());
        assert!(start_gate(&StartGate { awaiting_gas: true, ..ok }).is_err());
        assert!(start_gate(&StartGate { has_unincluded: true, ..ok }).is_err());
        assert!(start_gate(&StartGate { pending_in_unfinalized: true, ..ok }).is_err());
        assert_eq!(start_gate(&StartGate { unfinalized: MAX_UNFINALIZED - 1, ..ok }), Ok(()));
        assert!(start_gate(&StartGate { unfinalized: MAX_UNFINALIZED, ..ok }).is_err());
        assert!(start_gate(&StartGate { unfinalized: MAX_UNFINALIZED + 3, ..ok }).is_err());
        assert_eq!(MAX_UNFINALIZED, 8);
    }

    #[test]
    fn begin_tx_on_lane_assigns_sequential_nonces_without_rereading_the_chain() {
        let mut lane = LaneState::default();
        assert!(!can_allocate(&lane));
        assert_eq!(begin_tx_on_lane(&mut lane, 1), None);
        assert_eq!(lane, LaneState::default(), "failure changes nothing");
        lane.next_nonce = Some(7);
        assert!(can_allocate(&lane));
        assert_eq!(begin_tx_on_lane(&mut lane, 10), Some(7));
        assert_eq!((lane.next_nonce, lane.unincluded_tx), (Some(8), Some(10)));
        lane.unincluded_tx = None;
        assert_eq!(begin_tx_on_lane(&mut lane, 11), Some(8));
        // Overflow guard.
        lane.next_nonce = Some(u64::MAX);
        assert!(!can_allocate(&lane));
        assert_eq!(begin_tx_on_lane(&mut lane, 12), None);
        assert_eq!(lane.next_nonce, Some(u64::MAX));
    }

    // ----- in-memory chain for driver tests ---------------------------------------------

    #[derive(Default)]
    struct Counts {
        latest: u64,
        pending: u64,
        finalized: u64,
    }

    struct Chain {
        counts: BTreeMap<[u8; 20], Counts>,
        balance: u128,
        fee: FeeEstimate,
        receipts: BTreeMap<[u8; 32], Receipt>,
        canonical: BTreeMap<u64, [u8; 32]>,
        finalized_number: u64,
        sent: Vec<Vec<u8>>,
        send_error: bool,
        sign_counter: u64,
        fail_signs: u32,
        signed: Vec<Eip1559Tx>,
        rpc_down: bool,
        simulation: SimulationResult,
        /// Gas the mock mint needs; a simulation with less answers OutOfGas.
        mint_gas_needed: Option<u64>,
        simulated_gas: Vec<u64>,
        consumed: bool,
        receipt_calls: u32,
    }

    struct MockIo(RefCell<Chain>);

    impl MockIo {
        fn new() -> Self {
            let mut counts = BTreeMap::new();
            counts.insert(MINTER, Counts::default());
            counts.insert(ADMIN, Counts::default());
            MockIo(RefCell::new(Chain {
                counts,
                balance: 10_000_000_000_000_000_000,
                fee: FeeEstimate { base_fee_per_gas: 1_000, priority_fee_per_gas: 50 },
                receipts: BTreeMap::new(),
                canonical: BTreeMap::new(),
                finalized_number: 0,
                sent: vec![],
                send_error: false,
                sign_counter: 0,
                fail_signs: 0,
                signed: vec![],
                rpc_down: false,
                simulation: SimulationResult::Ok(vec![]),
                mint_gas_needed: None,
                simulated_gas: vec![],
                consumed: false,
                receipt_calls: 0,
            }))
        }

        fn chain(&self) -> std::cell::RefMut<'_, Chain> {
            self.0.borrow_mut()
        }

        /// A block containing `hash` appears at `number` with `block_hash`.
        fn include(&self, hash: [u8; 32], number: u64, block_hash: u8, success: bool, logs: Vec<abi::RawLog>, addr: [u8; 20]) {
            let mut c = self.chain();
            c.receipts.insert(
                hash,
                Receipt { tx_hash: hash, success, block_number: number, block_hash: [block_hash; 32], from: addr, to: Some(CONTRACT), logs },
            );
            c.canonical.insert(number, [block_hash; 32]);
            let n = c.counts.entry(addr).or_default();
            n.latest += 1;
            n.pending = n.pending.max(n.latest);
        }

        fn finalize_up_to(&self, number: u64, addr: [u8; 20]) {
            let mut c = self.chain();
            c.finalized_number = number;
            let n = c.counts.entry(addr).or_default();
            n.finalized = n.latest;
        }

        fn drop_receipt(&self, hash: [u8; 32]) {
            let mut c = self.chain();
            c.receipts.remove(&hash);
        }
    }

    fn lane_idx(addr: &[u8; 20]) -> LaneKind {
        if *addr == MINTER {
            LaneKind::Minter
        } else {
            LaneKind::Admin
        }
    }

    impl LaneIo for MockIo {
        async fn lane_address(&self, lane: LaneKind) -> Result<[u8; 20], String> {
            Ok(match lane {
                LaneKind::Minter => MINTER,
                LaneKind::Admin => ADMIN,
            })
        }
        async fn transaction_count(&self, address: &[u8; 20], tag: BlockTag) -> Result<u64, RpcError> {
            let c = self.0.borrow();
            if c.rpc_down {
                return Err(RpcError::AllFailed("down".into()));
            }
            let n = c.counts.get(address).ok_or_else(|| RpcError::Decode(format!("unknown {:?}", lane_idx(address))))?;
            Ok(match tag {
                BlockTag::Pending => n.pending,
                BlockTag::Finalized => n.finalized,
                _ => n.latest,
            })
        }
        async fn balance(&self, _address: &[u8; 20], _tag: BlockTag) -> Result<u128, RpcError> {
            let c = self.0.borrow();
            if c.rpc_down {
                return Err(RpcError::AllFailed("down".into()));
            }
            Ok(c.balance)
        }
        async fn fee_estimate(&self) -> Result<FeeEstimate, RpcError> {
            let c = self.0.borrow();
            if c.rpc_down {
                return Err(RpcError::AllFailed("down".into()));
            }
            Ok(c.fee)
        }
        async fn get_receipt(&self, tx_hash: &[u8; 32]) -> Result<Option<Receipt>, RpcError> {
            let mut c = self.0.borrow_mut();
            c.receipt_calls += 1;
            if c.rpc_down {
                return Err(RpcError::AllFailed("down".into()));
            }
            Ok(c.receipts.get(tx_hash).cloned())
        }
        async fn finalized_head(&self) -> Result<BlockRef, RpcError> {
            let c = self.0.borrow();
            if c.rpc_down {
                return Err(RpcError::AllFailed("down".into()));
            }
            Ok(BlockRef { number: c.finalized_number, hash: c.canonical.get(&c.finalized_number).copied().unwrap_or([0; 32]) })
        }
        async fn block_by_number(&self, number: u64) -> Result<BlockRef, RpcError> {
            let c = self.0.borrow();
            if c.rpc_down {
                return Err(RpcError::AllFailed("down".into()));
            }
            Ok(BlockRef { number, hash: c.canonical.get(&number).copied().unwrap_or([0; 32]) })
        }
        async fn send_raw(&self, raw: &[u8]) -> Vec<Result<String, String>> {
            let mut c = self.0.borrow_mut();
            c.sent.push(raw.to_vec());
            if c.send_error {
                vec![Err("timeout".into()), Err("timeout".into())]
            } else {
                vec![Ok("0xok".into()), Err("already known".into())]
            }
        }
        async fn eth_call(&self, _to: &[u8; 20], _data: &[u8], _tag: BlockTag) -> Result<Vec<u8>, RpcError> {
            let c = self.0.borrow();
            if c.rpc_down {
                return Err(RpcError::AllFailed("down".into()));
            }
            Ok(word_u64(u64::from(c.consumed)).to_vec())
        }
        async fn simulate_call(
            &self,
            _from: &[u8; 20],
            _to: &[u8; 20],
            _data: &[u8],
            gas: u64,
            _tag: BlockTag,
        ) -> Result<SimulationResult, RpcError> {
            let mut c = self.0.borrow_mut();
            if c.rpc_down {
                return Err(RpcError::AllFailed("down".into()));
            }
            c.simulated_gas.push(gas);
            if c.mint_gas_needed.is_some_and(|need| gas < need) {
                return Ok(SimulationResult::OutOfGas);
            }
            Ok(c.simulation.clone())
        }
        async fn sign(&self, _lane: LaneKind, t: &Eip1559Tx) -> Result<(Vec<u8>, [u8; 32]), String> {
            let mut c = self.0.borrow_mut();
            c.sign_counter += 1;
            if c.fail_signs > 0 {
                c.fail_signs -= 1;
                return Err("sign reply lost".into());
            }
            c.signed.push(t.clone());
            // Threshold ECDSA is randomized: every signing call yields different bytes.
            let mut seed = Vec::new();
            seed.extend_from_slice(&t.nonce.to_be_bytes());
            seed.extend_from_slice(&t.max_fee_per_gas.to_be_bytes());
            seed.extend_from_slice(&t.max_priority_fee_per_gas.to_be_bytes());
            seed.extend_from_slice(&c.sign_counter.to_be_bytes());
            seed.extend_from_slice(&t.data);
            seed.extend_from_slice(&t.to);
            let mut raw = vec![0x02];
            raw.extend_from_slice(&keccak256(&seed));
            let h = tx::tx_hash(&raw);
            Ok((raw, h))
        }
    }

    type SendHook = Box<dyn Fn(&[u8])>;

    /// Delegates to a [`MockIo`] and lets a test change state at chosen await points, the
    /// way another message could while the canister waits for a reply.
    struct Hooked<'a> {
        inner: &'a MockIo,
        after_simulate: Option<Box<dyn Fn()>>,
        after_tx_count: Option<Box<dyn Fn()>>,
        on_send: Option<SendHook>,
        fail_simulation: bool,
    }

    impl<'a> Hooked<'a> {
        fn new(inner: &'a MockIo) -> Self {
            Hooked { inner, after_simulate: None, after_tx_count: None, on_send: None, fail_simulation: false }
        }
        fn after_simulate(mut self, f: impl Fn() + 'static) -> Self {
            self.after_simulate = Some(Box::new(f));
            self
        }
        fn after_tx_count(mut self, f: impl Fn() + 'static) -> Self {
            self.after_tx_count = Some(Box::new(f));
            self
        }
        fn on_send(mut self, f: impl Fn(&[u8]) + 'static) -> Self {
            self.on_send = Some(Box::new(f));
            self
        }
        fn fail_simulation(mut self) -> Self {
            self.fail_simulation = true;
            self
        }
    }

    impl LaneIo for Hooked<'_> {
        async fn lane_address(&self, l: LaneKind) -> Result<[u8; 20], String> {
            self.inner.lane_address(l).await
        }
        async fn transaction_count(&self, a: &[u8; 20], t: BlockTag) -> Result<u64, RpcError> {
            let r = self.inner.transaction_count(a, t).await;
            if let Some(f) = &self.after_tx_count {
                f();
            }
            r
        }
        async fn balance(&self, a: &[u8; 20], t: BlockTag) -> Result<u128, RpcError> {
            self.inner.balance(a, t).await
        }
        async fn fee_estimate(&self) -> Result<FeeEstimate, RpcError> {
            self.inner.fee_estimate().await
        }
        async fn get_receipt(&self, h: &[u8; 32]) -> Result<Option<Receipt>, RpcError> {
            self.inner.get_receipt(h).await
        }
        async fn finalized_head(&self) -> Result<BlockRef, RpcError> {
            self.inner.finalized_head().await
        }
        async fn block_by_number(&self, n: u64) -> Result<BlockRef, RpcError> {
            self.inner.block_by_number(n).await
        }
        async fn send_raw(&self, raw: &[u8]) -> Vec<Result<String, String>> {
            if let Some(f) = &self.on_send {
                f(raw);
            }
            self.inner.send_raw(raw).await
        }
        async fn eth_call(&self, t: &[u8; 20], d: &[u8], b: BlockTag) -> Result<Vec<u8>, RpcError> {
            self.inner.eth_call(t, d, b).await
        }
        async fn simulate_call(
            &self,
            f: &[u8; 20],
            t: &[u8; 20],
            d: &[u8],
            g: u64,
            b: BlockTag,
        ) -> Result<SimulationResult, RpcError> {
            if self.fail_simulation {
                return Err(RpcError::NoQuorum("providers disagree".into()));
            }
            let r = self.inner.simulate_call(f, t, d, g, b).await;
            if let Some(h) = &self.after_simulate {
                h();
            }
            r
        }
        async fn sign(&self, l: LaneKind, t: &Eip1559Tx) -> Result<(Vec<u8>, [u8; 32]), String> {
            self.inner.sign(l, t).await
        }
    }

    // ----- state helpers --------------------------------------------------------------------

    fn setup() {
        state::reset_for_tests();
        test_env::set_now_ns(NOW);
        state::mutate_core(|c| {
            c.initialized = true;
            c.config.chain_id = state::CHAIN_BASE_SEPOLIA;
            c.config.pauses.deposits = false;
            c.config.gas = gas_cfg();
            c.config.contract = Some(state::ContractBinding {
                address: CONTRACT,
                bound_at_ns: 0,
                start_block: 1,
                supply_cap: u128::MAX,
                mint_window_limit: u128::MAX,
                mint_window_seconds: 1,
                min_bridge_out: 1,
            });
            c.minter_lane.address = Some(MINTER);
            c.admin_lane.address = Some(ADMIN);
        });
    }

    fn add_deposit(id: u64) -> DepositRecord {
        let d = DepositRecord {
            id,
            kind: DepositKind::UserDeposit,
            owner: Principal::from_slice(&[1; 10]),
            from: None,
            client_request_id: vec![id as u8; 16],
            base_recipient: [0xaa; 20],
            amount_e8s: 100 * E8,
            deposit_fee_e8s: 0,
            mint_amount_e8s: 100 * E8,
            mint_id: [id as u8 + 1; 32],
            state: DepositState::Escrowed,
            pull: None,
            refund: None,
            tx_ids: vec![],
            active_tx: None,
            delivered: None,
            hold_retry_at_ns: None,
            created_at_ns: NOW,
            updated_at_ns: NOW,
        };
        state::put_deposit(&d);
        state::mutate_core(|c| c.buckets.pending_mint += d.amount_e8s as u128);
        d
    }

    fn run(io: &MockIo, lane: LaneKind) -> LaneOutcome {
        block_on(run_lane_with(io, lane))
    }

    fn minter() -> LaneState {
        state::read_core(|c| c.minter_lane.clone())
    }

    fn admin_lane() -> LaneState {
        state::read_core(|c| c.admin_lane.clone())
    }

    fn tx(id: u64) -> EvmTxRecord {
        state::get_tx(id).expect("tx exists")
    }

    fn mint_log(d: &DepositRecord) -> abi::RawLog {
        abi::RawLog {
            address: CONTRACT,
            topics: vec![abi::BRIDGE_MINT_TOPIC0, d.mint_id, word_address(&d.base_recipient)],
            data: word_u64(d.mint_amount_e8s).to_vec(),
            block_number: 0,
            block_hash: [0; 32],
            tx_hash: [0; 32],
            log_index: 0,
            removed: false,
        }
    }

    /// Run until deposit 0 has a signed, broadcast attempt. Returns the tx id (0).
    fn start_one_mint(io: &MockIo) -> (DepositRecord, u64) {
        let d = add_deposit(0);
        run(io, LaneKind::Minter);
        let l = minter();
        (d, l.unincluded_tx.expect("a mint tx is current"))
    }

    // ----- driver tests ---------------------------------------------------------------------

    #[test]
    fn mint_is_created_signed_persisted_and_broadcast_in_one_run() {
        setup();
        let io = MockIo::new();
        let (d, id) = start_one_mint(&io);
        let t = tx(id);
        assert_eq!((t.nonce, t.lane), (0, LaneKind::Minter));
        assert_eq!(t.status, TxStatus::Pending);
        assert_eq!(t.attempts.len(), 1);
        let a = &t.attempts[0];
        assert_eq!(a.kind, AttemptKind::Original);
        assert_eq!(a.tx_hash, tx::tx_hash(&a.raw));
        assert_eq!(a.broadcast_count, 1);
        assert!(a.last_broadcast_ns.is_some());
        assert_eq!(a.max_fee_per_gas, 2_050);
        assert_eq!(a.max_priority_fee_per_gas, 50);
        assert_eq!(a.gas_limit, 150_000);
        assert_eq!(io.chain().sent, vec![a.raw.clone()]);
        assert_eq!(io.chain().signed[0].chain_id, state::CHAIN_BASE_SEPOLIA);
        assert_eq!(t.data, abi::encode_mint(&d.base_recipient, d.mint_amount_e8s, &d.mint_id));
        assert_eq!(minter().next_nonce, Some(1));
        let dep = state::get_deposit(0).unwrap();
        assert_eq!((dep.state, dep.active_tx, dep.tx_ids), (DepositState::MintInFlight, Some(id), vec![id]));
    }

    #[test]
    fn nonce_is_synced_once_and_never_reread() {
        setup();
        let io = MockIo::new();
        io.chain().counts.insert(MINTER, Counts { latest: 41, pending: 41, finalized: 41 });
        let (_, id) = start_one_mint(&io);
        assert_eq!(tx(id).nonce, 41);
        // The chain count is now bogus; a second mint must still use the local counter.
        let t = tx(id);
        io.include(t.attempts[0].tx_hash, 100, 0xb1, true, vec![], MINTER);
        io.chain().counts.get_mut(&MINTER).unwrap().latest = 7;
        io.chain().counts.get_mut(&MINTER).unwrap().pending = 7;
        add_deposit(1);
        run(&io, LaneKind::Minter);
        let second = minter().unincluded_tx.expect("second mint started");
        assert_eq!(tx(second).nonce, 42);
        assert_eq!(minter().next_nonce, Some(43));
    }

    #[test]
    fn nonce_sync_waits_when_latest_and_pending_disagree() {
        setup();
        let io = MockIo::new();
        io.chain().counts.insert(MINTER, Counts { latest: 3, pending: 4, finalized: 3 });
        add_deposit(0);
        let out = run(&io, LaneKind::Minter);
        assert_eq!(minter().next_nonce, None);
        assert!(minter().unincluded_tx.is_none());
        assert!(out.notes.iter().any(|n| n.contains("disagree")), "{:?}", out.notes);
        assert_eq!(state::get_deposit(0).unwrap().state, DepositState::Escrowed);
        // Agreement arrives later.
        io.chain().counts.get_mut(&MINTER).unwrap().pending = 3;
        run(&io, LaneKind::Minter);
        assert_eq!(minter().next_nonce, Some(4));
    }

    #[test]
    fn lost_sign_reply_creates_no_attempt_and_the_retry_uses_the_same_nonce() {
        setup();
        let io = MockIo::new();
        io.chain().fail_signs = 1;
        add_deposit(0);
        run(&io, LaneKind::Minter);
        let id = minter().unincluded_tx.expect("tx exists with its nonce");
        let t = tx(id);
        assert_eq!(t.status, TxStatus::Unsigned);
        assert!(t.attempts.is_empty(), "no attempt without signed bytes");
        assert!(io.chain().sent.is_empty(), "nothing was broadcast");
        assert_eq!(minter().next_nonce, Some(1));
        // Next run signs the same fields at the same nonce and does not create a second tx.
        run(&io, LaneKind::Minter);
        let t2 = tx(id);
        assert_eq!(t2.attempts.len(), 1);
        assert_eq!(t2.nonce, 0);
        assert_eq!(minter().next_nonce, Some(1));
        assert_eq!(minter().unincluded_tx, Some(id));
        let signed = io.chain().signed.clone();
        assert_eq!(signed.len(), 1);
        assert_eq!(signed[0].nonce, 0);
        assert_eq!(io.chain().sign_counter, 2, "signing was asked for twice");
        assert_eq!(state::get_deposit(0).unwrap().tx_ids, vec![id], "no second tx for the deposit");
    }

    #[test]
    fn broadcast_error_keeps_the_persisted_attempt_and_resends_identical_bytes() {
        setup();
        let io = MockIo::new();
        io.chain().send_error = true;
        let (_, id) = start_one_mint(&io);
        let first = tx(id).attempts[0].clone();
        assert_eq!(first.broadcast_count, 1);
        assert!(first.last_broadcast_note.as_deref().unwrap().contains("0 accepted"));
        // Too early: nothing sent.
        test_env::advance_secs(10);
        run(&io, LaneKind::Minter);
        assert_eq!(io.chain().sent.len(), 1);
        // 30 s after the last broadcast: the very same bytes go out again.
        test_env::advance_secs(25);
        run(&io, LaneKind::Minter);
        let sent = io.chain().sent.clone();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0], sent[1]);
        let t = tx(id);
        assert_eq!(t.attempts.len(), 1, "still one attempt");
        assert_eq!(t.attempts[0].raw, first.raw);
        assert_eq!(t.attempts[0].tx_hash, first.tx_hash);
        assert_eq!(t.attempts[0].broadcast_count, 2);
        assert_eq!(t.nonce, 0);
        assert_eq!(minter().next_nonce, Some(1));
        assert_eq!(io.chain().sign_counter, 1, "never re-signed");
    }

    #[test]
    fn attempt_is_persisted_before_the_broadcast_happens() {
        setup();
        let io = MockIo::new();
        add_deposit(0);
        let stored_at_send = std::rc::Rc::new(RefCell::new(Vec::<usize>::new()));
        let seen = stored_at_send.clone();
        let hooked = Hooked::new(&io).on_send(move |raw| {
            let stored = state::get_tx(0).map(|t| t.attempts.iter().filter(|a| a.raw == raw).count()).unwrap_or(0);
            seen.borrow_mut().push(stored);
        });
        block_on(run_lane_with(&hooked, LaneKind::Minter));
        assert_eq!(*stored_at_send.borrow(), vec![1], "the raw bytes were already stored when they were sent");
    }

    #[test]
    fn replacement_after_the_wait_when_latest_nonce_still_equals_the_tx_nonce() {
        setup();
        let io = MockIo::new();
        let (_, id) = start_one_mint(&io);
        let first = tx(id).attempts[0].clone();
        // Before replace_after_secs nothing new is signed.
        test_env::advance_secs(60);
        run(&io, LaneKind::Minter);
        assert_eq!(tx(id).attempts.len(), 1);
        // After it, with latest == nonce (0), a bumped attempt appears at the same nonce.
        test_env::advance_secs(35);
        run(&io, LaneKind::Minter);
        let t = tx(id);
        assert_eq!(t.attempts.len(), 2);
        let r = &t.attempts[1];
        assert_eq!(r.kind, AttemptKind::Replacement);
        assert_eq!(r.max_fee_per_gas, bump_once(first.max_fee_per_gas));
        assert_eq!(r.max_priority_fee_per_gas, bump_once(first.max_priority_fee_per_gas));
        assert_eq!(io.chain().signed[1].nonce, 0);
        assert_ne!(r.tx_hash, first.tx_hash);
        assert_eq!(t.nonce, 0);
        assert_eq!(minter().next_nonce, Some(1));
        // First send, one same-bytes rebroadcast at 60 s, then the replacement's own send.
        let sent = io.chain().sent.clone();
        assert_eq!(sent.len(), 3);
        assert_eq!(sent[0], sent[1]);
        assert_eq!(sent[0], first.raw);
        assert_eq!(sent[2], r.raw);
        // The original attempt is untouched.
        assert_eq!(t.attempts[0], {
            let mut a = first.clone();
            a.broadcast_count = t.attempts[0].broadcast_count;
            a.last_broadcast_ns = t.attempts[0].last_broadcast_ns;
            a.last_broadcast_note = t.attempts[0].last_broadcast_note.clone();
            a
        });
    }

    #[test]
    fn no_replacement_when_the_chain_nonce_moved_past_the_tx() {
        setup();
        let io = MockIo::new();
        let (_, id) = start_one_mint(&io);
        // Our attempt landed (latest nonce 1) but receipts lag behind.
        io.chain().counts.get_mut(&MINTER).unwrap().latest = 1;
        io.chain().counts.get_mut(&MINTER).unwrap().pending = 1;
        test_env::advance_secs(300);
        let out = run(&io, LaneKind::Minter);
        assert_eq!(tx(id).attempts.len(), 1, "no replacement");
        assert!(out.notes.iter().any(|n| n.contains("latest nonce is past")), "{:?}", out.notes);
        assert!(minter().halted.is_none());
    }

    #[test]
    fn receipt_of_the_older_attempt_is_accepted_after_a_replacement() {
        setup();
        let io = MockIo::new();
        let (d, id) = start_one_mint(&io);
        test_env::advance_secs(100);
        run(&io, LaneKind::Minter);
        let t = tx(id);
        assert_eq!(t.attempts.len(), 2);
        // The ORIGINAL (older) attempt is the one that gets mined.
        io.include(t.attempts[0].tx_hash, 200, 0xb1, true, vec![mint_log(&d)], MINTER);
        run(&io, LaneKind::Minter);
        let t = tx(id);
        match t.status {
            TxStatus::Included { attempt, block_number, success, .. } => {
                assert_eq!((attempt, block_number, success), (0, 200, true));
            }
            other => panic!("{other:?}"),
        }
        let l = minter();
        assert_eq!(l.unincluded_tx, None);
        assert_eq!(l.unfinalized_txs, vec![id]);
        // Finality picks the same attempt hash.
        io.finalize_up_to(200, MINTER);
        run(&io, LaneKind::Minter);
        assert_eq!(state::get_deposit(0).unwrap().state, DepositState::Delivered);
    }

    #[test]
    fn mint_happy_path_to_delivered_applies_the_outcome_exactly_once() {
        setup();
        let io = MockIo::new();
        let (d, id) = start_one_mint(&io);
        let hash = tx(id).attempts[0].tx_hash;
        io.include(hash, 300, 0xb1, true, vec![mint_log(&d)], MINTER);
        run(&io, LaneKind::Minter);
        assert!(matches!(tx(id).status, TxStatus::Included { .. }));
        // Included but the finalized head is below the block: nothing is settled.
        io.chain().finalized_number = 299;
        run(&io, LaneKind::Minter);
        assert_eq!(state::get_deposit(0).unwrap().state, DepositState::MintInFlight);
        // Finality arrives.
        io.finalize_up_to(300, MINTER);
        let out = run(&io, LaneKind::Minter);
        assert!(out.notes.iter().any(|n| n.contains("finalized")), "{:?}", out.notes);
        let t = tx(id);
        assert!(matches!(t.status, TxStatus::Finalized { block_number: 300, success: true, .. }));
        assert!(t.outcome_applied);
        let dep = state::get_deposit(0).unwrap();
        assert_eq!(dep.state, DepositState::Delivered);
        assert_eq!(dep.delivered.as_ref().unwrap().block_number, 300);
        let b = state::read_core(|c| c.buckets);
        assert_eq!((b.pending_mint, b.outstanding_base), (0, (100 * E8) as u128));
        assert!(minter().unfinalized_txs.is_empty());
        assert!(!out.pending_work);
        // Running again changes nothing (the outcome is not applied twice).
        let before = state::read_core(|c| c.buckets);
        run(&io, LaneKind::Minter);
        assert_eq!(state::read_core(|c| c.buckets), before);
        assert_eq!(state::get_deposit(0).unwrap().state, DepositState::Delivered);
    }

    #[test]
    fn apply_finalized_is_idempotent_even_if_called_twice() {
        setup();
        let io = MockIo::new();
        let (d, id) = start_one_mint(&io);
        let hash = tx(id).attempts[0].tx_hash;
        io.include(hash, 300, 0xb1, true, vec![mint_log(&d)], MINTER);
        run(&io, LaneKind::Minter);
        let included = tx(id).status;
        let rc = receipt_for(0, 300, 0xb1, true);
        let mut rc = rc;
        rc.tx_hash = hash;
        rc.logs = vec![mint_log(&d)];
        let mut st = RunState::new(LaneKind::Minter);
        apply_finalized(id, &included, rc.clone(), &mut st);
        let after_first = state::read_core(|c| c.buckets);
        assert_eq!(state::get_deposit(0).unwrap().state, DepositState::Delivered);
        // A second application finds the tx already finalized and does nothing.
        apply_finalized(id, &included, rc, &mut st);
        assert_eq!(state::read_core(|c| c.buckets), after_first);
        assert!(tx(id).outcome_applied);
    }

    #[test]
    fn finalized_revert_holds_the_deposit_without_delivering() {
        setup();
        let io = MockIo::new();
        let (_, id) = start_one_mint(&io);
        let hash = tx(id).attempts[0].tx_hash;
        io.include(hash, 300, 0xb1, false, vec![], MINTER);
        io.finalize_up_to(300, MINTER);
        run(&io, LaneKind::Minter);
        run(&io, LaneKind::Minter);
        let t = tx(id);
        assert!(matches!(t.status, TxStatus::Finalized { success: false, .. }));
        assert!(t.outcome_applied);
        let dep = state::get_deposit(0).unwrap();
        assert!(matches!(dep.state, DepositState::MintHeld { .. }), "{:?}", dep.state);
        assert_eq!(dep.active_tx, None);
        let b = state::read_core(|c| c.buckets);
        assert_eq!((b.pending_mint, b.outstanding_base), ((100 * E8) as u128, 0));
    }

    #[test]
    fn reorg_before_finality_returns_to_pending_and_rebroadcasts_the_same_bytes() {
        setup();
        let io = MockIo::new();
        let (d, id) = start_one_mint(&io);
        let t0 = tx(id);
        let hash = t0.attempts[0].tx_hash;
        io.include(hash, 300, 0xb1, true, vec![mint_log(&d)], MINTER);
        run(&io, LaneKind::Minter);
        assert!(matches!(tx(id).status, TxStatus::Included { .. }));
        let sent_before = io.chain().sent.len();
        // The block is reorged out: receipt gone, chain nonce back to 0, then finality passes the height.
        io.drop_receipt(hash);
        io.chain().counts.get_mut(&MINTER).unwrap().latest = 0;
        io.chain().counts.get_mut(&MINTER).unwrap().pending = 1;
        io.finalize_up_to(310, MINTER);
        io.chain().counts.get_mut(&MINTER).unwrap().finalized = 0;
        let out = run(&io, LaneKind::Minter);
        let t = tx(id);
        assert_eq!(t.status, TxStatus::Pending);
        assert_eq!(t.attempts.len(), 1, "attempts are kept, none added");
        assert_eq!(t.nonce, 0, "same nonce");
        assert!(minter().unfinalized_txs.contains(&id), "stays in the unfinalized list");
        assert_eq!(minter().unincluded_tx, None);
        let sent = io.chain().sent.clone();
        assert_eq!(sent.len(), sent_before + 1, "latest attempt rebroadcast");
        assert_eq!(sent.last().unwrap(), &t.attempts[0].raw);
        assert!(out.notes.iter().any(|n| n.contains("returned to pending")), "{:?}", out.notes);
        assert_eq!(state::get_deposit(0).unwrap().state, DepositState::MintInFlight);
        // No new tx can start while a reorged tx is pending, even with another deposit waiting.
        add_deposit(1);
        run(&io, LaneKind::Minter);
        assert_eq!(minter().next_nonce, Some(1));
        assert_eq!(minter().unincluded_tx, None);
        // The tx lands again in a new block and later finalizes normally.
        io.include(hash, 320, 0xc2, true, vec![mint_log(&d)], MINTER);
        run(&io, LaneKind::Minter);
        assert!(matches!(tx(id).status, TxStatus::Included { block_number: 320, .. }));
        io.finalize_up_to(320, MINTER);
        run(&io, LaneKind::Minter);
        assert_eq!(state::get_deposit(0).unwrap().state, DepositState::Delivered);
    }

    #[test]
    fn reorg_with_a_different_block_for_the_same_hash_goes_back_to_pending_then_included() {
        setup();
        let io = MockIo::new();
        let (d, id) = start_one_mint(&io);
        let hash = tx(id).attempts[0].tx_hash;
        io.include(hash, 300, 0xb1, true, vec![mint_log(&d)], MINTER);
        run(&io, LaneKind::Minter);
        // Re-mined in block 301 with another hash while the height 300 changed.
        io.include(hash, 301, 0xb2, true, vec![mint_log(&d)], MINTER);
        io.finalize_up_to(305, MINTER);
        run(&io, LaneKind::Minter);
        assert_eq!(tx(id).status, TxStatus::Pending);
        // The following run notices the receipt in its new block (no new attempt).
        run(&io, LaneKind::Minter);
        assert!(matches!(tx(id).status, TxStatus::Included { block_number: 301, .. }));
        assert_eq!(tx(id).attempts.len(), 1);
        run(&io, LaneKind::Minter);
        assert_eq!(state::get_deposit(0).unwrap().state, DepositState::Delivered);
    }

    #[test]
    fn non_canonical_block_at_the_receipt_height_is_a_reorg() {
        setup();
        let io = MockIo::new();
        let (d, id) = start_one_mint(&io);
        let hash = tx(id).attempts[0].tx_hash;
        io.include(hash, 300, 0xb1, true, vec![mint_log(&d)], MINTER);
        run(&io, LaneKind::Minter);
        // Providers still serve the receipt but the canonical block at 300 has another hash.
        io.chain().canonical.insert(300, [0xee; 32]);
        io.chain().finalized_number = 300;
        run(&io, LaneKind::Minter);
        assert_eq!(tx(id).status, TxStatus::Pending);
        assert_eq!(state::get_deposit(0).unwrap().state, DepositState::MintInFlight);
    }

    #[test]
    fn cancel_attempt_wins_and_the_deposit_is_held() {
        setup();
        let io = MockIo::new();
        let (_, id) = start_one_mint(&io);
        request_cancel(id).unwrap();
        assert!(tx(id).cancel_requested);
        // Age gate: nothing new yet.
        run(&io, LaneKind::Minter);
        assert_eq!(tx(id).attempts.len(), 1);
        test_env::advance_secs(100);
        run(&io, LaneKind::Minter);
        let t = tx(id);
        assert_eq!(t.attempts.len(), 2);
        let c = &t.attempts[1];
        assert_eq!(c.kind, AttemptKind::Cancel);
        assert_eq!((c.to, c.value_wei, c.data.len(), c.gas_limit), (MINTER, 0, 0, 21_000));
        assert_eq!(c.max_fee_per_gas, bump_once(t.attempts[0].max_fee_per_gas));
        assert_eq!(io.chain().signed[1].to, MINTER);
        assert_eq!(io.chain().signed[1].nonce, 0);
        // The cancel is mined and finalized.
        io.include(c.tx_hash, 400, 0xb4, true, vec![], MINTER);
        io.finalize_up_to(400, MINTER);
        run(&io, LaneKind::Minter);
        run(&io, LaneKind::Minter);
        let t = tx(id);
        assert!(matches!(t.status, TxStatus::Finalized { attempt: 1, success: true, .. }));
        let dep = state::get_deposit(0).unwrap();
        assert!(matches!(dep.state, DepositState::MintHeld { ref reason } if reason.contains("cancelled")), "{:?}", dep.state);
        assert_eq!(state::read_core(|c| c.buckets.outstanding_base), 0);
    }

    #[test]
    fn original_wins_even_though_a_cancel_was_requested() {
        setup();
        let io = MockIo::new();
        let (d, id) = start_one_mint(&io);
        request_cancel(id).unwrap();
        test_env::advance_secs(100);
        run(&io, LaneKind::Minter);
        let t = tx(id);
        assert_eq!(t.attempts[1].kind, AttemptKind::Cancel);
        // The ORIGINAL got mined after all.
        io.include(t.attempts[0].tx_hash, 410, 0xb5, true, vec![mint_log(&d)], MINTER);
        io.finalize_up_to(410, MINTER);
        run(&io, LaneKind::Minter);
        run(&io, LaneKind::Minter);
        let t = tx(id);
        assert!(matches!(t.status, TxStatus::Finalized { attempt: 0, success: true, .. }));
        assert_eq!(state::get_deposit(0).unwrap().state, DepositState::Delivered);
    }

    #[test]
    fn request_cancel_rules() {
        setup();
        let io = MockIo::new();
        assert_eq!(request_cancel(99), Err(BridgeError::NotFound));
        let (d, id) = start_one_mint(&io);
        // Idempotent.
        request_cancel(id).unwrap();
        request_cancel(id).unwrap();
        // Included transactions can no longer be cancelled.
        let hash = tx(id).attempts[0].tx_hash;
        io.include(hash, 300, 0xb1, true, vec![mint_log(&d)], MINTER);
        run(&io, LaneKind::Minter);
        assert!(matches!(request_cancel(id), Err(BridgeError::InvalidState(_))));
        // A tx that is not the lane's current unincluded tx is rejected too.
        let mut other = tx(id);
        other.id = 77;
        other.status = TxStatus::Pending;
        state::put_tx(&other);
        assert!(matches!(request_cancel(77), Err(BridgeError::InvalidState(_))));
    }

    #[test]
    fn cancel_requested_before_the_first_signature_signs_a_cancel_directly() {
        setup();
        let io = MockIo::new();
        io.chain().fail_signs = 1;
        add_deposit(0);
        run(&io, LaneKind::Minter);
        let id = minter().unincluded_tx.unwrap();
        assert_eq!(tx(id).status, TxStatus::Unsigned);
        request_cancel(id).unwrap();
        run(&io, LaneKind::Minter);
        let t = tx(id);
        assert_eq!(t.attempts.len(), 1);
        assert_eq!(t.attempts[0].kind, AttemptKind::Cancel);
        assert_eq!(t.nonce, 0);
        assert_eq!(t.attempts[0].to, MINTER);
    }

    #[test]
    fn insufficient_gas_sets_awaiting_gas_and_signs_nothing_until_funded() {
        setup();
        let io = MockIo::new();
        io.chain().balance = 1_000; // far below 150_000 * 2_050 + reserve
        add_deposit(0);
        let out = run(&io, LaneKind::Minter);
        let l = minter();
        assert!(l.awaiting_gas);
        assert_eq!(l.last_gas_balance_wei, Some(1_000));
        let id = l.unincluded_tx.unwrap();
        assert!(tx(id).attempts.is_empty());
        assert_eq!(io.chain().sign_counter, 0, "no signing without gas");
        assert!(out.notes.iter().any(|n| n.contains("awaiting gas")), "{:?}", out.notes);
        // A second deposit does not start another tx.
        add_deposit(1);
        run(&io, LaneKind::Minter);
        assert_eq!(minter().next_nonce, Some(1));
        // Funded: the same tx is signed and the flag clears.
        io.chain().balance = 150_000 * 2_050 + 1_000;
        run(&io, LaneKind::Minter);
        let l = minter();
        assert!(!l.awaiting_gas);
        assert_eq!(tx(id).attempts.len(), 1);
        assert_eq!(l.unincluded_tx, Some(id));
        // Exactly at the boundary is enough; one wei less is not (checked in gas_check_includes_reserve_and_value).
    }

    #[test]
    fn paused_mints_do_not_sign_an_unsigned_mint_but_do_not_lose_the_nonce() {
        setup();
        let io = MockIo::new();
        io.chain().balance = 1;
        add_deposit(0);
        run(&io, LaneKind::Minter);
        let id = minter().unincluded_tx.unwrap();
        io.chain().balance = 10_000_000_000_000_000_000;
        state::mutate_core(|c| c.config.pauses.mints = true);
        let out = run(&io, LaneKind::Minter);
        assert!(tx(id).attempts.is_empty());
        assert!(out.notes.iter().any(|n| n.contains("blocked")), "{:?}", out.notes);
        assert_eq!(minter().unincluded_tx, Some(id));
        // Unpaused: signs at the held nonce.
        state::mutate_core(|c| c.config.pauses.mints = false);
        run(&io, LaneKind::Minter);
        assert_eq!(tx(id).attempts.len(), 1);
        assert_eq!(tx(id).nonce, 0);
    }

    #[test]
    fn rpc_outage_never_creates_attempts_or_nonces() {
        setup();
        let io = MockIo::new();
        io.chain().rpc_down = true;
        add_deposit(0);
        run(&io, LaneKind::Minter);
        assert_eq!(minter().next_nonce, None);
        assert!(minter().unincluded_tx.is_none());
        assert_eq!(state::get_deposit(0).unwrap().state, DepositState::Escrowed);
        // Outage after the tx exists: no halt, no replacement, no new signatures.
        io.chain().rpc_down = false;
        let (_, id) = {
            run(&io, LaneKind::Minter);
            (0, minter().unincluded_tx.unwrap())
        };
        io.chain().rpc_down = true;
        test_env::advance_secs(1_000);
        let signs = io.chain().sign_counter;
        run(&io, LaneKind::Minter);
        assert_eq!(io.chain().sign_counter, signs);
        assert_eq!(tx(id).attempts.len(), 1);
        assert!(minter().halted.is_none());
    }

    #[test]
    fn preflight_revert_holds_the_deposit_and_spends_no_nonce() {
        setup();
        let io = MockIo::new();
        io.chain().simulation = SimulationResult::Reverted(vec![0xde, 0xad, 0xbe, 0xef, 0, 0]);
        add_deposit(0);
        run(&io, LaneKind::Minter);
        assert_eq!(minter().next_nonce, Some(0), "synced, nothing allocated");
        assert!(minter().unincluded_tx.is_none());
        match state::get_deposit(0).unwrap().state {
            DepositState::MintHeld { reason } => assert!(reason.contains("0xdeadbeef"), "{reason}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(io.chain().sign_counter, 0);
    }

    #[test]
    fn preflight_rpc_error_waits_without_a_hold() {
        setup();
        let io = MockIo::new();
        add_deposit(0);
        let hooked = Hooked::new(&io).fail_simulation();
        block_on(run_lane_with(&hooked, LaneKind::Minter));
        assert_eq!(state::get_deposit(0).unwrap().state, DepositState::Escrowed);
        assert!(minter().unincluded_tx.is_none());
        assert_eq!(minter().next_nonce, Some(0), "synced, never allocated");
    }

    #[test]
    fn mint_consumed_true_halts_the_bridge_before_any_nonce_is_used() {
        setup();
        let io = MockIo::new();
        io.chain().consumed = true;
        add_deposit(0);
        run(&io, LaneKind::Minter);
        assert!(state::is_halted());
        assert!(minter().unincluded_tx.is_none());
        assert_eq!(io.chain().sign_counter, 0);
        assert_eq!(state::get_deposit(0).unwrap().state, DepositState::Escrowed);
    }

    #[test]
    fn nonce_consumed_by_an_unknown_tx_halts_the_lane_and_parks_the_tx() {
        setup();
        let io = MockIo::new();
        let (_, id) = start_one_mint(&io);
        // Someone else used nonce 0 and it is finalized; none of our hashes has a receipt.
        {
            let mut c = io.chain();
            let n = c.counts.get_mut(&MINTER).unwrap();
            n.latest = 1;
            n.pending = 1;
            n.finalized = 1;
        }
        test_env::advance_secs(200);
        let signs = io.chain().sign_counter;
        let out = run(&io, LaneKind::Minter);
        let l = minter();
        assert!(l.halted.as_deref().unwrap().contains("nonce 0"), "{:?}", l.halted);
        assert_eq!(tx(id).note.as_deref(), Some(PARKED_NOTE));
        assert_eq!(io.chain().sign_counter, signs, "nothing re-signed");
        assert_eq!(tx(id).attempts.len(), 1);
        assert!(out.notes.iter().any(|n| n.contains("consumed")), "{:?}", out.notes);
        // Parked: further runs do not even look at the chain for it.
        let calls = io.chain().receipt_calls;
        let out = run(&io, LaneKind::Minter);
        assert_eq!(io.chain().receipt_calls, calls);
        assert!(out.notes.iter().any(|n| n.contains("parked")), "{:?}", out.notes);
        assert!(!out.pending_work, "a parked tx is not pending work");
        // A halted lane starts nothing new.
        add_deposit(1);
        run(&io, LaneKind::Minter);
        assert_eq!(minter().next_nonce, Some(1));
        // The halt is on the audit log.
        let audit = state::audit_page(0, 100);
        assert!(audit.iter().any(|r| r.note.contains("minter lane HALT")));
    }

    #[test]
    fn halted_lane_still_finalizes_what_is_already_included() {
        setup();
        let io = MockIo::new();
        let (d, id) = start_one_mint(&io);
        let hash = tx(id).attempts[0].tx_hash;
        io.include(hash, 300, 0xb1, true, vec![mint_log(&d)], MINTER);
        run(&io, LaneKind::Minter);
        halt_lane(LaneKind::Minter, "test halt");
        io.finalize_up_to(300, MINTER);
        run(&io, LaneKind::Minter);
        assert_eq!(state::get_deposit(0).unwrap().state, DepositState::Delivered);
    }

    #[test]
    fn eight_unfinalized_txs_stop_new_work_and_finalization_reopens_it() {
        setup();
        let io = MockIo::new();
        // Build the lane state directly: eight included txs and a ninth deposit waiting.
        for i in 0..MAX_UNFINALIZED as u64 {
            let d = add_deposit(i);
            let mut t = base_tx(i, vec![attempt(AttemptKind::Original, 10 + i as u8, 1_000, 10, NOW, Some(NOW))]);
            t.id = 100 + i;
            t.purpose = TxPurpose::Mint { deposit_id: d.id };
            t.status = TxStatus::Included { attempt: 0, block_number: 700 + i, block_hash: [0x70 + i as u8; 32], success: true };
            state::put_tx(&t);
            let mut dep = state::get_deposit(i).unwrap();
            dep.state = DepositState::MintInFlight;
            dep.active_tx = Some(t.id);
            dep.tx_ids = vec![t.id];
            state::put_deposit(&dep);
            state::mutate_core(|c| c.minter_lane.unfinalized_txs.push(100 + i));
            io.chain().receipts.insert(
                [10 + i as u8; 32],
                Receipt {
                    tx_hash: [10 + i as u8; 32],
                    success: true,
                    block_number: 700 + i,
                    block_hash: [0x70 + i as u8; 32],
                    from: MINTER,
                    to: Some(CONTRACT),
                    logs: vec![mint_log(&d)],
                },
            );
            io.chain().canonical.insert(700 + i, [0x70 + i as u8; 32]);
        }
        state::mutate_core(|c| c.minter_lane.next_nonce = Some(MAX_UNFINALIZED as u64));
        add_deposit(8);
        let out = run(&io, LaneKind::Minter);
        assert_eq!(minter().next_nonce, Some(8), "no ninth tx");
        assert!(minter().unincluded_tx.is_none());
        assert!(out.notes.iter().any(|n| n.contains("too many unfinalized")), "{:?}", out.notes);
        assert!(out.pending_work);
        // Finalize the first three: the lane has room again and starts deposit 8 in the same run (finality runs first).
        io.chain().finalized_number = 702;
        run(&io, LaneKind::Minter);
        assert_eq!(minter().unfinalized_txs.len(), 5);
        assert_eq!(minter().next_nonce, Some(9));
        assert!(minter().unincluded_tx.is_some());
        for i in 0..3 {
            assert_eq!(state::get_deposit(i).unwrap().state, DepositState::Delivered);
        }
    }

    // ----- atomic nonce assignment (no nonce gaps) --------------------------------------------

    /// A mint that passed preflight but whose deposit left `Escrowed` while the canister was
    /// waiting for the simulation reply is abandoned without touching `next_nonce`.
    fn assert_abandoned_without_nonce(io: &MockIo, hooked: &Hooked<'_>, out_note: &str) {
        let ids_before = state::read_core(|c| c.next_tx_id);
        let out = block_on(run_lane_with(hooked, LaneKind::Minter));
        let l = minter();
        assert_eq!(l.next_nonce, Some(0), "next_nonce unchanged after the nonce sync");
        assert_eq!(l.unincluded_tx, None);
        assert!(l.unfinalized_txs.is_empty());
        assert_eq!(state::read_core(|c| c.next_tx_id), ids_before, "no tx id burned either");
        assert!(state::get_tx(ids_before).is_none(), "no orphan tx record");
        assert_eq!(io.chain().sign_counter, 0);
        assert!(out.notes.iter().any(|n| n.contains("abandoned") && n.contains(out_note)), "{:?}", out.notes);
    }

    #[test]
    fn deposit_no_longer_escrowed_after_preflight_abandons_without_using_a_nonce() {
        setup();
        let io = MockIo::new();
        add_deposit(0);
        let hooked = Hooked::new(&io).after_simulate(|| {
            // For example an operator action moved the deposit while the simulation was in flight.
            let mut d = state::get_deposit(0).unwrap();
            d.state = DepositState::MintHeld { reason: "manual".into() };
            state::put_deposit(&d);
        });
        assert_abandoned_without_nonce(&io, &hooked, "no longer waiting");
        assert!(matches!(state::get_deposit(0).unwrap().state, DepositState::MintHeld { .. }));
        assert_eq!(state::get_deposit(0).unwrap().active_tx, None);
    }

    #[test]
    fn deposit_that_gained_an_active_tx_during_preflight_is_abandoned_too() {
        setup();
        let io = MockIo::new();
        add_deposit(0);
        let hooked = Hooked::new(&io).after_simulate(|| {
            let mut d = state::get_deposit(0).unwrap();
            d.active_tx = Some(999);
            state::put_deposit(&d);
        });
        assert_abandoned_without_nonce(&io, &hooked, "no longer waiting");
    }

    #[test]
    fn mints_paused_during_preflight_abandon_without_using_a_nonce() {
        setup();
        let io = MockIo::new();
        add_deposit(0);
        let hooked = Hooked::new(&io).after_simulate(|| state::mutate_core(|c| c.config.pauses.mints = true));
        assert_abandoned_without_nonce(&io, &hooked, "paused");
        assert_eq!(state::get_deposit(0).unwrap().state, DepositState::Escrowed, "the deposit is untouched");
    }

    #[test]
    fn bridge_halted_during_preflight_abandons_without_using_a_nonce() {
        setup();
        let io = MockIo::new();
        add_deposit(0);
        let hooked = Hooked::new(&io).after_simulate(|| state::halt("halted while waiting"));
        assert_abandoned_without_nonce(&io, &hooked, "halted");
    }

    #[test]
    fn lane_halted_or_busy_during_preflight_abandons_without_using_a_nonce() {
        setup();
        let io = MockIo::new();
        add_deposit(0);
        let hooked = Hooked::new(&io).after_simulate(|| {
            state::mutate_core(|c| c.minter_lane.halted = Some("anomaly".into()));
        });
        assert_abandoned_without_nonce(&io, &hooked, "lane");
        // Another message made the lane busy meanwhile.
        setup();
        let io = MockIo::new();
        add_deposit(0);
        let hooked = Hooked::new(&io).after_simulate(|| state::mutate_core(|c| c.minter_lane.unincluded_tx = Some(555)));
        let out = block_on(run_lane_with(&hooked, LaneKind::Minter));
        assert_eq!(minter().next_nonce, Some(0));
        assert_eq!(minter().unincluded_tx, Some(555));
        assert!(out.notes.iter().any(|n| n.contains("abandoned")), "{:?}", out.notes);
    }

    #[test]
    fn rollback_restores_the_nonce_and_leaves_no_tx_record_when_the_deposit_link_refuses() {
        setup();
        state::mutate_core(|c| c.minter_lane.next_nonce = Some(12));
        add_deposit(0);
        let next_id = state::read_core(|c| c.next_tx_id);
        let err = create_mint_tx_with(0, None, |_, _| Err("link refused".to_string())).unwrap_err();
        assert!(err.contains("link refused") && err.contains("nonce 12 released"), "{err}");
        let l = minter();
        assert_eq!(l.next_nonce, Some(12), "nonce handed back");
        assert_eq!(l.unincluded_tx, None);
        // The id was drawn but no record was written, so nothing can ever sign it.
        assert!(state::get_tx(next_id).is_none());
        assert_eq!(state::get_deposit(0).unwrap().state, DepositState::Escrowed);
        assert!(state::audit_page(0, 50).iter().any(|r| r.note.contains("rolled back")));
        // The next attempt works and receives that very nonce.
        let id = create_mint_tx(0).unwrap();
        assert_eq!(tx(id).nonce, 12);
        assert_eq!(minter().next_nonce, Some(13));
        assert_eq!(minter().unincluded_tx, Some(id));
        assert_eq!(state::get_deposit(0).unwrap().active_tx, Some(id));
    }

    #[test]
    fn creating_a_mint_tx_links_assigns_and_stores_in_one_step() {
        setup();
        state::mutate_core(|c| c.minter_lane.next_nonce = Some(5));
        let d = add_deposit(0);
        let id = create_mint_tx(0).unwrap();
        let t = tx(id);
        assert_eq!((t.nonce, t.status.clone(), t.lane), (5, TxStatus::Unsigned, LaneKind::Minter));
        assert_eq!(t.data, abi::encode_mint(&d.base_recipient, d.mint_amount_e8s, &d.mint_id));
        assert_eq!(minter().next_nonce, Some(6));
        let dep = state::get_deposit(0).unwrap();
        assert_eq!((dep.state, dep.active_tx), (DepositState::MintInFlight, Some(id)));
        // A second creation for the same deposit is refused and spends nothing.
        let ids = state::read_core(|c| c.next_tx_id);
        assert!(create_mint_tx(0).is_err());
        assert_eq!(minter().next_nonce, Some(6));
        assert_eq!(state::read_core(|c| c.next_tx_id), ids);
        // And so is a creation without a synced nonce.
        state::mutate_core(|c| {
            c.minter_lane.unincluded_tx = None;
            c.minter_lane.next_nonce = None;
        });
        add_deposit(1);
        assert!(create_mint_tx(1).unwrap_err().contains("no nonce"));
    }

    #[test]
    fn admin_nonce_is_assigned_only_when_the_tx_becomes_current() {
        setup();
        let io = MockIo::new();
        io.chain().counts.insert(ADMIN, Counts { latest: 9, pending: 9, finalized: 9 });
        let a = enqueue_admin_action(ContractAdminAction::PauseMint).unwrap();
        // The lane is halted while the canister waits for the nonce sync: nothing is assigned.
        let hooked = Hooked::new(&io).after_tx_count(|| state::mutate_core(|c| c.admin_lane.halted = Some("anomaly".into())));
        let out = block_on(run_lane_with(&hooked, LaneKind::Admin));
        let l = admin_lane();
        assert_eq!(l.next_nonce, Some(9), "synced but not advanced");
        assert_eq!(l.unincluded_tx, None);
        assert_eq!(l.queued_txs, vec![a], "still queued");
        assert_eq!(tx(a).nonce, 0, "no nonce stored on the queued tx");
        assert!(tx(a).attempts.is_empty());
        assert!(out.notes.iter().any(|n| n.contains("waits")), "{:?}", out.notes);
        assert_eq!(io.chain().sign_counter, 0);
        // Once the halt is gone the tx becomes current and takes exactly that nonce.
        state::mutate_core(|c| c.admin_lane.halted = None);
        run(&io, LaneKind::Admin);
        assert_eq!(tx(a).nonce, 9);
        assert_eq!(admin_lane().next_nonce, Some(10));
        assert_eq!(admin_lane().unincluded_tx, Some(a));
    }

    // ----- admin lane ------------------------------------------------------------------------

    #[test]
    fn enqueue_requires_a_bound_contract_and_records_an_unsigned_tx_without_a_nonce() {
        setup();
        state::mutate_core(|c| c.config.contract = None);
        assert!(matches!(enqueue_admin_action(ContractAdminAction::PauseMint), Err(BridgeError::NotConfigured(_))));
        setup();
        let a = enqueue_admin_action(ContractAdminAction::PauseMint).unwrap();
        let b = enqueue_admin_action(ContractAdminAction::SetSupplyCap { cap_e8s: 5 }).unwrap();
        assert!(b > a);
        let t = tx(a);
        assert_eq!((t.lane, t.nonce, t.status.clone(), t.attempts.len()), (LaneKind::Admin, 0, TxStatus::Unsigned, 0));
        assert_eq!(t.to, CONTRACT);
        assert_eq!(t.data, abi::encode_admin_action(&ContractAdminAction::PauseMint));
        assert_eq!(t.gas_limit, 100_000);
        assert!(matches!(t.purpose, TxPurpose::Admin { action: ContractAdminAction::PauseMint }));
        assert_eq!(admin_lane().queued_txs, vec![a, b]);
        assert_eq!(admin_lane().unincluded_tx, None);
        assert_eq!(admin_lane().next_nonce, None);
    }

    #[test]
    fn admin_queue_is_bounded() {
        setup();
        for _ in 0..MAX_ADMIN_QUEUE {
            enqueue_admin_action(ContractAdminAction::PauseMint).unwrap();
        }
        assert!(matches!(enqueue_admin_action(ContractAdminAction::PauseMint), Err(BridgeError::InvalidState(_))));
    }

    #[test]
    fn admin_actions_run_oldest_first_one_at_a_time_with_sequential_nonces() {
        setup();
        let io = MockIo::new();
        io.chain().counts.insert(ADMIN, Counts { latest: 5, pending: 5, finalized: 5 });
        let a = enqueue_admin_action(ContractAdminAction::PauseMint).unwrap();
        let b = enqueue_admin_action(ContractAdminAction::PauseBridgeOut).unwrap();
        let c = enqueue_admin_action(ContractAdminAction::UnpauseMint).unwrap();
        run(&io, LaneKind::Admin);
        let l = admin_lane();
        assert_eq!(l.unincluded_tx, Some(a));
        assert_eq!(l.queued_txs, vec![b, c]);
        assert_eq!(tx(a).nonce, 5);
        assert_eq!(tx(b).nonce, 0, "queued txs have no nonce yet");
        assert_eq!(tx(a).attempts.len(), 1);
        assert_eq!(tx(a).status, TxStatus::Pending);
        assert_eq!(io.chain().signed[0].gas_limit, 100_000);
        assert_eq!(io.chain().signed[0].data, abi::encode_admin_action(&ContractAdminAction::PauseMint));
        // While `a` is unincluded, `b` waits.
        run(&io, LaneKind::Admin);
        assert_eq!(admin_lane().queued_txs, vec![b, c]);
        // `a` is included: `b` starts at once with the next nonce, and `a` stays unfinalized.
        io.include(tx(a).attempts[0].tx_hash, 900, 0xa1, true, vec![], ADMIN);
        run(&io, LaneKind::Admin);
        let l = admin_lane();
        assert_eq!(l.unfinalized_txs, vec![a]);
        assert_eq!(l.unincluded_tx, Some(b));
        assert_eq!(l.queued_txs, vec![c]);
        assert_eq!(tx(b).nonce, 6);
        // Finalize `a`: audit note, lane list shrinks, outcome flag set.
        io.finalize_up_to(900, ADMIN);
        run(&io, LaneKind::Admin);
        assert!(admin_lane().unfinalized_txs.is_empty());
        assert!(tx(a).outcome_applied);
        assert!(matches!(tx(a).status, TxStatus::Finalized { success: true, .. }));
        let audit = state::audit_page(0, 100);
        assert!(audit.iter().any(|r| r.note.contains("PauseMint") && r.note.contains("succeeded")));
        // `b` mined, then `c` gets nonce 7.
        io.include(tx(b).attempts[0].tx_hash, 901, 0xa2, false, vec![], ADMIN);
        run(&io, LaneKind::Admin);
        assert_eq!(admin_lane().unincluded_tx, Some(c));
        assert_eq!(tx(c).nonce, 7);
        assert_eq!(admin_lane().next_nonce, Some(8));
        // A reverted admin call is recorded as such once final.
        io.finalize_up_to(901, ADMIN);
        run(&io, LaneKind::Admin);
        let audit = state::audit_page(0, 200);
        assert!(audit.iter().any(|r| r.note.contains("PauseBridgeOut") && r.note.contains("reverted")));
    }

    #[test]
    fn admin_lane_is_independent_of_the_minter_lane() {
        setup();
        let io = MockIo::new();
        add_deposit(0);
        enqueue_admin_action(ContractAdminAction::PauseMint).unwrap();
        run(&io, LaneKind::Minter);
        run(&io, LaneKind::Admin);
        assert_eq!(minter().next_nonce, Some(1));
        assert_eq!(admin_lane().next_nonce, Some(1));
        // Each lane signed with its own nonce space.
        let signed = io.chain().signed.clone();
        assert_eq!(signed.len(), 2);
        assert_eq!((signed[0].nonce, signed[1].nonce), (0, 0));
    }

    #[test]
    fn admin_actions_still_flow_while_the_bridge_is_globally_halted() {
        setup();
        let io = MockIo::new();
        state::halt("global");
        enqueue_admin_action(ContractAdminAction::PauseMint).unwrap();
        run(&io, LaneKind::Admin);
        assert!(admin_lane().unincluded_tx.is_some());
    }

    #[test]
    fn admin_queue_entry_without_a_record_is_dropped() {
        setup();
        let io = MockIo::new();
        state::mutate_core(|c| c.admin_lane.queued_txs.push(404));
        let ok = enqueue_admin_action(ContractAdminAction::PauseMint).unwrap();
        let out = run(&io, LaneKind::Admin);
        assert!(out.notes.iter().any(|n| n.contains("dropped")), "{:?}", out.notes);
        assert_eq!(admin_lane().queued_txs, vec![ok]);
        run(&io, LaneKind::Admin);
        assert_eq!(admin_lane().unincluded_tx, Some(ok));
    }

    // ----- clear_lane_halt ----------------------------------------------------------------------

    fn halted_clean_lane(next: u64) {
        setup();
        state::mutate_core(|c| {
            c.minter_lane.next_nonce = Some(next);
            c.minter_lane.halted = Some("test".into());
        });
    }

    #[test]
    fn clear_lane_halt_requires_agreement_between_chain_and_local_records() {
        halted_clean_lane(3);
        let io = MockIo::new();
        io.chain().counts.insert(MINTER, Counts { latest: 3, pending: 3, finalized: 3 });
        assert_eq!(block_on(clear_lane_halt_with(&io, LaneKind::Minter)), Ok(()));
        assert!(minter().halted.is_none());
        assert!(state::audit_page(0, 50).iter().any(|r| r.note.contains("halt cleared")));
        // Not halted any more.
        assert!(matches!(block_on(clear_lane_halt_with(&io, LaneKind::Minter)), Err(BridgeError::InvalidState(_))));
    }

    #[test]
    fn clear_lane_halt_rejects_a_finalized_count_that_differs() {
        for (finalized, latest) in [(2, 3), (4, 4), (3, 4), (4, 3)] {
            halted_clean_lane(3);
            let io = MockIo::new();
            io.chain().counts.insert(MINTER, Counts { latest, pending: latest, finalized });
            match block_on(clear_lane_halt_with(&io, LaneKind::Minter)) {
                Err(BridgeError::InvalidState(msg)) => {
                    assert!(msg.contains(&format!("finalized count {finalized}")), "{msg}");
                    assert!(msg.contains(&format!("latest count {latest}")), "{msg}");
                    assert!(msg.contains("local next nonce 3"), "{msg}");
                }
                other => panic!("({finalized},{latest}): {other:?}"),
            }
            assert!(minter().halted.is_some());
        }
    }

    #[test]
    fn clear_lane_halt_requires_an_empty_lane() {
        halted_clean_lane(3);
        let io = MockIo::new();
        io.chain().counts.insert(MINTER, Counts { latest: 3, pending: 3, finalized: 3 });
        state::mutate_core(|c| c.minter_lane.unincluded_tx = Some(1));
        assert!(matches!(block_on(clear_lane_halt_with(&io, LaneKind::Minter)), Err(BridgeError::InvalidState(_))));
        state::mutate_core(|c| {
            c.minter_lane.unincluded_tx = None;
            c.minter_lane.unfinalized_txs = vec![1];
        });
        assert!(matches!(block_on(clear_lane_halt_with(&io, LaneKind::Minter)), Err(BridgeError::InvalidState(_))));
        assert!(minter().halted.is_some());
    }

    #[test]
    fn clear_lane_halt_propagates_rpc_failure_without_clearing() {
        halted_clean_lane(3);
        let io = MockIo::new();
        io.chain().rpc_down = true;
        assert!(matches!(block_on(clear_lane_halt_with(&io, LaneKind::Minter)), Err(BridgeError::Rpc(_))));
        assert!(minter().halted.is_some());
    }

    // ----- misc ------------------------------------------------------------------------------------

    #[test]
    fn run_guard_blocks_overlap_and_is_reclaimed_when_stale() {
        setup();
        let g = RunGuard::acquire(LaneKind::Minter).expect("first");
        assert!(RunGuard::acquire(LaneKind::Minter).is_none());
        assert!(RunGuard::acquire(LaneKind::Admin).is_some(), "other lane is independent");
        let io = MockIo::new();
        let out = run(&io, LaneKind::Minter);
        assert!(out.notes.iter().any(|n| n.contains("already in progress")));
        // A guard left by a trapped (or very slow) run is reclaimed after ten minutes.
        test_env::advance_secs(601);
        let newer = RunGuard::acquire(LaneKind::Minter).expect("stale guard reclaimed");
        // The slow run finishing late must not clear the newer run's claim.
        drop(g);
        assert!(RunGuard::acquire(LaneKind::Minter).is_none(), "newer run still owns the lane");
        drop(newer);
        assert!(RunGuard::acquire(LaneKind::Minter).is_some());
    }

    #[test]
    fn preflight_simulates_with_the_configured_gas_limit() {
        setup();
        let io = MockIo::new();
        add_deposit(0);
        run(&io, LaneKind::Minter);
        let configured = state::read_core(|c| c.config.gas.mint_gas_limit);
        assert_eq!(io.chain().simulated_gas, vec![configured]);
        let id = minter().unincluded_tx.expect("mint created");
        assert_eq!(tx(id).gas_limit, configured, "the tx carries the simulated limit");
    }

    #[test]
    fn preflight_out_of_gas_holds_the_deposit_spends_no_nonce_and_retries_after_a_raise() {
        setup();
        let io = MockIo::new();
        let configured = state::read_core(|c| c.config.gas.mint_gas_limit);
        io.chain().mint_gas_needed = Some(configured + 1);
        add_deposit(0);
        run(&io, LaneKind::Minter);
        assert_eq!(minter().next_nonce, Some(0), "synced, nothing allocated");
        assert!(minter().unincluded_tx.is_none());
        assert_eq!(io.chain().sign_counter, 0, "nothing signed or broadcast");
        let d = state::get_deposit(0).unwrap();
        match &d.state {
            DepositState::MintHeld { reason } => assert!(reason.contains("ran out of gas"), "{reason}"),
            other => panic!("{other:?}"),
        }
        assert!(d.hold_retry_at_ns.is_some(), "a preflight hold retries automatically");
        // The operator raises the limit; the automatic retry then mints with the new limit.
        state::mutate_core(|c| c.config.gas.mint_gas_limit = configured + 1);
        test_env::advance_secs(24 * 3600);
        deposit::retry_due_holds();
        run(&io, LaneKind::Minter);
        let id = minter().unincluded_tx.expect("mint created after the raise");
        assert_eq!(tx(id).gas_limit, configured + 1);
    }

    #[test]
    fn a_gas_limit_change_between_simulation_and_creation_abandons_without_a_nonce() {
        setup();
        add_deposit(0);
        let configured = state::read_core(|c| c.config.gas.mint_gas_limit);
        let err = create_mint_tx_with(0, Some(configured - 1), deposit::on_mint_tx_created).unwrap_err();
        assert!(err.contains("gas limit changed"), "{err}");
        assert!(minter().unincluded_tx.is_none());
        assert_eq!(state::get_deposit(0).unwrap().state, DepositState::Escrowed);
    }

    #[test]
    fn idle_lane_reports_no_pending_work() {
        setup();
        let io = MockIo::new();
        let out = run(&io, LaneKind::Minter);
        assert!(!out.pending_work);
        assert_eq!(out.notes.len(), 0);
        enqueue_admin_action(ContractAdminAction::UnpauseMint).unwrap();
        assert!(lane_has_pending_work(LaneKind::Admin));
    }

    #[test]
    fn outcome_hook_failure_leaves_the_flag_false_and_records_a_note() {
        setup();
        let io = MockIo::new();
        let (d, id) = start_one_mint(&io);
        let hash = tx(id).attempts[0].tx_hash;
        io.include(hash, 300, 0xb1, true, vec![mint_log(&d)], MINTER);
        run(&io, LaneKind::Minter);
        // The deposit moved on by itself (for example an operator action): the hook will refuse.
        let mut dep = state::get_deposit(0).unwrap();
        dep.state = DepositState::MintHeld { reason: "manual".into() };
        dep.active_tx = None;
        state::put_deposit(&dep);
        io.finalize_up_to(300, MINTER);
        let out = run(&io, LaneKind::Minter);
        let t = tx(id);
        assert!(matches!(t.status, TxStatus::Finalized { .. }));
        assert!(!t.outcome_applied, "hook failed, so the flag stays false");
        assert!(t.note.as_deref().unwrap().contains("not waiting"), "{:?}", t.note);
        assert!(out.notes.iter().any(|n| n.contains("outcome not applied")), "{:?}", out.notes);
        assert!(minter().unfinalized_txs.is_empty(), "the lane is not blocked by it");
        // And it is never retried automatically.
        let audit_len = state::audit_len();
        run(&io, LaneKind::Minter);
        assert_eq!(state::audit_len(), audit_len);
    }

    #[test]
    fn production_io_fails_closed_on_the_host() {
        setup();
        // On the host the real RPC and signer are unavailable: a run must record notes and never panic.
        add_deposit(0);
        let out = block_on(run_lane(LaneKind::Minter));
        assert!(!out.notes.is_empty());
        assert!(minter().unincluded_tx.is_none());
        assert_eq!(state::get_deposit(0).unwrap().state, DepositState::Escrowed);
    }

    #[test]
    fn pending_receipts_are_checked_for_every_attempt_hash() {
        setup();
        let io = MockIo::new();
        let (_, id) = start_one_mint(&io);
        for _ in 0..3 {
            test_env::advance_secs(100);
            run(&io, LaneKind::Minter);
        }
        let t = tx(id);
        assert_eq!(t.attempts.len(), 4);
        let before = io.chain().receipt_calls;
        run(&io, LaneKind::Minter);
        assert_eq!(io.chain().receipt_calls - before, 4, "one lookup per attempt hash");
    }
}
