use candid::{CandidType, Decode, Encode, Principal};
use ic_canister_log::log;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::BTreeMap;

use crate::logs::INFO;
use crate::types::*;

// ─── Event log caps ───
// Prevents unbounded heap growth that could brick the canister by causing
// pre_upgrade to trap when serializing too much data. Oldest events are
// dropped when the cap is reached (ring buffer behavior).

pub const MAX_SWAP_EVENTS: usize = 50_000;
pub const MAX_LIQUIDITY_EVENTS: usize = 50_000;
pub const MAX_ADMIN_EVENTS: usize = 10_000;
pub const MAX_HOLDER_SNAPSHOTS: usize = 1_000; // ~500 days at 2/day
pub const MAX_PENDING_CLAIMS: usize = 1_000;
/// Maximum number of outstanding outbound transfers. Ambiguous rows are never
/// evicted; new value-moving operations fail closed when this capacity is full.
pub const MAX_OUTBOUND_PAYOUTS: usize = 1_000;
pub const MAX_REWARD_EVENTS: usize = 50_000;
pub const MAX_CLAIM_EVENTS: usize = 50_000;
pub const MAX_PROCESSED_NONCES: usize = 1024;
pub const REWARD_SCALE: u128 = 1_000_000_000_000; // 1e12 fixed-point for acc_reward_per_share
/// Minimum claimable amount: 10x the live icUSD ledger fee (100_000 e8s =
/// 0.001 icUSD), i.e. 1_000_000 e8s = 0.01 icUSD. `claim_rewards` pays out
/// via the journaled reward payout, which sends `claimable - fee` and fails
/// closed if `claimable <= fee`. Gating at 10x the fee guarantees any claim
/// that passes this check has `claimable` well above the fee, so the payout
/// nets clearly positive for the user and the fail-closed guard is never
/// reached in practice.
pub const MIN_CLAIM_E8S: u128 = 1_000_000;
pub const MAX_TVL_SAMPLES: usize = 800; // ~30 months at 1/day; ~5.5 months at 4/day

// ─── State ───

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct AmmState {
    pub admin: Principal,
    pub pools: BTreeMap<PoolId, Pool>,
    #[serde(default)]
    pub pool_creation_open: bool,
    #[serde(default)]
    pub maintenance_mode: bool,
    #[serde(default)]
    pub pending_claims: Vec<PendingClaim>,
    #[serde(default)]
    pub next_claim_id: u64,
    #[serde(default)]
    pub swap_events: Vec<AmmSwapEvent>,
    #[serde(default)]
    pub next_swap_event_id: u64,
    #[serde(default)]
    pub liquidity_events: Vec<AmmLiquidityEvent>,
    #[serde(default)]
    pub next_liquidity_event_id: u64,
    #[serde(default)]
    pub admin_events: Vec<AmmAdminEvent>,
    #[serde(default)]
    pub next_admin_event_id: u64,
    #[serde(default)]
    pub holder_snapshots: Vec<HolderSnapshot>,
    #[serde(default)]
    pub reward_events: Vec<AmmRewardEvent>,
    #[serde(default)]
    pub next_reward_event_id: u64,
    #[serde(default)]
    pub claim_events: Vec<AmmClaimEvent>,
    #[serde(default)]
    pub next_claim_event_id: u64,
    /// Principal allowed to call `notify_reward_received`. Set by admin via
    /// `set_protocol_backend_principal`. Defaults to None (no caller
    /// authorized) until configured.
    #[serde(default)]
    pub protocol_backend_principal: Option<Principal>,
    #[serde(default)]
    pub tvl_samples: Vec<TvlSample>,
    /// Durable pre-dispatch intents. Rows remain until the caller has applied
    /// confirmed-success accounting, or forever when a dispatch is ambiguous.
    #[serde(default)]
    pub outbound_payouts: Vec<OutboundPayout>,
    /// Durable accounting linkage for active exit/admin payouts. Unlike event
    /// history this list is bounded by active payouts and is never ring-pruned.
    #[serde(default)]
    pub outbound_payout_links: Vec<OutboundPayoutLink>,
    #[serde(default)]
    pub next_outbound_payout_id: u64,
    /// Durable identities for user-authorized deposits. Ambiguous rows are
    /// retained until an exact replay yields positive ledger evidence.
    #[serde(default)]
    pub inbound_operations: Vec<InboundOperation>,
    /// Global monotonic request sequence. Terminal rows can be compacted because
    /// any unseen request at or below this high-water is rejected forever.
    #[serde(default)]
    pub inbound_sequence_high_water: u64,
}

impl Default for AmmState {
    fn default() -> Self {
        Self {
            admin: Principal::anonymous(),
            pools: BTreeMap::new(),
            pool_creation_open: false,
            maintenance_mode: false,
            pending_claims: Vec::new(),
            next_claim_id: 0,
            swap_events: Vec::new(),
            next_swap_event_id: 0,
            liquidity_events: Vec::new(),
            next_liquidity_event_id: 0,
            admin_events: Vec::new(),
            next_admin_event_id: 0,
            holder_snapshots: Vec::new(),
            reward_events: Vec::new(),
            next_reward_event_id: 0,
            claim_events: Vec::new(),
            next_claim_event_id: 0,
            protocol_backend_principal: None,
            tvl_samples: Vec::new(),
            outbound_payouts: Vec::new(),
            outbound_payout_links: Vec::new(),
            next_outbound_payout_id: 0,
            inbound_operations: Vec::new(),
            inbound_sequence_high_water: 0,
        }
    }
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum OutboundPayoutStatus {
    Reserved,
    Dispatched,
    Ambiguous,
}

/// Immutable transfer parameters, persisted before dispatch. The sender's
/// gross liability equals `net_amount + fee`; `fee` is pinned in the ledger
/// call so later fee drift cannot exceed the booked debit.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutboundPayout {
    pub id: u64,
    pub operation_id: String,
    pub ledger: Principal,
    pub from: Principal,
    pub from_subaccount: Option<[u8; 32]>,
    pub to: Principal,
    pub to_subaccount: Option<[u8; 32]>,
    pub gross_amount: u128,
    pub net_amount: u128,
    pub fee: u128,
    pub memo: Vec<u8>,
    pub created_at_time: u64,
    pub status: OutboundPayoutStatus,
}

#[derive(CandidType, Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum OutboundPayoutLeg {
    TokenA,
    TokenB,
}

/// Proof that an outbound payout's corresponding internal liability was
/// atomically committed. Created in the same message as the reserve/fee
/// accounting mutation and retained until the payout row is finalized.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum OutboundPayoutPurpose {
    RemoveLiquidity {
        pool_id: PoolId,
        caller: Principal,
        leg: OutboundPayoutLeg,
        gross_amount: u128,
    },
    WithdrawProtocolFees {
        pool_id: PoolId,
        admin: Principal,
        leg: OutboundPayoutLeg,
        gross_amount: u128,
    },
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutboundPayoutLink {
    pub payout_id: u64,
    pub purpose: OutboundPayoutPurpose,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum InboundOperationKind {
    Swap,
    AddLiquidity,
}
#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum InboundLegStatus {
    Prepared,
    Confirmed(u64),
    ProvenNoEffect,
    Ambiguous,
}
#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum InboundOperationPhase {
    Prepared,
    InputsConfirmed,
    OutputPending,
    Completed,
    ProvenNoEffect,
    Held,
    ResultUnavailable,
}
#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct InboundLeg {
    pub ledger: Principal,
    pub from: Principal,
    pub to_subaccount: Option<[u8; 32]>,
    pub amount: u128,
    pub fee: Option<u128>,
    pub memo: Vec<u8>,
    pub created_at_time: u64,
    pub status: InboundLegStatus,
}
#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct InboundOperation {
    pub request_id: Vec<u8>,
    pub caller: Principal,
    pub pool_id: PoolId,
    pub kind: InboundOperationKind,
    pub argument_digest: Vec<u8>,
    pub legs: Vec<InboundLeg>,
    pub created_at_time: u64,
    pub phase: InboundOperationPhase,
    pub output_payout_id: Option<u64>,
    pub result_amount: Option<u128>,
    /// Ledger fee observed before accepting the input. Output dispatch must use
    /// this exact fee so the user's minimum cannot drift after the deposit.
    #[serde(default)]
    pub output_ledger_fee: Option<u128>,
    pub result_fee: Option<u128>,
    pub protocol_fee: Option<u128>,
    pub token_in: Option<Principal>,
    /// `Some(true)` marks IDs created under the sequence high-water protocol.
    /// `None`/`Some(false)` are frozen legacy random IDs and must not be
    /// interpreted or compacted as sequence IDs.
    #[serde(default)]
    pub sequence_managed: Option<bool>,
    #[serde(default)]
    pub held_reason: Option<String>,
}
#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct InboundOperationStatus {
    pub operation: InboundOperation,
    pub linked_payout_status: Option<OutboundPayoutStatus>,
}

