use candid::{CandidType, Decode, Encode, Principal};
use ic_canister_log::log;
use serde::{Deserialize, Serialize};
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
/// Permanent per-principal high-water rows prevent old V2 request IDs from
/// becoming replayable after successful settlement. New principals fail closed
/// when this bounded replay-defense table is full.
pub const MAX_INGRESS_OPERATIONS: usize = 10_000;
pub const MAX_REWARD_EVENTS: usize = 50_000;
pub const MAX_CLAIM_EVENTS: usize = 50_000;
/// Historical compatibility constant. Donation receipt identities are no
/// longer pruned at this boundary because backend retries are durable.
pub const MAX_PROCESSED_NONCES: usize = 1024;
pub const REWARD_SCALE: u128 = 1_000_000_000_000; // 1e12 fixed-point for acc_reward_per_share
/// Minimum claimable amount: 10x the live icUSD ledger fee (100_000 e8s =
/// 0.001 icUSD), i.e. 1_000_000 e8s = 0.01 icUSD. `claim_rewards` pays out
/// via `transfer_reward_icusd`, which sends `claimable - fee` and fails
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
    /// Exact, lossless outbound obligations. Unlike legacy `pending_claims`,
    /// these contain the original ledger transfer identity and are never
    /// evicted to make room for a new payout.
    #[serde(default)]
    pub pending_payouts: Vec<AmmPayoutAttempt>,
    #[serde(default)]
    pub next_payout_id: u64,
    /// Active/completed V2 requests are retained by caller until superseded by
    /// a strictly newer request id, preventing a lost reply from pulling twice.
    #[serde(default)]
    pub ingress_operations: Vec<AmmIngressOperation>,
    #[serde(default)]
    pub next_operation_id: u64,
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
            pending_payouts: Vec::new(),
            next_payout_id: 0,
            ingress_operations: Vec::new(),
            next_operation_id: 0,
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
        }
    }
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

/// V5 state shape — a frozen snapshot of `AmmState` AS OF the 2026-06-05 audit
/// (SAT-004 fix). It mirrors every field of the live `AmmState` at this commit.
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
/// MAINTENANCE RULE: whenever you add a non-`Option` field to `AmmState`,
/// FIRST add a new `AmmStateVN` that snapshots the *previous* shape (a copy of
/// this struct) and wire it into `try_decode_state` ahead of V4. Keep this V5
/// struct frozen — never edit it to track `AmmState`.
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

/// Try to deserialize an AMM state snapshot, walking known schema versions
/// in order (current, V4, V3, V2, V1). Returns `None` if no version decodes.
pub fn try_decode_state(bytes: &[u8]) -> Option<AmmState> {
    if let Ok(state) = Decode!(bytes, AmmState) {
        if !current_journal_options_match_wire(bytes, &state) {
            return None;
        }
        return Some(state);
    }
    // Candid permits decoding a record into a smaller record type, dropping
    // fields that the smaller type does not know. Do not enter that migration
    // chain for any wire record that contains a financial journal introduced
    // after V5; a nested type mismatch must leave the upgrade un-restored,
    // never silently clear those obligations.
    if !legacy_fallback_has_no_new_journals(bytes) {
        return None;
    }
    // V5: the frozen snapshot of the current shape. This is what protects a
    // future non-Option field addition from silently falling through to V4 and
    // wiping post-V4 state (SAT-004). It maps 1:1 onto AmmState today.
    if let Ok(v5) = Decode!(bytes, AmmStateV5) {
        return Some(AmmState {
            admin: v5.admin,
            pools: v5.pools,
            pool_creation_open: v5.pool_creation_open,
            maintenance_mode: v5.maintenance_mode,
            pending_claims: v5.pending_claims,
            next_claim_id: v5.next_claim_id,
            pending_payouts: Vec::new(),
            next_payout_id: 0,
            ingress_operations: Vec::new(),
            next_operation_id: 0,
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
            pending_payouts: Vec::new(),
            next_payout_id: 0,
            ingress_operations: Vec::new(),
            next_operation_id: 0,
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
            pending_payouts: Vec::new(),
            next_payout_id: 0,
            ingress_operations: Vec::new(),
            next_operation_id: 0,
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
            pending_payouts: Vec::new(),
            next_payout_id: 0,
            ingress_operations: Vec::new(),
            next_operation_id: 0,
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
            pending_payouts: Vec::new(),
            next_payout_id: 0,
            ingress_operations: Vec::new(),
            next_operation_id: 0,
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
        });
    }
    None
}

