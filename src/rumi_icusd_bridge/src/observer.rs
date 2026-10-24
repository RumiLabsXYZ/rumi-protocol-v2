//! Owned by card BB-05. Contract binding, finalized bridge-out scanning and Base
//! supply reconciliation.
//!
//! The observer is the ONLY path that turns a Base burn into a release. It trusts
//! nothing a single provider says and nothing that is not final:
//!
//! - Every read is a quorum read at a `finalized` block (see `evm::rpc`).
//! - The contract's `bridgeOutCount()` never decreases and rises by exactly one per
//!   `BridgeOut` event, and every event carries its sequence number. Logs are accepted
//!   only if they are well formed, inside the requested range, and carry the sequences
//!   `processed + 1, processed + 2, ...` with no gap. A provider that invents, reorders
//!   or duplicates an event therefore cannot move the cursor.
//! - A scanned range is also checked against the counter at its end block (the counter
//!   must have moved by exactly the number of logs found) whenever a quorum can read
//!   that old state. Non-archive providers may not serve it. Then the range is accepted
//!   on sequence continuity alone and noted in the audit log, and completeness is proven
//!   where it matters: at the finalized head the counter must equal the processed count.
//!   If it is higher, events were hidden somewhere after the last processed event, and
//!   the cursor is moved back there so the blocks are scanned again.
//! - Position (`cursor_block`, `processed_count`, `last_event_block`) moves only after
//!   the events of a range are recorded, and recording is idempotent on the transfer id,
//!   so a retry after a partial failure cannot double count. A recording error stops the
//!   range at that event; the events before it stay counted.
//! - A burn cannot hide a mint from the contract's cumulative `totalMinted()` counter
//!   (unlike `totalSupply()`, which a mint followed by a burn leaves looking normal). So
//!   before a pass records ANY burn it reads `totalMinted()` at the same finalized head and
//!   refuses to go on when the contract has minted more than this canister did itself
//!   (`cumulative_minted_e8s`) plus what it may have in flight. That halts the bridge before
//!   a release can be paid out of honest users' escrow for tokens nobody backed.
//! - `eth_getLogs` adapts to the provider: a failing range is halved, a single block that
//!   still fails raises the response cap, and both recover after successes. A dense stretch
//!   of burns can therefore slow the scan down but never wedge it.

use crate::accounting::Buckets;
use crate::admin;
use crate::env;
use crate::evm::abi::{self, BridgeOutEvent, RawLog, ViewCall};
use crate::evm::rpc::{self, BlockRef, BlockTag, RpcError};
use crate::evm::signer;
use crate::flows::release::{record_finalized_bridge_out, RecordOutcome};
use crate::ids::{self, LaneKind};
use crate::state::{self, AuditCtx, ContractBinding};
use crate::types::{BridgeError, BridgeResult, ContractBindingView};
use candid::Nat;
use std::cell::Cell;

/// Upper bound on `eth_getLogs` ranges scanned in one pass, so a long backlog is worked
/// off over several timer ticks instead of one very expensive one.
pub const MAX_CHUNKS_PER_PASS: usize = 5;
/// How often one pass may halve a failing `eth_getLogs` range before giving up.
pub const MAX_HALVINGS_PER_PASS: u32 = 6;
/// Successful ranges in a row after which a raised response cap returns to the default.
pub const CAP_RESET_AFTER_SUCCESSES: u32 = 10;
/// Consecutive failed passes after which the observer backs off.
pub const BACKOFF_AFTER_FAILURES: u32 = 3;

const NS_PER_SEC: u64 = 1_000_000_000;
pub const BACKOFF_BASE_NS: u64 = 30 * NS_PER_SEC;
pub const BACKOFF_MAX_NS: u64 = 3_600 * NS_PER_SEC;
/// A cursor rewind that did not fix the omission is not repeated for the same position
/// sooner than this, so a provider set that keeps hiding an event cannot make the
/// observer rescan the same blocks over and over.
pub const REWIND_COOLDOWN_NS: u64 = 3_600 * NS_PER_SEC;

thread_local! {
    /// Ranges in a row whose `eth_getLogs` succeeded. Heap only: after an upgrade the
    /// raised cap simply takes another `CAP_RESET_AFTER_SUCCESSES` ranges to relax.
    static LOG_SUCCESSES: Cell<u32> = const { Cell::new(0) };
    /// `(processed_count, rewind target, time)` of the last cursor rewind. Heap only: it
    /// only damps repeated rewinds, it is not needed for correctness.
    static LAST_REWIND: Cell<Option<(u64, u64, u64)>> = const { Cell::new(None) };
}

/// Forget the heap-only observer memory (binding, tests).
pub(crate) fn reset_runtime_memory() {
    LOG_SUCCESSES.with(|s| s.set(0));
    LAST_REWIND.with(|r| r.set(None));
}

/// How long the observer waits after `failures` consecutive failed passes, if at all:
/// 30 s for the third, doubling each time, never more than an hour.
pub fn backoff_ns(failures: u32) -> Option<u64> {
    if failures < BACKOFF_AFTER_FAILURES {
        return None;
    }
    let exp = (failures - BACKOFF_AFTER_FAILURES).min(16);
    Some(BACKOFF_BASE_NS.saturating_mul(1u64 << exp).min(BACKOFF_MAX_NS))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ObserverOutcome {
    pub recorded: u32,
    pub pending_work: bool,
    pub note: Option<String>,
}

// ---------------------------------------------------------------------------
// Contract reads
// ---------------------------------------------------------------------------

async fn view_bytes(contract: &[u8; 20], call: ViewCall, at: u64) -> Result<Vec<u8>, String> {
    rpc::eth_call(contract, &abi::encode_view(&call), BlockTag::Number(at))
        .await
        .map_err(|e| format!("{call:?} at block {at}: {e}"))
}

async fn view_u128(contract: &[u8; 20], call: ViewCall, at: u64) -> Result<u128, String> {
    let ret = view_bytes(contract, call.clone(), at).await?;
    abi::decode_uint_u128(&ret).map_err(|e| format!("{call:?} at block {at}: {e:?}"))
}

async fn view_bool(contract: &[u8; 20], call: ViewCall, at: u64) -> Result<bool, String> {
    let ret = view_bytes(contract, call.clone(), at).await?;
    abi::decode_bool(&ret).map_err(|e| format!("{call:?} at block {at}: {e:?}"))
}

async fn view_address(contract: &[u8; 20], call: ViewCall, at: u64) -> Result<[u8; 20], String> {
    let ret = view_bytes(contract, call.clone(), at).await?;
    abi::decode_address(&ret).map_err(|e| format!("{call:?} at block {at}: {e:?}"))
}

async fn bridge_out_count(contract: &[u8; 20], at: u64) -> Result<u64, String> {
    let v = view_u128(contract, ViewCall::BridgeOutCount, at).await?;
    u64::try_from(v).map_err(|_| format!("bridgeOutCount {v} at block {at} does not fit u64"))
}

/// The cumulative-mint guard. The contract's `totalMinted()` counts every unit ever minted
/// and never decreases; this canister counts every mint it made and saw finalized
/// (`cumulative_minted_e8s`). Mints it has signed but not seen finalized may already be on
/// Base, so they are allowed on top. Anything above that is a mint this canister did not
/// make (a bug, or a compromised mint authority), even if it was burned again right away.
///
/// Returns the halt reason on a violation. Reads local state with no await in between, so
/// a mint that finalizes meanwhile only moves from "in flight" to "minted".
fn cumulative_mint_violation(total_minted: u128) -> Option<String> {
    let minted = state::read_core(|c| c.cumulative_minted_e8s);
    let inflight = state::inflight_mint_e8s();
    (total_minted > minted.saturating_add(inflight)).then(|| {
        format!("Base cumulative mints {total_minted} exceed the bridge's minted {minted} plus in flight {inflight}")
    })
}

// ---------------------------------------------------------------------------
// Binding
// ---------------------------------------------------------------------------

/// Binding is once-only and only on a bridge with nothing owed to or by anyone.
fn check_bindable() -> BridgeResult<()> {
    state::read_core(|c| {
        if !c.initialized {
            return Err(BridgeError::NotConfigured("the bridge is not initialized".into()));
        }
        if c.config.contract.is_some() {
            return Err(BridgeError::InvalidState("a Base contract is already bound; binding is once-only".into()));
        }
        if c.buckets != Buckets::default() {
            return Err(BridgeError::InvalidState(format!("cannot bind while accounting buckets are non-zero: {:?}", c.buckets)));
        }
        if state::active_deposit_count() > 0 || state::active_release_count() > 0 {
            return Err(BridgeError::InvalidState("cannot bind while deposits or releases are active".into()));
        }
        Ok(())
    })
}

/// What it costs to release a burn: the bridge's release fee plus the ledger fee. The
/// ledger fee is learned by the timer, so a bridge that has not ticked yet cannot say.
fn release_cost_e8s() -> BridgeResult<u128> {
    let (release_fee, ledger_fee) = state::read_core(|c| (c.config.fees.release_fee_e8s, c.ledger_fee_e8s));
    match ledger_fee {
        Some(f) => Ok(release_fee as u128 + f as u128),
        None => Err(BridgeError::InvalidState("ledger fee unknown; run a tick first".into())),
    }
}

/// Every burn the contract accepts must cover the cost of releasing it, otherwise a
/// dust burn could only be re-minted to its sender at the operator's gas cost.
fn check_min_bridge_out(min_bridge_out: u128) -> BridgeResult<()> {
    let cost = release_cost_e8s()?;
    if min_bridge_out <= cost {
        return Err(BridgeError::InvalidState(format!(
            "contract minBridgeOut() {min_bridge_out} must exceed the release fee plus the ledger fee ({cost})"
        )));
    }
    Ok(())
}

fn rpc_err(context: &str, e: impl std::fmt::Display) -> BridgeError {
    BridgeError::Rpc(format!("{context}: {e}"))
}

fn require(ok: bool, what: impl FnOnce() -> String) -> BridgeResult<()> {
    if ok {
        Ok(())
    } else {
        Err(BridgeError::InvalidState(what()))
    }
}

/// Bind the Base `BridgedIcUSD` contract. Every check is a quorum read at the same
/// finalized block, and the contract must be pristine: no supply, no bridge-outs, the
/// canister's minter lane as the only minter and its admin lane as the admin. The RPC
/// providers must serve the configured chain, and every burn the contract accepts must
/// cover the release cost.
pub async fn bind_contract(address: String) -> BridgeResult<ContractBindingView> {
    let contract = ids::parse_address(&address).map_err(BridgeError::InvalidArgument)?;
    if contract == [0u8; 20] {
        return Err(BridgeError::InvalidArgument("the zero address cannot be bound".into()));
    }
    check_bindable()?;
    release_cost_e8s()?;
    let minter = signer::lane_address(LaneKind::Minter).await.map_err(|e| BridgeError::Internal(format!("minter lane address: {e}")))?;
    let admin_addr = signer::lane_address(LaneKind::Admin).await.map_err(|e| BridgeError::Internal(format!("admin lane address: {e}")))?;
    if minter == admin_addr {
        return Err(BridgeError::Internal("minter and admin lane addresses are identical".into()));
    }
    if contract == minter || contract == admin_addr {
        return Err(BridgeError::InvalidArgument("the contract address equals one of this canister's own Base addresses".into()));
    }

    let want_chain = state::read_core(|c| c.config.chain_id);
    let served_chain = rpc::chain_id().await.map_err(|e| rpc_err("chain id", e))?;
    require(served_chain == want_chain, || {
        format!("the RPC providers serve chain id {served_chain}, this bridge is configured for chain id {want_chain}")
    })?;

    let head = rpc::finalized_head().await.map_err(|e| rpc_err("finalized head", e))?;
    let at = head.number;
    let read = |e: String| BridgeError::Rpc(e);

    let version_bytes = view_bytes(&contract, ViewCall::Version, at).await.map_err(read)?;
    let version = abi::decode_string(&version_bytes)
        .map_err(|e| BridgeError::InvalidState(format!("VERSION() is not a valid string (is there a contract at this address at finalized block {at}?): {e:?}")))?;
    require(version == ids::CONTRACT_VERSION, || format!("VERSION() is {version:?}, expected {:?}", ids::CONTRACT_VERSION))?;

    let decimals = view_u128(&contract, ViewCall::Decimals, at).await.map_err(read)?;
    require(decimals == 8, || format!("decimals() is {decimals}, expected 8"))?;
    let total_supply = view_u128(&contract, ViewCall::TotalSupply, at).await.map_err(read)?;
    require(total_supply == 0, || format!("totalSupply() is {total_supply}, expected 0"))?;
    let out_count = view_u128(&contract, ViewCall::BridgeOutCount, at).await.map_err(read)?;
    require(out_count == 0, || format!("bridgeOutCount() is {out_count}, expected 0"))?;
    // The cumulative-mint guard starts from zero on both sides; a contract that cannot
    // answer this (an older build) would leave the guard unable to ever run.
    let total_minted = view_u128(&contract, ViewCall::TotalMinted, at).await.map_err(read)?;
    require(total_minted == 0, || format!("totalMinted() is {total_minted}, expected 0"))?;

    let minter_has = view_bool(&contract, ViewCall::HasRole { role: abi::MINTER_ROLE, account: minter }, at).await.map_err(read)?;
    require(minter_has, || "the canister's minter address does not hold MINTER_ROLE".to_string())?;
    let admin_has = view_bool(&contract, ViewCall::HasRole { role: abi::MINTER_ROLE, account: admin_addr }, at).await.map_err(read)?;
    require(!admin_has, || "the canister's admin address holds MINTER_ROLE".to_string())?;
    let default_admin = view_address(&contract, ViewCall::DefaultAdmin, at).await.map_err(read)?;
    require(default_admin == admin_addr, || format!("defaultAdmin() is {}, expected the canister's admin address", ids::hex0x(&default_admin)))?;

    let supply_cap = view_u128(&contract, ViewCall::SupplyCap, at).await.map_err(read)?;
    let mint_window_limit = view_u128(&contract, ViewCall::MintWindowLimit, at).await.map_err(read)?;
    let mint_window_seconds = view_u128(&contract, ViewCall::MintWindowSeconds, at).await.map_err(read)?;
    let mint_window_seconds =
        u64::try_from(mint_window_seconds).map_err(|_| BridgeError::InvalidState(format!("mintWindowSeconds() {mint_window_seconds} does not fit u64")))?;
    let min_bridge_out = view_u128(&contract, ViewCall::MinBridgeOut, at).await.map_err(read)?;
    check_min_bridge_out(min_bridge_out)?;

    let now = env::now_ns();
    let binding = ContractBinding {
        address: contract,
        bound_at_ns: now,
        start_block: head.number,
        supply_cap,
        mint_window_limit,
        mint_window_seconds,
        min_bridge_out,
    };
    // The canister's own limits must never let more through than the contract allows.
    let limits = state::read_core(|c| c.config.limits.clone());
    admin::check_limits_within_contract(&limits, &binding).map_err(BridgeError::InvalidState)?;

    // Everything above awaited: check again that nothing changed under us (including the
    // fees the minimum was compared with), then commit in one write.
    check_bindable()?;
    check_min_bridge_out(min_bridge_out)?;
    state::mutate_core(|c| {
        c.config.contract = Some(binding.clone());
        c.observer.cursor_block = head.number;
        c.observer.processed_count = 0;
        c.observer.last_event_block = 0;
        c.observer.last_finalized = Some((head.number, head.hash));
        c.observer.last_error = None;
        c.observer.consecutive_failures = 0;
        c.observer.next_attempt_ns = None;
        c.observer.adaptive_chunk_blocks = None;
        c.observer.adaptive_logs_cap = None;
    });
    reset_runtime_memory();
    state::append_audit(
        None,
        AuditCtx::note(format!(
            "Base contract {} bound at finalized block {} ({}); supplyCap {supply_cap}, mintWindowLimit {mint_window_limit}, mintWindowSeconds {mint_window_seconds}, minBridgeOut {min_bridge_out}",
            ids::hex0x(&contract),
            head.number,
            ids::hex0x(&head.hash),
        )),
    );
    Ok(ContractBindingView {
        address: ids::hex0x(&contract),
        bound_at_ns: now,
        start_block: head.number,
        supply_cap_e8s: Nat::from(supply_cap),
        mint_window_limit_e8s: Nat::from(mint_window_limit),
        mint_window_seconds,
        min_bridge_out_e8s: Nat::from(min_bridge_out),
    })
}

// ---------------------------------------------------------------------------
// Scanning
// ---------------------------------------------------------------------------

/// Why a scanned range was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
enum DecodeFail {
    /// The first new log carries a sequence above the next expected one: events
    /// `processed + 1 ..` are missing, either hidden inside this range or in blocks
    /// scanned earlier.
    Gap(String),
    Bad(String),
}

impl std::fmt::Display for DecodeFail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeFail::Gap(m) | DecodeFail::Bad(m) => write!(f, "{m}"),
        }
    }
}