/// Frozen journal shape before output-fee pinning was introduced.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct InboundOperationV7 {
    pub request_id: Vec<u8>,
    pub caller: Principal,
    pub pool_id: PoolId,
    pub kind: InboundOperationKind,
    pub argument_digest: Vec<u8>,
    pub legs: Vec<InboundLeg>,
    pub created_at_time: u64,
    pub phase: InboundOperationPhase,
    pub output_payout_id: Option<u64>,
    pub result_amount: Option<u128>,
    pub result_fee: Option<u128>,
    pub protocol_fee: Option<u128>,
    pub token_in: Option<Principal>,
}

/// Frozen predecessor state used when upgrading from the initial inbound journal.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
struct AmmStateV7 {
    pub admin: Principal,
    pub pools: BTreeMap<PoolId, Pool>,
    pub pool_creation_open: bool,
    pub maintenance_mode: bool,
    pub pending_claims: Vec<PendingClaim>,
    pub next_claim_id: u64,
    pub swap_events: Vec<AmmSwapEvent>,
    pub next_swap_event_id: u64,
    pub liquidity_events: Vec<AmmLiquidityEvent>,
    pub next_liquidity_event_id: u64,
    pub admin_events: Vec<AmmAdminEvent>,
    pub next_admin_event_id: u64,
    pub holder_snapshots: Vec<HolderSnapshot>,
    pub reward_events: Vec<AmmRewardEvent>,
    pub next_reward_event_id: u64,
    pub claim_events: Vec<AmmClaimEvent>,
    pub next_claim_event_id: u64,
    pub protocol_backend_principal: Option<Principal>,
    pub tvl_samples: Vec<TvlSample>,
    pub outbound_payouts: Vec<OutboundPayout>,
    pub next_outbound_payout_id: u64,
    pub inbound_operations: Vec<InboundOperationV7>,
}

/// Frozen predecessor after fee pinning and before sequence-based compaction.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
struct AmmStateV8 {
    pub admin: Principal,
    pub pools: BTreeMap<PoolId, Pool>,
    pub pool_creation_open: bool,
    pub maintenance_mode: bool,
    pub pending_claims: Vec<PendingClaim>,
    pub next_claim_id: u64,
    pub swap_events: Vec<AmmSwapEvent>,
    pub next_swap_event_id: u64,
    pub liquidity_events: Vec<AmmLiquidityEvent>,
    pub next_liquidity_event_id: u64,
    pub admin_events: Vec<AmmAdminEvent>,
    pub next_admin_event_id: u64,
    pub holder_snapshots: Vec<HolderSnapshot>,
    pub reward_events: Vec<AmmRewardEvent>,
    pub next_reward_event_id: u64,
    pub claim_events: Vec<AmmClaimEvent>,
    pub next_claim_event_id: u64,
    pub protocol_backend_principal: Option<Principal>,
    pub tvl_samples: Vec<TvlSample>,
    pub outbound_payouts: Vec<OutboundPayout>,
    pub next_outbound_payout_id: u64,
    pub inbound_operations: Vec<InboundOperation>,
}

/// Bound unresolved liabilities, not lifetime throughput. Terminal requests are
/// compacted into the per-caller sequence high-water plus a recent replay window.
pub const MAX_ACTIVE_INBOUND_OPERATIONS: usize = 1_000;
pub const INBOUND_TERMINAL_REPLAY_WINDOW: u64 = 128;
pub fn next_inbound_sequence() -> Result<u64, String> {
    read_state(|s| s.inbound_sequence_high_water.checked_add(1))
        .ok_or_else(|| "caller request sequence exhausted".to_string())
}

fn hold_legacy_swap_without_fee(operation: &mut InboundOperation) {
    if operation.kind == InboundOperationKind::Swap && operation.output_ledger_fee.is_none() {
        // V7 did not persist the output fee in the operation. Preserve it as
        // explicitly held rather than inventing a net result or redispatching
        // an unknown amount.
        operation.phase = if operation.phase == InboundOperationPhase::Completed {
            InboundOperationPhase::ResultUnavailable
        } else {
            InboundOperationPhase::Held
        };
        operation.held_reason = Some(
            "legacy swap has no pinned output fee; exact payout result unavailable; manual positive-proof reconciliation required".into(),
        );
    }
}

fn request_sequence(request_id: &[u8]) -> Option<u64> {
    let bytes: [u8; 8] = request_id.get(..8)?.try_into().ok()?;
    Some(u64::from_be_bytes(bytes))
}

fn compact_terminal_rows(s: &mut AmmState) {
    let floor = s
        .inbound_sequence_high_water
        .saturating_sub(INBOUND_TERMINAL_REPLAY_WINDOW);
    s.inbound_operations.retain(|row| {
        if !matches!(
            row.phase,
            InboundOperationPhase::Completed
                | InboundOperationPhase::ProvenNoEffect
                | InboundOperationPhase::ResultUnavailable
        ) {
            return true;
        }
        // Keep legacy random-ID rows intact: their first eight bytes are not
        // sequence metadata and can accidentally look like any u64 value.
        match row.sequence_managed {
            Some(true) => request_sequence(&row.request_id)
                .map(|sequence| sequence > floor)
                .unwrap_or(true),
            None | Some(false) => true,
        }
    });
}

pub fn reserve_inbound_operation(op: InboundOperation) -> Result<InboundOperation, String> {
    if op.request_id.len() != 32 {
        return Err("request ID must be exactly 32 bytes".to_string());
    }
    mutate_state(|s| reserve_inbound_operation_in(s, op))
}

fn reserve_inbound_operation_in(
    s: &mut AmmState,
    op: InboundOperation,
) -> Result<InboundOperation, String> {
    if let Some(old) = s
        .inbound_operations
        .iter()
        .find(|x| x.caller == op.caller && x.request_id == op.request_id)
    {
        if old.argument_digest != op.argument_digest
            || old.pool_id != op.pool_id
            || old.kind != op.kind
        {
            return Err("request ID already bound to different arguments".to_string());
        }
        return Ok(old.clone());
    }
    let sequence = request_sequence(&op.request_id)
        .filter(|sequence| *sequence > 0)
        .ok_or_else(|| {
            "request ID must begin with a nonzero 64-bit big-endian sequence".to_string()
        })?;
    let high_water = s.inbound_sequence_high_water;
    if sequence <= high_water {
        if sequence_was_consumed_by_other_request(s, sequence, op.caller, &op.request_id) {
            return Err(SEQUENCE_BOUND_TO_DIFFERENT_REQUEST.to_string());
        }
        return Err(RESULT_UNAVAILABLE_STALE_SEQUENCE.to_string());
    }
    let expected = high_water
        .checked_add(1)
        .ok_or_else(|| "global request sequence exhausted".to_string())?;
    if sequence != expected {
        return Err(
            "stale AMM request sequence; operation was not started; fetch the next sequence"
                .to_string(),
        );
    }
    let mut op = op;
    op.sequence_managed = Some(true);
    compact_terminal_rows(s);
    let active_count = s
        .inbound_operations
        .iter()
        .filter(|row| {
            !matches!(
                row.phase,
                InboundOperationPhase::Completed
                    | InboundOperationPhase::ProvenNoEffect
                    | InboundOperationPhase::ResultUnavailable
            )
        })
        .count();
    if active_count >= MAX_ACTIVE_INBOUND_OPERATIONS {
        return Err(
            "inbound operation journal full; unresolved rows require reconciliation".to_string(),
        );
    }
    let pool = s
        .pools
        .get_mut(&op.pool_id)
        .ok_or_else(|| "pool not found".to_string())?;
    if pool.paused {
        return Err("pool is paused".to_string());
    }
    // Stable pause is the durable pool fence. It survives callback traps
    // and upgrades after the heap-only PoolGuard is lost.
    pool.paused = true;
    s.inbound_sequence_high_water = sequence;
    s.inbound_operations.push(op.clone());
    Ok(op)
}