fn legacy_fallback_has_no_new_journals(bytes: &[u8]) -> bool {
    use candid::{IDLArgs, IDLValue};

    let Ok(args) = IDLArgs::from_bytes(bytes) else {
        return false;
    };
    let [IDLValue::Record(fields)] = args.args.as_slice() else {
        return false;
    };
    const NEW_FINANCIAL_JOURNALS: &[&str] = &["pending_payouts", "ingress_operations"];
    !fields.iter().any(|field| {
        NEW_FINANCIAL_JOURNALS
            .iter()
            .any(|name| field.id.get_id() == candid::idl_hash(name))
    })
}

fn current_journal_options_match_wire(bytes: &[u8], state: &AmmState) -> bool {
    use candid::IDLValue;

    let Some(args) = candid::IDLArgs::from_bytes(bytes).ok() else {
        return false;
    };
    let Some(fields) = state_record_fields(&args) else {
        return false;
    };
    let Some(payout_values) = record_vec_field(&fields, "pending_payouts") else {
        return false;
    };
    if payout_values.len() != state.pending_payouts.len()
        || !payout_values
            .iter()
            .zip(&state.pending_payouts)
            .all(|(wire, payout)| {
                let IDLValue::Record(fields) = wire else {
                    return false;
                };
                payout_options_match_wire(fields, payout)
            })
    {
        return false;
    }

    let Some(ingress_values) = record_vec_field(&fields, "ingress_operations") else {
        return false;
    };
    ingress_values.len() == state.ingress_operations.len()
        && ingress_values
            .iter()
            .zip(&state.ingress_operations)
            .all(|(wire, operation)| {
                let IDLValue::Record(fields) = wire else {
                    return false;
                };
                if !optional_field_matches(fields, "result", operation.result.is_some())
                    || !optional_field_matches(fields, "last_error", operation.last_error.is_some())
                {
                    return false;
                }
                let Some(legs) = record_vec_field(fields, "legs") else {
                    return false;
                };
                legs.len() == operation.legs.len()
                    && legs.iter().zip(&operation.legs).all(|(wire_leg, leg)| {
                        let IDLValue::Record(fields) = wire_leg else {
                            return false;
                        };
                        optional_field_matches(fields, "transfer_fee", leg.transfer_fee.is_some())
                            && optional_field_matches(
                                fields,
                                "block_index",
                                leg.block_index.is_some(),
                            )
                            && optional_field_matches(
                                fields,
                                "receipt_scan_start",
                                leg.receipt_scan_start.is_some(),
                            )
                            && optional_field_matches(
                                fields,
                                "receipt_scan_end",
                                leg.receipt_scan_end.is_some(),
                            )
                    })
            })
}

fn payout_options_match_wire(
    fields: &[candid::types::value::IDLField],
    payout: &AmmPayoutAttempt,
) -> bool {
    optional_field_matches(fields, "send_amount", payout.send_amount.is_some())
        && optional_field_matches(fields, "fee", payout.fee.is_some())
        && optional_field_matches(
            fields,
            "receipt_scan_start",
            payout.receipt_scan_start.is_some(),
        )
        && optional_field_matches(
            fields,
            "receipt_scan_end",
            payout.receipt_scan_end.is_some(),
        )
        && optional_field_matches(fields, "last_error", payout.last_error.is_some())
}

fn optional_field_matches(
    fields: &[candid::types::value::IDLField],
    name: &str,
    decoded_has_value: bool,
) -> bool {
    let Some(field) = fields
        .iter()
        .find(|field| field.id.get_id() == candid::idl_hash(name))
    else {
        return !decoded_has_value;
    };
    match &field.val {
        candid::IDLValue::None => !decoded_has_value,
        candid::IDLValue::Opt(_) => decoded_has_value,
        _ => false,
    }
}

fn record_vec_field<'a>(
    fields: &'a [candid::types::value::IDLField],
    name: &str,
) -> Option<&'a [candid::IDLValue]> {
    let field = fields
        .iter()
        .find(|field| field.id.get_id() == candid::idl_hash(name))?;
    match &field.val {
        candid::IDLValue::Vec(values) => Some(values),
        _ => None,
    }
}