/// An event this canister already recorded, seen again (the scan restarts at the block of
/// the last processed event after a rewind or a partial failure). It must be exactly the
/// event on record.
fn matches_recorded(ev: &BridgeOutEvent, log: &RawLog) -> bool {
    state::release_by_sequence(ev.sequence) == Some(ev.transfer_id)
        && state::get_release(&ev.transfer_id).is_some_and(|r| {
            r.sequence == ev.sequence
                && r.amount_e8s == ev.amount_e8s
                && r.base_sender == ev.from
                && r.raw_icp_principal == ev.icp_principal
                && r.icp_subaccount == ev.icp_subaccount
                && r.base_tx_hash == log.tx_hash
                && r.base_block_number == log.block_number
                && r.log_index == log.log_index
        })
}

/// Decode and check the logs of one scanned range `[from, to]`; returns the NEW events.
///
/// Every log must be inside the range, be a well-formed `BridgeOut` of the bound
/// contract (not removed, exact topics and data). The slice is already ordered by
/// `(block, log index)`. Logs with a sequence at or below `processed` are only allowed at
/// the very start (a rescan from the block of the last processed event) and must match
/// the stored record exactly; every other log must carry the next expected sequence.
fn decode_chunk_typed(
    logs: &[RawLog],
    contract: &[u8; 20],
    from: u64,
    to: u64,
    processed: u64,
) -> Result<Vec<(BridgeOutEvent, RawLog)>, DecodeFail> {
    let mut out: Vec<(BridgeOutEvent, RawLog)> = Vec::with_capacity(logs.len());
    let mut previous_known: Option<u64> = None;
    for log in logs {
        if log.block_number < from || log.block_number > to {
            return Err(DecodeFail::Bad(format!("log at block {} is outside the scanned range [{from}, {to}]", log.block_number)));
        }
        let ev = abi::decode_bridge_out(log, contract)
            .map_err(|e| DecodeFail::Bad(format!("undecodable BridgeOut log at block {} index {}: {e:?}", log.block_number, log.log_index)))?;
        if ev.sequence <= processed {
            let in_order = out.is_empty() && previous_known.map(|p| ev.sequence == p + 1).unwrap_or(true);
            if !in_order || !matches_recorded(&ev, log) {
                return Err(DecodeFail::Bad(format!(
                    "BridgeOut sequence {} at block {} is at or below the {processed} already processed but does not match the recorded event",
                    ev.sequence, log.block_number
                )));
            }
            previous_known = Some(ev.sequence);
            continue;
        }
        let want = processed.saturating_add(1).saturating_add(out.len() as u64);
        if ev.sequence != want {
            let msg = format!(
                "BridgeOut sequence {} at block {} where {want} was expected (gap, duplicate or reorder)",
                ev.sequence, log.block_number
            );
            return Err(if out.is_empty() && ev.sequence > want { DecodeFail::Gap(msg) } else { DecodeFail::Bad(msg) });
        }
        out.push((ev, log.clone()));
    }
    Ok(out)
}

/// The observer's position and what it must keep consistent with it.
#[derive(Clone, Copy, Debug)]
struct Progress {
    cursor: u64,
    processed: u64,
    last_event_block: u64,
}

/// Write a new position, but only if nobody moved it since this pass read it.
/// `tuning` (when given) replaces the persisted adaptive range and response cap.
fn commit_progress(
    head: BlockRef,
    from: (u64, u64),
    to: Progress,
    tuning: Option<(Option<u64>, Option<u64>)>,
) -> Result<(), String> {
    state::mutate_core(|c| {
        if (c.observer.cursor_block, c.observer.processed_count) != from {
            return Err("observer position changed during the pass; discarding it".to_string());
        }
        c.observer.cursor_block = to.cursor;
        c.observer.processed_count = to.processed;
        c.observer.last_event_block = c.observer.last_event_block.max(to.last_event_block);
        if let Some((span, cap)) = tuning {
            c.observer.adaptive_chunk_blocks = span;
            c.observer.adaptive_logs_cap = cap;
        }
        note_head_in(c, head);
        Ok(())
    })
}

fn commit(head: BlockRef, from: (u64, u64), to: (u64, u64)) -> Result<(), String> {
    commit_progress(head, from, Progress { cursor: to.0, processed: to.1, last_event_block: 0 }, None)
}

fn note_head_in(c: &mut state::BridgeCore, head: BlockRef) {
    // Never move the recorded head backwards: a lagging quorum is not a regression.
    if c.observer.last_finalized.map(|(n, _)| head.number >= n).unwrap_or(true) {
        c.observer.last_finalized = Some((head.number, head.hash));
    }
}

#[derive(Clone, Copy, Debug)]
struct ScanCfg {
    contract: [u8; 20],
    start_block: u64,
    chunk_blocks: u64,
    default_cap: u64,
}

/// What the persisted tuning fields hold for a range and cap: `None` means the default.
fn tuning_values(cfg: &ScanCfg, span: u64, cap: u64) -> (Option<u64>, Option<u64>) {
    ((span < cfg.chunk_blocks).then_some(span), (cap > cfg.default_cap).then_some(cap))
}

fn save_tuning(cfg: &ScanCfg, span: u64, cap: u64) {
    let (s, c) = tuning_values(cfg, span, cap);
    state::mutate_core(|core| {
        core.observer.adaptive_chunk_blocks = s;
        core.observer.adaptive_logs_cap = c;
    });
}

/// Where a rescan starts: the block before the last processed event (so events after it
/// in the same block are found too), never before the binding.
fn rewind_target(processed: u64, last_event_block: u64, start_block: u64) -> u64 {
    if processed == 0 || last_event_block == 0 {
        start_block
    } else {
        last_event_block.saturating_sub(1).max(start_block)
    }
}

/// Move the cursor back to [`rewind_target`] because events are missing that may lie in
/// blocks the cursor has already passed. Returns the new cursor, or `None` when there is
/// nothing to rewind to, or when an earlier rewind for this very position did not help
/// and the cooldown has not passed (the failure then stays visible and the observer backs
/// off instead of rescanning forever).
fn rewind(at: Progress, start_block: u64) -> Option<u64> {
    let target = rewind_target(at.processed, at.last_event_block, start_block);
    if at.cursor <= target {
        return None;
    }
    let now = env::now_ns();
    let repeated = LAST_REWIND
        .with(|r| r.get())
        .is_some_and(|(p, t, when)| (p, t) == (at.processed, target) && now.saturating_sub(when) < REWIND_COOLDOWN_NS);
    if repeated {
        return None;
    }
    let moved = state::mutate_core(|c| {
        if (c.observer.cursor_block, c.observer.processed_count) != (at.cursor, at.processed) {
            return false;
        }
        c.observer.cursor_block = target;
        true
    });
    if !moved {
        return None;
    }
    LAST_REWIND.with(|r| r.set(Some((at.processed, target, now))));
    state::append_audit(
        None,
        AuditCtx::note(format!(
            "observer cursor moved back from block {} to {target}: bridgeOutCount() is ahead of the {} processed events",
            at.cursor, at.processed
        )),
    );
    Some(target)
}

/// `msg`, with a rewind of the cursor when one is warranted.
fn with_rewind(msg: String, at: Progress, start_block: u64) -> String {
    match rewind(at, start_block) {
        Some(to) => format!("{msg}; the missing events may lie in blocks already passed, so the cursor moved back from {} to {to} to scan them again", at.cursor),
        None => msg,
    }
}

/// A pass that cannot fetch the next logs. When it already moved the cursor this pass,
/// that is progress, not a failure (the reason is the note and the next pass continues);
/// when it did not, it is an error, so a stuck observer is counted and backs off.
fn stop_early(chunks_done: usize, recorded: u32, reason: String) -> Result<ObserverOutcome, String> {
    if chunks_done > 0 {
        Ok(ObserverOutcome { recorded, pending_work: true, note: Some(reason) })
    } else {
        Err(reason)
    }
}

async fn scan(cfg: ScanCfg) -> Result<ObserverOutcome, String> {
    let contract = cfg.contract;
    let (mut cursor, mut processed, mut last_event_block, previous, saved_span, saved_cap) = state::read_core(|c| {
        (
            c.observer.cursor_block,
            c.observer.processed_count,
            c.observer.last_event_block,
            c.observer.last_finalized,
            c.observer.adaptive_chunk_blocks,
            c.observer.adaptive_logs_cap,
        )
    });

    let head = rpc::finalized_head().await.map_err(|e| format!("finalized head: {e}"))?;
    if let Some((n, h)) = previous {
        if n == head.number && h != head.hash {
            let reason = format!(
                "finalized block {n} changed hash from {} to {}; a finalized block must never change",
                ids::hex0x(&h),
                ids::hex0x(&head.hash)
            );
            state::halt(reason.clone());
            return Err(reason);
        }
    }
    // Nothing to do at or behind the cursor, or while the head is behind events that are
    // already recorded (a lagging quorum after a partial pass).
    if head.number <= cursor || head.number < last_event_block {
        state::mutate_core(|c| note_head_in(c, head));
        let note = (head.number < cursor.max(last_event_block))
            .then(|| format!("finalized head {} is behind the cursor {}", head.number, cursor.max(last_event_block)));
        return Ok(ObserverOutcome { recorded: 0, pending_work: false, note });
    }

    // One read tells whether anything at all happened in (cursor, head].
    let count_head = bridge_out_count(&contract, head.number).await?;
    if count_head < processed {
        let reason = format!(
            "bridgeOutCount decreased: {count_head} at finalized block {} but {processed} events were already processed",
            head.number
        );
        state::halt(reason.clone());
        return Err(reason);
    }
    if count_head == processed {
        // Every event up to the head is accounted for, whatever was or was not verified
        // on the way.
        commit(head, (cursor, processed), (head.number, processed))?;
        LAST_REWIND.with(|r| r.set(None));
        return Ok(ObserverOutcome::default());
    }

    // Burns are about to be recorded (the counter is ahead of what is processed). Before
    // ANY of them is, make sure no mint happened that this canister did not make: a burn
    // of such tokens would be released out of escrow that backs honest users. Same head
    // as the count above. A failed read records nothing and does not advance.
    let total_minted = view_u128(&contract, ViewCall::TotalMinted, head.number)
        .await
        .map_err(|e| format!("cumulative mint guard: {e}; no burn is recorded until it can be read"))?;
    if let Some(reason) = cumulative_mint_violation(total_minted) {
        state::halt(reason.clone());
        return Err(reason);
    }

    let mut span = saved_span.unwrap_or(cfg.chunk_blocks).clamp(1, cfg.chunk_blocks);
    let mut cap = saved_cap.unwrap_or(cfg.default_cap).max(cfg.default_cap).min(rpc::MAX_RESPONSE_BYTES);
    let mut recorded = 0u32;
    let mut halvings = 0u32;
    let mut chunks = 0usize;
    let mut unverified = 0u32;
    while chunks < MAX_CHUNKS_PER_PASS && cursor < head.number {
        let from = cursor + 1;
        let to = cursor.saturating_add(span).min(head.number);
        let at = Progress { cursor, processed, last_event_block };
        let logs = match rpc::get_logs_with_cap(&contract, &abi::BRIDGE_OUT_TOPIC0, from, to, cap).await {
            Ok(l) => l,
            Err(e) => {
                LOG_SUCCESSES.with(|s| s.set(0));
                // The quorum errors already name the call and range.
                let what = match &e {
                    RpcError::AllFailed(_) | RpcError::NoQuorum(_) => e.to_string(),
                    other => format!("eth_getLogs [{from}, {to}]: {other}"),
                };
                if !matches!(e, RpcError::AllFailed(_) | RpcError::NoQuorum(_)) {
                    // Not something a smaller request can fix (configuration, bad range).
                    return Err(what);
                }
                if to > from {
                    // The response may be too large for the cap or the range too much for
                    // a provider: halve the range and try again within this pass.
                    if halvings >= MAX_HALVINGS_PER_PASS {
                        return stop_early(
                            chunks,
                            recorded,
                            format!("{what} (still failing after {MAX_HALVINGS_PER_PASS} range halvings in this pass)"),
                        );
                    }
                    halvings += 1;
                    // Half of the range just tried (it may be shorter than the span when it
                    // ends at the head), at least one block: floor((to - from + 1) / 2).
                    span = (to - from).div_ceil(2).max(1);
                    save_tuning(&cfg, span, cap);
                    continue;
                }
                // One block still fails. When EVERY provider failed, the response most
                // likely does not fit the cap: allow a larger one from the next pass on.
                // Providers that answered but disagree are not a size problem.
                if matches!(e, RpcError::NoQuorum(_)) {
                    return stop_early(chunks, recorded, format!("{what} (a single block; the providers that answered disagree)"));
                }
                if cap >= rpc::MAX_RESPONSE_BYTES {
                    return stop_early(
                        chunks,
                        recorded,
                        format!("{what} (a single block fails even at the largest response cap, {} bytes)", rpc::MAX_RESPONSE_BYTES),
                    );
                }
                let raised = cap.saturating_mul(2).min(rpc::MAX_RESPONSE_BYTES);
                save_tuning(&cfg, span, raised);
                return stop_early(chunks, recorded, format!("{what}; response cap raised to {raised} bytes for the next pass"));
            }
        };
        let events = match decode_chunk_typed(&logs, &contract, from, to, processed) {
            Ok(e) => e,
            Err(DecodeFail::Gap(msg)) => return Err(with_rewind(msg, at, cfg.start_block)),
            Err(DecodeFail::Bad(msg)) => return Err(msg),
        };
        let found = events.len() as u64;
        let expect = processed + found;
        // The completeness proof: the counter moved by exactly the number of logs found.
        // The head count is known; an older block may be beyond what the providers serve.
        let count_to = if to == head.number { Some(count_head) } else { bridge_out_count(&contract, to).await.ok() };
        let chunk_unverified = count_to.is_none();
        match count_to {
            Some(c) if c != expect => {
                let msg = format!(
                    "range [{from}, {to}] returned {found} BridgeOut logs but bridgeOutCount went from {processed} to {c}; not advancing"
                );
                // More events were counted than found: some may hide in blocks scanned
                // earlier without a count check.
                return Err(if c > expect { with_rewind(msg, at, cfg.start_block) } else { msg });
            }
            Some(_) => {}
            None if expect > count_head => {
                // Without a count at the range end the head count still bounds what can exist.
                return Err(format!(
                    "range [{from}, {to}] returned {found} BridgeOut logs, making {expect} processed, but bridgeOutCount is only {count_head} at the finalized head; not advancing"
                ));
            }
            None => {}
        }

        let mut done = 0u64;
        let mut newly = 0u32;
        let mut last_block = last_event_block;
        let mut failure = None;
        for (ev, log) in &events {
            match record_finalized_bridge_out(ev, log) {
                Ok(outcome) => {
                    if outcome == RecordOutcome::Recorded {
                        newly += 1;
                    }
                    done += 1;
                    last_block = last_block.max(log.block_number);
                }
                Err(e) => {
                    failure = Some(format!("recording BridgeOut {}: {e}", ev.sequence));
                    break;
                }
            }
        }
        recorded += newly;
        if let Some(msg) = failure {
            // Stop at the failing event. The ones before it are stored, so they stay
            // counted; the cursor does not pass the range, and the next pass starts here
            // again, skipping the events on record.
            if done > 0 {
                let kept = Progress { cursor, processed: processed + done, last_event_block: last_block };
                commit_progress(head, (cursor, processed), kept, None).map_err(|e| format!("{msg} ({e})"))?;
            }
            return Err(msg);
        }

        let next_processed = expect;
        // Once every event up to the head is recorded, nothing is left to find after
        // `to` either, so skip the remaining empty ranges.
        let next_cursor = if next_processed == count_head { head.number } else { to };
        let successes = LOG_SUCCESSES.with(|s| {
            let n = s.get().saturating_add(1);
            s.set(n);
            n
        });
        if successes >= CAP_RESET_AFTER_SUCCESSES {
            cap = cfg.default_cap;
        }
        span = span.saturating_mul(2).min(cfg.chunk_blocks);
        let next = Progress { cursor: next_cursor, processed: next_processed, last_event_block: last_block };
        commit_progress(head, (cursor, processed), next, Some(tuning_values(&cfg, span, cap)))?;
        if chunk_unverified {
            unverified += 1;
            state::append_audit(
                None,
                AuditCtx::note(format!(
                    "BridgeOut range [{from}, {to}] accepted on sequence continuity alone: bridgeOutCount() at block {to} could not be read by a quorum; completeness is proven at the finalized head"
                )),
            );
        }
        cursor = next.cursor;
        processed = next.processed;
        last_event_block = next.last_event_block;
        chunks += 1;
    }
    if cursor >= head.number {
        LAST_REWIND.with(|r| r.set(None));
    }
    let note = (unverified > 0)
        .then(|| format!("{unverified} range(s) accepted without a count check at their end block (historical state unavailable)"));
    Ok(ObserverOutcome { recorded, pending_work: cursor < head.number, note })
}