pub fn inbound_operation(caller: Principal, request_id: &[u8]) -> Result<InboundOperation, String> {
    read_state(|s| inbound_operation_in(s, caller, request_id))
}

fn inbound_operation_in(
    s: &AmmState,
    caller: Principal,
    request_id: &[u8],
) -> Result<InboundOperation, String> {
    if let Some(operation) = s
        .inbound_operations
        .iter()
        .find(|x| x.caller == caller && x.request_id == request_id)
    {
        return Ok(operation.clone());
    }
    let sequence = request_sequence(request_id).unwrap_or(0);
    if sequence > 0 && sequence <= s.inbound_sequence_high_water {
        if sequence_was_consumed_by_other_request(s, sequence, caller, request_id) {
            Err(SEQUENCE_BOUND_TO_DIFFERENT_REQUEST.to_string())
        } else {
            Err(RESULT_UNAVAILABLE_STALE_SEQUENCE.to_string())
        }
    } else {
        Err("inbound operation not found".to_string())
    }
}

const SEQUENCE_BOUND_TO_DIFFERENT_REQUEST: &str = "AMM request sequence is bound to another request; this request was definitely not started and no input transfer was dispatched";
const RESULT_UNAVAILABLE_STALE_SEQUENCE: &str = "stale request sequence; result unavailable and this ID cannot execute again (its terminal row may have been compacted)";

/// A retained sequence-managed row is durable evidence that the global
/// sequence was consumed by a different identity. Absence is not evidence:
/// terminal rows may have been compacted, so those IDs remain unavailable.
fn sequence_was_consumed_by_other_request(
    s: &AmmState,
    sequence: u64,
    caller: Principal,
    request_id: &[u8],
) -> bool {
    s.inbound_operations.iter().any(|row| {
        row.sequence_managed == Some(true)
            && request_sequence(&row.request_id) == Some(sequence)
            && (row.caller != caller || row.request_id.as_slice() != request_id)
    })
}

pub fn set_inbound_leg_status(
    caller: Principal,
    request_id: &[u8],
    leg: usize,
    status: InboundLegStatus,
) -> Result<(), String> {
    mutate_state(|s| {
        let op = s
            .inbound_operations
            .iter_mut()
            .find(|x| x.caller == caller && x.request_id == request_id)
            .ok_or_else(|| "inbound operation not found".to_string())?;
        let row = op
            .legs
            .get_mut(leg)
            .ok_or_else(|| "inbound leg not found".to_string())?;
        row.status = status;
        Ok(())
    })
}

pub fn update_inbound_operation<F>(caller: Principal, request_id: &[u8], f: F) -> Result<(), String>
where
    F: FnOnce(&mut InboundOperation),
{
    mutate_state(|s| {
        let op = s
            .inbound_operations
            .iter_mut()
            .find(|x| x.caller == caller && x.request_id == request_id)
            .ok_or_else(|| "inbound operation not found".to_string())?;
        f(op);
        compact_terminal_rows(s);
        Ok(())
    })
}

impl AmmState {
    pub fn initialize(&mut self, args: AmmInitArgs) {
        self.admin = args.admin;
    }

    pub fn record_swap_event(
        &mut self,
        caller: Principal,
        pool_id: PoolId,
        token_in: Principal,
        amount_in: u128,
        token_out: Principal,
        amount_out: u128,
        fee: u128,
    ) {
        if self.swap_events.len() >= MAX_SWAP_EVENTS {
            self.swap_events.remove(0);
        }
        let event = AmmSwapEvent {
            id: self.next_swap_event_id,
            caller,
            pool_id,
            token_in,
            amount_in,
            token_out,
            amount_out,
            fee,
            timestamp: ic_cdk::api::time(),
        };
        self.swap_events.push(event);
        self.next_swap_event_id += 1;
    }

    pub fn record_liquidity_event(
        &mut self,
        caller: Principal,
        pool_id: PoolId,
        action: AmmLiquidityAction,
        token_a: Principal,
        amount_a: u128,
        token_b: Principal,
        amount_b: u128,
        lp_shares: u128,
    ) {
        if self.liquidity_events.len() >= MAX_LIQUIDITY_EVENTS {
            self.liquidity_events.remove(0);
        }
        let event = AmmLiquidityEvent {
            id: self.next_liquidity_event_id,
            caller,
            pool_id,
            action,
            token_a,
            amount_a,
            token_b,
            amount_b,
            lp_shares,
            timestamp: ic_cdk::api::time(),
        };
        self.liquidity_events.push(event);
        self.next_liquidity_event_id += 1;
    }

    pub fn record_admin_event(&mut self, caller: Principal, action: AmmAdminAction) {
        if self.admin_events.len() >= MAX_ADMIN_EVENTS {
            self.admin_events.remove(0);
        }
        let event = AmmAdminEvent {
            id: self.next_admin_event_id,
            caller,
            action,
            timestamp: ic_cdk::api::time(),
        };
        self.admin_events.push(event);
        self.next_admin_event_id += 1;
    }

    pub fn record_reward_event(
        &mut self,
        pool_id: PoolId,
        amount: u128,
        total_shares_at_time: u128,
        nonce: u64,
    ) {
        if self.reward_events.len() >= MAX_REWARD_EVENTS {
            self.reward_events.remove(0);
        }
        self.reward_events.push(AmmRewardEvent {
            id: self.next_reward_event_id,
            pool_id,
            amount,
            total_shares_at_time,
            nonce,
            timestamp: ic_cdk::api::time(),
        });
        self.next_reward_event_id += 1;
    }

    pub fn record_claim_event(&mut self, pool_id: PoolId, claimant: Principal, amount: u128) {
        if self.claim_events.len() >= MAX_CLAIM_EVENTS {
            self.claim_events.remove(0);
        }
        self.claim_events.push(AmmClaimEvent {
            id: self.next_claim_event_id,
            pool_id,
            claimant,
            amount,
            timestamp: ic_cdk::api::time(),
        });
        self.next_claim_event_id += 1;
    }
}

