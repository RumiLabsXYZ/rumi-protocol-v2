//! Owned by card BB-05. Contract binding, finalized bridge-out scanning and Base
//! supply reconciliation.
//!
//! The observer is the ONLY path that turns a Base burn into a release. It trusts
//! nothing a single provider says and nothing that is not final:
//!
//! - Every read is a quorum read at a `finalized` block (see `evm::rpc`).
//! - The contract's `bridgeOutCount()` never decreases and rises by exactly one per
//!   `BridgeOut` event. A scanned range is accepted only if the number of logs found
//!   equals the change in that counter over the range, and the logs carry sequences
//!   `processed + 1, processed + 2, ...` with no gap. A provider that hides, invents or
//!   reorders an event therefore cannot move the cursor.
//! - Position (`cursor_block`, `processed_count`) moves only after the events of a range
//!   are recorded, and recording is idempotent on the transfer id, so a retry after a
//!   partial failure cannot double count.

use crate::accounting::Buckets;
use crate::admin;
use crate::env;
use crate::evm::abi::{self, BridgeOutEvent, RawLog, ViewCall};
use crate::evm::rpc::{self, BlockRef, BlockTag};
use crate::evm::signer;
use crate::flows::release::{record_finalized_bridge_out, RecordOutcome};
use crate::ids::{self, LaneKind};
use crate::state::{self, AuditCtx, ContractBinding};
use crate::types::{BridgeError, BridgeResult, ContractBindingView};
use candid::Nat;