/// A full pass (finalized head read, which needs a large response cap) runs at least
/// this often even when nothing changed, so the cursor stays near the head.
pub const FULL_PASS_EVERY_NS: u64 = 3_600 * 1_000_000_000;

/// Cheap idle probe: `bridgeOutCount()` at the `finalized` tag, one small round. Returns
/// true only when a quorum agrees nothing new was burned. Any disagreement, error or
/// change means the caller must run a full pass.
async fn idle_probe_unchanged(contract: &[u8; 20], processed: u64) -> bool {
    match rpc::eth_call(contract, &abi::encode_view(&ViewCall::BridgeOutCount), rpc::BlockTag::Finalized).await {
        Ok(ret) => abi::decode_uint_u128(&ret).map(|v| v == processed as u128).unwrap_or(false),
        Err(_) => false,
    }
}

/// One bounded scan, called by the timer. On any failure the position is left where the
/// last fully verified range ended, the failure is recorded, and the error is returned.
/// After three failed passes in a row the observer backs off (30 s, doubling, at most an
/// hour); a pass that starts inside the back-off window does nothing.
pub async fn run_pass() -> Result<ObserverOutcome, String> {
    let Some(cfg) = state::read_core(|c| {
        c.config.contract.as_ref().map(|b| ScanCfg {
            contract: b.address,
            start_block: b.start_block,
            chunk_blocks: c.config.observer.logs_chunk_blocks.max(1),
            default_cap: c.config.observer.logs_max_response_bytes,
        })
    }) else {
        return Ok(ObserverOutcome::default());
    };
    let started = env::now_ns();
    let (processed, last_full, wait_until) =
        state::read_core(|c| (c.observer.processed_count, c.observer.last_full_pass_ns, c.observer.next_attempt_ns));
    if wait_until.is_some_and(|t| started < t) {
        return Ok(ObserverOutcome::default());
    }
    let full_due = last_full.map(|t| started.saturating_sub(t) >= FULL_PASS_EVERY_NS).unwrap_or(true);
    if !full_due && idle_probe_unchanged(&cfg.contract, processed).await {
        // Nothing new at finalized; leave the cursor for the next full pass.
        state::mutate_core(|c| {
            if c.observer.processed_count == processed {
                c.observer.last_run_ns = Some(env::now_ns());
                c.observer.last_error = None;
                c.observer.consecutive_failures = 0;
                c.observer.next_attempt_ns = None;
            }
        });
        return Ok(ObserverOutcome::default());
    }
    let result = scan(cfg).await;
    let now = env::now_ns();
    state::mutate_core(|c| {
        c.observer.last_run_ns = Some(now);
        match &result {
            Ok(_) => {
                c.observer.last_error = None;
                c.observer.consecutive_failures = 0;
                c.observer.next_attempt_ns = None;
                c.observer.last_full_pass_ns = Some(now);
            }
            Err(e) => {
                c.observer.last_error = Some(e.clone());
                c.observer.consecutive_failures = c.observer.consecutive_failures.saturating_add(1);
                c.observer.next_attempt_ns = backoff_ns(c.observer.consecutive_failures).map(|d| now.saturating_add(d));
            }
        }
    });
    result
}

// ---------------------------------------------------------------------------
// Supply reconciliation
// ---------------------------------------------------------------------------