/// Persist a complete outbound transfer identity before dispatch. Capacity is
/// bounded and unresolved entries are never evicted.
pub fn reserve_outbound_payout(
    operation_id: String,
    ledger: Principal,
    from_subaccount: Option<[u8; 32]>,
    to: Principal,
    to_subaccount: Option<[u8; 32]>,
    gross_amount: u128,
    fee: u128,
    created_at_time: u64,
) -> Result<u64, String> {
    mutate_state(|s| {
        if let Some(existing) = s
            .outbound_payouts
            .iter()
            .find(|payout| payout.operation_id == operation_id)
        {
            if existing.ledger == ledger
                && existing.from == ic_cdk::id()
                && existing.from_subaccount == from_subaccount
                && existing.to == to
                && existing.to_subaccount == to_subaccount
                && existing.gross_amount == gross_amount
                && existing.fee == fee
            {
                return Ok(existing.id);
            }
            return Err(format!(
                "outbound operation {} is already bound to a different transfer tuple",
                operation_id
            ));
        }
        if s.outbound_payouts.len() >= MAX_OUTBOUND_PAYOUTS {
            return Err(format!(
                "outbound payout journal is full ({})",
                MAX_OUTBOUND_PAYOUTS
            ));
        }
        let id = s.next_outbound_payout_id;
        let next_id = id
            .checked_add(1)
            .ok_or_else(|| "outbound payout id exhausted".to_string())?;
        let mut hasher = Sha256::new();
        hasher.update(b"rumi_amm:payout:v1:");
        hasher.update(ic_cdk::id().as_slice());
        hasher.update(id.to_be_bytes());
        let memo = hasher.finalize().to_vec();
        s.next_outbound_payout_id = next_id;
        s.outbound_payouts.push(OutboundPayout {
            id,
            operation_id,
            ledger,
            from: ic_cdk::id(),
            from_subaccount,
            to,
            to_subaccount,
            gross_amount,
            net_amount: gross_amount.saturating_sub(fee),
            fee,
            memo,
            created_at_time,
            status: OutboundPayoutStatus::Reserved,
        });
        Ok(id)
    })
}

pub fn outbound_payout(id: u64) -> Result<OutboundPayout, String> {
    read_state(|s| s.outbound_payouts.iter().find(|p| p.id == id).cloned())
        .ok_or_else(|| format!("outbound payout {} not found", id))
}

pub fn outbound_payout_by_operation(operation_id: &str) -> Option<OutboundPayout> {
    read_state(|s| {
        s.outbound_payouts
            .iter()
            .find(|p| p.operation_id == operation_id)
            .cloned()
    })
}

pub fn unresolved_inbound_for_pool(pool_id: &str) -> bool {
    read_state(|s| {
        s.inbound_operations.iter().any(|op| {
            op.pool_id == pool_id
                && !matches!(
                    op.phase,
                    InboundOperationPhase::Completed
                        | InboundOperationPhase::ProvenNoEffect
                        | InboundOperationPhase::ResultUnavailable
                )
        })
    })
}

pub fn has_dispatched_or_ambiguous_outbound_payouts() -> bool {
    read_state(|s| {
        s.outbound_payouts.iter().any(|p| {
            matches!(
                p.status,
                OutboundPayoutStatus::Dispatched | OutboundPayoutStatus::Ambiguous
            )
        })
    })
}

pub fn set_outbound_payout_status(id: u64, status: OutboundPayoutStatus) -> Result<(), String> {
    mutate_state(|s| {
        let payout = s
            .outbound_payouts
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| format!("outbound payout {} not found", id))?;
        payout.status = status;
        Ok(())
    })
}

/// Attach durable evidence that a payout corresponds to a committed internal
/// accounting mutation. Call from the same `mutate_state` as that mutation.
pub fn link_outbound_payout_in(
    s: &mut AmmState,
    payout_id: u64,
    purpose: OutboundPayoutPurpose,
) -> Result<(), String> {
    let row = s.outbound_payouts.iter().find(|row| row.id == payout_id)
        .ok_or_else(|| format!("cannot link missing outbound payout {}", payout_id))?;
    if row.status != OutboundPayoutStatus::Reserved {
        return Err(format!("cannot attach accounting link to non-reserved payout {}", payout_id));
    }
    if let Some(existing) = s.outbound_payout_links.iter().find(|link| link.payout_id == payout_id) {
        return if existing.purpose == purpose {
            Ok(())
        } else {
            Err(format!("outbound payout {} is already linked to different accounting", payout_id))
        };
    }
    if s.outbound_payout_links.len() >= MAX_OUTBOUND_PAYOUTS {
        return Err("outbound accounting-link capacity reached".into());
    }
    s.outbound_payout_links.push(OutboundPayoutLink { payout_id, purpose });
    Ok(())
}

/// Release a reservation only when no outbound call was made, or after the
/// caller has atomically applied all accounting for a confirmed success.
pub fn finish_outbound_payout(id: u64) -> Result<(), String> {
    mutate_state(|s| {
        let idx = s
            .outbound_payouts
            .iter()
            .position(|p| p.id == id)
            .ok_or_else(|| format!("outbound payout {} not found", id))?;
        if s.outbound_payouts[idx].status == OutboundPayoutStatus::Ambiguous {
            return Err(format!(
                "outbound payout {} is ambiguous and cannot be cleared",
                id
            ));
        }
        s.outbound_payouts.remove(idx);
        s.outbound_payout_links.retain(|link| link.payout_id != id);
        Ok(())
    })
}

// ─── Thread-local state ───

thread_local! {
    static STATE: RefCell<AmmState> = RefCell::new(AmmState::default());
}

pub fn mutate_state<F, R>(f: F) -> R
where
    F: FnOnce(&mut AmmState) -> R,
{
    STATE.with(|s| f(&mut s.borrow_mut()))
}

pub fn read_state<F, R>(f: F) -> R
where
    F: FnOnce(&AmmState) -> R,
{
    STATE.with(|s| f(&s.borrow()))
}

pub fn replace_state(new_state: AmmState) {
    STATE.with(|s| {
        *s.borrow_mut() = new_state;
    });
}

// ─── Stable memory persistence ───

// SAFETY (UPG-004): this writes the encoded state at raw stable-memory offset 0
// using `stable64_write`, with a leading 8-byte length prefix. It does NOT use
// `ic_stable_structures::MemoryManager`. A future migration that introduces
// MemoryManager MUST first read the legacy blob into RAM via the same raw
// `stable64_read(0, ...)` path before calling `MemoryManager::init`, because
// `MemoryManager::init` unconditionally writes its 'MGR' magic header at
// physical offset 0 and would destructively overwrite the legacy state. See
// `liquidation_bot::post_upgrade` for the canonical "rescue legacy blob first,
// then init MemoryManager" pattern.
pub fn save_to_stable_memory() {
    STATE.with(|s| {
        let state = s.borrow();
        let bytes = Encode!(&*state).expect("Failed to encode AMM state");
        let len = bytes.len() as u64;

        let needed_pages = (len + 8 + 65535) / 65536;
        let current_pages = ic_cdk::api::stable::stable64_size();
        if needed_pages > current_pages {
            ic_cdk::api::stable::stable64_grow(needed_pages - current_pages)
                .expect("Failed to grow stable memory");
        }

        ic_cdk::api::stable::stable64_write(0, &len.to_le_bytes());
        ic_cdk::api::stable::stable64_write(8, &bytes);
    });
}