/// Upper bound on `eth_getLogs` ranges scanned in one pass, so a long backlog is worked
/// off over several timer ticks instead of one very expensive one.
pub const MAX_CHUNKS_PER_PASS: usize = 5;

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
        if !c.active_deposits.is_empty() || !c.active_releases.is_empty() {
            return Err(BridgeError::InvalidState("cannot bind while deposits or releases are active".into()));
        }
        Ok(())
    })
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
/// canister's minter lane as the only minter and its admin lane as the admin.
pub async fn bind_contract(address: String) -> BridgeResult<ContractBindingView> {
    let contract = ids::parse_address(&address).map_err(BridgeError::InvalidArgument)?;
    if contract == [0u8; 20] {
        return Err(BridgeError::InvalidArgument("the zero address cannot be bound".into()));
    }
    check_bindable()?;
    let minter = signer::lane_address(LaneKind::Minter).await.map_err(|e| BridgeError::Internal(format!("minter lane address: {e}")))?;
    let admin_addr = signer::lane_address(LaneKind::Admin).await.map_err(|e| BridgeError::Internal(format!("admin lane address: {e}")))?;
    if minter == admin_addr {
        return Err(BridgeError::Internal("minter and admin lane addresses are identical".into()));
    }
    if contract == minter || contract == admin_addr {
        return Err(BridgeError::InvalidArgument("the contract address equals one of this canister's own Base addresses".into()));
    }

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

    // Everything above awaited: check again that nothing changed under us, then commit
    // in one write.
    check_bindable()?;
    state::mutate_core(|c| {
        c.config.contract = Some(binding.clone());
        c.observer.cursor_block = head.number;
        c.observer.processed_count = 0;
        c.observer.last_finalized = Some((head.number, head.hash));
        c.observer.last_error = None;
        c.observer.consecutive_failures = 0;
    });
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

/// Decode and check the logs of one scanned range `[from, to]`.
///
/// Every log must be inside the range, be a well-formed `BridgeOut` of the bound
/// contract (not removed, exact topics and data), and carry the next expected sequence.
/// The slice is already ordered by `(block, log index)`.
fn decode_chunk(
    logs: &[RawLog],
    contract: &[u8; 20],
    from: u64,
    to: u64,
    processed: u64,
) -> Result<Vec<(BridgeOutEvent, RawLog)>, String> {
    let mut out = Vec::with_capacity(logs.len());
    for (i, log) in logs.iter().enumerate() {
        if log.block_number < from || log.block_number > to {
            return Err(format!("log at block {} is outside the scanned range [{from}, {to}]", log.block_number));
        }
        let ev = abi::decode_bridge_out(log, contract)
            .map_err(|e| format!("undecodable BridgeOut log at block {} index {}: {e:?}", log.block_number, log.log_index))?;
        let want = processed.saturating_add(1).saturating_add(i as u64);
        if ev.sequence != want {
            return Err(format!(
                "BridgeOut sequence {} at block {} where {want} was expected (gap, duplicate or reorder)",
                ev.sequence, log.block_number
            ));
        }
        out.push((ev, log.clone()));
    }
    Ok(out)
}

/// Write the new position, but only if nobody moved it since this pass read it.
fn commit(head: BlockRef, from: (u64, u64), to: (u64, u64)) -> Result<(), String> {
    state::mutate_core(|c| {
        if (c.observer.cursor_block, c.observer.processed_count) != from {
            return Err("observer position changed during the pass; discarding it".to_string());
        }
        c.observer.cursor_block = to.0;
        c.observer.processed_count = to.1;
        note_head_in(c, head);
        Ok(())
    })
}

fn note_head_in(c: &mut state::BridgeCore, head: BlockRef) {
    // Never move the recorded head backwards: a lagging quorum is not a regression.
    if c.observer.last_finalized.map(|(n, _)| head.number >= n).unwrap_or(true) {
        c.observer.last_finalized = Some((head.number, head.hash));
    }
}

async fn scan(contract: [u8; 20], chunk_blocks: u64) -> Result<ObserverOutcome, String> {
    let (mut cursor, mut processed, previous) =
        state::read_core(|c| (c.observer.cursor_block, c.observer.processed_count, c.observer.last_finalized));

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
    if head.number <= cursor {
        state::mutate_core(|c| note_head_in(c, head));
        let note = (head.number < cursor).then(|| format!("finalized head {} is behind the cursor {cursor}", head.number));
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
        commit(head, (cursor, processed), (head.number, processed))?;
        return Ok(ObserverOutcome::default());
    }

    let mut recorded = 0u32;
    for _ in 0..MAX_CHUNKS_PER_PASS {
        let from = cursor + 1;
        let to = cursor.saturating_add(chunk_blocks).min(head.number);
        let logs = rpc::get_logs(&contract, &abi::BRIDGE_OUT_TOPIC0, from, to)
            .await
            .map_err(|e| format!("eth_getLogs [{from}, {to}]: {e}"))?;
        let decoded = decode_chunk(&logs, &contract, from, to, processed)?;
        let found = decoded.len() as u64;
        // The completeness proof: the counter moved by exactly the number of logs found.
        let count_to = if to == head.number { count_head } else { bridge_out_count(&contract, to).await? };
        if count_to != processed + found {
            return Err(format!(
                "range [{from}, {to}] returned {found} BridgeOut logs but bridgeOutCount went from {processed} to {count_to}; not advancing"
            ));
        }
        for (ev, log) in &decoded {
            match record_finalized_bridge_out(ev, log).map_err(|e| format!("recording BridgeOut {}: {e}", ev.sequence))? {
                RecordOutcome::Recorded => recorded += 1,
                RecordOutcome::AlreadyRecorded => {}
            }
        }
        let next_processed = processed + found;
        // Once every event up to the head is recorded, nothing is left to find after
        // `to` either, so skip the remaining empty ranges.
        let next_cursor = if next_processed == count_head { head.number } else { to };
        commit(head, (cursor, processed), (next_cursor, next_processed))?;
        cursor = next_cursor;
        processed = next_processed;
        if cursor >= head.number {
            break;
        }
    }
    Ok(ObserverOutcome { recorded, pending_work: cursor < head.number, note: None })
}

/// One bounded scan, called by the timer. On any failure the position is left where the
/// last fully verified range ended, the failure is recorded, and the error is returned.
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

pub async fn run_pass() -> Result<ObserverOutcome, String> {
    let Some((contract, chunk_blocks)) =
        state::read_core(|c| c.config.contract.as_ref().map(|b| (b.address, c.config.observer.logs_chunk_blocks.max(1))))
    else {
        return Ok(ObserverOutcome::default());
    };
    let (processed, last_full) = state::read_core(|c| (c.observer.processed_count, c.observer.last_full_pass_ns));
    let started = env::now_ns();
    let full_due = last_full.map(|t| started.saturating_sub(t) >= FULL_PASS_EVERY_NS).unwrap_or(true);
    if !full_due && idle_probe_unchanged(&contract, processed).await {
        // Nothing new at finalized; leave the cursor for the next full pass.
        state::mutate_core(|c| {
            if c.observer.processed_count == processed {
                c.observer.last_run_ns = Some(env::now_ns());
                c.observer.last_error = None;
                c.observer.consecutive_failures = 0;
            }
        });
        return Ok(ObserverOutcome::default());
    }
    let result = scan(contract, chunk_blocks).await;
    let now = env::now_ns();
    state::mutate_core(|c| {
        c.observer.last_run_ns = Some(now);
        match &result {
            Ok(_) => {
                c.observer.last_error = None;
                c.observer.consecutive_failures = 0;
                c.observer.last_full_pass_ns = Some(now);
            }
            Err(e) => {
                c.observer.last_error = Some(e.clone());
                c.observer.consecutive_failures = c.observer.consecutive_failures.saturating_add(1);
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
/// The exact match (C3) is only claimed when nothing can legitimately differ: the
/// observer has recorded every burn up to the head, every settled mint is at or below
/// the head, and no mint is in flight or pending.
pub async fn check_base_supply() -> Result<state::BaseSupplySnapshot, String> {
    let Some(contract) = state::read_core(|c| c.config.contract.as_ref().map(|b| b.address)) else {
        return Err("no Base contract is bound".into());
    };
    let head = rpc::finalized_head().await.map_err(|e| format!("finalized head: {e}"))?;
    let total = view_u128(&contract, ViewCall::TotalSupply, head.number).await?;
    let count_head = bridge_out_count(&contract, head.number).await?;
    // No await between reading local state and building the snapshot.
    let quiescent = state::read_core(|c| {
        count_head == c.observer.processed_count
            && c.observer.cursor_block >= head.number
            && head.number >= c.observer.max_settled_block
            && c.buckets.pending_mint == 0
    }) && state::inflight_mint_e8s() == 0;
    Ok(crate::timer::base_supply_snapshot(total, head.number, quiescent))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evm::rpc::testutil::*;
    use crate::evm::rpc::set_mock_transport;
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
        supply_cap: u128,
        window_limit: u128,
        window_seconds: u128,
        min_out: u128,
        roles: Vec<([u8; 32], [u8; 20])>,
        default_admin: [u8; 20],
        /// `(provider, method, block or range)`.
        calls: Vec<Call>,
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
                supply_cap: 100_000 * E8 as u128,
                window_limit: 10_000 * E8 as u128,
                window_seconds: 86_400,
                min_out: E8 as u128,
                roles: vec![(abi::MINTER_ROLE, MINTER)],
                default_admin: ADMIN,
                calls: vec![],
            }
        }

        fn add_event(&mut self, seq: u64, block: u64, amount: u64) {
            self.logs.push(bridge_out_log(seq, block, 0, amount, Principal::from_slice(&[0x11; 29]).as_slice()));
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
                "eth_getBlockByNumber" => {
                    let n = block_arg(prov, &params[0]);
                    Ok(ok_body(block_json(n, h32(n, prov.salt))))
                }
                "eth_getLogs" => {
                    let (from, to) = range.unwrap();
                    let src = prov.logs.as_ref().unwrap_or(&self.logs);
                    let hits: Vec<Value> = src.iter().filter(|l| l.block_number >= from && l.block_number <= to).map(log_json).collect();
                    Ok(ok_body(Value::Array(hits)))
                }
                "eth_call" => {
                    assert_eq!(params[0]["to"], json!(ids::hex0x(&CONTRACT)), "views must target the bound contract");
                    let data = hex::decode(params[0]["data"].as_str().unwrap().trim_start_matches("0x")).unwrap();
                    let at = range.unwrap().0;
                    let sel: [u8; 4] = data[..4].try_into().unwrap();
                    let word = |v: u128| word_u128(v).to_vec();
                    let ret = match sel {
                        abi::SEL_VERSION => abi_string(&self.version),
                        abi::SEL_DECIMALS => word(self.decimals),
                        abi::SEL_TOTAL_SUPPLY => word(prov.supply.unwrap_or(self.total_supply)),
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
        });
    }

    fn pos() -> (u64, u64) {
        state::read_core(|c| (c.observer.cursor_block, c.observer.processed_count))
    }

    fn run() -> Result<ObserverOutcome, String> {
        block_on(run_pass())
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
        // One head round, one count round, one logs round; the head count is reused for
        // the chunk that ends at the head.
        assert_eq!(count_of(&sim, "eth_getLogs"), 3);
        assert_eq!(count_of(&sim, "eth_call"), 3);
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
        // Counts read at 130 (head), 110 and 120; 130 is reused for the last chunk.
        let at: Vec<u64> = sim.borrow().calls.iter().filter(|(p, m, _)| *p == 0 && m == "eth_call").map(|(_, _, r)| r.unwrap().0).collect();
        assert_eq!(at, vec![130, 110, 120]);
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
        assert!(run().is_err());
        assert_eq!(pos(), (100, 0));
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
        assert!(run().unwrap_err().contains("eth_getLogs"));
        assert_eq!(pos(), (100, 0));
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
        assert!(run().is_err());
        assert_eq!(pos(), (100, 0));
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
            Err(BridgeError::Rpc(m)) => assert!(m.contains("finalized head"), "{m}"),
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
        state::mutate_core(|c| {
            c.active_deposits.insert(1);
        });
        install(Sim::new(3, 500));
        assert_refused("active", bind(&contract_hex()));
        unbound(3, 2);
        state::mutate_core(|c| {
            c.active_releases.insert([1; 32]);
        });
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

        // Not quiescent: the observer has not reached the head.
        state::mutate_core(|c| c.observer.cursor_block = 149);
        assert!(!block_on(check_base_supply()).unwrap().quiescent);
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
        let owner = Principal::from_slice(&[1; 10]);
        state::put_deposit(&state::DepositRecord {
            id: 1,
            kind: state::DepositKind::UserDeposit,
            owner,
            from: None,
            client_request_id: vec![1; 16],
            base_recipient: [1; 20],
            amount_e8s: 7,
            deposit_fee_e8s: 0,
            mint_amount_e8s: 7,
            mint_id: [2; 32],
            state: state::DepositState::MintInFlight,
            pull: None,
            refund: None,
            tx_ids: vec![0],
            active_tx: Some(0),
            delivered: None,
            hold_retry_at_ns: None,
            created_at_ns: 0,
            updated_at_ns: 0,
        });
        state::put_tx(&state::EvmTxRecord {
            id: 0,
            lane: LaneKind::Minter,
            nonce: 0,
            purpose: state::TxPurpose::Mint { deposit_id: 1 },
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
}