/// Read `totalSupply()` at the finalized head and evaluate invariants C2/C3.
///
/// Recorded burns reduce the outstanding Base supply, so the reading is only meaningful
/// when every recorded burn is at or below the head (`processed_count <= bridgeOutCount`
/// at the head); otherwise the check is skipped. The exact match (C3) is only claimed
/// when nothing can legitimately differ: every burn up to the head is recorded, every
/// settled mint is at or below the head, and no mint is in flight or pending. It does not
/// depend on how far the cheap idle observer has moved its cursor.
pub async fn check_base_supply() -> Result<state::BaseSupplySnapshot, String> {
    let Some(contract) = state::read_core(|c| c.config.contract.as_ref().map(|b| b.address)) else {
        return Err("no Base contract is bound".into());
    };
    let head = rpc::finalized_head().await.map_err(|e| format!("finalized head: {e}"))?;
    let total = view_u128(&contract, ViewCall::TotalSupply, head.number).await?;
    let count_head = bridge_out_count(&contract, head.number).await?;
    let total_minted = view_u128(&contract, ViewCall::TotalMinted, head.number).await?;
    // The cumulative guard needs neither quiescence nor the burn bookkeeping below: it holds
    // whatever is in flight and whatever the observer has processed. A violation halts at
    // once, and the snapshot reports the supply as NOT within bound whatever `totalSupply`
    // says (a mint burned again leaves it looking normal). The timer stores that snapshot and
    // `admin_clear_halt` needs a fresh within-bound one, so the halt cannot be cleared while
    // the excess exists; a cumulative count never comes back down.
    let violation = cumulative_mint_violation(total_minted);
    if let Some(reason) = &violation {
        state::halt(reason.clone());
    }
    // No await between reading local state and building the snapshot.
    let (skip, quiescent) = state::read_core(|c| {
        (
            c.observer.processed_count > count_head,
            count_head == c.observer.processed_count && head.number >= c.observer.max_settled_block && c.buckets.pending_mint == 0,
        )
    });
    if skip {
        return Err("recorded burns are above the finalized head; supply check skipped".into());
    }
    let quiescent = quiescent && state::inflight_mint_e8s() == 0;
    let mut snapshot = crate::timer::base_supply_snapshot(total, head.number, quiescent);
    if violation.is_some() {
        snapshot.within_bound = false;
    }
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evm::rpc::testutil::*;
    use crate::evm::rpc::{mock_calls, set_mock_transport};
    use crate::ids::{word_address, word_u128, word_u64};
    use crate::state::TxStatus;
    use candid::Principal;
    use icrc_ledger_types::icrc1::account::Account;
    use serde_json::{json, Value};
    use std::cell::RefCell;
    use std::rc::Rc;

    const CONTRACT: [u8; 20] = [0xcc; 20];
    const MINTER: [u8; 20] = [0xaa; 20];
    const ADMIN: [u8; 20] = [0xbb; 20];
    const E8: u64 = 100_000_000;

    // ----- a scripted Base chain ----------------------------------------------------

    type Call = (usize, String, Option<(u64, u64)>);

    struct Prov {
        down: bool,
        finalized: u64,
        /// Mixed into this provider's block hashes; 0 is canonical.
        salt: u8,
        /// Replaces the canonical logs for this provider.
        logs: Option<Vec<RawLog>>,
        /// Replaces `totalSupply()` for this provider.
        supply: Option<u128>,
    }

    /// Cost of one scripted log in a response body, measured once.
    fn wire_len(logs: &[RawLog]) -> usize {
        ok_body(Value::Array(logs.iter().map(log_json).collect())).len()
    }

    struct Sim {
        providers: Vec<Prov>,
        /// Logs a node would return for the BridgeOut filter.
        logs: Vec<RawLog>,
        /// The block of each event the contract's counter has counted.
        count_blocks: Vec<u64>,
        forced_count: Option<u128>,
        version: String,
        decimals: u128,
        total_supply: u128,
        /// The contract's cumulative `totalMinted()` (what the canister minted, plus any
        /// mint it did not make).
        total_minted: u128,
        /// `totalMinted()` fails on every provider.
        fail_total_minted: bool,
        /// How many `totalMinted()` calls the providers served (per provider call).
        total_minted_reads: u32,
        supply_cap: u128,
        window_limit: u128,
        window_seconds: u128,
        min_out: u128,
        roles: Vec<([u8; 32], [u8; 20])>,
        default_admin: [u8; 20],
        /// `(provider, method, block or range)`.
        calls: Vec<Call>,
        chain_id: u64,
        /// Fail an `eth_getLogs` response larger than the response cap the canister sent
        /// (the EVM RPC canister reports an oversized body as a failed call).
        enforce_cap: bool,
        /// `eth_call` at a block below this one fails (a non-archive node).
        pruned_below: u64,
        /// While `omit_budget > 0`, an `eth_getLogs` that ends at block `omit_at_to` loses
        /// its last log (providers that all truncate the same response).
        omit_at_to: u64,
        omit_budget: u32,
        /// Any `eth_getLogs` response with at least one log fails, whatever the cap.
        fail_logs_with_hits: bool,
    }

    fn abi_string(s: &str) -> Vec<u8> {
        let mut out = word_u64(0x20).to_vec();
        out.extend_from_slice(&word_u64(s.len() as u64));
        out.extend_from_slice(s.as_bytes());
        out.resize(64 + s.len().div_ceil(32) * 32, 0);
        out
    }

    impl Sim {
        fn new(n: usize, finalized: u64) -> Sim {
            Sim {
                providers: (0..n).map(|_| Prov { down: false, finalized, salt: 0, logs: None, supply: None }).collect(),
                logs: vec![],
                count_blocks: vec![],
                forced_count: None,
                version: ids::CONTRACT_VERSION.to_string(),
                decimals: 8,
                total_supply: 0,
                total_minted: 0,
                fail_total_minted: false,
                total_minted_reads: 0,
                supply_cap: 100_000 * E8 as u128,
                window_limit: 10_000 * E8 as u128,
                window_seconds: 86_400,
                min_out: E8 as u128,
                roles: vec![(abi::MINTER_ROLE, MINTER)],
                default_admin: ADMIN,
                calls: vec![],
                chain_id: state::CHAIN_BASE_SEPOLIA,
                enforce_cap: true,
                pruned_below: 0,
                omit_at_to: 0,
                omit_budget: 0,
                fail_logs_with_hits: false,
            }
        }

        fn add_event(&mut self, seq: u64, block: u64, amount: u64) {
            self.add_event_at(seq, block, 0, amount);
        }

        fn add_event_at(&mut self, seq: u64, block: u64, index: u64, amount: u64) {
            self.logs.push(bridge_out_log(seq, block, index, amount, Principal::from_slice(&[0x11; 29]).as_slice()));
            self.count_blocks.push(block);
        }

        fn count_at(&self, n: u64) -> u128 {
            self.forced_count.unwrap_or_else(|| self.count_blocks.iter().filter(|b| **b <= n).count() as u128)
        }

        fn handle(&mut self, p: usize, body: &str) -> Result<String, String> {
            let (method, params) = request_of(body);
            let num = |v: &Value| -> u64 { u64::from_str_radix(v.as_str().unwrap().trim_start_matches("0x"), 16).unwrap() };
            let block_arg = |prov: &Prov, v: &Value| -> u64 {
                match v.as_str().unwrap() {
                    "finalized" | "latest" => prov.finalized,
                    hex => u64::from_str_radix(hex.trim_start_matches("0x"), 16).unwrap(),
                }
            };
            let range = match method.as_str() {
                "eth_getLogs" => Some((num(&params[0]["fromBlock"]), num(&params[0]["toBlock"]))),
                "eth_call" => {
                    let b = block_arg(&self.providers[p], &params[1]);
                    Some((b, b))
                }
                _ => None,
            };
            self.calls.push((p, method.clone(), range));
            let prov = &self.providers[p];
            if prov.down {
                return Err("provider down".into());
            }
            match method.as_str() {
                "eth_chainId" => Ok(ok_body(json!(format!("0x{:x}", self.chain_id)))),
                "eth_getBlockByNumber" => {
                    let n = block_arg(prov, &params[0]);
                    Ok(ok_body(block_json(n, h32(n, prov.salt))))
                }
                "eth_getLogs" => {
                    let (from, to) = range.unwrap();
                    let src = prov.logs.as_ref().unwrap_or(&self.logs);
                    let mut hits: Vec<RawLog> = src.iter().filter(|l| l.block_number >= from && l.block_number <= to).cloned().collect();
                    if self.omit_budget > 0 && to == self.omit_at_to && !hits.is_empty() {
                        self.omit_budget -= 1;
                        hits.pop();
                    }
                    let body = ok_body(Value::Array(hits.iter().map(log_json).collect()));
                    let cap = mock_calls().last().expect("the transport records the call first").max_response_bytes;
                    if (self.enforce_cap && body.len() as u64 > cap) || (self.fail_logs_with_hits && !hits.is_empty()) {
                        return Err(format!("Http body exceeds size limit of {cap} bytes"));
                    }
                    Ok(body)
                }
                "eth_call" => {
                    assert_eq!(params[0]["to"], json!(ids::hex0x(&CONTRACT)), "views must target the bound contract");
                    let data = hex::decode(params[0]["data"].as_str().unwrap().trim_start_matches("0x")).unwrap();
                    let at = range.unwrap().0;
                    if at < self.pruned_below {
                        return Err(format!("missing trie node (state at block {at} is pruned)"));
                    }
                    let sel: [u8; 4] = data[..4].try_into().unwrap();
                    let word = |v: u128| word_u128(v).to_vec();
                    let ret = match sel {
                        abi::SEL_VERSION => abi_string(&self.version),
                        abi::SEL_DECIMALS => word(self.decimals),
                        abi::SEL_TOTAL_SUPPLY => word(prov.supply.unwrap_or(self.total_supply)),
                        abi::SEL_TOTAL_MINTED if self.fail_total_minted => return Err("totalMinted() is not served".into()),
                        abi::SEL_TOTAL_MINTED => {
                            self.total_minted_reads += 1;
                            word(self.total_minted)
                        }
                        abi::SEL_BRIDGE_OUT_COUNT => word(self.count_at(at)),
                        abi::SEL_SUPPLY_CAP => word(self.supply_cap),
                        abi::SEL_MINT_WINDOW_LIMIT => word(self.window_limit),
                        abi::SEL_MINT_WINDOW_SECONDS => word(self.window_seconds),
                        abi::SEL_MIN_BRIDGE_OUT => word(self.min_out),
                        abi::SEL_DEFAULT_ADMIN => word_address(&self.default_admin).to_vec(),
                        abi::SEL_HAS_ROLE => {
                            let role: [u8; 32] = data[4..36].try_into().unwrap();
                            let account: [u8; 20] = data[48..68].try_into().unwrap();
                            word(self.roles.contains(&(role, account)) as u128)
                        }
                        other => return Err(format!("unexpected selector {other:?}")),
                    };
                    Ok(ok_body(json!(ids::hex0x(&ret))))
                }
                other => Err(format!("unexpected method {other}")),
            }
        }
    }

    fn bridge_out_log(seq: u64, block: u64, index: u64, amount: u64, principal: &[u8]) -> RawLog {
        let mut tid = [0u8; 32];
        tid[..8].copy_from_slice(&seq.to_be_bytes());
        tid[31] = 0xaa;
        let mut data = Vec::new();
        data.extend_from_slice(&word_u64(amount));
        data.extend_from_slice(&word_u64(seq));
        data.extend_from_slice(&word_u64(0x80));
        data.extend_from_slice(&[0u8; 32]);
        data.extend_from_slice(&word_u64(principal.len() as u64));
        let mut pw = [0u8; 32];
        pw[..principal.len()].copy_from_slice(principal);
        data.extend_from_slice(&pw);
        RawLog {
            address: CONTRACT,
            topics: vec![abi::BRIDGE_OUT_TOPIC0, tid, word_address(&[0x55; 20])],
            data,
            block_number: block,
            block_hash: h32(block, 0),
            tx_hash: h32(seq, 0x77),
            log_index: index,
            removed: false,
        }
    }

    fn install(sim: Sim) -> Rc<RefCell<Sim>> {
        let sim = Rc::new(RefCell::new(sim));
        let s = sim.clone();
        set_mock_transport(move |svc, body| s.borrow_mut().handle(provider_index(svc), body));
        sim
    }

    fn lanes() {
        state::mutate_core(|c| {
            c.minter_lane.address = Some(MINTER);
            c.minter_lane.public_key = Some(vec![2; 33]);
            c.admin_lane.address = Some(ADMIN);
            c.admin_lane.public_key = Some(vec![2; 33]);
        });
    }

    /// Providers configured, canister initialized with a bound contract at `cursor`.
    fn bound(n: usize, quorum: u8, cursor: u64) {
        configure(n, quorum);
        lanes();
        state::mutate_core(|c| {
            c.initialized = true;
            c.config.chain_id = state::CHAIN_BASE_SEPOLIA;
            c.config.contract = Some(ContractBinding {
                address: CONTRACT,
                bound_at_ns: 1,
                start_block: cursor,
                supply_cap: u128::MAX,
                mint_window_limit: u128::MAX,
                mint_window_seconds: 1,
                min_bridge_out: 1,
            });
            c.instance_salt = [9; 32];
            c.ledger_fee_e8s = Some(10_000);
            c.ledger_minting_account = Some(Account { owner: Principal::from_slice(&[0x4d; 10]), subaccount: None });
            c.buckets.outstanding_base = 1_000 * E8 as u128;
            c.observer.cursor_block = cursor;
        });
    }

    /// Providers configured, initialized, nothing bound yet.
    fn unbound(n: usize, quorum: u8) {
        configure(n, quorum);
        lanes();
        state::mutate_core(|c| {
            c.initialized = true;
            c.config.chain_id = state::CHAIN_BASE_SEPOLIA;
            c.ledger_fee_e8s = Some(10_000);
        });
    }

    fn pos() -> (u64, u64) {
        state::read_core(|c| (c.observer.cursor_block, c.observer.processed_count))
    }

    fn run() -> Result<ObserverOutcome, String> {
        block_on(run_pass())
    }

    fn decode_chunk(logs: &[RawLog], contract: &[u8; 20], from: u64, to: u64, processed: u64) -> Result<Vec<(BridgeOutEvent, RawLog)>, String> {
        decode_chunk_typed(logs, contract, from, to, processed).map_err(|e| e.to_string())
    }

    fn count_of(sim: &Rc<RefCell<Sim>>, method: &str) -> usize {
        sim.borrow().calls.iter().filter(|(_, m, _)| m == method).count()
    }

    // ----- idle path ----------------------------------------------------------------

    #[test]
    fn idle_pass_moves_the_cursor_without_asking_for_logs() {
        bound(3, 2, 100);
        let sim = install(Sim::new(3, 150));
        let out = run().unwrap();
        assert_eq!(out, ObserverOutcome::default());
        assert_eq!(pos(), (150, 0));
        assert_eq!(state::read_core(|c| c.observer.last_finalized), Some((150, h32(150, 0))));
        assert_eq!(count_of(&sim, "eth_getLogs"), 0);
        // Two rounds with level providers: the finalized block, then bridgeOutCount.
        assert_eq!(sim.borrow().calls.len(), 6);
        let (err, fails, last) = state::read_core(|c| (c.observer.last_error.clone(), c.observer.consecutive_failures, c.observer.last_run_ns));
        assert_eq!((err, fails), (None, 0));
        assert!(last.is_some());
    }

    #[test]
    fn idle_probe_skips_the_head_read_between_full_passes() {
        bound(3, 2, 100);
        let sim = install(Sim::new(3, 150));
        run().unwrap();
        assert_eq!(pos(), (150, 0));
        sim.borrow_mut().calls.clear();
        // Within the hour and nothing new: one small eth_call round, no block reads.
        for p in &mut sim.borrow_mut().providers {
            p.finalized = 180;
        }
        assert_eq!(run().unwrap(), ObserverOutcome::default());
        assert_eq!(count_of(&sim, "eth_getBlockByNumber"), 0);
        assert_eq!(count_of(&sim, "eth_call"), 3);
        assert_eq!(pos(), (150, 0), "the cursor waits for the next full pass");
        // An hour later a full pass runs and advances the cursor.
        crate::env::test_env::advance_secs(3_601);
        sim.borrow_mut().calls.clear();
        run().unwrap();
        assert!(count_of(&sim, "eth_getBlockByNumber") > 0);
        assert_eq!(pos(), (180, 0));
    }

    #[test]
    fn idle_probe_that_sees_a_new_burn_runs_a_full_pass() {
        bound(3, 2, 100);
        let sim = install(Sim::new(3, 150));
        run().unwrap();
        sim.borrow_mut().calls.clear();
        // Any provider disagreement or error on the probe also forces a full pass.
        sim.borrow_mut().providers[0].down = true;
        sim.borrow_mut().providers[1].down = true;
        let _ = run();
        assert!(count_of(&sim, "eth_getBlockByNumber") > 0, "probe failure falls back to a full pass");
    }

    #[test]
    fn a_head_at_or_behind_the_cursor_changes_nothing_and_reads_no_counts() {
        bound(3, 2, 200);
        let sim = install(Sim::new(3, 200));
        assert_eq!(run().unwrap(), ObserverOutcome::default());
        assert_eq!(count_of(&sim, "eth_call"), 0);
        assert_eq!(pos(), (200, 0));
        // Lagging quorum: reported with a note, last_finalized does not regress. Force a
        // full pass (the cheap idle probe would otherwise answer within the hour).
        state::mutate_core(|c| {
            c.observer.last_finalized = Some((200, h32(200, 0)));
            c.observer.last_full_pass_ns = None;
        });
        for p in &mut sim.borrow_mut().providers {
            p.finalized = 190;
        }
        let out = run().unwrap();
        assert!(out.note.unwrap().contains("behind the cursor"));
        assert_eq!(pos(), (200, 0));
        assert_eq!(state::read_core(|c| c.observer.last_finalized), Some((200, h32(200, 0))));
    }

    #[test]
    fn an_unbound_bridge_does_nothing() {
        unbound(3, 2);
        let sim = install(Sim::new(3, 150));
        assert_eq!(run().unwrap(), ObserverOutcome::default());
        assert!(sim.borrow().calls.is_empty());
    }

    // ----- recording ----------------------------------------------------------------

    #[test]
    fn two_events_in_one_chunk_are_recorded_once_and_replay_is_a_no_op() {
        bound(3, 2, 100);
        let mut s = Sim::new(3, 150);
        s.add_event(1, 120, 100 * E8);
        s.add_event(2, 130, 50 * E8);
        let sim = install(s);
        let out = run().unwrap();
        assert_eq!(out, ObserverOutcome { recorded: 2, pending_work: false, note: None });
        assert_eq!(pos(), (150, 2));
        let b = state::read_core(|c| c.buckets);
        assert_eq!(b.outstanding_base, (850 * E8) as u128);
        assert_eq!(b.pending_release, (150 * E8) as u128);
        assert_eq!(state::releases_in_state(|_| true, 10).len(), 2);
        // One head round, one count round, one cumulative-mint round, one logs round; the
        // head count is reused for the chunk that ends at the head.
        assert_eq!(count_of(&sim, "eth_getLogs"), 3);
        assert_eq!(count_of(&sim, "eth_call"), 6);
        assert_eq!(sim.borrow().calls.iter().find(|(_, m, _)| m == "eth_getLogs").unwrap().2, Some((101, 150)));

        // Replay the same range from the start: no second effect.
        state::mutate_core(|c| {
            c.observer.cursor_block = 100;
            c.observer.processed_count = 0;
        });
        let out = run().unwrap();
        assert_eq!(out.recorded, 0);
        assert_eq!(state::read_core(|c| c.buckets), b);
        assert_eq!(state::releases_in_state(|_| true, 10).len(), 2);
        assert_eq!(pos(), (150, 2));
    }

    #[test]
    fn several_chunks_each_prove_completeness_with_their_own_count() {
        bound(3, 2, 100);
        state::mutate_core(|c| c.config.observer.logs_chunk_blocks = 10);
        let mut s = Sim::new(3, 130);
        s.add_event(1, 105, 10 * E8);
        s.add_event(2, 125, 10 * E8);
        let sim = install(s);
        let out = run().unwrap();
        assert_eq!(out, ObserverOutcome { recorded: 2, pending_work: false, note: None });
        assert_eq!(pos(), (130, 2));
        let ranges: Vec<(u64, u64)> = {
            let s = sim.borrow();
            let per_provider = s.calls.iter().filter(|(p, m, _)| *p == 0 && m == "eth_getLogs").map(|(_, _, r)| r.unwrap());
            per_provider.collect()
        };
        assert_eq!(ranges, vec![(101, 110), (111, 120), (121, 130)]);
        // Reads at 130 (head count, then totalMinted), then counts at 110 and 120; the head
        // count is reused for the last chunk.
        let at: Vec<u64> = sim.borrow().calls.iter().filter(|(p, m, _)| *p == 0 && m == "eth_call").map(|(_, _, r)| r.unwrap().0).collect();
        assert_eq!(at, vec![130, 130, 110, 120]);
    }

    #[test]
    fn at_most_five_chunks_per_pass_and_empty_tail_is_skipped() {
        bound(3, 2, 100);
        state::mutate_core(|c| c.config.observer.logs_chunk_blocks = 10);
        let mut s = Sim::new(3, 300);
        s.add_event(1, 290, 10 * E8);
        let sim = install(s);
        let out = run().unwrap();
        assert_eq!(out, ObserverOutcome { recorded: 0, pending_work: true, note: None });
        assert_eq!(pos(), (150, 0));
        assert_eq!(count_of(&sim, "eth_getLogs"), 5 * 3);
        assert!(run().unwrap().pending_work);
        assert_eq!(pos(), (200, 0));
        assert!(run().unwrap().pending_work);
        assert_eq!(pos(), (250, 0));
        // The fourth pass reaches the event; every event up to the head is then known, so
        // the cursor jumps to the head instead of scanning [291, 300] separately.
        let out = run().unwrap();
        assert_eq!(out, ObserverOutcome { recorded: 1, pending_work: false, note: None });
        assert_eq!(pos(), (300, 1));
    }

    // ----- anomalies: nothing advances -----------------------------------------------

    /// The scan may creep up to a block the providers cannot agree on, but never past it,
    /// and nothing is counted.
    fn assert_blocked_before(block: u64) {
        let (cursor, processed) = pos();
        assert!(cursor < block, "the cursor {cursor} must stop before block {block}");
        assert_eq!(processed, 0);
    }

    fn assert_stuck(expect_in_error: &str) {
        let (err, fails) = state::read_core(|c| (c.observer.last_error.clone(), c.observer.consecutive_failures));
        let err = err.expect("a failure must be visible");
        assert!(err.contains(expect_in_error), "{err}");
        assert_eq!(fails, 1);
        assert!(state::releases_in_state(|_| true, 10).is_empty(), "nothing may be recorded");
        assert_eq!(state::read_core(|c| c.buckets.pending_release), 0);
    }

    #[test]
    fn a_missing_event_does_not_advance_even_with_a_chunk_ending_before_the_head() {
        // Chunk ends at the head: count (2) exceeds logs found (1).
        bound(3, 2, 100);
        let mut s = Sim::new(3, 150);
        s.add_event(1, 120, 10 * E8);
        s.add_event(2, 130, 10 * E8);
        s.logs.pop();
        install(s);
        assert!(run().is_err());
        assert_eq!(pos(), (100, 0));
        assert_stuck("bridgeOutCount went from 0 to 2");

        // Chunk ends before the head: counted at the chunk end.
        bound(3, 2, 100);
        state::mutate_core(|c| c.config.observer.logs_chunk_blocks = 10);
        let mut s = Sim::new(3, 150);
        s.add_event(1, 105, 10 * E8);
        s.logs.clear();
        install(s);
        assert!(run().is_err());
        assert_eq!(pos(), (100, 0));
        assert_stuck("returned 0 BridgeOut logs but bridgeOutCount went from 0 to 1");
    }

    #[test]
    fn an_invented_event_does_not_advance() {
        bound(3, 2, 100);
        let mut s = Sim::new(3, 150);
        s.add_event(1, 120, 10 * E8);
        s.count_blocks.clear();
        s.count_blocks.push(500); // the counter says nothing happened by block 150
        install(s);
        assert!(run().is_ok(), "count 0 at the head means the idle path, the invented log is never even fetched");
        assert_eq!(pos(), (150, 0));
        // With a counter that moved by one but two logs returned, completeness fails.
        bound(3, 2, 100);
        let mut s = Sim::new(3, 150);
        s.add_event(1, 120, 10 * E8);
        s.logs.push(bridge_out_log(2, 130, 0, 10 * E8, &[0x11; 29]));
        install(s);
        assert!(run().is_err());
        assert_eq!(pos(), (100, 0));
        assert_stuck("bridgeOutCount went from 0 to 1");
    }

    #[test]
    fn a_sequence_gap_duplicate_or_late_start_does_not_advance() {
        for (name, seqs, counted) in [
            ("gap", vec![(1u64, 120u64), (3, 130)], 2usize),
            ("duplicate", vec![(1, 120), (1, 130)], 2),
            ("late start", vec![(2, 120)], 1),
        ] {
            bound(3, 2, 100);
            let mut s = Sim::new(3, 150);
            for (seq, block) in seqs {
                s.logs.push(bridge_out_log(seq, block, 0, 10 * E8, &[0x11; 29]));
            }
            s.count_blocks = vec![125; counted];
            install(s);
            assert!(run().is_err(), "{name}");
            assert_eq!(pos(), (100, 0), "{name}");
            assert_stuck("where");
        }
    }

    #[test]
    fn removed_logs_are_rejected() {
        bound(3, 2, 100);
        let mut s = Sim::new(3, 150);
        s.add_event(1, 120, 10 * E8);
        s.logs[0].removed = true;
        install(s);
        assert!(run().is_err());
        assert_eq!(pos(), (100, 0));
        assert_stuck("marked removed");
    }

    #[test]
    fn logs_from_another_address_are_rejected_by_the_transport_and_by_the_decoder() {
        bound(3, 2, 100);
        let mut s = Sim::new(3, 150);
        s.add_event(1, 120, 10 * E8);
        s.logs[0].address = [0xdd; 20];
        install(s);
        run_until_err();
        assert_blocked_before(120);
        assert_stuck("different address");
        // The observer's own check, independent of the transport's filter check.
        let mut l = bridge_out_log(1, 120, 0, 10 * E8, &[0x11; 29]);
        l.address = [0xdd; 20];
        assert!(decode_chunk(&[l], &CONTRACT, 101, 150, 0).unwrap_err().contains("undecodable"));
    }

    #[test]
    fn decode_chunk_checks_range_decoding_and_sequence() {
        let ok = |seq, block| bridge_out_log(seq, block, 0, E8, &[0x11; 29]);
        let got = decode_chunk(&[ok(5, 110), ok(6, 111)], &CONTRACT, 101, 150, 4).unwrap();
        assert_eq!(got.iter().map(|(e, _)| e.sequence).collect::<Vec<_>>(), vec![5, 6]);
        assert!(decode_chunk(&[], &CONTRACT, 101, 150, 4).unwrap().is_empty());
        assert!(decode_chunk(&[ok(5, 100)], &CONTRACT, 101, 150, 4).unwrap_err().contains("outside"));
        assert!(decode_chunk(&[ok(5, 151)], &CONTRACT, 101, 150, 4).unwrap_err().contains("outside"));
        assert!(decode_chunk(&[ok(6, 110)], &CONTRACT, 101, 150, 4).unwrap_err().contains("expected"));
        assert!(decode_chunk(&[ok(5, 110), ok(7, 111)], &CONTRACT, 101, 150, 4).is_err());
        assert!(decode_chunk(&[ok(5, 110), ok(5, 111)], &CONTRACT, 101, 150, 4).is_err());
        let mut bad = ok(5, 110);
        bad.data.pop();
        assert!(decode_chunk(&[bad], &CONTRACT, 101, 150, 4).unwrap_err().contains("undecodable"));
        let mut other_topic = ok(5, 110);
        other_topic.topics[0] = [1; 32];
        assert!(decode_chunk(&[other_topic], &CONTRACT, 101, 150, 4).is_err());
        let mut extra_topic = ok(5, 110);
        extra_topic.topics.push([0; 32]);
        assert!(decode_chunk(&[extra_topic], &CONTRACT, 101, 150, 4).is_err());
    }

    #[test]
    fn a_counter_that_goes_down_halts_the_bridge() {
        bound(3, 2, 100);
        state::mutate_core(|c| c.observer.processed_count = 3);
        let mut s = Sim::new(3, 150);
        s.add_event(1, 120, 10 * E8);
        s.add_event(2, 130, 10 * E8);
        install(s);
        let err = run().unwrap_err();
        assert!(err.contains("decreased"), "{err}");
        assert!(state::is_halted());
        assert!(state::read_core(|c| c.halted.clone().unwrap().reason).contains("bridgeOutCount decreased"));
        assert_eq!(pos(), (100, 3));
    }

    #[test]
    fn a_finalized_block_that_changes_hash_halts_the_bridge() {
        bound(3, 2, 100);
        state::mutate_core(|c| c.observer.last_finalized = Some((150, h32(150, 9))));
        install(Sim::new(3, 150));
        let err = run().unwrap_err();
        assert!(err.contains("changed hash"), "{err}");
        assert!(state::is_halted());
        assert_eq!(pos(), (100, 0));
    }

    #[test]
    fn provider_disagreement_leaves_state_unchanged() {
        // Three different block hashes at the head.
        bound(3, 2, 100);
        let mut s = Sim::new(3, 150);
        s.add_event(1, 120, 10 * E8);
        for (i, p) in s.providers.iter_mut().enumerate() {
            p.salt = i as u8 + 1;
        }
        install(s);
        let before = state::read_core(|c| (c.buckets, c.observer.last_finalized));
        assert!(run().unwrap_err().contains("finalized head"));
        assert_eq!(pos(), (100, 0));
        assert_eq!(state::read_core(|c| (c.buckets, c.observer.last_finalized)), before);
        assert_stuck("finalized");

        // Quorum 3: a single provider serving different logs blocks the range.
        bound(3, 3, 100);
        let mut s = Sim::new(3, 150);
        s.add_event(1, 120, 10 * E8);
        let mut other = s.logs.clone();
        other[0].data[0..32].copy_from_slice(&word_u64(11 * E8));
        s.providers[2].logs = Some(other);
        install(s);
        assert!(run_until_err().contains("eth_getLogs"));
        assert_blocked_before(120);
        assert_stuck("no quorum");
    }

    #[test]
    fn one_provider_down_or_lying_does_not_stop_a_two_of_three_quorum() {
        bound(3, 2, 100);
        let mut s = Sim::new(3, 150);
        s.add_event(1, 120, 10 * E8);
        s.providers[0].down = true;
        let mut other = s.logs.clone();
        other.clear();
        s.providers[1].logs = Some(other);
        install(s);
        // p1 down, p2 hides the log, p3 right: only one answer for the logs.
        run_until_err();
        assert_blocked_before(120);
        // With p2 honest again, p1 down is tolerated.
        bound(3, 2, 100);
        let mut s = Sim::new(3, 150);
        s.add_event(1, 120, 10 * E8);
        s.providers[0].down = true;
        install(s);
        assert_eq!(run().unwrap().recorded, 1);
        assert_eq!(pos(), (150, 1));
    }

    #[test]
    fn a_recording_failure_stops_without_advancing_and_a_retry_completes() {
        bound(3, 2, 100);
        state::mutate_core(|c| c.ledger_fee_e8s = None);
        let mut s = Sim::new(3, 150);
        s.add_event(1, 120, 10 * E8);
        s.add_event(2, 130, 10 * E8);
        install(s);
        let err = run().unwrap_err();
        assert!(err.contains("ledger fee unknown"), "{err}");
        assert_eq!(pos(), (100, 0));
        state::mutate_core(|c| c.ledger_fee_e8s = Some(10_000));
        assert_eq!(run().unwrap().recorded, 2);
        assert_eq!(pos(), (150, 2));
        assert_eq!(state::read_core(|c| c.observer.last_error.clone()), None);
        assert_eq!(state::read_core(|c| c.observer.consecutive_failures), 0);
    }

    #[test]
    fn failures_count_up_and_a_success_resets_them() {
        bound(3, 2, 100);
        let sim = install(Sim::new(3, 150));
        for p in &mut sim.borrow_mut().providers {
            p.down = true;
        }
        assert!(run().is_err());
        assert!(run().is_err());
        assert_eq!(state::read_core(|c| c.observer.consecutive_failures), 2);
        assert_eq!(pos(), (100, 0));
        for p in &mut sim.borrow_mut().providers {
            p.down = false;
        }
        assert!(run().is_ok());
        assert_eq!(state::read_core(|c| (c.observer.consecutive_failures, c.observer.last_error.clone())), (0, None));
    }

    #[test]
    fn a_position_changed_during_the_pass_is_not_overwritten() {
        bound(3, 2, 100);
        install(Sim::new(3, 150));
        let head = BlockRef { number: 150, hash: h32(150, 0) };
        // Another writer moved the cursor after this pass read it.
        state::mutate_core(|c| c.observer.cursor_block = 120);
        assert!(commit(head, (100, 0), (150, 0)).unwrap_err().contains("changed"));
        assert_eq!(pos(), (120, 0));
        assert!(commit(head, (120, 0), (150, 0)).is_ok());
        assert_eq!(pos(), (150, 0));
    }

    // ----- binding --------------------------------------------------------------------

    fn bind(addr: &str) -> BridgeResult<ContractBindingView> {
        block_on(bind_contract(addr.to_string()))
    }

    fn contract_hex() -> String {
        ids::hex0x(&CONTRACT)
    }

    fn assert_refused(expect: &str, r: BridgeResult<ContractBindingView>) {
        match r {
            Err(BridgeError::InvalidState(m)) | Err(BridgeError::Rpc(m)) | Err(BridgeError::InvalidArgument(m)) => {
                assert!(m.contains(expect), "{m}")
            }
            other => panic!("expected a refusal containing {expect:?}, got {other:?}"),
        }
        assert!(state::read_core(|c| c.config.contract.is_none()), "a refused bind must leave nothing bound");
        assert_eq!(pos(), (0, 0));
    }

    #[test]
    fn bind_happy_path_reads_everything_at_one_finalized_block_and_stores_the_binding() {
        unbound(3, 2);
        let sim = install(Sim::new(3, 500));
        let view = bind(&contract_hex()).unwrap();
        assert_eq!(view.address, contract_hex());
        assert_eq!(view.start_block, 500);
        assert_eq!(view.supply_cap_e8s, Nat::from(100_000 * E8));
        assert_eq!(view.mint_window_limit_e8s, Nat::from(10_000 * E8));
        assert_eq!(view.mint_window_seconds, 86_400);
        assert_eq!(view.min_bridge_out_e8s, Nat::from(E8));
        let b = state::read_core(|c| c.config.contract.clone().unwrap());
        assert_eq!((b.address, b.start_block, b.supply_cap, b.mint_window_limit, b.mint_window_seconds, b.min_bridge_out), (CONTRACT, 500, (100_000 * E8) as u128, (10_000 * E8) as u128, 86_400, E8 as u128));
        assert_eq!(b.bound_at_ns, view.bound_at_ns);
        assert_eq!(state::read_core(|c| (c.observer.cursor_block, c.observer.processed_count, c.observer.last_finalized)), (500, 0, Some((500, h32(500, 0)))));
        // Every contract read is pinned to the finalized block.
        assert!(sim.borrow().calls.iter().filter(|(_, m, _)| m == "eth_call").all(|(_, _, r)| *r == Some((500, 500))));
        assert!(state::audit_page(0, 100).iter().any(|r| r.note.contains("bound at finalized block 500")));
        // Binding is once-only.
        assert!(matches!(bind(&contract_hex()), Err(BridgeError::InvalidState(m)) if m.contains("already bound")));
        assert!(matches!(bind(&ids::hex0x(&[0xee; 20])), Err(BridgeError::InvalidState(_))));
        assert_eq!(state::read_core(|c| c.config.contract.clone().unwrap().address), CONTRACT);
    }

    #[test]
    fn bind_refuses_a_contract_that_is_not_pristine_or_not_ours() {
        type Tweak = Box<dyn Fn(&mut Sim)>;
        let cases: Vec<(&str, &str, Tweak)> = vec![
            ("wrong version", "VERSION()", Box::new(|s: &mut Sim| s.version = "other-bridge-v1".into())),
            ("wrong decimals", "decimals()", Box::new(|s: &mut Sim| s.decimals = 18)),
            ("existing supply", "totalSupply()", Box::new(|s: &mut Sim| s.total_supply = 1)),
            ("existing bridge-outs", "bridgeOutCount()", Box::new(|s: &mut Sim| s.count_blocks = vec![10])),
            ("existing cumulative mints", "totalMinted()", Box::new(|s: &mut Sim| s.total_minted = 1)),
            ("no totalMinted() getter", "TotalMinted", Box::new(|s: &mut Sim| s.fail_total_minted = true)),
            ("minter lacks the role", "MINTER_ROLE", Box::new(|s: &mut Sim| s.roles.clear())),
            ("admin holds the minter role", "admin address holds MINTER_ROLE", Box::new(|s: &mut Sim| s.roles.push((abi::MINTER_ROLE, ADMIN)))),
            ("someone else is default admin", "defaultAdmin()", Box::new(|s: &mut Sim| s.default_admin = [0x99; 20])),
            ("contract cap below the canister limit", "supplyCap", Box::new(|s: &mut Sim| s.supply_cap = 1_000 * E8 as u128)),
            ("contract window limit below the canister limit", "mintWindowLimit", Box::new(|s: &mut Sim| s.window_limit = 100 * E8 as u128)),
            ("contract window longer than the canister's", "mintWindowSeconds", Box::new(|s: &mut Sim| s.window_seconds = 172_800)),
            ("contract minimum above the canister minimum", "minBridgeOut", Box::new(|s: &mut Sim| s.min_out = 5 * E8 as u128)),
        ];
        for (name, expect, tweak) in cases {
            unbound(3, 2);
            let mut s = Sim::new(3, 500);
            tweak(&mut s);
            install(s);
            assert_refused(expect, bind(&contract_hex()));
            eprintln!("refused: {name}");
        }
    }

    #[test]
    fn bind_refuses_when_there_is_no_contract_code_or_the_chain_is_unreadable() {
        // `eth_call` to an address without code returns "0x".
        unbound(3, 2);
        set_mock_transport(|_, body| {
            let (m, params) = request_of(body);
            match m.as_str() {
                "eth_chainId" => Ok(ok_body(json!("0x14a34"))),
                "eth_getBlockByNumber" => {
                    let n = match params[0].as_str().unwrap() {
                        "finalized" => 500,
                        hex => u64::from_str_radix(hex.trim_start_matches("0x"), 16).unwrap(),
                    };
                    Ok(ok_body(block_json(n, h32(n, 0))))
                }
                _ => Ok(ok_body(json!("0x"))),
            }
        });
        assert_refused("VERSION()", bind(&contract_hex()));
        // Every provider down.
        unbound(3, 2);
        set_mock_transport(|_, _| Err("down".into()));
        match bind(&contract_hex()) {
            Err(BridgeError::Rpc(m)) => assert!(m.contains("chain id"), "{m}"),
            other => panic!("{other:?}"),
        }
        assert!(state::read_core(|c| c.config.contract.is_none()));
        // Providers that disagree about a value we need.
        unbound(3, 2);
        let mut s = Sim::new(3, 500);
        for (i, p) in s.providers.iter_mut().enumerate() {
            p.supply = Some(i as u128);
        }
        install(s);
        match bind(&contract_hex()) {
            Err(BridgeError::Rpc(m)) => assert!(m.contains("no quorum"), "{m}"),
            other => panic!("{other:?}"),
        }
        assert!(state::read_core(|c| c.config.contract.is_none()));
    }

    #[test]
    fn bind_refuses_with_liabilities_active_operations_or_bad_input() {
        // Non-zero buckets.
        unbound(3, 2);
        state::mutate_core(|c| c.buckets.fees = 1);
        install(Sim::new(3, 500));
        assert_refused("buckets", bind(&contract_hex()));
        // Active deposit / release.
        unbound(3, 2);
        state::test_mark_active(Some(1), None);
        install(Sim::new(3, 500));
        assert_refused("active", bind(&contract_hex()));
        unbound(3, 2);
        state::test_mark_active(None, Some((1, [1; 32])));
        assert_refused("active", bind(&contract_hex()));
        // No RPC is made for any of these.
        let sim = install(Sim::new(3, 500));
        unbound(3, 2);
        state::mutate_core(|c| c.buckets.pending_mint = 5);
        assert!(bind(&contract_hex()).is_err());
        assert!(sim.borrow().calls.is_empty(), "refusals before the first await must not spend cycles");
        // Bad addresses.
        unbound(3, 2);
        for bad in ["", "0x1234", "cccccccccccccccccccccccccccccccccccccccc", "0xzzcccccccccccccccccccccccccccccccccccccc"] {
            assert!(matches!(bind(bad), Err(BridgeError::InvalidArgument(_))), "{bad:?}");
        }
        assert!(matches!(bind(&ids::hex0x(&[0; 20])), Err(BridgeError::InvalidArgument(_))));
        assert!(matches!(bind(&ids::hex0x(&MINTER)), Err(BridgeError::InvalidArgument(_))));
        assert!(matches!(bind(&ids::hex0x(&ADMIN)), Err(BridgeError::InvalidArgument(_))));
        // Uninitialized bridge.
        state::reset_for_tests();
        assert!(matches!(bind(&contract_hex()), Err(BridgeError::NotConfigured(_))));
        // Lane addresses unavailable (host has no tECDSA).
        unbound(3, 2);
        state::mutate_core(|c| {
            c.minter_lane.address = None;
            c.minter_lane.public_key = None;
        });
        assert!(matches!(bind(&contract_hex()), Err(BridgeError::Internal(_))));
    }

    /// A user deposit whose mint is signed but not final: `amount` e8s in flight.
    fn put_inflight_mint(id: u64, amount: u64) {
        let owner = Principal::from_slice(&[1; 10]);
        state::put_deposit(&state::DepositRecord {
            id,
            kind: state::DepositKind::UserDeposit,
            owner,
            from: None,
            client_request_id: vec![1; 16],
            base_recipient: [1; 20],
            amount_e8s: amount,
            deposit_fee_e8s: 0,
            mint_amount_e8s: amount,
            mint_id: [2; 32],
            state: state::DepositState::MintInFlight,
            pull: None,
            refund: None,
            tx_ids: vec![id],
            active_tx: Some(id),
            delivered: None,
            hold_retry_at_ns: None,
            created_at_ns: 0,
            updated_at_ns: 0,
        });
        state::put_tx(&state::EvmTxRecord {
            id,
            lane: LaneKind::Minter,
            nonce: 0,
            purpose: state::TxPurpose::Mint { deposit_id: id },
            to: CONTRACT,
            value_wei: 0,
            data: vec![],
            gas_limit: 1,
            attempts: vec![state::TxAttempt {
                kind: state::AttemptKind::Original,
                to: CONTRACT,
                value_wei: 0,
                data: vec![],
                max_fee_per_gas: 1,
                max_priority_fee_per_gas: 1,
                gas_limit: 1,
                raw: vec![2],
                tx_hash: [1; 32],
                created_at_ns: 0,
                broadcast_count: 0,
                last_broadcast_ns: None,
                last_broadcast_note: None,
            }],
            status: TxStatus::Pending,
            cancel_requested: false,
            outcome_applied: false,
            created_at_ns: 0,
            updated_at_ns: 0,
            last_checked_ns: None,
            note: None,
        });
    }

    // ----- supply check -----------------------------------------------------------------

    #[test]
    fn supply_check_reports_quiescence_only_when_everything_is_caught_up() {
        bound(3, 2, 150);
        let mut s = Sim::new(3, 150);
        s.total_supply = 1_000 * E8 as u128;
        let sim = install(s);
        // Caught up, nothing in flight: exact match is evaluated and holds.
        let snap = block_on(check_base_supply()).unwrap();
        assert_eq!((snap.total_supply, snap.block_number, snap.quiescent, snap.within_bound, snap.exact_match), (1_000 * E8 as u128, 150, true, true, Some(true)));

        // Not quiescent: pending mint.
        state::mutate_core(|c| c.buckets.pending_mint = 5);
        let snap = block_on(check_base_supply()).unwrap();
        assert!(!snap.quiescent);
        assert_eq!(snap.exact_match, None);
        state::mutate_core(|c| c.buckets.pending_mint = 0);

        // The idle observer's cursor is irrelevant: only the counts matter.
        state::mutate_core(|c| c.observer.cursor_block = 10);
        assert!(block_on(check_base_supply()).unwrap().quiescent);
        state::mutate_core(|c| c.observer.cursor_block = 150);

        // Not quiescent: a mint settled above the head.
        state::mutate_core(|c| c.observer.max_settled_block = 151);
        assert!(!block_on(check_base_supply()).unwrap().quiescent);
        state::mutate_core(|c| c.observer.max_settled_block = 150);

        // Not quiescent: an event exists that the observer has not processed.
        sim.borrow_mut().count_blocks = vec![140];
        assert!(!block_on(check_base_supply()).unwrap().quiescent);
        sim.borrow_mut().count_blocks.clear();

        // Not quiescent: a mint with a signed attempt that is not final.
        put_inflight_mint(1, 7);
        let snap = block_on(check_base_supply()).unwrap();
        assert!(!snap.quiescent);
        assert_eq!(snap.inflight_mint, 7);
    }

    #[test]
    fn supply_check_flags_a_supply_above_outstanding_and_needs_a_bound_contract() {
        bound(3, 2, 150);
        let mut s = Sim::new(3, 150);
        s.total_supply = 1_001 * E8 as u128;
        install(s);
        let snap = block_on(check_base_supply()).unwrap();
        assert!(!snap.within_bound);
        assert_eq!(snap.exact_match, Some(false));
        // Provider disagreement is an error, not a snapshot.
        let mut s = Sim::new(3, 150);
        for (i, p) in s.providers.iter_mut().enumerate() {
            p.salt = i as u8 + 1;
        }
        install(s);
        assert!(block_on(check_base_supply()).unwrap_err().contains("finalized head"));
        unbound(3, 2);
        assert!(block_on(check_base_supply()).unwrap_err().contains("no Base contract"));
    }

    // ----- cumulative mint guard ---------------------------------------------------------

    /// A canister that has minted `minted` itself and sees `minted + extra` minted on Base.
    fn guarded(minted: u64, extra: u64) -> Sim {
        state::mutate_core(|c| c.cumulative_minted_e8s = minted as u128);
        let mut s = Sim::new(3, 150);
        s.total_minted = (minted + extra) as u128;
        s
    }

    #[test]
    fn an_extra_mint_followed_by_a_burn_in_the_same_pass_halts_before_any_release_is_recorded() {
        bound(3, 2, 100);
        // The canister minted 100; Base shows 140 minted (40 nobody here made). The
        // attacker burned those 40 at once, so totalSupply looks normal (100 - 0 here).
        let mut s = guarded(100 * E8, 40 * E8);
        s.total_supply = 100 * E8 as u128;
        s.add_event(1, 120, 40 * E8);
        let sim = install(s);
        let buckets = state::read_core(|c| c.buckets);

        let err = run().unwrap_err();
        assert!(err.contains("Base cumulative mints") && err.contains("exceed"), "{err}");
        assert!(err.contains(&(140 * E8).to_string()) && err.contains(&(100 * E8).to_string()), "{err}");
        let halted = state::read_core(|c| c.halted.clone()).expect("the bridge halts");
        assert_eq!(halted.reason, err);
        assert_eq!(release_count(), 0, "no release is recorded");
        assert_eq!(state::read_core(|c| c.buckets), buckets, "no bucket moved");
        assert_eq!(pos(), (100, 0), "the cursor does not advance");
        assert_eq!(count_of(&sim, "eth_getLogs"), 0, "the burn was not even fetched");
        assert_stuck("Base cumulative mints");

        // The excess never goes away: every later pass refuses again, still recording nothing.
        state::mutate_core(|c| c.observer.next_attempt_ns = None);
        assert!(run().unwrap_err().contains("Base cumulative mints"));
        assert_eq!(release_count(), 0);
        assert_eq!(pos(), (100, 0));
    }

    #[test]
    fn a_legitimate_in_flight_mint_does_not_halt() {
        bound(3, 2, 100);
        // 100 minted and seen final; a further 40 signed and already on Base but not yet
        // seen final by the canister: allowed, and the burn is recorded.
        put_inflight_mint(1, (40 * E8) as u64);
        let mut s = guarded(100 * E8, 40 * E8);
        s.add_event(1, 120, 10 * E8);
        install(s);
        let out = run().unwrap();
        assert_eq!(out.recorded, 1);
        assert!(!state::is_halted());
        assert_eq!(release_count(), 1);
        assert_eq!(pos(), (150, 1));
    }

    #[test]
    fn the_guard_allows_exactly_minted_plus_in_flight_and_nothing_more() {
        // Exactly equal is fine (nothing in flight).
        bound(3, 2, 100);
        let mut s = guarded(100 * E8, 0);
        s.add_event(1, 120, 10 * E8);
        install(s);
        assert!(run().is_ok());
        assert!(!state::is_halted());

        // One unit more than minted plus in flight halts.
        state::reset_for_tests();
        bound(3, 2, 100);
        put_inflight_mint(1, 7);
        let mut s = guarded(100 * E8, 7);
        s.total_minted += 1;
        s.add_event(1, 120, 10 * E8);
        install(s);
        assert!(run().unwrap_err().contains("plus in flight 7"));
        assert!(state::is_halted());
        assert_eq!(release_count(), 0);
    }

    #[test]
    fn a_failed_total_minted_read_records_nothing_and_does_not_advance() {
        bound(3, 2, 100);
        let mut s = guarded(100 * E8, 0);
        s.add_event(1, 120, 10 * E8);
        s.fail_total_minted = true;
        let sim = install(s);
        let err = run().unwrap_err();
        assert!(err.contains("cumulative mint guard"), "{err}");
        assert_eq!(release_count(), 0);
        assert_eq!(pos(), (100, 0));
        assert_eq!(count_of(&sim, "eth_getLogs"), 0, "no burn is fetched while the guard cannot be read");
        assert!(!state::is_halted(), "an unreadable guard is a retryable error, not a halt");
        assert_stuck("cumulative mint guard");

        // Once the read works again the burn is recorded.
        sim.borrow_mut().fail_total_minted = false;
        let out = run().unwrap();
        assert_eq!(out.recorded, 1);
        assert_eq!(pos(), (150, 1));
    }

    #[test]
    fn the_guard_is_read_at_the_same_finalized_head_as_the_count() {
        bound(3, 2, 100);
        let mut s = guarded(100 * E8, 0);
        s.add_event(1, 120, 10 * E8);
        let sim = install(s);
        run().unwrap();
        let reads: Vec<u64> = sim.borrow().calls.iter().filter(|(p, m, _)| *p == 0 && m == "eth_call").map(|(_, _, r)| r.unwrap().0).collect();
        assert_eq!(reads, vec![150, 150], "bridgeOutCount and totalMinted, both at the head");
    }

    #[test]
    fn an_idle_pass_records_no_burn_and_so_does_not_read_the_guard() {
        bound(3, 2, 100);
        // Base shows an excess, but nothing was burned: the cheap path records nothing and
        // the reconcile timer is what reports the excess.
        let sim = install(guarded(100 * E8, 40 * E8));
        assert_eq!(run().unwrap(), ObserverOutcome::default());
        assert!(!state::is_halted());
        assert_eq!(pos(), (150, 0));
        assert_eq!(sim.borrow().total_minted_reads, 0, "totalMinted() was read on the idle path");
        assert_eq!(count_of(&sim, "eth_call"), 3);
    }

    #[test]
    fn check_base_supply_halts_on_a_cumulative_excess_even_when_the_supply_looks_normal() {
        bound(3, 2, 150);
        // totalSupply equals outstanding (1000), so the C2 bound holds, but Base minted 40
        // more than this canister did: they were burned again before this reading.
        let mut s = guarded(1_000 * E8, 40 * E8);
        s.total_supply = 1_000 * E8 as u128;
        install(s);
        let snap = block_on(check_base_supply()).unwrap();
        assert!(!snap.within_bound, "a cumulative excess is never within bound, whatever totalSupply says");
        assert_eq!(snap.total_supply, 1_000 * E8 as u128);
        let reason = state::read_core(|c| c.halted.clone()).expect("the bridge halts").reason;
        assert!(reason.contains("Base cumulative mints") && reason.contains("exceed"), "{reason}");
        assert!(reason.contains(&(1_040 * E8).to_string()) && reason.contains(&(1_000 * E8).to_string()), "{reason}");
    }

    #[test]
    fn check_base_supply_allows_in_flight_mints_and_an_exact_match() {
        bound(3, 2, 150);
        put_inflight_mint(1, (40 * E8) as u64);
        let mut s = guarded(1_000 * E8, 40 * E8);
        s.total_supply = 1_000 * E8 as u128;
        install(s);
        let snap = block_on(check_base_supply()).unwrap();
        assert!(snap.within_bound);
        assert!(!state::is_halted());

        // Exactly the canister's own total, nothing in flight.
        state::reset_for_tests();
        bound(3, 2, 150);
        let mut s = guarded(1_000 * E8, 0);
        s.total_supply = 1_000 * E8 as u128;
        install(s);
        assert!(block_on(check_base_supply()).is_ok());
        assert!(!state::is_halted());
    }

    #[test]
    fn check_base_supply_cannot_read_the_guard_so_it_reports_an_error_without_a_snapshot() {
        bound(3, 2, 150);
        let mut s = guarded(1_000 * E8, 0);
        s.total_supply = 1_000 * E8 as u128;
        s.fail_total_minted = true;
        install(s);
        let err = block_on(check_base_supply()).unwrap_err();
        assert!(err.contains("TotalMinted"), "{err}");
        assert!(!state::is_halted());
    }

    #[test]
    fn the_cumulative_halt_cannot_be_cleared_because_every_fresh_snapshot_is_out_of_bound() {
        bound(3, 2, 150);
        let mut s = guarded(1_000 * E8, 40 * E8);
        s.total_supply = 1_000 * E8 as u128;
        install(s);
        assert!(!block_on(check_base_supply()).unwrap().within_bound);
        assert!(state::is_halted());
        // A second check (what admin_clear_halt runs) is out of bound again, and the halt
        // keeps its first reason.
        let first = state::read_core(|c| c.halted.clone()).unwrap().reason;
        assert!(!block_on(check_base_supply()).unwrap().within_bound);
        assert_eq!(state::read_core(|c| c.halted.clone()).unwrap().reason, first);
    }

    // ----- adaptive log ranges ---------------------------------------------------------

    /// Run passes (an hour apart, so no back-off applies) until `done`.
    fn run_until(done: impl Fn() -> bool, max_passes: usize) -> usize {
        for i in 0..max_passes {
            if done() {
                return i;
            }
            crate::env::test_env::advance_secs(4_000);
            let _ = run();
        }
        assert!(
            done(),
            "not done after {max_passes} passes: position {:?}, error {:?}",
            pos(),
            state::read_core(|c| c.observer.last_error.clone())
        );
        max_passes
    }

    /// Passes until one fails (the scan may creep forward first), an hour apart.
    fn run_until_err() -> String {
        for _ in 0..20 {
            crate::env::test_env::advance_secs(4_000);
            if let Err(e) = run() {
                return e;
            }
        }
        panic!("no pass failed; position {:?}", pos());
    }

    fn observer_state() -> state::ObserverState {
        state::read_core(|c| c.observer.clone())
    }

    fn release_count() -> usize {
        state::releases_in_state(|_| true, 10_000).len()
    }

    #[test]
    fn dense_logs_are_scanned_by_halving_the_range_and_every_event_is_recorded_exactly_once() {
        bound(3, 2, 100);
        let mut s = Sim::new(3, 2_100);
        // 200 burns inside one default-sized (2000 block) span: the whole span's logs do
        // not fit the 64 KiB response cap, so the first request fails.
        for i in 0..200u64 {
            s.add_event(i + 1, 101 + i * 10, E8);
        }
        assert!(wire_len(&s.logs) > 65_536, "the scenario must exceed the cap");
        assert!(wire_len(&s.logs[..50]) < 65_536, "and a quarter of it must fit");
        let sim = install(s);
        run_until(|| pos() == (2_100, 200), 20);
        // Every event recorded exactly once.
        assert_eq!(release_count(), 200);
        for seq in 1..=200u64 {
            assert!(state::release_by_sequence(seq).is_some(), "sequence {seq}");
        }
        let b = state::read_core(|c| c.buckets);
        assert_eq!((b.pending_release, b.outstanding_base), ((200 * E8) as u128, (800 * E8) as u128));
        assert_eq!(observer_state().last_event_block, 101 + 199 * 10);
        // The first request for the whole span failed and the range was halved.
        let ranges: Vec<(u64, u64)> =
            sim.borrow().calls.iter().filter(|(p, m, _)| *p == 0 && m == "eth_getLogs").map(|(_, _, r)| r.unwrap()).collect();
        assert_eq!(ranges[0], (101, 2_100));
        assert_eq!(ranges[1], (101, 1_100));
        assert_eq!(ranges[2], (101, 600));
        // Failed requests were retried smaller instead of being repeated.
        let mut seen = std::collections::BTreeSet::new();
        assert!(ranges.iter().all(|r| seen.insert(*r)), "no identical range was requested twice: {ranges:?}");
    }

    #[test]
    fn a_pass_gives_up_after_six_halvings_keeps_the_smaller_range_and_resumes() {
        bound(3, 2, 100);
        let mut s = Sim::new(3, 2_100);
        // 200 burns packed into 40 blocks: even a 31 block range does not fit.
        for i in 0..200u64 {
            s.add_event_at(i + 1, 101 + i / 5, i % 5, E8 / 10);
        }
        assert!(wire_len(&s.logs[..155]) > 65_536);
        let sim = install(s);
        let err = run().unwrap_err();
        assert!(err.contains("6 range halvings"), "{err}");
        let ranges: Vec<u64> = sim
            .borrow()
            .calls
            .iter()
            .filter(|(p, m, _)| *p == 0 && m == "eth_getLogs")
            .map(|(_, _, r)| r.unwrap().1 - r.unwrap().0 + 1)
            .collect();
        assert_eq!(ranges, vec![2_000, 1_000, 500, 250, 125, 62, 31], "six halvings, then the pass stops");
        // The shrunken range is remembered, nothing was lost.
        assert_eq!(observer_state().adaptive_chunk_blocks, Some(31));
        assert_eq!(pos(), (100, 0));
        assert_eq!(release_count(), 0);
        run_until(|| pos() == (2_100, 200), 30);
        assert_eq!(release_count(), 200);
        assert_eq!(state::read_core(|c| c.buckets.pending_release), (20 * E8) as u128);
    }

    #[test]
    fn a_pass_that_crept_forward_before_a_dense_stretch_reports_a_note_not_a_failure() {
        bound(3, 2, 100);
        let mut s = Sim::new(3, 2_100);
        // A quiet start, then 200 burns packed into 40 blocks near the end.
        for i in 0..200u64 {
            s.add_event_at(i + 1, 1_900 + i / 5, i % 5, E8 / 10);
        }
        install(s);
        let mut notes = vec![];
        let mut errors = vec![];
        for _ in 0..30 {
            if pos() == (2_100, 200) {
                break;
            }
            crate::env::test_env::advance_secs(4_000);
            match run() {
                Ok(out) => notes.extend(out.note),
                Err(e) => errors.push(e),
            }
        }
        assert_eq!(pos(), (2_100, 200), "{errors:?}");
        assert_eq!(release_count(), 200);
        assert!(notes.iter().any(|n| n.contains("range halvings")), "{notes:?} {errors:?}");
        // Failing passes (no progress at all) are the only ones counted against the
        // observer, and the final state is clean.
        assert_eq!(observer_state().consecutive_failures, 0);
        assert_eq!(observer_state().last_error, None);
    }

    #[test]
    fn a_single_block_with_more_logs_than_the_cap_succeeds_after_the_cap_escalates() {
        bound(3, 2, 100);
        let mut s = Sim::new(3, 150);
        for i in 0..200u64 {
            s.add_event_at(i + 1, 120, i, E8 / 10);
        }
        let needed = wire_len(&s.logs) as u64;
        assert!(needed > 65_536 * 2, "the block needs more than two doublings of the default cap: {needed}");
        let mut want = 65_536u64;
        while want < needed {
            want *= 2;
        }
        let sim = install(s);
        let mut saw_raise = false;
        for _ in 0..40 {
            if pos() == (150, 200) {
                break;
            }
            crate::env::test_env::advance_secs(4_000);
            match run() {
                Err(e) => saw_raise |= e.contains("response cap raised"),
                Ok(out) => saw_raise |= out.note.is_some_and(|n| n.contains("response cap raised")),
            }
        }
        assert_eq!(pos(), (150, 200), "{:?}", observer_state().last_error);
        assert!(saw_raise, "the pass that hit the dense block reports the raised cap");
        assert_eq!(observer_state().consecutive_failures, 0);
        assert_eq!(release_count(), 200);
        assert_eq!(state::read_core(|c| c.buckets.pending_release), (20 * E8) as u128);
        let caps: Vec<u64> = mock_calls().iter().filter(|c| c.method == "eth_getLogs").map(|c| c.max_response_bytes).collect();
        assert_eq!(caps.iter().copied().max(), Some(want), "the cap doubles until the block fits, no further");
        assert!(caps.contains(&65_536));
        assert_eq!(observer_state().last_event_block, 120);
        // The EVM RPC canister was charged for the larger caps.
        assert!(mock_calls().iter().filter(|c| c.max_response_bytes == want).all(|c| c.cycles > 0));
        let _ = sim;
    }

    #[test]
    fn the_cap_stops_doubling_at_the_canister_maximum() {
        bound(3, 2, 100);
        let mut s = Sim::new(3, 150);
        s.add_event(1, 120, E8);
        // Block 120's logs never fit, whatever the cap.
        s.fail_logs_with_hits = true;
        install(s);
        state::mutate_core(|c| c.observer.adaptive_logs_cap = Some(1_000_000));
        // What each pass reports: its error, or the note of a pass that crept forward first.
        let mut said = vec![];
        for _ in 0..30 {
            crate::env::test_env::advance_secs(4_000);
            match run() {
                Err(e) => said.push(e),
                Ok(out) => said.extend(out.note),
            }
        }
        assert!(said.iter().any(|e| e.contains("response cap raised to 2000000 bytes")), "{said:?}");
        let last = said.last().unwrap();
        assert!(last.contains("largest response cap, 2000000 bytes"), "{last}");
        assert!(state::read_core(|c| c.observer.consecutive_failures) > 10, "a stuck observer counts as failing");
        assert_eq!(observer_state().adaptive_logs_cap, Some(2_000_000));
        assert_eq!(pos(), (119, 0), "stuck at the block that cannot be fetched, visibly");
        assert!(mock_calls().iter().all(|c| c.max_response_bytes <= 2_000_000));
        assert!(observer_state().last_error.unwrap().contains("largest response cap"));
        assert_eq!(release_count(), 0);
    }

    #[test]
    fn providers_that_disagree_on_a_single_block_do_not_raise_the_cap() {
        bound(3, 3, 100);
        let mut s = Sim::new(3, 150);
        s.add_event(1, 120, E8);
        let mut other = s.logs.clone();
        other[0].data[0..32].copy_from_slice(&word_u64(2 * E8));
        s.providers[2].logs = Some(other);
        install(s);
        for _ in 0..30 {
            crate::env::test_env::advance_secs(4_000);
            let _ = run();
        }
        assert_eq!(observer_state().adaptive_logs_cap, None, "no escalation for a disagreement");
        assert_eq!(pos(), (119, 0), "the scan stops at the disputed block");
        assert!(observer_state().last_error.unwrap().contains("disagree"));
        assert!(mock_calls().iter().all(|c| c.max_response_bytes <= 65_536));
    }

    #[test]
    fn range_and_cap_grow_back_after_successes() {
        bound(3, 2, 100);
        state::mutate_core(|c| {
            c.config.observer.logs_chunk_blocks = 40;
            c.observer.adaptive_chunk_blocks = Some(5);
            c.observer.adaptive_logs_cap = Some(262_144);
        });
        let mut s = Sim::new(3, 400);
        s.add_event(1, 399, E8);
        let sim = install(s);
        // Ten successful ranges in two passes.
        let out = run().unwrap();
        assert!(out.pending_work);
        assert_eq!(observer_state().adaptive_chunk_blocks, None, "five ranges doubled 5 -> 10 -> 20 -> 40: back at the default");
        assert_eq!(observer_state().adaptive_logs_cap, Some(262_144), "five successes are not enough to relax the cap");
        crate::env::test_env::advance_secs(4_000);
        run().unwrap();
        assert_eq!(pos(), (400, 1));
        assert_eq!(observer_state().adaptive_logs_cap, None, "ten successes in a row restore the default cap");
        let ranges: Vec<(u64, u64)> =
            sim.borrow().calls.iter().filter(|(p, m, _)| *p == 0 && m == "eth_getLogs").map(|(_, _, r)| r.unwrap()).collect();
        let spans: Vec<u64> = ranges.iter().map(|(a, b)| b - a + 1).collect();
        assert_eq!(&spans[..6], &[5, 10, 20, 40, 40, 40]);
        let caps: Vec<u64> = mock_calls().iter().filter(|c| c.method == "eth_getLogs" && c.service == state::read_core(|c| c.config.rpc_services[0].clone())).map(|c| c.max_response_bytes).collect();
        assert!(caps.iter().all(|c| *c == 262_144), "{caps:?}");
    }

    // ----- completeness without historical state --------------------------------------

    #[test]
    fn unreadable_historical_counts_do_not_stop_the_scan_and_are_noted() {
        bound(3, 2, 100);
        state::mutate_core(|c| c.config.observer.logs_chunk_blocks = 10);
        let mut s = Sim::new(3, 130);
        s.add_event(1, 105, 10 * E8);
        s.add_event(2, 125, 10 * E8);
        // A non-archive node: state before the head is gone.
        s.pruned_below = 130;
        let sim = install(s);
        let out = run().unwrap();
        assert_eq!((out.recorded, out.pending_work), (2, false));
        assert!(out.note.as_deref().unwrap_or("").contains("2 range(s)"), "{out:?}");
        assert_eq!(pos(), (130, 2));
        assert_eq!(observer_state().last_event_block, 125);
        assert_eq!(release_count(), 2);
        // The count reads were attempted (and failed) at the chunk ends before the head.
        let at: Vec<u64> = sim.borrow().calls.iter().filter(|(p, m, _)| *p == 0 && m == "eth_call").map(|(_, _, r)| r.unwrap().0).collect();
        assert_eq!(at, vec![130, 130, 110, 120]);
        // Each unverified range is in the audit log.
        let notes = state::audit_page(0, 1_000).into_iter().filter(|r| r.note.contains("continuity alone")).count();
        assert_eq!(notes, 2);
    }

    #[test]
    fn an_event_hidden_at_the_end_of_an_unverified_range_is_caught_at_the_head_and_rescanned() {
        bound(3, 2, 100);
        state::mutate_core(|c| c.config.observer.logs_chunk_blocks = 10);
        let mut s = Sim::new(3, 130);
        s.add_event(1, 105, 10 * E8);
        s.pruned_below = 130;
        // Every provider drops the only log of [101, 110], once (quorum-agreed omission).
        s.omit_at_to = 110;
        s.omit_budget = 3;
        install(s);
        let err = run().unwrap_err();
        assert!(err.contains("bridgeOutCount went from 0 to 1"), "{err}");
        assert!(err.contains("cursor moved back from 120 to 100"), "{err}");
        assert_eq!(pos(), (100, 0), "rewound to the binding: no event was ever processed");
        assert_eq!(release_count(), 0);
        assert!(state::audit_page(0, 100).iter().any(|r| r.note.contains("cursor moved back")));
        // The rescan finds it; it is recorded once and the cursor reaches the head.
        let out = run().unwrap();
        assert_eq!(out.recorded, 1);
        assert_eq!(pos(), (130, 1));
        assert_eq!(release_count(), 1);
        assert_eq!(state::read_core(|c| c.buckets.pending_release), (10 * E8) as u128);
    }

    #[test]
    fn a_hidden_event_before_later_ones_is_a_gap_that_rewinds_to_the_last_event_without_double_counting() {
        bound(3, 2, 100);
        state::mutate_core(|c| c.config.observer.logs_chunk_blocks = 10);
        let mut s = Sim::new(3, 130);
        s.add_event(1, 105, 10 * E8);
        s.add_event(2, 115, 20 * E8);
        s.add_event(3, 125, 30 * E8);
        s.pruned_below = 130;
        s.omit_at_to = 120; // seq 2 is dropped from [111, 120]
        s.omit_budget = 3;
        install(s);
        let err = run().unwrap_err();
        assert!(err.contains("sequence 3") && err.contains("where 2 was expected"), "{err}");
        assert!(err.contains("cursor moved back from 120 to 104"), "{err}");
        // Event 1 is recorded and stays counted; the cursor sits just before its block.
        assert_eq!(pos(), (104, 1));
        assert_eq!(observer_state().last_event_block, 105);
        assert_eq!(release_count(), 1);
        // The rescan skips event 1 (already on record), finds 2 and 3.
        let out = run().unwrap();
        assert_eq!(out.recorded, 2);
        assert_eq!(pos(), (130, 3));
        assert_eq!(release_count(), 3);
        let b = state::read_core(|c| c.buckets);
        assert_eq!((b.pending_release, b.outstanding_base), ((60 * E8) as u128, (940 * E8) as u128), "nothing counted twice");
        assert_eq!(observer_state().last_event_block, 125);
    }

    #[test]
    fn an_event_hidden_in_the_block_of_the_last_processed_event_is_found_after_the_rewind() {
        bound(3, 2, 100);
        state::mutate_core(|c| c.config.observer.logs_chunk_blocks = 10);
        let mut s = Sim::new(3, 130);
        s.add_event_at(1, 105, 0, 10 * E8);
        s.add_event_at(2, 105, 1, 10 * E8);
        s.pruned_below = 130;
        s.omit_at_to = 110;
        s.omit_budget = 3;
        install(s);
        assert!(run().is_err());
        assert_eq!(pos(), (104, 1), "one block before the last event, so its block is scanned whole");
        let out = run().unwrap();
        assert_eq!(out.recorded, 1, "event 1 is on record, only event 2 is new");
        assert_eq!(pos(), (130, 2));
        assert_eq!(release_count(), 2);
        assert_eq!(state::read_core(|c| c.buckets.pending_release), (20 * E8) as u128);
    }

    #[test]
    fn a_persistent_omission_is_rewound_once_per_cooldown_and_never_wedges_into_a_loop() {
        bound(3, 2, 100);
        state::mutate_core(|c| c.config.observer.logs_chunk_blocks = 10);
        let mut s = Sim::new(3, 130);
        s.add_event(1, 105, 10 * E8);
        s.pruned_below = 130;
        s.omit_at_to = 110;
        s.omit_budget = u32::MAX;
        install(s);
        assert!(run().unwrap_err().contains("cursor moved back"));
        assert_eq!(pos(), (100, 0));
        // The rescan hides the event again: the failure stays visible, no second rewind.
        let err = run().unwrap_err();
        assert!(err.contains("bridgeOutCount went from 0 to 1") && !err.contains("cursor moved back"), "{err}");
        assert_eq!(pos(), (120, 0));
        assert!(run().unwrap_err().contains("bridgeOutCount went from 0 to 1"));
        assert_eq!(pos(), (120, 0));
        assert_eq!(release_count(), 0);
        // After the cooldown the observer may try the rescan again.
        crate::env::test_env::advance_secs(3_601);
        assert!(run().unwrap_err().contains("cursor moved back"));
        assert_eq!(pos(), (100, 0));
    }

    #[test]
    fn a_verified_range_that_misses_an_event_is_not_advanced_and_is_not_rewound_when_it_starts_at_the_rewind_point() {
        // Counts are readable, so the mismatch is caught in the range itself: no rewind.
        bound(3, 2, 100);
        state::mutate_core(|c| c.config.observer.logs_chunk_blocks = 10);
        let mut s = Sim::new(3, 130);
        s.add_event(1, 105, 10 * E8);
        s.omit_at_to = 110;
        s.omit_budget = 3;
        install(s);
        let err = run().unwrap_err();
        assert!(err.contains("returned 0 BridgeOut logs but bridgeOutCount went from 0 to 1") && !err.contains("moved back"), "{err}");
        assert_eq!(pos(), (100, 0));
        // The omission was transient: the next pass records the event.
        assert_eq!(run().unwrap().recorded, 1);
        assert_eq!(pos(), (130, 1));
    }

    #[test]
    fn rescanned_events_must_match_the_record_and_only_at_the_start() {
        bound(3, 2, 100);
        let mut s = Sim::new(3, 150);
        s.add_event(1, 120, 10 * E8);
        s.add_event(2, 130, 10 * E8);
        let logs = s.logs.clone();
        install(s);
        run().unwrap();
        assert_eq!(pos(), (150, 2));
        // Seen again from a rescan: accepted and skipped.
        assert!(decode_chunk(&logs, &CONTRACT, 101, 150, 2).unwrap().is_empty());
        let third = bridge_out_log(3, 140, 0, 10 * E8, &[0x11; 29]);
        let mut with_new = logs.clone();
        with_new.push(third.clone());
        let got = decode_chunk(&with_new, &CONTRACT, 101, 150, 2).unwrap();
        assert_eq!(got.iter().map(|(e, _)| e.sequence).collect::<Vec<_>>(), vec![3]);
        // Only the start may overlap, in order and without holes.
        assert!(decode_chunk(&[third.clone(), logs[0].clone()], &CONTRACT, 101, 150, 2).is_err());
        assert!(decode_chunk(&[logs[1].clone(), logs[0].clone()], &CONTRACT, 101, 150, 2).is_err());
        // A different event at a known sequence is refused (amount, block, transaction, index).
        let mut other_amount = logs[0].clone();
        other_amount.data[0..32].copy_from_slice(&word_u64(11 * E8));
        let mut other_block = logs[0].clone();
        other_block.block_number = 121;
        let mut other_tx = logs[0].clone();
        other_tx.tx_hash = h32(99, 1);
        let mut other_index = logs[0].clone();
        other_index.log_index = 7;
        for (name, bad) in [("amount", other_amount), ("block", other_block), ("tx", other_tx), ("index", other_index)] {
            let e = decode_chunk(&[bad], &CONTRACT, 101, 150, 2).unwrap_err();
            assert!(e.contains("does not match the recorded event"), "{name}: {e}");
        }
        // A sequence at or below the count that was never recorded is refused.
        let unknown = bridge_out_log(1, 120, 0, 10 * E8, &[0x22; 29]);
        assert!(decode_chunk(&[unknown], &CONTRACT, 101, 150, 2).unwrap_err().contains("does not match"));
    }

    #[test]
    fn a_head_behind_an_already_recorded_event_waits_instead_of_halting() {
        bound(3, 2, 100);
        // A partial pass recorded an event at block 120 that the cursor has not passed.
        state::mutate_core(|c| {
            c.observer.processed_count = 1;
            c.observer.last_event_block = 120;
        });
        let sim = install(Sim::new(3, 110));
        let out = run().unwrap();
        assert!(out.note.unwrap().contains("behind"));
        assert!(!state::is_halted(), "a lagging quorum is not a counter that went down");
        assert_eq!(pos(), (100, 1));
        assert_eq!(count_of(&sim, "eth_call"), 0);
    }

    // ----- recording errors --------------------------------------------------------------

    #[test]
    fn a_recording_error_midway_keeps_the_recorded_prefix_and_a_retry_does_not_double_count() {
        bound(3, 2, 100);
        // Enough outstanding Base supply for the first burn only.
        state::mutate_core(|c| c.buckets.outstanding_base = (15 * E8) as u128);
        let mut s = Sim::new(3, 150);
        s.add_event(1, 120, 10 * E8);
        s.add_event(2, 130, 10 * E8);
        s.add_event(3, 140, 5 * E8);
        install(s);
        let err = run().unwrap_err();
        assert!(err.contains("recording BridgeOut 2"), "{err}");
        // Event 1 is stored and counted; nothing past the failing event moved.
        let o = observer_state();
        assert_eq!((o.cursor_block, o.processed_count, o.last_event_block), (100, 1, 120));
        assert_eq!(release_count(), 1);
        assert_eq!(state::read_core(|c| (c.buckets.pending_release, c.buckets.outstanding_base)), ((10 * E8) as u128, (5 * E8) as u128));
        // Repair the cause and retry: the stored event is skipped, the rest is recorded.
        state::mutate_core(|c| {
            c.buckets.outstanding_base += (10 * E8) as u128;
            c.halted = None;
        });
        let out = run().unwrap();
        assert_eq!(out.recorded, 2);
        let o = observer_state();
        assert_eq!((o.cursor_block, o.processed_count, o.last_event_block), (150, 3, 140));
        assert_eq!(release_count(), 3);
        let b = state::read_core(|c| c.buckets);
        assert_eq!((b.pending_release, b.outstanding_base), ((25 * E8) as u128, 0u128), "no burn counted twice");
    }

    #[test]
    fn a_burn_that_waits_for_an_in_flight_mint_stops_the_scan_without_halting_and_a_retry_completes() {
        bound(3, 2, 100);
        // The burn is finalized before the lane has settled the mint that funded it.
        state::mutate_core(|c| c.buckets.outstanding_base = 0);
        put_inflight_mint(1, 10 * E8);
        let mut s = Sim::new(3, 150);
        s.add_event(1, 120, 10 * E8);
        install(s);
        let err = run().unwrap_err();
        assert!(err.contains("recording BridgeOut 1"), "{err}");
        assert!(!state::is_halted(), "a retryable error is not a halt");
        let o = observer_state();
        assert_eq!((o.cursor_block, o.processed_count, o.last_event_block), (100, 0, 0));
        assert_eq!(release_count(), 0);
        // The mint settles; the next pass records the burn.
        state::mutate_core(|c| c.buckets.outstanding_base = (10 * E8) as u128);
        assert_eq!(run().unwrap().recorded, 1);
        let o = observer_state();
        assert_eq!((o.cursor_block, o.processed_count, o.last_event_block), (150, 1, 120));
        assert_eq!(release_count(), 1);
        assert_eq!(state::read_core(|c| (c.buckets.pending_release, c.buckets.outstanding_base)), ((10 * E8) as u128, 0));
    }

    #[test]
    fn events_already_on_record_count_as_processed_but_not_as_new() {
        bound(3, 2, 100);
        let mut s = Sim::new(3, 150);
        s.add_event(1, 120, 10 * E8);
        s.add_event(2, 130, 10 * E8);
        let first = s.logs[0].clone();
        install(s);
        let ev = abi::decode_bridge_out(&first, &CONTRACT).unwrap();
        assert_eq!(record_finalized_bridge_out(&ev, &first), Ok(RecordOutcome::Recorded));
        let out = run().unwrap();
        assert_eq!(out.recorded, 1, "only event 2 is new");
        assert_eq!(pos(), (150, 2));
        assert_eq!(observer_state().last_event_block, 130);
        let b = state::read_core(|c| c.buckets);
        assert_eq!((b.pending_release, b.outstanding_base), ((20 * E8) as u128, (980 * E8) as u128));
        assert_eq!(release_count(), 2);
    }

    // ----- back-off ----------------------------------------------------------------------

    #[test]
    fn back_off_schedule() {
        assert_eq!((backoff_ns(0), backoff_ns(1), backoff_ns(2)), (None, None, None));
        let s = |n: u64| n * 1_000_000_000;
        assert_eq!(backoff_ns(3), Some(s(30)));
        assert_eq!(backoff_ns(4), Some(s(60)));
        assert_eq!(backoff_ns(5), Some(s(120)));
        assert_eq!(backoff_ns(8), Some(s(960)));
        assert_eq!(backoff_ns(9), Some(s(1_920)));
        assert_eq!(backoff_ns(10), Some(s(3_600)), "never more than an hour");
        assert_eq!(backoff_ns(u32::MAX), Some(s(3_600)));
    }

    #[test]
    fn repeated_failures_back_the_observer_off_and_a_success_resets_it() {
        bound(3, 2, 100);
        let sim = install(Sim::new(3, 150));
        for p in &mut sim.borrow_mut().providers {
            p.down = true;
        }
        let secs = |n: u64| n * 1_000_000_000;
        assert!(run().is_err());
        assert!(run().is_err());
        assert_eq!(observer_state().next_attempt_ns, None, "two failures do not back off");
        let t = crate::env::now_ns();
        assert!(run().is_err());
        assert_eq!(observer_state().consecutive_failures, 3);
        assert_eq!(observer_state().next_attempt_ns, Some(t + secs(30)));
        // Inside the window a pass does nothing at all.
        sim.borrow_mut().calls.clear();
        crate::env::test_env::advance_secs(29);
        assert_eq!(run(), Ok(ObserverOutcome::default()));
        assert!(sim.borrow().calls.is_empty(), "no RPC while backing off");
        assert_eq!(observer_state().consecutive_failures, 3);
        // At the end of the window it tries again; the next wait doubles.
        crate::env::test_env::advance_secs(1);
        let t = crate::env::now_ns();
        assert!(run().is_err());
        assert!(!sim.borrow().calls.is_empty());
        assert_eq!(observer_state().consecutive_failures, 4);
        assert_eq!(observer_state().next_attempt_ns, Some(t + secs(60)));
        // Long outages settle at an hour.
        state::mutate_core(|c| c.observer.consecutive_failures = 40);
        crate::env::test_env::advance_secs(60);
        let t = crate::env::now_ns();
        assert!(run().is_err());
        assert_eq!(observer_state().next_attempt_ns, Some(t + secs(3_600)));
        // Recovery: the pass after the window succeeds and clears the back-off.
        for p in &mut sim.borrow_mut().providers {
            p.down = false;
        }
        crate::env::test_env::advance_secs(3_599);
        assert_eq!(run(), Ok(ObserverOutcome::default()));
        assert_eq!(pos(), (100, 0), "still inside the window");
        crate::env::test_env::advance_secs(1);
        assert!(run().is_ok());
        let o = observer_state();
        assert_eq!((o.consecutive_failures, o.next_attempt_ns, o.last_error), (0, None, None));
        assert_eq!(pos(), (150, 0));
    }

    // ----- chain id and the release cost at bind time ----------------------------------

    #[test]
    fn bind_requires_the_providers_to_serve_the_configured_chain() {
        unbound(3, 2);
        let mut s = Sim::new(3, 500);
        s.chain_id = state::CHAIN_BASE_MAINNET;
        let sim = install(s);
        assert_refused("serve chain id 8453", bind(&contract_hex()));
        assert!(sim.borrow().calls.iter().all(|(_, m, _)| m == "eth_chainId"), "nothing else is read from the wrong chain");
        // One provider on another chain is outvoted by the quorum.
        unbound(3, 2);
        let s = Sim::new(3, 500);
        let sim = install(s);
        let inner = sim.clone();
        set_mock_transport(move |svc, body| {
            let (m, _) = request_of(body);
            if m == "eth_chainId" && provider_index(svc) == 2 {
                return Ok(ok_body(json!("0x1")));
            }
            inner.borrow_mut().handle(provider_index(svc), body)
        });
        assert!(bind(&contract_hex()).is_ok());
    }

    #[test]
    fn bind_needs_the_ledger_fee_before_it_spends_anything() {
        unbound(3, 2);
        state::mutate_core(|c| c.ledger_fee_e8s = None);
        let sim = install(Sim::new(3, 500));
        assert_refused("ledger fee unknown; run a tick first", bind(&contract_hex()));
        assert!(sim.borrow().calls.is_empty());
    }

    #[test]
    fn bind_requires_the_contract_minimum_to_exceed_the_release_cost() {
        // (contract minBridgeOut, release fee, binds?) with a ledger fee of 10_000.
        for (min_out, release_fee, ok) in [
            (E8, 0u64, true),
            (1_010_001u64, 1_000_000u64, true),
            (1_010_000, 1_000_000, false),
            (1_000_000, 1_000_000, false),
            (E8, E8, false),
        ] {
            unbound(3, 2);
            state::mutate_core(|c| c.config.fees.release_fee_e8s = release_fee);
            let mut s = Sim::new(3, 500);
            s.min_out = min_out as u128;
            install(s);
            let r = bind(&contract_hex());
            if ok {
                assert!(r.is_ok(), "{min_out} / {release_fee}: {r:?}");
            } else {
                assert_refused("must exceed the release fee plus the ledger fee", r);
            }
        }
    }

    // ----- supply check, count based --------------------------------------------------------

    #[test]
    fn the_exact_supply_match_is_reported_after_the_head_moved_past_an_idle_cursor() {
        bound(3, 2, 100);
        let mut s = Sim::new(3, 150);
        s.total_supply = 1_000 * E8 as u128;
        install(s);
        // The cursor sits at 100: the idle observer has not walked to the head. No burn
        // happened, so nothing is missing.
        assert_eq!(pos(), (100, 0));
        let snap = block_on(check_base_supply()).unwrap();
        assert_eq!((snap.block_number, snap.quiescent, snap.within_bound, snap.exact_match), (150, true, true, Some(true)));
    }

    #[test]
    fn the_supply_check_is_skipped_when_recorded_burns_are_above_the_head() {
        bound(3, 2, 100);
        // Two burns recorded, but the quorum head only counts one: totalSupply at that
        // head still includes the second burn, which outstanding already excludes.
        state::mutate_core(|c| {
            c.observer.processed_count = 2;
            c.observer.cursor_block = 100;
            c.observer.last_event_block = 140;
        });
        let mut s = Sim::new(3, 150);
        s.add_event(1, 120, 10 * E8);
        s.add_event(2, 160, 10 * E8);
        s.total_supply = 1_000 * E8 as u128;
        install(s);
        let err = block_on(check_base_supply()).unwrap_err();
        assert!(err.contains("recorded burns are above the finalized head; supply check skipped"), "{err}");
        // With the head past the second burn the check runs again.
        state::mutate_core(|c| c.observer.last_event_block = 160);
        let mut s = Sim::new(3, 170);
        s.add_event(1, 120, 10 * E8);
        s.add_event(2, 160, 10 * E8);
        s.total_supply = 980 * E8 as u128;
        install(s);
        let snap = block_on(check_base_supply()).unwrap();
        assert_eq!((snap.quiescent, snap.exact_match), (true, Some(false)), "supply 980 against outstanding 1000 is below the bound but not exact");
        assert!(snap.within_bound);
    }
}