/// V5 state shape — a frozen snapshot of `AmmState` as of the 2026-06-05 audit
/// (SAT-004 fix), before outbound payout journaling was added. It intentionally
/// remains unchanged; decoding V5 initializes the newer journal fields empty.
///
/// WHY THIS EXISTS: the live `AmmState` carries ~13 fields beyond V4
/// (`swap_events`, `liquidity_events`, `admin_events`, `holder_snapshots`,
/// `reward_events`, `claim_events`, `protocol_backend_principal`,
/// `tvl_samples`, and their id counters). Before V5, the newest snapshot in
/// the fallback chain was V4. So the next time anyone added a NON-`Option`
/// field to `AmmState`, the on-chain bytes would fail `Decode!(_, AmmState)`
/// and fall through to V4, silently resetting `protocol_backend_principal`
/// (halting reward distribution) and dropping all post-V4 state WITHOUT a
/// trap — the exact 2026-05-18 state-wipe incident class (UPG-002).
///
/// MAINTENANCE RULE: before the next non-optional `AmmState` field is added,
/// snapshot the current shape as V6 and wire it into `try_decode_state` before
/// V5. Keep this V5 struct frozen — never edit it to track `AmmState`.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
struct AmmStateV5 {
    pub admin: Principal,
    pub pools: BTreeMap<PoolId, Pool>,
    pub pool_creation_open: bool,
    pub maintenance_mode: bool,
    pub pending_claims: Vec<PendingClaim>,
    pub next_claim_id: u64,
    pub swap_events: Vec<AmmSwapEvent>,
    pub next_swap_event_id: u64,
    pub liquidity_events: Vec<AmmLiquidityEvent>,
    pub next_liquidity_event_id: u64,
    pub admin_events: Vec<AmmAdminEvent>,
    pub next_admin_event_id: u64,
    pub holder_snapshots: Vec<HolderSnapshot>,
    pub reward_events: Vec<AmmRewardEvent>,
    pub next_reward_event_id: u64,
    pub claim_events: Vec<AmmClaimEvent>,
    pub next_claim_event_id: u64,
    pub protocol_backend_principal: Option<Principal>,
    pub tvl_samples: Vec<TvlSample>,
}

/// V4 state shape (has pending_claims but no swap_events).
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
struct AmmStateV4 {
    pub admin: Principal,
    pub pools: BTreeMap<PoolId, Pool>,
    pub pool_creation_open: bool,
    pub maintenance_mode: bool,
    pub pending_claims: Vec<PendingClaim>,
    pub next_claim_id: u64,
}

/// V3 state shape (has pool_creation_open + maintenance_mode, but no pending_claims).
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
struct AmmStateV3 {
    pub admin: Principal,
    pub pools: BTreeMap<PoolId, Pool>,
    pub pool_creation_open: bool,
    pub maintenance_mode: bool,
}

/// V2 state shape (has pool_creation_open but not maintenance_mode).
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
struct AmmStateV2 {
    pub admin: Principal,
    pub pools: BTreeMap<PoolId, Pool>,
    pub pool_creation_open: bool,
}

/// V1 state shape (before pool_creation_open was added).
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
struct AmmStateV1 {
    pub admin: Principal,
    pub pools: BTreeMap<PoolId, Pool>,
}

/// Frozen predecessor of the inbound-operation journal.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
struct AmmStateV6 {
    pub admin: Principal,
    pub pools: BTreeMap<PoolId, Pool>,
    pub pool_creation_open: bool,
    pub maintenance_mode: bool,
    pub pending_claims: Vec<PendingClaim>,
    pub next_claim_id: u64,
    pub swap_events: Vec<AmmSwapEvent>,
    pub next_swap_event_id: u64,
    pub liquidity_events: Vec<AmmLiquidityEvent>,
    pub next_liquidity_event_id: u64,
    pub admin_events: Vec<AmmAdminEvent>,
    pub next_admin_event_id: u64,
    pub holder_snapshots: Vec<HolderSnapshot>,
    pub reward_events: Vec<AmmRewardEvent>,
    pub next_reward_event_id: u64,
    pub claim_events: Vec<AmmClaimEvent>,
    pub next_claim_event_id: u64,
    pub protocol_backend_principal: Option<Principal>,
    pub tvl_samples: Vec<TvlSample>,
    pub outbound_payouts: Vec<OutboundPayout>,
    pub next_outbound_payout_id: u64,
}