fn state_record_fields(args: &candid::IDLArgs) -> Option<&[candid::types::value::IDLField]> {
    match args.args.as_slice() {
        [candid::IDLValue::Record(fields)] => Some(fields),
        _ => None,
    }
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
mod payout_state_migration_tests {
    use super::*;

    #[test]
    fn incompatible_nested_payout_option_is_not_accepted_as_none() {
        use candid::types::{value::IDLField, Label};

        let wire_field = IDLField {
            id: Label::Named("send_amount".into()),
            val: candid::IDLValue::Text("wrong wire type".into()),
        };
        assert!(!optional_field_matches(&[wire_field], "send_amount", false));

        let wire_none = IDLField {
            id: Label::Named("send_amount".into()),
            val: candid::IDLValue::None,
        };
        assert!(optional_field_matches(&[wire_none], "send_amount", false));
    }

    #[test]
    fn current_snapshot_with_coerced_nested_payout_option_is_rejected() {
        #[derive(CandidType)]
        struct PayoutWithWrongSendAmount {
            id: u64,
            operation_id: u64,
            pool_id: PoolId,
            claimant: Principal,
            ledger: Principal,
            subaccount: [u8; 32],
            gross_amount: u128,
            send_amount: String,
            fee: Option<u128>,
            memo: Vec<u8>,
            created_at_time: u64,
            attempt_generation: u32,
            dispatch_count: u32,
            phase: AmmPayoutPhase,
            receipt_scan_start: Option<u64>,
            receipt_scan_cursor: u64,
            receipt_scan_end: Option<u64>,
            last_error: Option<String>,
        }

        #[derive(CandidType)]
        struct StateWithWrongPayoutOption {
            admin: Principal,
            pools: BTreeMap<PoolId, Pool>,
            pool_creation_open: bool,
            maintenance_mode: bool,
            pending_claims: Vec<PendingClaim>,
            next_claim_id: u64,
            pending_payouts: Vec<PayoutWithWrongSendAmount>,
            next_payout_id: u64,
            ingress_operations: Vec<AmmIngressOperation>,
            next_operation_id: u64,
            swap_events: Vec<AmmSwapEvent>,
            next_swap_event_id: u64,
            liquidity_events: Vec<AmmLiquidityEvent>,
            next_liquidity_event_id: u64,
            admin_events: Vec<AmmAdminEvent>,
            next_admin_event_id: u64,
            holder_snapshots: Vec<HolderSnapshot>,
            reward_events: Vec<AmmRewardEvent>,
            next_reward_event_id: u64,
            claim_events: Vec<AmmClaimEvent>,
            next_claim_event_id: u64,
            protocol_backend_principal: Option<Principal>,
            tvl_samples: Vec<TvlSample>,
        }

        let mut state = AmmState::default();
        state.pending_payouts.push(AmmPayoutAttempt {
            id: 1,
            operation_id: 1,
            pool_id: "pool".into(),
            claimant: Principal::self_authenticating(b"claimant"),
            ledger: Principal::self_authenticating(b"ledger"),
            subaccount: [0; 32],
            gross_amount: 1,
            send_amount: None,
            fee: None,
            memo: Vec::new(),
            created_at_time: 1,
            attempt_generation: 0,
            dispatch_count: 0,
            phase: AmmPayoutPhase::Staged,
            receipt_scan_start: None,
            receipt_scan_cursor: 0,
            receipt_scan_end: None,
            last_error: None,
        });
        let wrong_payout = PayoutWithWrongSendAmount {
            id: 1,
            operation_id: 1,
            pool_id: "pool".into(),
            claimant: state.pending_payouts[0].claimant,
            ledger: state.pending_payouts[0].ledger,
            subaccount: [0; 32],
            gross_amount: 1,
            send_amount: "wrong type coerced to None".into(),
            fee: None,
            memo: Vec::new(),
            created_at_time: 1,
            attempt_generation: 0,
            dispatch_count: 0,
            phase: AmmPayoutPhase::Staged,
            receipt_scan_start: None,
            receipt_scan_cursor: 0,
            receipt_scan_end: None,
            last_error: None,
        };
        let newer = StateWithWrongPayoutOption {
            admin: state.admin,
            pools: state.pools,
            pool_creation_open: state.pool_creation_open,
            maintenance_mode: state.maintenance_mode,
            pending_claims: state.pending_claims,
            next_claim_id: state.next_claim_id,
            pending_payouts: vec![wrong_payout],
            next_payout_id: state.next_payout_id,
            ingress_operations: state.ingress_operations,
            next_operation_id: state.next_operation_id,
            swap_events: state.swap_events,
            next_swap_event_id: state.next_swap_event_id,
            liquidity_events: state.liquidity_events,
            next_liquidity_event_id: state.next_liquidity_event_id,
            admin_events: state.admin_events,
            next_admin_event_id: state.next_admin_event_id,
            holder_snapshots: state.holder_snapshots,
            reward_events: state.reward_events,
            next_reward_event_id: state.next_reward_event_id,
            claim_events: state.claim_events,
            next_claim_event_id: state.next_claim_event_id,
            protocol_backend_principal: state.protocol_backend_principal,
            tvl_samples: state.tvl_samples,
        };
        let bytes = Encode!(&newer).expect("encode current snapshot with bad nested option");
        let decoded = Decode!(&bytes, AmmState).expect("Candid opt coercion should decode");
        assert!(decoded.pending_payouts[0].send_amount.is_none());
        assert!(try_decode_state(&bytes).is_none());
    }

    #[test]
    fn incompatible_new_journal_never_falls_back_to_v5() {
        #[derive(CandidType)]
        struct NewerSnapshotWithIncompatiblePayoutJournal {
            admin: Principal,
            pools: BTreeMap<PoolId, Pool>,
            pool_creation_open: bool,
            maintenance_mode: bool,
            pending_claims: Vec<PendingClaim>,
            next_claim_id: u64,
            swap_events: Vec<AmmSwapEvent>,
            next_swap_event_id: u64,
            liquidity_events: Vec<AmmLiquidityEvent>,
            next_liquidity_event_id: u64,
            admin_events: Vec<AmmAdminEvent>,
            next_admin_event_id: u64,
            holder_snapshots: Vec<HolderSnapshot>,
            reward_events: Vec<AmmRewardEvent>,
            next_reward_event_id: u64,
            claim_events: Vec<AmmClaimEvent>,
            next_claim_event_id: u64,
            protocol_backend_principal: Option<Principal>,
            tvl_samples: Vec<TvlSample>,
            pending_payouts: String,
        }

        let snapshot = AmmStateV5 {
            admin: Principal::self_authenticating(b"test admin"),
            pools: BTreeMap::new(),
            pool_creation_open: true,
            maintenance_mode: false,
            pending_claims: Vec::new(),
            next_claim_id: 1,
            swap_events: Vec::new(),
            next_swap_event_id: 2,
            liquidity_events: Vec::new(),
            next_liquidity_event_id: 3,
            admin_events: Vec::new(),
            next_admin_event_id: 4,
            holder_snapshots: Vec::new(),
            reward_events: Vec::new(),
            next_reward_event_id: 5,
            claim_events: Vec::new(),
            next_claim_event_id: 6,
            protocol_backend_principal: Some(Principal::self_authenticating(b"test backend")),
            tvl_samples: Vec::new(),
        };
        let newer = NewerSnapshotWithIncompatiblePayoutJournal {
            admin: snapshot.admin,
            pools: snapshot.pools,
            pool_creation_open: snapshot.pool_creation_open,
            maintenance_mode: snapshot.maintenance_mode,
            pending_claims: snapshot.pending_claims,
            next_claim_id: snapshot.next_claim_id,
            swap_events: snapshot.swap_events,
            next_swap_event_id: snapshot.next_swap_event_id,
            liquidity_events: snapshot.liquidity_events,
            next_liquidity_event_id: snapshot.next_liquidity_event_id,
            admin_events: snapshot.admin_events,
            next_admin_event_id: snapshot.next_admin_event_id,
            holder_snapshots: snapshot.holder_snapshots,
            reward_events: snapshot.reward_events,
            next_reward_event_id: snapshot.next_reward_event_id,
            claim_events: snapshot.claim_events,
            next_claim_event_id: snapshot.next_claim_event_id,
            protocol_backend_principal: snapshot.protocol_backend_principal,
            tvl_samples: snapshot.tvl_samples,
            pending_payouts: "incompatible journal shape".into(),
        };
        let incompatible = Encode!(&newer).expect("encode newer incompatible snapshot");

        assert!(Decode!(&incompatible, AmmState).is_err());
        assert!(Decode!(&incompatible, AmmStateV5).is_ok());
        assert!(try_decode_state(&incompatible).is_none());
    }

    #[test]
    fn pre_v6_v5_snapshot_migrates_with_empty_journals_and_preserved_state() {
        let admin = Principal::self_authenticating(b"v5 admin");
        let backend = Principal::self_authenticating(b"v5 backend");
        let old = AmmStateV5 {
            admin,
            pools: BTreeMap::new(),
            pool_creation_open: true,
            maintenance_mode: false,
            pending_claims: Vec::new(),
            next_claim_id: 17,
            swap_events: Vec::new(),
            next_swap_event_id: 23,
            liquidity_events: Vec::new(),
            next_liquidity_event_id: 29,
            admin_events: Vec::new(),
            next_admin_event_id: 31,
            holder_snapshots: Vec::new(),
            reward_events: Vec::new(),
            next_reward_event_id: 37,
            claim_events: Vec::new(),
            next_claim_event_id: 41,
            protocol_backend_principal: Some(backend),
            tvl_samples: Vec::new(),
        };
        let bytes = Encode!(&old).expect("encode exact pre-candidate V5 snapshot");
        let migrated = try_decode_state(&bytes).expect("V5 state remains decodable");
        assert_eq!(migrated.admin, admin);
        assert_eq!(migrated.protocol_backend_principal, Some(backend));
        assert_eq!(migrated.next_claim_id, 17);
        assert_eq!(migrated.next_swap_event_id, 23);
        assert!(migrated.pending_payouts.is_empty());
        assert_eq!(migrated.next_payout_id, 0);
        assert!(migrated.ingress_operations.is_empty());
        assert_eq!(migrated.next_operation_id, 0);
    }
}