/// Try to deserialize an AMM state snapshot, walking known schema versions
/// in order (current, V6, V5, V4, V3, V2, V1). Returns `None` if no version decodes.
pub fn try_decode_state(bytes: &[u8]) -> Option<AmmState> {
    if let Ok(mut state) = Decode!(bytes, AmmState) {
        // Candid may decode an older record with the newly-added optional fee
        // field set to None, so a V7 snapshot can succeed as the current type
        // and never reach the explicit V7 fallback below. Normalize that
        // migration here as well using the durable linked payout liability.
        let payout_fees: BTreeMap<u64, u128> = state
            .outbound_payouts
            .iter()
            .map(|payout| (payout.id, payout.fee))
            .collect();
        for operation in &mut state.inbound_operations {
            operation.sequence_managed.get_or_insert(false);
            if operation.output_ledger_fee.is_none() {
                operation.output_ledger_fee = operation
                    .output_payout_id
                    .and_then(|id| payout_fees.get(&id).copied());
            }
            hold_legacy_swap_without_fee(operation);
        }
        return Some(state);
    }
    if let Ok(v8) = Decode!(bytes, AmmStateV8) {
        let payout_fees: BTreeMap<u64, u128> = v8
            .outbound_payouts
            .iter()
            .map(|payout| (payout.id, payout.fee))
            .collect();
        let mut inbound_operations = v8.inbound_operations;
        for operation in &mut inbound_operations {
            operation.sequence_managed.get_or_insert(false);
            if operation.output_ledger_fee.is_none() {
                operation.output_ledger_fee = operation
                    .output_payout_id
                    .and_then(|id| payout_fees.get(&id).copied());
            }
            hold_legacy_swap_without_fee(operation);
        }
        return Some(AmmState {
            admin: v8.admin,
            pools: v8.pools,
            pool_creation_open: v8.pool_creation_open,
            maintenance_mode: v8.maintenance_mode,
            pending_claims: v8.pending_claims,
            next_claim_id: v8.next_claim_id,
            swap_events: v8.swap_events,
            next_swap_event_id: v8.next_swap_event_id,
            liquidity_events: v8.liquidity_events,
            next_liquidity_event_id: v8.next_liquidity_event_id,
            admin_events: v8.admin_events,
            next_admin_event_id: v8.next_admin_event_id,
            holder_snapshots: v8.holder_snapshots,
            reward_events: v8.reward_events,
            next_reward_event_id: v8.next_reward_event_id,
            claim_events: v8.claim_events,
            next_claim_event_id: v8.next_claim_event_id,
            protocol_backend_principal: v8.protocol_backend_principal,
            tvl_samples: v8.tvl_samples,
            outbound_payouts: v8.outbound_payouts,
            outbound_payout_links: Vec::new(),
            next_outbound_payout_id: v8.next_outbound_payout_id,
            inbound_operations,
            inbound_sequence_high_water: 0,
        });
    }
    if let Ok(v7) = Decode!(bytes, AmmStateV7) {
        let payout_fees: BTreeMap<u64, u128> = v7
            .outbound_payouts
            .iter()
            .map(|payout| (payout.id, payout.fee))
            .collect();
        return Some(AmmState {
            admin: v7.admin,
            pools: v7.pools,
            pool_creation_open: v7.pool_creation_open,
            maintenance_mode: v7.maintenance_mode,
            pending_claims: v7.pending_claims,
            next_claim_id: v7.next_claim_id,
            swap_events: v7.swap_events,
            next_swap_event_id: v7.next_swap_event_id,
            liquidity_events: v7.liquidity_events,
            next_liquidity_event_id: v7.next_liquidity_event_id,
            admin_events: v7.admin_events,
            next_admin_event_id: v7.next_admin_event_id,
            holder_snapshots: v7.holder_snapshots,
            reward_events: v7.reward_events,
            next_reward_event_id: v7.next_reward_event_id,
            claim_events: v7.claim_events,
            next_claim_event_id: v7.next_claim_event_id,
            protocol_backend_principal: v7.protocol_backend_principal,
            tvl_samples: v7.tvl_samples,
            outbound_payouts: v7.outbound_payouts,
            outbound_payout_links: Vec::new(),
            next_outbound_payout_id: v7.next_outbound_payout_id,
            inbound_operations: v7
                .inbound_operations
                .into_iter()
                .map(|op| {
                    let mut migrated = InboundOperation {
                        request_id: op.request_id,
                        caller: op.caller,
                        pool_id: op.pool_id,
                        kind: op.kind,
                        argument_digest: op.argument_digest,
                        legs: op.legs,
                        created_at_time: op.created_at_time,
                        phase: op.phase,
                        output_payout_id: op.output_payout_id,
                        result_amount: op.result_amount,
                        // If an old operation already reserved its output payout,
                        // recover the exact fee from that durable liability row.
                        output_ledger_fee: op
                            .output_payout_id
                            .and_then(|id| payout_fees.get(&id).copied()),
                        result_fee: op.result_fee,
                        protocol_fee: op.protocol_fee,
                        token_in: op.token_in,
                        sequence_managed: Some(false),
                        held_reason: None,
                    };
                    hold_legacy_swap_without_fee(&mut migrated);
                    migrated
                })
                .collect(),
            inbound_sequence_high_water: 0,
        });
    }
    if let Ok(v6) = Decode!(bytes, AmmStateV6) {
        return Some(AmmState {
            admin: v6.admin,
            pools: v6.pools,
            pool_creation_open: v6.pool_creation_open,
            maintenance_mode: v6.maintenance_mode,
            pending_claims: v6.pending_claims,
            next_claim_id: v6.next_claim_id,
            swap_events: v6.swap_events,
            next_swap_event_id: v6.next_swap_event_id,
            liquidity_events: v6.liquidity_events,
            next_liquidity_event_id: v6.next_liquidity_event_id,
            admin_events: v6.admin_events,
            next_admin_event_id: v6.next_admin_event_id,
            holder_snapshots: v6.holder_snapshots,
            reward_events: v6.reward_events,
            next_reward_event_id: v6.next_reward_event_id,
            claim_events: v6.claim_events,
            next_claim_event_id: v6.next_claim_event_id,
            protocol_backend_principal: v6.protocol_backend_principal,
            tvl_samples: v6.tvl_samples,
            outbound_payouts: v6.outbound_payouts,
            outbound_payout_links: Vec::new(),
            next_outbound_payout_id: v6.next_outbound_payout_id,
            inbound_operations: Vec::new(),
            inbound_sequence_high_water: 0,
        });
    }
    // V5: the frozen pre-payout-journal state shape. This fallback preserves
    // deployed fields and initializes only the newly introduced journal.
    if let Ok(v5) = Decode!(bytes, AmmStateV5) {
        return Some(AmmState {
            admin: v5.admin,
            pools: v5.pools,
            pool_creation_open: v5.pool_creation_open,
            maintenance_mode: v5.maintenance_mode,
            pending_claims: v5.pending_claims,
            next_claim_id: v5.next_claim_id,
            swap_events: v5.swap_events,
            next_swap_event_id: v5.next_swap_event_id,
            liquidity_events: v5.liquidity_events,
            next_liquidity_event_id: v5.next_liquidity_event_id,
            admin_events: v5.admin_events,
            next_admin_event_id: v5.next_admin_event_id,
            holder_snapshots: v5.holder_snapshots,
            reward_events: v5.reward_events,
            next_reward_event_id: v5.next_reward_event_id,
            claim_events: v5.claim_events,
            next_claim_event_id: v5.next_claim_event_id,
            protocol_backend_principal: v5.protocol_backend_principal,
            tvl_samples: v5.tvl_samples,
            outbound_payouts: Vec::new(),
            outbound_payout_links: Vec::new(),
            inbound_operations: Vec::new(),
            inbound_sequence_high_water: 0,
            next_outbound_payout_id: 0,
        });
    }
    if let Ok(v4) = Decode!(bytes, AmmStateV4) {
        return Some(AmmState {
            admin: v4.admin,
            pools: v4.pools,
            pool_creation_open: v4.pool_creation_open,
            maintenance_mode: v4.maintenance_mode,
            pending_claims: v4.pending_claims,
            next_claim_id: v4.next_claim_id,
            swap_events: Vec::new(),
            next_swap_event_id: 0,
            liquidity_events: Vec::new(),
            next_liquidity_event_id: 0,
            admin_events: Vec::new(),
            next_admin_event_id: 0,
            holder_snapshots: Vec::new(),
            reward_events: Vec::new(),
            next_reward_event_id: 0,
            claim_events: Vec::new(),
            next_claim_event_id: 0,
            protocol_backend_principal: None,
            tvl_samples: Vec::new(),
            outbound_payouts: Vec::new(),
            outbound_payout_links: Vec::new(),
            inbound_operations: Vec::new(),
            inbound_sequence_high_water: 0,
            next_outbound_payout_id: 0,
        });
    }
    if let Ok(v3) = Decode!(bytes, AmmStateV3) {
        return Some(AmmState {
            admin: v3.admin,
            pools: v3.pools,
            pool_creation_open: v3.pool_creation_open,
            maintenance_mode: v3.maintenance_mode,
            pending_claims: Vec::new(),
            next_claim_id: 0,
            swap_events: Vec::new(),
            next_swap_event_id: 0,
            liquidity_events: Vec::new(),
            next_liquidity_event_id: 0,
            admin_events: Vec::new(),
            next_admin_event_id: 0,
            holder_snapshots: Vec::new(),
            reward_events: Vec::new(),
            next_reward_event_id: 0,
            claim_events: Vec::new(),
            next_claim_event_id: 0,
            protocol_backend_principal: None,
            tvl_samples: Vec::new(),
            outbound_payouts: Vec::new(),
            outbound_payout_links: Vec::new(),
            inbound_operations: Vec::new(),
            inbound_sequence_high_water: 0,
            next_outbound_payout_id: 0,
        });
    }
    if let Ok(v2) = Decode!(bytes, AmmStateV2) {
        return Some(AmmState {
            admin: v2.admin,
            pools: v2.pools,
            pool_creation_open: v2.pool_creation_open,
            maintenance_mode: false,
            pending_claims: Vec::new(),
            next_claim_id: 0,
            swap_events: Vec::new(),
            next_swap_event_id: 0,
            liquidity_events: Vec::new(),
            next_liquidity_event_id: 0,
            admin_events: Vec::new(),
            next_admin_event_id: 0,
            holder_snapshots: Vec::new(),
            reward_events: Vec::new(),
            next_reward_event_id: 0,
            claim_events: Vec::new(),
            next_claim_event_id: 0,
            protocol_backend_principal: None,
            tvl_samples: Vec::new(),
            outbound_payouts: Vec::new(),
            outbound_payout_links: Vec::new(),
            inbound_operations: Vec::new(),
            inbound_sequence_high_water: 0,
            next_outbound_payout_id: 0,
        });
    }
    if let Ok(v1) = Decode!(bytes, AmmStateV1) {
        return Some(AmmState {
            admin: v1.admin,
            pools: v1.pools,
            pool_creation_open: false,
            maintenance_mode: false,
            pending_claims: Vec::new(),
            next_claim_id: 0,
            swap_events: Vec::new(),
            next_swap_event_id: 0,
            liquidity_events: Vec::new(),
            next_liquidity_event_id: 0,
            admin_events: Vec::new(),
            next_admin_event_id: 0,
            holder_snapshots: Vec::new(),
            reward_events: Vec::new(),
            next_reward_event_id: 0,
            claim_events: Vec::new(),
            next_claim_event_id: 0,
            protocol_backend_principal: None,
            tvl_samples: Vec::new(),
            outbound_payouts: Vec::new(),
            outbound_payout_links: Vec::new(),
            inbound_operations: Vec::new(),
            inbound_sequence_high_water: 0,
            next_outbound_payout_id: 0,
        });
    }
    None
}

/// Restore state from stable memory (called from post_upgrade).
///
/// Walk the V-current..V1 fallback chain via `try_decode_state`. If a known
/// version decodes, restore it.
///
/// If EVERY known version fails, TRAP (audit 2026-06-05). The previous behavior
/// fell back to empty state on the theory that "AMM positions are
/// reconstructable from underlying ledger balances" — but that is false: pool
/// `reserve_a/reserve_b` are pure internal accounting (never re-synced from
/// `icrc1_balance_of`) and the per-LP reward state (`acc_reward_per_share`,
/// `lp_rewards`, `pending_claims`) cannot be reconstructed at all. A silent
/// wipe of live pools is exactly the 2026-05-18 incident class. Trapping keeps
/// the canister on its old wasm with state intact until a fix ships, matching
/// the backend (UPG-001) and the other satellites.
pub fn load_from_stable_memory() {
    let mut len_bytes = [0u8; 8];
    ic_cdk::api::stable::stable64_read(0, &mut len_bytes);
    let len = u64::from_le_bytes(len_bytes) as usize;

    if len == 0 {
        return;
    }

    let mut bytes = vec![0u8; len];
    ic_cdk::api::stable::stable64_read(8, &mut bytes);

    if let Some(state) = try_decode_state(&bytes) {
        replace_state(state);
        return;
    }

    let preview_len = bytes.len().min(64);
    let preview_hex: String = bytes[..preview_len]
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();
    log!(
        INFO,
        "CRITICAL UPG-002: AMM snapshot decode failed for all known schema versions \
         (current, V5, V4, V3, V2, V1). snapshot_len={} bytes, first_{}_bytes_hex={}. \
         Trapping to preserve on-chain state (old wasm + stable memory stay intact) \
         rather than wiping live pools and reward state. Ship a wasm with a matching \
         AmmStateVN snapshot to recover.",
        bytes.len(),
        preview_len,
        preview_hex
    );
    ic_cdk::trap(
        "AMM post_upgrade: stable state did not decode under any known schema version \
         (current, V5, V4, V3, V2, V1); refusing to wipe live pools — see CRITICAL log",
    );
}

#[cfg(test)]
mod inbound_sequence_tests {
    use super::*;

    fn pool(token_a: Principal, token_b: Principal) -> Pool {
        Pool {
            token_a,
            token_b,
            reserve_a: 1_000_000,
            reserve_b: 1_000_000,
            fee_bps: 30,
            protocol_fee_bps: 0,
            curve: CurveType::ConstantProduct,
            lp_shares: BTreeMap::new(),
            total_lp_shares: 0,
            protocol_fees_a: 0,
            protocol_fees_b: 0,
            paused: false,
            subaccount_a: [1; 32],
            subaccount_b: [2; 32],
            lp_rewards: BTreeMap::new(),
            acc_reward_per_share: 0,
            pending_no_lp: 0,
            total_rewards_distributed: 0,
            processed_donation_nonces: Default::default(),
            reward_balance_snapshot: 0,
        }
    }

    fn op(caller: Principal, pool_id: &str, sequence: u64) -> InboundOperation {
        let mut request_id = vec![0; 32];
        request_id[..8].copy_from_slice(&sequence.to_be_bytes());
        request_id[8..].fill(0xA5);
        InboundOperation {
            request_id,
            caller,
            pool_id: pool_id.to_string(),
            kind: InboundOperationKind::Swap,
            argument_digest: vec![sequence as u8; 32],
            legs: vec![InboundLeg {
                ledger: Principal::anonymous(),
                from: caller,
                to_subaccount: None,
                amount: 10,
                fee: None,
                memo: vec![sequence as u8; 32],
                created_at_time: sequence,
                status: InboundLegStatus::Prepared,
            }],
            created_at_time: sequence,
            phase: InboundOperationPhase::Prepared,
            output_payout_id: None,
            result_amount: Some(1),
            output_ledger_fee: Some(0),
            result_fee: Some(0),
            protocol_fee: Some(0),
            token_in: Some(Principal::anonymous()),
            sequence_managed: Some(true),
            held_reason: None,
        }
    }

    #[test]
    fn global_sequence_keeps_old_active_ids_replayable_across_pools() {
        let caller = Principal::self_authenticating(&[1]);
        let token_a = Principal::self_authenticating(&[2]);
        let token_b = Principal::self_authenticating(&[3]);
        let mut state = AmmState::default();
        state.pools.insert("a".into(), pool(token_a, token_b));
        state.pools.insert("b".into(), pool(token_a, token_b));

        let first = op(caller, "a", 1);
        reserve_inbound_operation_in(&mut state, first.clone()).unwrap();
        reserve_inbound_operation_in(&mut state, op(caller, "b", 2)).unwrap();
        assert_eq!(state.inbound_sequence_high_water, 2);

        // Existing exact active ID is checked before the global stale-sequence
        // fence; this permits recovery even after another pool advances it.
        let replay = reserve_inbound_operation_in(&mut state, first.clone()).unwrap();
        assert_eq!(replay.request_id, first.request_id);
        let mut different_args = first.clone();
        different_args.argument_digest = vec![0xCC; 32];
        assert!(reserve_inbound_operation_in(&mut state, different_args).is_err());
    }

    #[test]
    fn compacted_terminal_ids_remain_permanently_non_executable() {
        let caller = Principal::self_authenticating(&[11]);
        let token_a = Principal::self_authenticating(&[12]);
        let token_b = Principal::self_authenticating(&[13]);
        let mut state = AmmState::default();
        state.pools.insert("a".into(), pool(token_a, token_b));
        let terminal = op(caller, "a", 1);
        reserve_inbound_operation_in(&mut state, terminal.clone()).unwrap();
        state.inbound_operations[0].phase = InboundOperationPhase::Completed;
        state.inbound_sequence_high_water = 1_000;
        compact_terminal_rows(&mut state);
        assert!(state.inbound_operations.is_empty());
        assert_eq!(
            inbound_operation_in(&state, caller, &terminal.request_id).unwrap_err(),
            RESULT_UNAVAILABLE_STALE_SEQUENCE
        );
        assert_eq!(
            reserve_inbound_operation_in(&mut state, terminal).unwrap_err(),
            RESULT_UNAVAILABLE_STALE_SEQUENCE
        );
        assert_eq!(state.inbound_sequence_high_water, 1_000);
    }

    #[test]
    fn retained_sequence_owner_proves_other_request_never_started() {
        let owner = Principal::self_authenticating(&[21]);
        let other = Principal::self_authenticating(&[22]);
        let token_a = Principal::self_authenticating(&[23]);
        let token_b = Principal::self_authenticating(&[24]);
        let mut state = AmmState::default();
        state.pools.insert("a".into(), pool(token_a, token_b));
        let consumed = op(owner, "a", 1);
        reserve_inbound_operation_in(&mut state, consumed.clone()).unwrap();

        let other_caller_same_sequence = op(other, "a", 1);
        assert_eq!(
            inbound_operation_in(&state, other, &other_caller_same_sequence.request_id)
                .unwrap_err(),
            SEQUENCE_BOUND_TO_DIFFERENT_REQUEST
        );
        assert_eq!(
            reserve_inbound_operation_in(&mut state, other_caller_same_sequence).unwrap_err(),
            SEQUENCE_BOUND_TO_DIFFERENT_REQUEST
        );

        let mut same_caller_different_id = consumed.clone();
        same_caller_different_id.request_id[31] ^= 1;
        assert_eq!(
            inbound_operation_in(&state, owner, &same_caller_different_id.request_id)
                .unwrap_err(),
            SEQUENCE_BOUND_TO_DIFFERENT_REQUEST
        );
        assert_eq!(
            reserve_inbound_operation_in(&mut state, same_caller_different_id).unwrap_err(),
            SEQUENCE_BOUND_TO_DIFFERENT_REQUEST
        );

        // The retained exact row remains replayable despite its consumed seq.
        assert!(reserve_inbound_operation_in(&mut state, consumed).is_ok());
    }

    #[test]
    fn legacy_random_terminal_ids_are_not_misread_as_sequences() {
        let caller = Principal::self_authenticating(&[15]);
        let mut state = AmmState::default();
        state.inbound_sequence_high_water = 10_000;
        let mut legacy = op(caller, "old-pool", 1);
        legacy.phase = InboundOperationPhase::Completed;
        legacy.sequence_managed = Some(false);
        state.inbound_operations.push(legacy.clone());
        compact_terminal_rows(&mut state);
        assert_eq!(state.inbound_operations.len(), 1);
        assert_eq!(state.inbound_operations[0].request_id, legacy.request_id);
    }

    #[test]
    fn successful_lifetime_throughput_does_not_hit_active_row_cap() {
        let caller = Principal::self_authenticating(&[21]);
        let token_a = Principal::self_authenticating(&[22]);
        let token_b = Principal::self_authenticating(&[23]);
        let mut state = AmmState::default();
        state.pools.insert("pool".into(), pool(token_a, token_b));

        for sequence in 1..=1_500 {
            let operation = op(caller, "pool", sequence);
            reserve_inbound_operation_in(&mut state, operation).unwrap();
            state.inbound_operations.last_mut().unwrap().phase = InboundOperationPhase::Completed;
            state.pools.get_mut("pool").unwrap().paused = false;
            compact_terminal_rows(&mut state);
        }

        assert_eq!(state.inbound_sequence_high_water, 1_500);
        assert_eq!(
            state.inbound_operations.len(),
            INBOUND_TERMINAL_REPLAY_WINDOW as usize
        );
        assert_eq!(
            next_sequence_from_state(&state),
            1_501,
            "high-water advances independently from compacted terminal rows"
        );
    }

    #[test]
    fn v7_output_pending_migration_recovers_fee_from_saved_payout() {
        let caller = Principal::self_authenticating(&[31]);
        let mut state = AmmState::default();
        state.outbound_payouts.push(OutboundPayout {
            id: 77,
            operation_id: "swap:v2:request".into(),
            ledger: caller,
            from: caller,
            from_subaccount: Some([1; 32]),
            to: caller,
            to_subaccount: None,
            gross_amount: 1_010,
            net_amount: 1_000,
            fee: 10,
            memo: vec![4; 32],
            created_at_time: 123,
            status: OutboundPayoutStatus::Ambiguous,
        });
        let mut v7 = AmmStateV7 {
            admin: caller,
            pools: BTreeMap::new(),
            pool_creation_open: false,
            maintenance_mode: false,
            pending_claims: Vec::new(),
            next_claim_id: 0,
            swap_events: Vec::new(),
            next_swap_event_id: 0,
            liquidity_events: Vec::new(),
            next_liquidity_event_id: 0,
            admin_events: Vec::new(),
            next_admin_event_id: 0,
            holder_snapshots: Vec::new(),
            reward_events: Vec::new(),
            next_reward_event_id: 0,
            claim_events: Vec::new(),
            next_claim_event_id: 0,
            protocol_backend_principal: None,
            tvl_samples: Vec::new(),
            outbound_payouts: state.outbound_payouts,
            next_outbound_payout_id: 78,
            inbound_operations: vec![InboundOperationV7 {
                request_id: vec![1; 32],
                caller,
                pool_id: "pool".into(),
                kind: InboundOperationKind::Swap,
                argument_digest: vec![2; 32],
                legs: Vec::new(),
                created_at_time: 123,
                phase: InboundOperationPhase::OutputPending,
                output_payout_id: Some(77),
                result_amount: Some(1_010),
                result_fee: Some(0),
                protocol_fee: Some(0),
                token_in: Some(caller),
            }],
        };
        let snapshot = Encode!(&v7).expect("encode V7 predecessor fixture");
        let migrated = try_decode_state(&snapshot).expect("decode V7 predecessor fixture");
        let op = &migrated.inbound_operations[0];
        assert_eq!(op.phase, InboundOperationPhase::OutputPending);
        assert_eq!(op.output_payout_id, Some(77));
        assert_eq!(op.output_ledger_fee, Some(10));
        assert_eq!(migrated.outbound_payouts[0].net_amount, 1_000);

        // A V7 completed swap whose payout row has already been retired has
        // no source for the exact fee/net result. Migration must hold it, not
        // report a guessed result or attempt another transfer.
        v7.inbound_operations[0].phase = InboundOperationPhase::Completed;
        v7.outbound_payouts.clear();
        let snapshot = Encode!(&v7).expect("encode completed V7 predecessor fixture");
        let migrated = try_decode_state(&snapshot).expect("decode completed V7 fixture");
        let op = &migrated.inbound_operations[0];
        assert_eq!(op.phase, InboundOperationPhase::ResultUnavailable);
        assert_eq!(op.output_ledger_fee, None);
        assert!(op
            .held_reason
            .as_deref()
            .unwrap()
            .contains("pinned output fee"));
    }

    #[test]
    fn v8_ambiguous_payout_migrates_without_invented_accounting_link() {
        let caller = Principal::self_authenticating(&[41]);
        let payout = OutboundPayout {
            id: 8,
            operation_id: "remove_liquidity_b:legacy:caller".into(),
            ledger: caller,
            from: caller,
            from_subaccount: Some([9; 32]),
            to: caller,
            to_subaccount: None,
            gross_amount: 20,
            net_amount: 10,
            fee: 10,
            memo: vec![8; 32],
            created_at_time: 123,
            status: OutboundPayoutStatus::Ambiguous,
        };
        let v8 = AmmStateV8 {
            admin: caller,
            pools: BTreeMap::new(),
            pool_creation_open: false,
            maintenance_mode: true,
            pending_claims: Vec::new(),
            next_claim_id: 0,
            swap_events: Vec::new(),
            next_swap_event_id: 0,
            liquidity_events: Vec::new(),
            next_liquidity_event_id: 0,
            admin_events: Vec::new(),
            next_admin_event_id: 0,
            holder_snapshots: Vec::new(),
            reward_events: Vec::new(),
            next_reward_event_id: 0,
            claim_events: Vec::new(),
            next_claim_event_id: 0,
            protocol_backend_principal: None,
            tvl_samples: Vec::new(),
            outbound_payouts: vec![payout.clone()],
            next_outbound_payout_id: 9,
            inbound_operations: Vec::new(),
        };

        let bytes = Encode!(&v8).unwrap();
        let migrated = try_decode_state(&bytes).expect("frozen V8 state migrates");
        assert!(migrated.outbound_payout_links.is_empty());
        assert_eq!(migrated.outbound_payouts, vec![payout]);
        assert!(migrated.maintenance_mode, "migration retains the held rollout state");
    }

    #[test]
    fn payout_link_is_not_created_by_reservation_and_is_bound_to_reserved_row() {
        let caller = Principal::self_authenticating(&[51]);
        let payout = OutboundPayout {
            id: 3,
            operation_id: "remove_liquidity_a:pool:user".into(),
            ledger: caller,
            from: caller,
            from_subaccount: Some([1; 32]),
            to: caller,
            to_subaccount: None,
            gross_amount: 20,
            net_amount: 10,
            fee: 10,
            memo: vec![3; 32],
            created_at_time: 123,
            status: OutboundPayoutStatus::Reserved,
        };
        let mut state = AmmState::default();
        state.outbound_payouts.push(payout);
        assert!(state.outbound_payout_links.is_empty(), "reservation alone is not accounting proof");

        let purpose = OutboundPayoutPurpose::RemoveLiquidity {
            pool_id: "pool".into(),
            caller,
            leg: OutboundPayoutLeg::TokenA,
            gross_amount: 20,
        };
        link_outbound_payout_in(&mut state, 3, purpose.clone()).unwrap();
        assert_eq!(state.outbound_payout_links.len(), 1);
        state.outbound_payouts[0].status = OutboundPayoutStatus::Dispatched;
        assert!(link_outbound_payout_in(&mut state, 3, purpose).is_err());
    }

    fn next_sequence_from_state(state: &AmmState) -> u64 {
        state.inbound_sequence_high_water + 1
    }
}
