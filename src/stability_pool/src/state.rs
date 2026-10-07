use candid::{CandidType, Decode, Encode, Principal};
use ic_canister_log::log;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use crate::logs::INFO;
use crate::types::*;

pub const ICUSD_TRANSFER_FEE_E8S: u64 = 100_000;
pub const CK_STABLE_TRANSFER_FEE_E6: u64 = 10_000;
pub const THREE_USD_TRANSFER_FEE: u64 = 0;

/// Maximum number of source mint receipts retained for interest distribution.
/// Receipts outside the exact replay window fail closed instead of being credited again.
pub const MAX_PROCESSED_INTEREST_MINT_BLOCKS: usize = 10_000;
pub const MAX_COMPLETED_SP_THREE_USD_ABSORBS: usize = 256;
/// Maximum lifetime source receipts retained for unallocated-interest forwards.
/// At capacity, new receipts remain pending at the backend for reconciliation.
pub const MAX_UNALLOCATED_INTEREST_MINT_RECEIPTS: usize = 10_000;

/// Split a debit pro rata with deterministic remainder assignment, while
/// never charging a position more than its available balance.
fn exact_proportional_debit_allocations(
    balances: &[(Principal, u64)],
    amount: u64,
) -> Result<BTreeMap<Principal, u64>, StabilityPoolError> {
    if amount == 0 {
        return Ok(BTreeMap::new());
    }
    let total = balances.iter().try_fold(0u64, |sum, (_, balance)| {
        sum.checked_add(*balance)
            .ok_or(StabilityPoolError::SystemBusy)
    })?;
    if total < amount || total == 0 {
        return Err(StabilityPoolError::InsufficientPoolBalance);
    }
    let mut allocations = BTreeMap::new();
    let mut allocated = 0u64;
    for (owner, balance) in balances {
        let share = ((amount as u128) * (*balance as u128) / (total as u128)) as u64;
        allocated = allocated
            .checked_add(share)
            .ok_or(StabilityPoolError::SystemBusy)?;
        allocations.insert(*owner, share);
    }
    let mut remainder = amount
        .checked_sub(allocated)
        .ok_or(StabilityPoolError::SystemBusy)?;
    for (owner, balance) in balances {
        if remainder == 0 {
            break;
        }
        let share = allocations.get(owner).copied().unwrap_or(0);
        if share < *balance {
            allocations.insert(*owner, share + 1);
            remainder -= 1;
        }
    }
    if remainder != 0 {
        return Err(StabilityPoolError::InsufficientPoolBalance);
    }
    Ok(allocations)
}

fn three_usd_snapshot_allocations(
    row: &PendingSpThreeUsdAbsorb,
    amount: u64,
    eligible_only: bool,
) -> Result<BTreeMap<Principal, u64>, StabilityPoolError> {
    if amount == 0 {
        return Ok(BTreeMap::new());
    }
    let weights: Vec<(Principal, u64)> = row
        .depositor_snapshot
        .iter()
        .filter_map(|(owner, snapshot)| {
            let eligible = !eligible_only || snapshot.collateral_opted_in;
            (eligible && snapshot.balance > 0).then_some((*owner, snapshot.balance))
        })
        .collect();
    let total = weights
        .iter()
        .try_fold(0u64, |sum, (_, weight)| sum.checked_add(*weight))
        .ok_or(StabilityPoolError::SystemBusy)?;
    if total == 0 || amount > total {
        return Err(StabilityPoolError::InsufficientPoolBalance);
    }
    let mut allocations = BTreeMap::new();
    let mut allocated = 0u64;
    for (owner, weight) in &weights {
        let share = (amount as u128)
            .checked_mul(*weight as u128)
            .ok_or(StabilityPoolError::SystemBusy)?
            / total as u128;
        let share = u64::try_from(share).map_err(|_| StabilityPoolError::SystemBusy)?;
        allocated = allocated
            .checked_add(share)
            .ok_or(StabilityPoolError::SystemBusy)?;
        allocations.insert(*owner, share);
    }
    let mut remainder = amount
        .checked_sub(allocated)
        .ok_or(StabilityPoolError::SystemBusy)?;
    for (owner, weight) in weights.iter().rev() {
        if remainder == 0 {
            break;
        }
        let current = allocations.get(owner).copied().unwrap_or(0);
        let capacity = weight
            .checked_sub(current)
            .ok_or(StabilityPoolError::SystemBusy)?;
        let extra = capacity.min(remainder);
        if extra > 0 {
            allocations.insert(
                *owner,
                current
                    .checked_add(extra)
                    .ok_or(StabilityPoolError::SystemBusy)?,
            );
            remainder = remainder
                .checked_sub(extra)
                .ok_or(StabilityPoolError::SystemBusy)?;
        }
    }
    if remainder != 0
        || allocations
            .values()
            .try_fold(0u64, |sum, value| sum.checked_add(*value))
            != Some(amount)
    {
        return Err(StabilityPoolError::SystemBusy);
    }
    Ok(allocations)
}

fn validate_three_usd_refund(
    receipt: Option<&rumi_protocol_backend::state::ThreeUsdReserveIngressRefundReceipt>,
    expected_gross: u64,
    expected_source: Principal,
    expected_destination: Principal,
) -> Result<u64, StabilityPoolError> {
    if expected_gross == 0 {
        return if receipt.is_none() {
            Ok(0)
        } else {
            Err(StabilityPoolError::SystemBusy)
        };
    }
    let receipt = receipt.ok_or(StabilityPoolError::SystemBusy)?;
    if receipt.tuple.amount_e8s != expected_gross
        || receipt.tuple.source_owner != expected_source
        || receipt.tuple.source_subaccount.is_some()
        || receipt.tuple.destination.owner != expected_destination
        || receipt.tuple.destination.subaccount.is_some()
        || receipt.tuple.created_at_time_ns == 0
    {
        return Err(StabilityPoolError::SystemBusy);
    }
    Ok(receipt.tuple.fee_e8s)
}

/// Split an exact payout amount by positive weights. Any floor remainder is
/// assigned in principal order, so allocation sums exactly to the receipt.
fn exact_weight_allocations(
    weights: &[(Principal, u64)],
    amount: u64,
) -> Result<BTreeMap<Principal, u64>, StabilityPoolError> {
    if amount == 0 {
        return Ok(BTreeMap::new());
    }
    let total = weights.iter().try_fold(0u64, |sum, (_, weight)| {
        sum.checked_add(*weight)
            .ok_or(StabilityPoolError::SystemBusy)
    })?;
    if total == 0 {
        return Err(StabilityPoolError::InsufficientPoolBalance);
    }
    let mut allocations = BTreeMap::new();
    let mut allocated = 0u64;
    for (owner, weight) in weights.iter().filter(|(_, weight)| *weight > 0) {
        let share = ((amount as u128) * (*weight as u128) / (total as u128)) as u64;
        allocated = allocated
            .checked_add(share)
            .ok_or(StabilityPoolError::SystemBusy)?;
        allocations.insert(*owner, share);
    }
    let mut remainder = amount
        .checked_sub(allocated)
        .ok_or(StabilityPoolError::SystemBusy)?;
    for (owner, _weight) in weights.iter().filter(|(_, weight)| *weight > 0) {
        if remainder == 0 {
            break;
        }
        let share = allocations.get(owner).copied().unwrap_or(0);
        allocations.insert(
            *owner,
            share.checked_add(1).ok_or(StabilityPoolError::SystemBusy)?,
        );
        remainder -= 1;
    }
    if remainder != 0 {
        return Err(StabilityPoolError::SystemBusy);
    }
    Ok(allocations)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterestMintReceiptStatus {
    New,
    Duplicate,
    PayloadMismatch,
    PendingForward(u64),
    OutsideReplayWindow,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterestMintReceiptPayload {
    pub token_ledger: Principal,
    pub amount: u64,
    pub collateral_type: Option<Principal>,
}

pub fn known_stablecoin_transfer_fee(symbol: &str, decimals: u8) -> Option<u64> {
    match (symbol, decimals) {
        ("icUSD", 8) => Some(ICUSD_TRANSFER_FEE_E8S),
        ("ckUSDT" | "ckUSDC", 6) => Some(CK_STABLE_TRANSFER_FEE_E6),
        ("3USD", _) => Some(THREE_USD_TRANSFER_FEE),
        _ => None,
    }
}

fn normalize_known_stablecoin_transfer_fee(config: &mut StablecoinConfig) -> bool {
    let Some(known_fee) = known_stablecoin_transfer_fee(&config.symbol, config.decimals) else {
        return false;
    };
    let corrected_fee = config.transfer_fee.unwrap_or(0).max(known_fee);
    if config.transfer_fee == Some(corrected_fee) {
        return false;
    }
    config.transfer_fee = Some(corrected_fee);
    true
}

/// Maximum number of liquidation records retained in memory.
/// Older entries are dropped when this limit is exceeded.
const MAX_LIQUIDATION_HISTORY: usize = 1_000;

/// Deterministic Principal key for chain-native collateral. This is a metadata
/// key, never an ICRC ledger canister. Must match the backend discovery helper.
pub fn chain_collateral_sentinel(chain_id: u32) -> Principal {
    let mut bytes = [0u8; 29];
    let prefix = b"rumi-chain-collateral";
    bytes[..prefix.len()].copy_from_slice(prefix);
    bytes[24..28].copy_from_slice(&chain_id.to_le_bytes());
    bytes[28] = 0x7f;
    Principal::from_slice(&bytes)
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct StabilityPoolState {
    // Depositor positions
    pub deposits: BTreeMap<Principal, DepositPosition>,

    // Aggregate stablecoin balances per token
    pub total_stablecoin_balances: BTreeMap<Principal, u64>,

    // Registries
    pub stablecoin_registry: BTreeMap<Principal, StablecoinConfig>,
    pub collateral_registry: BTreeMap<Principal, CollateralInfo>,
    /// Deterministic chain-native collateral sentinel principals. `Option` keeps
    /// Candid stable memory upgrade-compatible when decoding old snapshots.
    #[serde(default)]
    pub chain_collateral_sentinels: Option<BTreeSet<Principal>>,
    /// Backend chain-liquidity claims available to pay SP depositor CFX claims,
    /// keyed by chain sentinel. `Option` keeps Candid stable memory upgrades safe.
    #[serde(default)]
    pub chain_claim_sources: Option<BTreeMap<Principal, Vec<ChainClaimSource>>>,
    /// Retry journal for chain-vault SP absorbs that may have burned icUSD but
    /// not yet finalized local depositor accounting.
    #[serde(default)]
    pub pending_chain_absorbs: Option<BTreeMap<u64, ChainSpAbsorbIntent>>,
    /// Local idempotency record for completed chain-vault SP absorbs.
    #[serde(default)]
    pub completed_chain_absorbs: Option<BTreeMap<u64, ChainSpAbsorbCompletion>>,
    /// Retry journal for native-XRP SP absorbs that may have burned icUSD but
    /// not yet finalized local depositor payout reminders.
    #[serde(default)]
    pub pending_native_xrp_absorbs: Option<BTreeMap<u64, NativeXrpAbsorbIntent>>,
    /// Disabled-by-default automatic chain absorb scheduler configuration.
    #[serde(default)]
    pub chain_absorb_auto_config: Option<ChainAbsorbAutoConfig>,
    /// Latest automatic chain absorb tick, retained as bounded operator status.
    #[serde(default)]
    pub chain_absorb_auto_last_tick: Option<ChainAbsorbAutoTickRecord>,
    /// Durable idempotency journal for backend-confirmed failed CFX claim payout
    /// recovery. Without this, retrying the backend callback would double-credit
    /// both the depositor claim and the backend claim source.
    #[serde(default)]
    pub completed_cfx_claim_payout_recoveries:
        Option<BTreeMap<CfxClaimPayoutRecoveryKey, CfxClaimPayoutRecoveryRecord>>,
    /// Compact idempotency watermark for recovery records evicted from
    /// `completed_cfx_claim_payout_recoveries`. A replay with an op_id at or
    /// below the per-sentinel floor is treated as already recovered.
    #[serde(default)]
    pub completed_cfx_claim_payout_recovery_floor: Option<BTreeMap<Principal, u64>>,

    // Canister references
    pub protocol_canister_id: Principal,

    // Admin / operational
    pub configuration: PoolConfiguration,
    pub liquidation_history: Vec<PoolLiquidationRecord>,
    pub in_flight_liquidations: BTreeSet<u64>,
    pub total_liquidations_executed: u64,
    pub pool_creation_timestamp: u64,
    /// Lifetime interest revenue received from backend (e8s).
    /// `Option` is required for Candid backward-compatible stable memory upgrades.
    #[serde(default)]
    pub total_interest_received_e8s: Option<u64>,
    /// DEPRECATED: Circuit breaker was removed — liquidations now skip failed tokens without
    /// suspending them. Field retained for upgrade compatibility (serde default).
    #[serde(default)]
    pub token_consecutive_failures: Option<BTreeMap<Principal, u32>>,
    /// Cached virtual price for LP tokens (fetched from 3pool periodically).
    /// Keyed by LP token ledger principal. Scaled by 1e18.
    #[serde(default)]
    pub cached_virtual_prices: Option<BTreeMap<Principal, u128>>,
    /// Backend canister to receive 3USD as fallback protocol reserves.
    #[serde(default)]
    pub protocol_reserve_address: Option<Principal>,
    /// Admin-configured treasury destination for interest minted to the pool
    /// while no icUSD depositor is eligible. This is distinct from the 3USD
    /// protocol reserve address above.
    #[serde(default)]
    pub interest_treasury: Option<Principal>,
    /// Durable receipts for unallocated interest forwards.  A receipt is
    /// created before any ledger await so an ambiguous response can be retried
    /// with identical ICRC-003 fields.
    #[serde(default)]
    pub unallocated_interest_forward_batches:
        Option<BTreeMap<u64, UnallocatedInterestForwardBatch>>,
    #[serde(default)]
    pub next_unallocated_interest_forward_batch_id: Option<u64>,
    /// O(log n) source mint receipt lookup for forward batches. Missing means
    /// an old/oversized snapshot could not be indexed and must fail closed.
    #[serde(default)]
    pub unallocated_interest_mint_index: Option<BTreeMap<u64, u64>>,
    pub is_initialized: bool,
    /// Event log for deposits, withdrawals, claims, interest.
    /// `Option` for backward-compatible upgrade (deserializes as None from old state).
    #[serde(default)]
    pub pool_events: Option<Vec<PoolEvent>>,
    #[serde(default)]
    pub next_event_id: Option<u64>,
    /// Failed `deposit_as_3usd` refunds awaiting recovery via
    /// `claim_pending_refund`, keyed by refund id (audit IC-S-001).
    /// `Option` is required for Candid backward-compatible stable memory upgrades.
    #[serde(default)]
    pub pending_refunds: Option<BTreeMap<u64, PendingRefund>>,
    #[serde(default)]
    pub next_pending_refund_id: Option<u64>,
    /// Monotonic ICRC-2 `created_at_time` allocator for deposits. This avoids
    /// identical same-round requests aliasing at the ledger.
    #[serde(default)]
    pub last_deposit_transfer_created_at: Option<u64>,
    /// Exact caller-scoped deposit requests retained across lost replies and
    /// upgrades so Duplicate can only complete the matching transfer once.
    #[serde(default)]
    pub pending_deposit_intents: Option<BTreeMap<Principal, PendingDepositIntent>>,
    /// Exact outbound withdrawals and collateral claims awaiting a definite
    /// ledger receipt or no-effect result. Missing on old snapshots is fail-closed.
    #[serde(default)]
    pub pending_outbound_payouts: Option<BTreeMap<(Principal, Principal), PendingOutboundPayout>>,
    /// Monotonic ICRC-1 created_at_time allocator shared by payout ledgers.
    #[serde(default)]
    pub last_outbound_payout_created_at_ns: Option<u64>,
    /// Monotonic durable identity for receipt-bound non-LP liquidations.
    #[serde(default)]
    pub next_sp_liquidation_request_id: Option<u64>,
    /// Exact V2 liquidation requests and their stable/collateral receipts.
    /// Entries remain until a terminal no-effect or completion is persisted.
    #[serde(default)]
    pub pending_sp_liquidations_v2: Option<BTreeMap<u64, PendingSpLiquidationV2>>,
    #[serde(default)]
    pub sp_liquidation_v2_recovery_cursor: Option<u64>,
    /// Bounded completed tombstones so an upgrade/lost reply cannot reapply a
    /// successful V2 liquidation under its old request ID.
    #[serde(default)]
    pub completed_sp_liquidations_v2: Option<BTreeMap<u64, PendingSpLiquidationV2>>,
    #[serde(default)]
    pub completed_sp_liquidation_request_floor: Option<u64>,
    /// Exact legacy ICRC-2 approvals awaiting a definite no-effect or a
    /// verified ICRC-3 receipt. Old snapshots decode as an empty journal.
    #[serde(default)]
    pub pending_sp_legacy_approval_fees:
        Option<BTreeMap<(u64, Principal), PendingSpLegacyApprovalFee>>,
    /// Independent durable journal for the 3USD reserve-ingress saga. Missing
    /// on old snapshots initializes empty and never adopts generic icUSD rows.
    #[serde(default)]
    pub next_sp_three_usd_absorb_id: Option<u64>,
    #[serde(default)]
    pub pending_sp_three_usd_absorbs: Option<BTreeMap<u64, PendingSpThreeUsdAbsorb>>,
    /// Round-robin cursor for autonomous recovery of 3USD rows whose approval
    /// has already been proven. Ambiguous approvals remain controller-only.
    #[serde(default)]
    pub sp_three_usd_recovery_cursor: Option<u64>,
    #[serde(default)]
    pub completed_sp_three_usd_absorbs: Option<BTreeMap<u64, PendingSpThreeUsdAbsorb>>,
    #[serde(default)]
    pub completed_sp_three_usd_absorb_floor: Option<u64>,
    /// Recent source-ledger mint blocks already allocated to eligible SP depositors.
    #[serde(default)]
    pub processed_interest_mint_blocks: Option<BTreeSet<u64>>,
    /// Exact V2 payload bound to each retained source mint block. Old snapshots
    /// lack these values and therefore fail closed on replay after upgrade.
    #[serde(default)]
    pub processed_interest_mint_payloads: Option<BTreeMap<u64, InterestMintReceiptPayload>>,
    /// Highest processed mint block used to bound replay protection memory.
    #[serde(default)]
    pub processed_interest_mint_block_high_watermark: Option<u64>,
    /// Per-ledger protocol-funded refund fee capacity, credited only by an
    /// independently verified admin transfer receipt.
    #[serde(default)]
    pub pending_refund_fee_reserves: Option<BTreeMap<Principal, u64>>,
    /// ICRC-3 funding blocks already credited; prevents receipt replay.
    #[serde(default)]
    pub pending_refund_fee_funding_blocks: Option<BTreeSet<(Principal, u64)>>,
}

impl Default for StabilityPoolState {
    fn default() -> Self {
        Self {
            deposits: BTreeMap::new(),
            total_stablecoin_balances: BTreeMap::new(),
            stablecoin_registry: BTreeMap::new(),
            collateral_registry: BTreeMap::new(),
            chain_collateral_sentinels: Some(BTreeSet::new()),
            chain_claim_sources: Some(BTreeMap::new()),
            pending_chain_absorbs: Some(BTreeMap::new()),
            completed_chain_absorbs: Some(BTreeMap::new()),
            pending_native_xrp_absorbs: Some(BTreeMap::new()),
            chain_absorb_auto_config: Some(ChainAbsorbAutoConfig::default()),
            chain_absorb_auto_last_tick: None,
            completed_cfx_claim_payout_recoveries: Some(BTreeMap::new()),
            completed_cfx_claim_payout_recovery_floor: Some(BTreeMap::new()),
            protocol_canister_id: Principal::anonymous(),
            configuration: PoolConfiguration {
                min_deposit_e8s: 1_000_000, // 0.01 USD
                max_liquidations_per_batch: 10,
                emergency_pause: false,
                authorized_admins: Vec::new(),
            },
            liquidation_history: Vec::new(),
            in_flight_liquidations: BTreeSet::new(),
            total_liquidations_executed: 0,
            pool_creation_timestamp: 0,
            total_interest_received_e8s: Some(0),
            token_consecutive_failures: Some(BTreeMap::new()),
            cached_virtual_prices: Some(BTreeMap::new()),
            protocol_reserve_address: None,
            interest_treasury: None,
            unallocated_interest_forward_batches: Some(BTreeMap::new()),
            next_unallocated_interest_forward_batch_id: Some(0),
            unallocated_interest_mint_index: Some(BTreeMap::new()),
            is_initialized: false,
            pool_events: Some(Vec::new()),
            next_event_id: Some(0),
            pending_refunds: Some(BTreeMap::new()),
            next_pending_refund_id: Some(0),
            last_deposit_transfer_created_at: None,
            pending_deposit_intents: Some(BTreeMap::new()),
            pending_outbound_payouts: Some(BTreeMap::new()),
            last_outbound_payout_created_at_ns: None,
            next_sp_liquidation_request_id: Some(1),
            pending_sp_liquidations_v2: Some(BTreeMap::new()),
            sp_liquidation_v2_recovery_cursor: None,
            completed_sp_liquidations_v2: Some(BTreeMap::new()),
            completed_sp_liquidation_request_floor: Some(1),
            pending_sp_legacy_approval_fees: Some(BTreeMap::new()),
            next_sp_three_usd_absorb_id: Some(1),
            pending_sp_three_usd_absorbs: Some(BTreeMap::new()),
            sp_three_usd_recovery_cursor: None,
            completed_sp_three_usd_absorbs: Some(BTreeMap::new()),
            completed_sp_three_usd_absorb_floor: Some(1),
            processed_interest_mint_blocks: Some(BTreeSet::new()),
            processed_interest_mint_payloads: Some(BTreeMap::new()),
            processed_interest_mint_block_high_watermark: None,
            pending_refund_fee_reserves: Some(BTreeMap::new()),
            pending_refund_fee_funding_blocks: Some(BTreeSet::new()),
        }
    }
}

/// Maximum pool events retained in memory.
const MAX_POOL_EVENTS: usize = 10_000;

pub const MAX_PENDING_CHAIN_ABSORBS: usize = 1_000;
pub const MAX_PENDING_NATIVE_XRP_ABSORBS: usize = 1_000;
pub const MAX_COMPLETED_CHAIN_ABSORBS: usize = 10_000;
pub const MAX_COMPLETED_CFX_CLAIM_PAYOUT_RECOVERIES: usize = 10_000;
pub const MAX_PENDING_OUTBOUND_PAYOUTS: usize = 10_000;
pub const MAX_PENDING_SP_LIQUIDATIONS_V2: usize = 1_000;
pub const MAX_COMPLETED_SP_LIQUIDATIONS_V2: usize = 10_000;
pub const MAX_XRP_SP_PAYOUT_ALLOCATIONS: usize = 500;

impl StabilityPoolState {
    pub fn initialize(&mut self, args: StabilityPoolInitArgs) {
        self.protocol_canister_id = args.protocol_canister_id;
        self.configuration.authorized_admins = args.authorized_admins;
        self.pool_creation_timestamp = ic_cdk::api::time();
        self.is_initialized = true;
    }

    /// Append a pool event. Trims oldest events if over capacity.
    pub fn push_event(&mut self, caller: Principal, event_type: PoolEventType) {
        self.push_event_at(caller, event_type, ic_cdk::api::time());
    }

    /// `push_event` with an explicit timestamp, for the pure state functions
    /// that already thread a `now_ns` through so they stay testable off-canister
    /// (`ic_cdk::api::time()` is unavailable outside the IC runtime).
    pub fn push_event_at(&mut self, caller: Principal, event_type: PoolEventType, now_ns: u64) {
        let id = self.next_event_id.unwrap_or(0);
        self.next_event_id = Some(id + 1);

        let events = self.pool_events.get_or_insert_with(Vec::new);
        events.push(PoolEvent {
            id,
            timestamp: now_ns,
            caller,
            event_type,
        });

        // Trim oldest events when over capacity
        if events.len() > MAX_POOL_EVENTS {
            let excess = events.len() - MAX_POOL_EVENTS;
            events.drain(..excess);
        }
    }

    pub fn pool_events(&self) -> &[PoolEvent] {
        match &self.pool_events {
            Some(v) => v,
            None => &[],
        }
    }

    pub fn pool_event_count(&self) -> u64 {
        self.pool_events
            .as_ref()
            .map(|v| v.len() as u64)
            .unwrap_or(0)
    }

    /// Queue a no-recipient interest mint for treasury forwarding.  Receipts
    /// with the same ledger/destination aggregate while no transfer has been
    /// attempted; this lets fee dust accumulate instead of becoming stranded.
    /// A repeated backend notification returns its original batch id.
    pub fn queue_unallocated_interest_forward(
        &mut self,
        source_mint_block: u64,
        token_ledger: Principal,
        amount: u64,
    ) -> Result<u64, StabilityPoolError> {
        self.queue_unallocated_interest_forward_at(
            source_mint_block,
            token_ledger,
            amount,
            ic_cdk::api::time(),
        )
    }

    pub fn queue_unallocated_interest_forward_at(
        &mut self,
        source_mint_block: u64,
        token_ledger: Principal,
        amount: u64,
        now: u64,
    ) -> Result<u64, StabilityPoolError> {
        let index = self
            .unallocated_interest_mint_index
            .as_ref()
            .ok_or(StabilityPoolError::SystemBusy)?;
        if let Some(existing) = index.get(&source_mint_block) {
            return Ok(*existing);
        }
        if index.len() >= MAX_UNALLOCATED_INTEREST_MINT_RECEIPTS {
            return Err(StabilityPoolError::SystemBusy);
        }
        let treasury = self.interest_treasury;
        let batches = self
            .unallocated_interest_forward_batches
            .get_or_insert_with(BTreeMap::new);
        // The lifetime receipt index caps historical source IDs at the same
        // bound, so this coalescing search can never walk unbounded history.
        if batches.len() > MAX_UNALLOCATED_INTEREST_MINT_RECEIPTS {
            return Err(StabilityPoolError::SystemBusy);
        }
        if let Some((id, batch)) = batches
            .iter_mut()
            .take(MAX_UNALLOCATED_INTEREST_MINT_RECEIPTS)
            .find(|(_, batch)| {
                batch.token_ledger == token_ledger
                && batch.treasury == treasury
                && batch.transfer_block_index.is_none()
                // A fee-bearing batch that has not crossed the fee threshold
                // has made no ledger call; keep accumulating that dust.
                && (batch.fee.is_none() || batch.gross_amount <= batch.fee.unwrap_or(0))
                && batch.source_mint_blocks.len() < 1_000
            })
        {
            let new_gross_amount = batch
                .gross_amount
                .checked_add(amount)
                .ok_or(StabilityPoolError::SystemBusy)?;
            batch.source_mint_blocks.push(source_mint_block);
            batch.gross_amount = new_gross_amount;
            self.unallocated_interest_mint_index
                .as_mut()
                .expect("checked receipt index above")
                .insert(source_mint_block, *id);
            return Ok(*id);
        }
        let id = self.next_unallocated_interest_forward_batch_id.unwrap_or(0);
        self.next_unallocated_interest_forward_batch_id =
            Some(id.checked_add(1).ok_or(StabilityPoolError::SystemBusy)?);
        batches.insert(
            id,
            UnallocatedInterestForwardBatch {
                id,
                source_mint_blocks: vec![source_mint_block],
                token_ledger,
                treasury,
                gross_amount: amount,
                fee: None,
                transfer_created_at_ns: None,
                transfer_block_index: None,
                treasury_recorded: false,
                created_at_ns: now,
                last_error: None,
            },
        );
        self.unallocated_interest_mint_index
            .as_mut()
            .expect("checked receipt index above")
            .insert(source_mint_block, id);
        Ok(id)
    }

    pub fn queue_interest_forward_with_receipt(
        &mut self,
        source_mint_block: u64,
        payload: InterestMintReceiptPayload,
    ) -> Result<u64, StabilityPoolError> {
        self.queue_interest_forward_with_receipt_at(source_mint_block, payload, ic_cdk::api::time())
    }

    fn queue_interest_forward_with_receipt_at(
        &mut self,
        source_mint_block: u64,
        payload: InterestMintReceiptPayload,
        now: u64,
    ) -> Result<u64, StabilityPoolError> {
        if self.interest_mint_receipt_status(source_mint_block, &payload)
            != InterestMintReceiptStatus::New
        {
            return Err(StabilityPoolError::SystemBusy);
        }
        let batch_id = self.queue_unallocated_interest_forward_at(
            source_mint_block,
            payload.token_ledger,
            payload.amount,
            now,
        )?;
        // These synchronous state updates occur in the same message as queue
        // creation, before any ledger await can happen.
        self.insert_interest_mint_receipt(source_mint_block, payload);
        Ok(batch_id)
    }

    /// Rebuilds the new source-receipt index once after upgrading a snapshot
    /// written before the index existed. Work is bounded by the maximum index
    /// capacity; oversized history disables new receipt acceptance safely.
    pub fn initialize_unallocated_interest_mint_index(&mut self) {
        if self.unallocated_interest_mint_index.is_some() {
            return;
        }
        let mut index = BTreeMap::new();
        if let Some(batches) = &self.unallocated_interest_forward_batches {
            if batches.len() > MAX_UNALLOCATED_INTEREST_MINT_RECEIPTS {
                self.unallocated_interest_mint_index = None;
                return;
            }
            for (batch_id, batch) in batches {
                for source_mint_block in &batch.source_mint_blocks {
                    index.insert(*source_mint_block, *batch_id);
                    if index.len() > MAX_UNALLOCATED_INTEREST_MINT_RECEIPTS {
                        self.unallocated_interest_mint_index = None;
                        return;
                    }
                }
            }
        }
        self.unallocated_interest_mint_index = Some(index);
    }

    pub fn unallocated_interest_forward_batch(
        &self,
        id: u64,
    ) -> Option<UnallocatedInterestForwardBatch> {
        self.unallocated_interest_forward_batches
            .as_ref()
            .and_then(|batches| batches.get(&id).cloned())
    }

    pub fn pending_unallocated_interest_forwards(&self) -> Vec<UnallocatedInterestForwardBatch> {
        self.unallocated_interest_forward_batches
            .as_ref()
            .map(|batches| {
                batches
                    .values()
                    .filter(|batch| !batch.treasury_recorded)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn prepare_unallocated_interest_forward(
        &mut self,
        id: u64,
        fee: u64,
    ) -> Result<UnallocatedInterestForwardBatch, StabilityPoolError> {
        self.prepare_unallocated_interest_forward_at(id, fee, ic_cdk::api::time())
    }

    pub fn prepare_unallocated_interest_forward_at(
        &mut self,
        id: u64,
        fee: u64,
        now: u64,
    ) -> Result<UnallocatedInterestForwardBatch, StabilityPoolError> {
        let batch = self
            .unallocated_interest_forward_batches
            .as_mut()
            .and_then(|batches| batches.get_mut(&id))
            .ok_or(StabilityPoolError::RefundClaimNotFound)?;
        if batch.transfer_block_index.is_none() && batch.fee.is_none() {
            batch.fee = Some(fee);
            batch.transfer_created_at_ns = Some(now);
        }
        Ok(batch.clone())
    }

    pub fn update_unallocated_interest_forward_fee(&mut self, id: u64, fee: u64) {
        if let Some(batch) = self
            .unallocated_interest_forward_batches
            .as_mut()
            .and_then(|batches| batches.get_mut(&id))
        {
            if batch.transfer_block_index.is_none() {
                batch.fee = Some(fee);
                batch.last_error = None;
            }
        }
    }

    pub fn mark_unallocated_interest_forward_transferred(&mut self, id: u64, block_index: u64) {
        if let Some(batch) = self
            .unallocated_interest_forward_batches
            .as_mut()
            .and_then(|batches| batches.get_mut(&id))
        {
            batch.transfer_block_index = Some(block_index);
            batch.last_error = None;
        }
    }

    pub fn mark_unallocated_interest_forward_recorded(&mut self, id: u64) {
        if let Some(batch) = self
            .unallocated_interest_forward_batches
            .as_mut()
            .and_then(|batches| batches.get_mut(&id))
        {
            batch.treasury_recorded = true;
            batch.last_error = None;
        }
    }

    pub fn record_unallocated_interest_forward_error(&mut self, id: u64, error: String) {
        if let Some(batch) = self
            .unallocated_interest_forward_batches
            .as_mut()
            .and_then(|batches| batches.get_mut(&id))
        {
            batch.last_error = Some(error);
        }
    }

    /// Assign a treasury only to receipts that have not begun a transfer. A
    /// destination change after a receipt is on the ledger is forbidden.
    pub fn set_interest_treasury(
        &mut self,
        treasury: Option<Principal>,
    ) -> Result<(), StabilityPoolError> {
        let has_started = self
            .unallocated_interest_forward_batches
            .as_ref()
            .map(|batches| {
                batches.values().any(|batch| {
                    !batch.treasury_recorded
                        && batch.treasury.is_some()
                        && batch.treasury != treasury
                })
            })
            .unwrap_or(false);
        if has_started {
            return Err(StabilityPoolError::LedgerTransferFailed {
                reason: "cannot change interest treasury while a forward is pending".to_string(),
            });
        }
        self.interest_treasury = treasury;
        if let Some(treasury) = treasury {
            if let Some(batches) = self.unallocated_interest_forward_batches.as_mut() {
                for batch in batches.values_mut() {
                    if !batch.treasury_recorded && batch.transfer_block_index.is_none() {
                        batch.treasury = Some(treasury);
                    }
                }
            }
        }
        Ok(())
    }

    pub fn is_admin(&self, caller: &Principal) -> bool {
        self.configuration.authorized_admins.contains(caller)
    }

    // ─── Stablecoin Registry ───

    pub fn register_stablecoin(&mut self, config: StablecoinConfig) {
        let mut config = config;
        normalize_known_stablecoin_transfer_fee(&mut config);
        self.total_stablecoin_balances
            .entry(config.ledger_id)
            .or_insert(0);
        self.stablecoin_registry.insert(config.ledger_id, config);
    }

    pub fn normalize_registered_stablecoin_transfer_fees(&mut self) -> usize {
        let mut corrected = 0;
        for config in self.stablecoin_registry.values_mut() {
            if normalize_known_stablecoin_transfer_fee(config) {
                corrected += 1;
            }
        }
        corrected
    }

    pub fn get_stablecoin_config(&self, ledger: &Principal) -> Option<&StablecoinConfig> {
        self.stablecoin_registry.get(ledger)
    }

    pub fn is_accepted_stablecoin(&self, ledger: &Principal) -> bool {
        self.stablecoin_registry
            .get(ledger)
            .map(|c| c.is_active)
            .unwrap_or(false)
    }

    /// Get cached virtual prices (empty if None for upgrade compat).
    pub fn virtual_prices(&self) -> &BTreeMap<Principal, u128> {
        static EMPTY: std::sync::LazyLock<BTreeMap<Principal, u128>> =
            std::sync::LazyLock::new(BTreeMap::new);
        self.cached_virtual_prices.as_ref().unwrap_or(&EMPTY)
    }

    // ─── Collateral Registry ───

    pub fn register_collateral(&mut self, info: CollateralInfo) {
        self.collateral_registry.insert(info.ledger_id, info);
    }

    pub fn register_chain_collateral_sentinel(&mut self, sentinel: Principal) {
        self.chain_collateral_sentinels
            .get_or_insert_with(BTreeSet::new)
            .insert(sentinel);
    }

    pub fn register_chain_collateral(
        &mut self,
        chain_id: u32,
        symbol: String,
        decimals: u8,
    ) -> Result<Principal, StabilityPoolError> {
        let sentinel = chain_collateral_sentinel(chain_id);
        self.register_collateral(CollateralInfo {
            ledger_id: sentinel,
            symbol,
            decimals,
            status: CollateralStatus::Active,
        });
        self.register_chain_collateral_sentinel(sentinel);
        Ok(sentinel)
    }

    pub fn is_chain_collateral_sentinel(&self, collateral_type: &Principal) -> bool {
        self.chain_collateral_sentinels
            .as_ref()
            .map(|s| s.contains(collateral_type))
            .unwrap_or(false)
    }

    /// Native/off-IC collateral cannot be paid out by an ICRC ledger transfer.
    /// Today XRP is the only such collateral in the SP registry. It is opt-in by
    /// stored payout address rather than opt-out by default.
    pub fn collateral_requires_payout_address(&self, collateral_type: &Principal) -> bool {
        // Gate strictly on the native-XRP synthetic principal identity. XRP is
        // registered in the SP under `xrp_collateral_principal()`, so this is
        // exact. A symbol heuristic ("XRP") would misclassify any future
        // chain-registered collateral that happens to carry the XRP symbol
        // (e.g. bridged XRP on an EVM sidechain): it would route a CFX-style
        // sentinel opt-in through the payout-address branch and silently break
        // that depositor's absorption. Identity-only avoids that hazard.
        *collateral_type == rumi_protocol_backend::state::xrp_collateral_principal()
    }

    pub fn native_payout_address(
        &self,
        user: &Principal,
        collateral_type: &Principal,
    ) -> Option<String> {
        self.deposits
            .get(user)
            .and_then(|pos| pos.native_payout_addresses.as_ref())
            .and_then(|addresses| addresses.get(collateral_type).cloned())
    }

    pub fn native_payout_destination_tag(
        &self,
        user: &Principal,
        collateral_type: &Principal,
    ) -> Option<u32> {
        self.deposits
            .get(user)
            .and_then(|pos| pos.native_payout_destination_tags.as_ref())
            .and_then(|tags| tags.get(collateral_type).copied())
    }

    pub fn ensure_icrc_claimable_collateral(
        &self,
        collateral_type: &Principal,
    ) -> Result<(), StabilityPoolError> {
        if self.collateral_requires_payout_address(collateral_type) {
            return Err(StabilityPoolError::PayoutAddressRequired {
                collateral: *collateral_type,
            });
        }
        Ok(())
    }

    pub fn get_claimable_icrc_collateral_gains(
        &self,
        user: &Principal,
    ) -> BTreeMap<Principal, u64> {
        self.get_collateral_gains(user)
            .into_iter()
            .filter(|(collateral, amount)| {
                *amount > 0 && self.ensure_icrc_claimable_collateral(collateral).is_ok()
            })
            .collect()
    }

    /// Unified opt-in check across all collateral models:
    /// - XRP-style native collateral opts in by storing a payout address.
    /// - CFX-style chain collateral opts in via the sentinel set.
    /// - Normal ICP collateral is opt-out by default.
    fn position_opted_in_for(&self, pos: &DepositPosition, collateral_type: &Principal) -> bool {
        if self.collateral_requires_payout_address(collateral_type) {
            return pos
                .native_payout_addresses
                .as_ref()
                .map(|addresses| addresses.contains_key(collateral_type))
                .unwrap_or(false);
        }
        if self.is_chain_collateral_sentinel(collateral_type) {
            return pos.is_opted_in_for_chain(collateral_type);
        }
        pos.is_opted_in(collateral_type)
    }

    // ─── Deposits ───

    /// Reserve a globally unique ICRC-2 transfer timestamp across every
    /// Stability Pool pull workflow. Separate deposit entry points must share
    /// this allocator so identical calls in one IC time round cannot alias at
    /// the ledger.
    pub fn reserve_deposit_transfer_timestamp(&mut self, now_ns: u64) -> Result<u64, ()> {
        let timestamp = match self.last_deposit_transfer_created_at {
            Some(last) if now_ns <= last => last.checked_add(1).ok_or(())?,
            _ => now_ns,
        };
        self.last_deposit_transfer_created_at = Some(timestamp);
        Ok(timestamp)
    }

    /// Begin or replay a caller's exact pending ICRC-2 deposit. Distinct terms
    /// cannot replace an unresolved pull. The timestamp remains unique even
    /// when multiple calls observe the same IC time round.
    pub fn begin_deposit_intent(
        &mut self,
        caller: Principal,
        token_ledger: Principal,
        amount: u64,
        now_ns: u64,
    ) -> Result<u64, ()> {
        if let Some(intent) = self
            .pending_deposit_intents
            .as_mut()
            .and_then(|intents| intents.get_mut(&caller))
        {
            if intent.token_ledger != token_ledger || intent.amount != amount {
                return Err(());
            }
            if intent.too_old_rejected == Some(true) {
                return Err(());
            }
            match intent.in_flight_attempts {
                Some(attempts) => {
                    intent.in_flight_attempts = Some(attempts.checked_add(1).ok_or(())?);
                }
                // An old snapshot cannot distinguish an in-flight call from a
                // lost reply. Keep that uncertainty permanently fail-closed.
                None => {
                    intent.ambiguous_seen = true;
                    intent.in_flight_attempts = Some(1);
                }
            }
            return Ok(intent.transfer_created_at_time_ns);
        }

        let timestamp = self.reserve_deposit_transfer_timestamp(now_ns)?;
        self.pending_deposit_intents
            .get_or_insert_with(BTreeMap::new)
            .insert(
                caller,
                PendingDepositIntent {
                    token_ledger,
                    amount,
                    transfer_created_at_time_ns: timestamp,
                    transfer_block_index: None,
                    in_flight_attempts: Some(1),
                    ambiguous_seen: false,
                    too_old_rejected: None,
                    history_scan_cursor: None,
                    history_scan_tip: None,
                    reconciliation_in_progress: Some(false),
                    reconciliation_generation: Some(0),
                    reconciliation_started_at_ns: None,
                    reconciliation_next_allowed_at_ns: None,
                    attempt_no: Some(0),
                },
            );
        Ok(timestamp)
    }

    pub fn deposit_intent_matches(
        &self,
        caller: Principal,
        token_ledger: Principal,
        amount: u64,
        timestamp: u64,
    ) -> bool {
        self.pending_deposit_intents
            .as_ref()
            .and_then(|intents| intents.get(&caller))
            .is_some_and(|intent| {
                intent.token_ledger == token_ledger
                    && intent.amount == amount
                    && intent.transfer_created_at_time_ns == timestamp
            })
    }

    /// Credit the exact pull once. False means another callback already
    /// completed it or the supplied transfer identity is stale.
    pub fn complete_deposit_intent(
        &mut self,
        caller: Principal,
        token_ledger: Principal,
        amount: u64,
        timestamp: u64,
        now_ns: u64,
    ) -> bool {
        if !self.deposit_intent_matches(caller, token_ledger, amount, timestamp)
            || self
                .pending_deposit_intents
                .as_ref()
                .and_then(|intents| intents.get(&caller))
                .and_then(|intent| intent.transfer_block_index)
                .is_none()
        {
            return false;
        }
        self.add_deposit_at(caller, token_ledger, amount, now_ns);
        self.pending_deposit_intents
            .as_mut()
            .expect("pending deposit map initialized")
            .remove(&caller);
        self.push_event_at(
            caller,
            PoolEventType::Deposit {
                token_ledger,
                amount,
            },
            now_ns,
        );
        true
    }

    pub fn record_deposit_receipt(
        &mut self,
        caller: Principal,
        token_ledger: Principal,
        amount: u64,
        timestamp: u64,
        block_index: u64,
    ) -> bool {
        let Some(intent) = self
            .pending_deposit_intents
            .as_mut()
            .and_then(|intents| intents.get_mut(&caller))
        else {
            return false;
        };
        if intent.token_ledger != token_ledger
            || intent.amount != amount
            || intent.transfer_created_at_time_ns != timestamp
            || intent
                .transfer_block_index
                .is_some_and(|existing| existing != block_index)
        {
            return false;
        }
        if let Some(attempts) = intent.in_flight_attempts.as_mut() {
            if *attempts > 0 {
                *attempts -= 1;
            }
        } else {
            // Receipt identity proves the transfer, but absent attempt metadata
            // still reflects a pre-migration unresolved call.
            intent.ambiguous_seen = true;
        }
        intent.transfer_block_index = Some(block_index);
        true
    }

    /// Commit an authenticated ICRC-3 receipt and credit the exact intent in
    /// one state mutation. Positive proof is sufficient even while a callback
    /// remains in flight: removing the row fences any late callback. Unlike a
    /// ledger callback this does not decrement the in-flight counter.
    pub fn complete_reconciled_deposit(
        &mut self,
        caller: Principal,
        token_ledger: Principal,
        amount: u64,
        timestamp: u64,
        block_index: u64,
        now_ns: u64,
        reconciliation_generation: u64,
    ) -> bool {
        let Some(intent) = self
            .pending_deposit_intents
            .as_mut()
            .and_then(|intents| intents.get(&caller))
        else {
            return false;
        };
        if intent.token_ledger != token_ledger
            || intent.amount != amount
            || intent.transfer_created_at_time_ns != timestamp
            || intent.reconciliation_in_progress != Some(true)
            || intent.reconciliation_generation != Some(reconciliation_generation)
            || intent
                .transfer_block_index
                .is_some_and(|existing| existing != block_index)
        {
            return false;
        }
        self.add_deposit_at(caller, token_ledger, amount, now_ns);
        self.pending_deposit_intents
            .as_mut()
            .expect("pending deposit map initialized")
            .remove(&caller);
        self.push_event_at(
            caller,
            PoolEventType::Deposit {
                token_ledger,
                amount,
            },
            now_ns,
        );
        true
    }

    /// Finish one dispatch with a definitive no-effect reply. Clear the intent
    /// only after every concurrent dispatch has also returned no-effect and no
    /// prior dispatch had an ambiguous outcome.
    pub fn clear_deposit_intent_after_no_effect(
        &mut self,
        caller: Principal,
        token_ledger: Principal,
        amount: u64,
        timestamp: u64,
    ) -> bool {
        if !self.deposit_intent_matches(caller, token_ledger, amount, timestamp) {
            return false;
        }
        let Some(intent) = self
            .pending_deposit_intents
            .as_mut()
            .and_then(|intents| intents.get_mut(&caller))
        else {
            return false;
        };
        match intent.in_flight_attempts.as_mut() {
            Some(attempts) if *attempts > 0 => *attempts -= 1,
            _ => {
                // A callback without a matching tracked dispatch cannot prove
                // that every earlier transfer attempt had no effect.
                intent.ambiguous_seen = true;
                return false;
            }
        }
        if intent.ambiguous_seen
            || intent.transfer_block_index.is_some()
            || intent.in_flight_attempts != Some(0)
        {
            return false;
        }
        self.pending_deposit_intents
            .as_mut()
            .expect("pending deposit map initialized")
            .remove(&caller);
        true
    }

    /// Record a transport or otherwise ambiguous reply, finishing exactly one
    /// dispatch while retaining the intent for an exact-identity retry.
    pub fn mark_deposit_intent_ambiguous(
        &mut self,
        caller: Principal,
        token_ledger: Principal,
        amount: u64,
        timestamp: u64,
    ) {
        let Some(intent) = self
            .pending_deposit_intents
            .as_mut()
            .and_then(|intents| intents.get_mut(&caller))
        else {
            return;
        };
        if intent.token_ledger != token_ledger
            || intent.amount != amount
            || intent.transfer_created_at_time_ns != timestamp
        {
            return;
        }
        intent.ambiguous_seen = true;
        if let Some(attempts) = intent.in_flight_attempts.as_mut() {
            if *attempts > 0 {
                *attempts -= 1;
            }
        }
    }

    /// Mark the exact persisted deposit identity after a typed ledger TooOld
    /// result. TooOld rejects this dispatch, but does not establish whether an
    /// earlier ambiguous dispatch committed, so identity rotation still
    /// requires a complete history scan.
    pub fn mark_deposit_intent_too_old(
        &mut self,
        caller: Principal,
        token_ledger: Principal,
        amount: u64,
        timestamp: u64,
    ) -> bool {
        let Some(intent) = self
            .pending_deposit_intents
            .as_mut()
            .and_then(|intents| intents.get_mut(&caller))
        else {
            return false;
        };
        if intent.token_ledger != token_ledger
            || intent.amount != amount
            || intent.transfer_created_at_time_ns != timestamp
            || intent.transfer_block_index.is_some()
        {
            return false;
        }
        intent.ambiguous_seen = true;
        intent.too_old_rejected = Some(true);
        intent.history_scan_cursor = None;
        intent.history_scan_tip = None;
        if let Some(attempts) = intent.in_flight_attempts.as_mut() {
            if *attempts > 0 {
                *attempts -= 1;
            }
        }
        true
    }

    pub fn start_deposit_history_scan(
        &mut self,
        caller: Principal,
        log_length: u64,
        reconciliation_generation: u64,
    ) -> Result<PendingDepositIntent, &'static str> {
        let intent = self
            .pending_deposit_intents
            .as_mut()
            .and_then(|intents| intents.get_mut(&caller))
            .ok_or("pending deposit intent not found")?;
        if intent.too_old_rejected != Some(true) {
            return Err("history scan requires a typed TooOld response for this deposit");
        }
        if intent.reconciliation_in_progress != Some(true)
            || intent.reconciliation_generation != Some(reconciliation_generation)
        {
            return Err("deposit history scan lease is not held");
        }
        if intent.in_flight_attempts != Some(0) || intent.transfer_block_index.is_some() {
            return Err("deposit dispatches or a saved receipt remain unresolved");
        }
        match (intent.history_scan_cursor, intent.history_scan_tip) {
            (None, None) => {
                intent.history_scan_cursor = Some(0);
                intent.history_scan_tip = Some(log_length);
            }
            (Some(_), Some(tip)) if tip == log_length => {}
            (Some(_), Some(_)) => return Err("deposit history scan tip changed"),
            _ => return Err("deposit history scan journal is incomplete"),
        }
        Ok(intent.clone())
    }

    /// Reserve one scan page before any inter-canister await so concurrent
    /// requests cannot duplicate archive fetch work for the same cursor.
    pub fn claim_deposit_history_scan(
        &mut self,
        caller: Principal,
        now_ns: u64,
    ) -> Result<PendingDepositIntent, &'static str> {
        let intent = self
            .pending_deposit_intents
            .as_mut()
            .and_then(|intents| intents.get_mut(&caller))
            .ok_or("pending deposit intent not found")?;
        if intent.too_old_rejected != Some(true) {
            return Err("history scan requires a typed TooOld response for this deposit");
        }
        if intent.in_flight_attempts != Some(0) || intent.transfer_block_index.is_some() {
            return Err("deposit dispatches or a saved receipt remain unresolved");
        }
        if intent.reconciliation_in_progress == Some(true)
            && intent
                .reconciliation_started_at_ns
                .is_some_and(|started| now_ns.saturating_sub(started) < 900_000_000_000)
        {
            return Err("deposit history scan already in progress");
        }
        if intent
            .reconciliation_next_allowed_at_ns
            .is_some_and(|next| now_ns < next)
        {
            return Err("deposit proof reconciliation cooldown is active");
        }
        intent.reconciliation_generation = Some(
            intent
                .reconciliation_generation
                .unwrap_or(0)
                .checked_add(1)
                .ok_or("deposit reconciliation generation exhausted")?,
        );
        intent.reconciliation_in_progress = Some(true);
        intent.reconciliation_started_at_ns = Some(now_ns);
        Ok(intent.clone())
    }

    /// Claim exact positive-proof reconciliation, including while an original
    /// transfer callback remains in flight. The shared per-intent lease also
    /// bounds concurrent arbitrary block-index probes to one archive fetch.
    pub fn claim_pending_deposit_reconciliation(
        &mut self,
        caller: Principal,
        now_ns: u64,
    ) -> Result<PendingDepositIntent, &'static str> {
        let intent = self
            .pending_deposit_intents
            .as_mut()
            .and_then(|intents| intents.get_mut(&caller))
            .ok_or("pending deposit intent not found")?;
        if intent.reconciliation_in_progress == Some(true)
            && intent
                .reconciliation_started_at_ns
                .is_some_and(|started| now_ns.saturating_sub(started) < 900_000_000_000)
        {
            return Err("pending deposit proof reconciliation already in progress");
        }
        if intent
            .reconciliation_next_allowed_at_ns
            .is_some_and(|next| now_ns < next)
        {
            return Err("deposit proof reconciliation cooldown is active");
        }
        intent.reconciliation_generation = Some(
            intent
                .reconciliation_generation
                .unwrap_or(0)
                .checked_add(1)
                .ok_or("deposit reconciliation generation exhausted")?,
        );
        intent.reconciliation_in_progress = Some(true);
        intent.reconciliation_started_at_ns = Some(now_ns);
        Ok(intent.clone())
    }

    pub fn release_deposit_history_scan(
        &mut self,
        caller: Principal,
        reconciliation_generation: u64,
        now_ns: u64,
    ) {
        if let Some(intent) = self
            .pending_deposit_intents
            .as_mut()
            .and_then(|intents| intents.get_mut(&caller))
        {
            if intent.reconciliation_generation != Some(reconciliation_generation)
                || intent.reconciliation_in_progress != Some(true)
            {
                return;
            }
            intent.reconciliation_in_progress = Some(false);
            intent.reconciliation_started_at_ns = None;
            intent.reconciliation_next_allowed_at_ns = Some(now_ns.saturating_add(5_000_000_000));
        }
    }

    pub fn advance_deposit_history_scan(
        &mut self,
        caller: Principal,
        expected_cursor: u64,
        fixed_tip: u64,
        next_cursor: u64,
        now_ns: u64,
        reconciliation_generation: u64,
    ) -> Result<PendingDepositIntent, &'static str> {
        let intent = self
            .pending_deposit_intents
            .as_mut()
            .and_then(|intents| intents.get_mut(&caller))
            .ok_or("pending deposit intent not found")?;
        if intent.too_old_rejected != Some(true)
            || intent.in_flight_attempts != Some(0)
            || intent.history_scan_cursor != Some(expected_cursor)
            || intent.history_scan_tip != Some(fixed_tip)
            || intent.reconciliation_in_progress != Some(true)
            || intent.reconciliation_generation != Some(reconciliation_generation)
            || next_cursor < expected_cursor
            || next_cursor > fixed_tip
        {
            return Err("deposit history scan state changed or range is invalid");
        }
        intent.history_scan_cursor = Some(next_cursor);
        intent.reconciliation_in_progress = Some(false);
        intent.reconciliation_started_at_ns = None;
        intent.reconciliation_next_allowed_at_ns = Some(now_ns.saturating_add(5_000_000_000));
        Ok(intent.clone())
    }

    /// Rotate an identity only after a typed TooOld response and a complete
    /// fixed-tip archive-aware scan prove there is no matching transfer block.
    pub fn rotate_deposit_after_no_effect(
        &mut self,
        caller: Principal,
        now_ns: u64,
    ) -> Result<PendingDepositIntent, &'static str> {
        let current = self
            .pending_deposit_intents
            .as_ref()
            .and_then(|intents| intents.get(&caller))
            .ok_or("pending deposit intent not found")?;
        if current.too_old_rejected != Some(true)
            || current.in_flight_attempts != Some(0)
            || current.transfer_block_index.is_some()
            || current.reconciliation_in_progress == Some(true)
            || current.history_scan_cursor != current.history_scan_tip
            || current.history_scan_tip.is_none()
        {
            return Err("fresh deposit identity requires a complete no-effect history proof");
        }
        let next_attempt = current
            .attempt_no
            .unwrap_or(0)
            .checked_add(1)
            .ok_or("pending deposit attempt counter exhausted")?;
        let timestamp = self
            .reserve_deposit_transfer_timestamp(now_ns)
            .map_err(|_| "deposit timestamp allocator exhausted")?;
        let intent = self
            .pending_deposit_intents
            .as_mut()
            .and_then(|intents| intents.get_mut(&caller))
            .ok_or("pending deposit intent changed")?;
        intent.transfer_created_at_time_ns = timestamp;
        intent.transfer_block_index = None;
        intent.in_flight_attempts = Some(0);
        intent.ambiguous_seen = false;
        intent.too_old_rejected = None;
        intent.history_scan_cursor = None;
        intent.history_scan_tip = None;
        intent.reconciliation_in_progress = Some(false);
        intent.reconciliation_started_at_ns = None;
        intent.reconciliation_next_allowed_at_ns = None;
        intent.attempt_no = Some(next_attempt);
        Ok(intent.clone())
    }

    /// Resolve dispatches interrupted by upgrade. Their ledger outcomes are
    /// unknown even if the old snapshot did not record an ambiguity flag.
    pub fn reconcile_pending_deposit_attempts_after_upgrade(&mut self) {
        let Some(intents) = self.pending_deposit_intents.as_mut() else {
            return;
        };
        for intent in intents.values_mut() {
            intent.reconciliation_in_progress = Some(false);
            intent.reconciliation_generation = Some(
                intent
                    .reconciliation_generation
                    .unwrap_or(0)
                    .saturating_add(1),
            );
            intent.reconciliation_started_at_ns = None;
            match intent.in_flight_attempts {
                Some(0) => {}
                Some(_) | None => {
                    intent.in_flight_attempts = Some(0);
                    intent.ambiguous_seen = true;
                }
            }
        }
    }

    /// An upgrade may interrupt any outbound ledger await after dispatch. Keep
    /// its position debit and exact tuple fenced for Duplicate reconciliation.
    pub fn reconcile_pending_outbound_payouts_after_upgrade(&mut self) {
        let Some(payouts) = self.pending_outbound_payouts.as_mut() else {
            return;
        };
        for payout in payouts.values_mut() {
            if payout.dispatch_in_flight {
                payout.dispatch_in_flight = false;
                payout.ambiguous_seen = true;
                payout.last_error = Some("dispatch interrupted by upgrade; outcome unknown".into());
            }
        }
    }

    pub fn pending_outbound_payout(
        &self,
        caller: &Principal,
        ledger: &Principal,
    ) -> Option<PendingOutboundPayout> {
        self.pending_outbound_payouts
            .as_ref()?
            .get(&(*caller, *ledger))
            .cloned()
    }

    pub fn pending_collateral_payouts_for(&self, caller: &Principal) -> Vec<(Principal, u64)> {
        self.pending_outbound_payouts
            .as_ref()
            .into_iter()
            .flat_map(|payouts| {
                payouts.iter().filter_map(move |((owner, ledger), payout)| {
                    (*owner == *caller && payout.kind == OutboundPayoutKind::CollateralClaim)
                        .then_some((*ledger, payout.transfer_amount))
                })
            })
            .collect()
    }

    pub fn pending_outbound_payouts_for(
        &self,
        caller: &Principal,
    ) -> Vec<PendingOutboundPayoutStatus> {
        self.pending_outbound_payouts
            .as_ref()
            .into_iter()
            .flat_map(|payouts| {
                payouts.iter().filter_map(move |((owner, ledger), payout)| {
                    (*owner == *caller).then(|| PendingOutboundPayoutStatus {
                        ledger: *ledger,
                        kind: payout.kind,
                        request_amount: payout.request_amount,
                        gross_amount: payout.gross_amount,
                        transfer_amount: payout.transfer_amount,
                        transfer_fee: payout.transfer_fee,
                        transfer_created_at_time_ns: payout.transfer_created_at_time_ns,
                        transfer_memo: payout.transfer_memo.clone(),
                        dispatch_in_flight: payout.dispatch_in_flight,
                        ambiguous_seen: payout.ambiguous_seen,
                        last_error: payout.last_error.clone(),
                    })
                })
            })
            .collect()
    }

    pub fn has_pending_outbound_withdrawals(&self) -> bool {
        self.pending_outbound_payouts
            .as_ref()
            .is_some_and(|payouts| {
                payouts
                    .values()
                    .any(|p| p.kind == OutboundPayoutKind::Withdraw)
            })
    }

    /// Old snapshots predate this journal, so initialize the absent field once
    /// during upgrade. A present map is never replaced or cleared.
    pub fn initialize_pending_outbound_payouts(&mut self) {
        self.pending_outbound_payouts
            .get_or_insert_with(BTreeMap::new);
    }

    pub fn initialize_sp_liquidation_v2_journal(&mut self) {
        self.next_sp_liquidation_request_id.get_or_insert(1);
        self.pending_sp_liquidations_v2
            .get_or_insert_with(BTreeMap::new);
        self.completed_sp_liquidations_v2
            .get_or_insert_with(BTreeMap::new);
        self.completed_sp_liquidation_request_floor.get_or_insert(1);
    }

    pub fn initialize_sp_three_usd_absorb_journal(&mut self) {
        self.next_sp_three_usd_absorb_id.get_or_insert(1);
        self.pending_sp_three_usd_absorbs
            .get_or_insert_with(BTreeMap::new);
        self.completed_sp_three_usd_absorbs
            .get_or_insert_with(BTreeMap::new);
        self.completed_sp_three_usd_absorb_floor.get_or_insert(1);
    }

    pub fn prepare_sp_three_usd_absorb(
        &mut self,
        vault_id: u64,
        stability_pool: Principal,
        ledger: Principal,
        collateral_type: Principal,
        collateral_price_e8s: u64,
        started_at_ns: u64,
        debt_covered_e8s: u64,
        three_usd_amount: u64,
        virtual_price_e18: u128,
        approval: SpThreeUsdApprovalIntent,
    ) -> Result<PendingSpThreeUsdAbsorb, StabilityPoolError> {
        self.ensure_stablecoin_aggregate_matches_positions(ledger)?;
        let config = self
            .stablecoin_registry
            .get(&ledger)
            .ok_or(StabilityPoolError::SystemBusy)?;
        let covered_from_lp = (three_usd_amount as u128)
            .checked_mul(virtual_price_e18)
            .ok_or(StabilityPoolError::SystemBusy)?
            / 1_000_000_000_000_000_000u128;
        let covered_from_lp =
            u64::try_from(covered_from_lp).map_err(|_| StabilityPoolError::SystemBusy)?;
        if config.symbol != "3USD"
            || config.is_lp_token != Some(true)
            || stability_pool == Principal::anonymous()
            || self.protocol_canister_id == Principal::anonymous()
            || collateral_price_e8s == 0
            || debt_covered_e8s != covered_from_lp
            || self
                .in_flight_liquidations
                .iter()
                .any(|active_vault| *active_vault != vault_id)
            || self.has_pending_pool_absorbs()
            || self
                .pending_sp_liquidations_v2
                .as_ref()
                .is_none_or(|rows| !rows.is_empty())
            || self
                .pending_outbound_payouts
                .as_ref()
                .is_none_or(|rows| !rows.is_empty())
            || self
                .pending_refunds
                .as_ref()
                .is_none_or(|rows| !rows.is_empty())
            || self
                .pending_chain_absorbs
                .as_ref()
                .is_none_or(|rows| !rows.is_empty())
            || self
                .pending_native_xrp_absorbs
                .as_ref()
                .is_none_or(|rows| !rows.is_empty())
            || self
                .completed_sp_liquidations_v2
                .as_ref()
                .is_none_or(|rows| rows.values().any(|row| !row.backend_acknowledged))
            || debt_covered_e8s == 0
            || three_usd_amount == 0
            || virtual_price_e18 == 0
            || approval.ledger != ledger
            || approval.allowance != three_usd_amount
            || approval.memo.len() > 32
            || approval.created_at_time_ns >= approval.expires_at_ns
            || self
                .pending_sp_three_usd_absorbs
                .as_ref()
                .is_none_or(|rows| !rows.is_empty())
            || self.completed_sp_three_usd_absorbs.is_none()
        {
            return Err(StabilityPoolError::SystemBusy);
        }
        let total = self
            .total_stablecoin_balances
            .get(&ledger)
            .copied()
            .unwrap_or(0);
        if three_usd_amount > total {
            return Err(StabilityPoolError::InsufficientPoolBalance);
        }
        let absorb_id = self
            .next_sp_three_usd_absorb_id
            .ok_or(StabilityPoolError::SystemBusy)?;
        let floor = self
            .completed_sp_three_usd_absorb_floor
            .ok_or(StabilityPoolError::SystemBusy)?;
        if absorb_id == 0
            || absorb_id < floor
            || self
                .completed_sp_three_usd_absorbs
                .as_ref()
                .is_none_or(|rows| rows.contains_key(&absorb_id))
        {
            return Err(StabilityPoolError::SystemBusy);
        }
        let next_id = absorb_id
            .checked_add(1)
            .ok_or(StabilityPoolError::SystemBusy)?;
        let depositor_snapshot = self
            .deposits
            .iter()
            .map(|(owner, position)| {
                (
                    *owner,
                    SpThreeUsdDepositorSnapshot {
                        balance: position
                            .stablecoin_balances
                            .get(&ledger)
                            .copied()
                            .unwrap_or(0),
                        collateral_opted_in: self.position_opted_in_for(position, &collateral_type),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let snapshot_total = depositor_snapshot
            .values()
            .try_fold(0u64, |sum, row| sum.checked_add(row.balance))
            .ok_or(StabilityPoolError::SystemBusy)?;
        let opted_in_total = depositor_snapshot
            .values()
            .filter(|row| row.collateral_opted_in)
            .try_fold(0u64, |sum, row| sum.checked_add(row.balance))
            .ok_or(StabilityPoolError::SystemBusy)?;
        if snapshot_total != total {
            return Err(StabilityPoolError::SystemBusy);
        }
        if three_usd_amount > opted_in_total {
            return Err(StabilityPoolError::InsufficientPoolBalance);
        }
        let row = PendingSpThreeUsdAbsorb {
            absorb_id,
            vault_id,
            stability_pool,
            protocol_canister_id: self.protocol_canister_id,
            ledger,
            collateral_type,
            collateral_price_e8s,
            started_at_ns,
            debt_covered_e8s,
            three_usd_amount,
            virtual_price_e18,
            aggregate_balance: total,
            depositor_snapshot,
            approval,
            approval_dispatch_may_have_happened: false,
            approval_receipt_block_index: None,
            backend_dispatch_may_have_happened: false,
            phase: SpThreeUsdAbsorbPhase::ApprovalPending,
            terminal: None,
            allocation: None,
            last_error: None,
        };
        // Approval and maximum principal debits must already fit every pinned
        // depositor balance before the first ledger await. Waiting until the
        // terminal allocation is too late because the backend may have pulled.
        let fee_debits = three_usd_snapshot_allocations(&row, row.approval.fee, false)?;
        let principal_debits = three_usd_snapshot_allocations(&row, row.three_usd_amount, true)?;
        for (owner, snapshot) in &row.depositor_snapshot {
            let combined = fee_debits
                .get(owner)
                .copied()
                .unwrap_or(0)
                .checked_add(principal_debits.get(owner).copied().unwrap_or(0))
                .ok_or(StabilityPoolError::SystemBusy)?;
            if combined > snapshot.balance {
                return Err(StabilityPoolError::InsufficientPoolBalance);
            }
        }
        self.next_sp_three_usd_absorb_id = Some(next_id);
        self.pending_sp_three_usd_absorbs
            .as_mut()
            .ok_or(StabilityPoolError::SystemBusy)?
            .insert(absorb_id, row.clone());
        Ok(row)
    }

    pub fn mark_sp_three_usd_approval_dispatch(
        &mut self,
        absorb_id: u64,
    ) -> Result<(), StabilityPoolError> {
        let row = self
            .pending_sp_three_usd_absorbs
            .as_mut()
            .and_then(|rows| rows.get_mut(&absorb_id))
            .ok_or(StabilityPoolError::SystemBusy)?;
        if row.approval_dispatch_may_have_happened
            && row.phase == SpThreeUsdAbsorbPhase::ApprovalPending
            && row.approval_receipt_block_index.is_none()
        {
            return Ok(());
        }
        if row.phase != SpThreeUsdAbsorbPhase::ApprovalPending
            || row.approval_receipt_block_index.is_some()
        {
            return Err(StabilityPoolError::SystemBusy);
        }
        row.approval_dispatch_may_have_happened = true;
        Ok(())
    }

    pub fn record_sp_three_usd_approval_receipt(
        &mut self,
        absorb_id: u64,
        block_index: u64,
        fee: u64,
    ) -> Result<(), StabilityPoolError> {
        let row = self
            .pending_sp_three_usd_absorbs
            .as_mut()
            .and_then(|rows| rows.get_mut(&absorb_id))
            .ok_or(StabilityPoolError::SystemBusy)?;
        if !row.approval_dispatch_may_have_happened || fee != row.approval.fee {
            return Err(StabilityPoolError::SystemBusy);
        }
        if row.phase == SpThreeUsdAbsorbPhase::ApprovalProven
            && row.approval_receipt_block_index == Some(block_index)
        {
            return Ok(());
        }
        if !matches!(
            row.phase,
            SpThreeUsdAbsorbPhase::ApprovalPending | SpThreeUsdAbsorbPhase::Held
        ) || row.approval_receipt_block_index.is_some()
        {
            return Err(StabilityPoolError::SystemBusy);
        }
        row.approval_receipt_block_index = Some(block_index);
        row.phase = SpThreeUsdAbsorbPhase::ApprovalProven;
        Ok(())
    }

    pub fn mark_sp_three_usd_backend_dispatch(
        &mut self,
        absorb_id: u64,
    ) -> Result<(), StabilityPoolError> {
        let row = self
            .pending_sp_three_usd_absorbs
            .as_mut()
            .and_then(|rows| rows.get_mut(&absorb_id))
            .ok_or(StabilityPoolError::SystemBusy)?;
        if row.phase == SpThreeUsdAbsorbPhase::BackendPending
            && row.backend_dispatch_may_have_happened
        {
            return Ok(());
        }
        if !matches!(
            row.phase,
            SpThreeUsdAbsorbPhase::ApprovalProven | SpThreeUsdAbsorbPhase::Held
        ) || row.approval_receipt_block_index.is_none()
        {
            return Err(StabilityPoolError::SystemBusy);
        }
        row.backend_dispatch_may_have_happened = true;
        row.phase = SpThreeUsdAbsorbPhase::BackendPending;
        Ok(())
    }

    pub fn hold_sp_three_usd_absorb(
        &mut self,
        absorb_id: u64,
        reason: &str,
    ) -> Result<(), StabilityPoolError> {
        let row = self
            .pending_sp_three_usd_absorbs
            .as_mut()
            .and_then(|rows| rows.get_mut(&absorb_id))
            .ok_or(StabilityPoolError::SystemBusy)?;
        row.phase = SpThreeUsdAbsorbPhase::Held;
        row.last_error = Some(reason.chars().take(512).collect());
        Ok(())
    }

    /// Persist validated terminal evidence and exact allocations without
    /// changing depositor balances. The caller must have verified the backend
    /// status and every receipt against the pinned request before calling.
    pub fn plan_sp_three_usd_terminal(
        &mut self,
        absorb_id: u64,
        evidence: SpThreeUsdTerminalEvidence,
    ) -> Result<SpThreeUsdAllocationPlan, StabilityPoolError> {
        let row = self
            .pending_sp_three_usd_absorbs
            .as_ref()
            .and_then(|rows| rows.get(&absorb_id))
            .cloned()
            .ok_or(StabilityPoolError::SystemBusy)?;
        if !row.backend_dispatch_may_have_happened
            || row.approval_receipt_block_index.is_none()
            || !matches!(
                row.phase,
                SpThreeUsdAbsorbPhase::BackendPending
                    | SpThreeUsdAbsorbPhase::Held
                    | SpThreeUsdAbsorbPhase::TerminalProven
            )
        {
            return Err(StabilityPoolError::SystemBusy);
        }
        let (principal_consumed, refund_amount_received, collateral_received) = match &evidence {
            SpThreeUsdTerminalEvidence::PreTransferRejected {
                backend_vault_id,
                backend_absorb_id,
                request,
                reason,
            } => {
                if *backend_vault_id != row.vault_id
                    || *backend_absorb_id != row.absorb_id
                    || request.ledger != row.ledger
                    || request.icusd_debt_covered_e8s != row.debt_covered_e8s
                    || request.three_usd_amount_e8s != row.three_usd_amount
                    || reason.chars().count() > 512
                {
                    return Err(StabilityPoolError::SystemBusy);
                }
                (0, 0, 0)
            }
            SpThreeUsdTerminalEvidence::Absorbed {
                backend_vault_id,
                backend_absorb_id,
                request,
                transfer_tuple,
                transfer_block_index,
                observed_transfer_fee,
                proof,
                result,
                proportional_refund,
                payout_receipt,
            } => {
                let expected_collateral_type = result
                    .collateral_type
                    .parse::<Principal>()
                    .map_err(|_| StabilityPoolError::SystemBusy)?;
                let payout = &payout_receipt.tuple;
                if *backend_vault_id != row.vault_id
                    || *backend_absorb_id != row.absorb_id
                    || request.ledger != row.ledger
                    || request.icusd_debt_covered_e8s != row.debt_covered_e8s
                    || request.three_usd_amount_e8s != row.three_usd_amount
                    || transfer_tuple.amount_e8s != row.three_usd_amount
                    || transfer_tuple.spender_owner != row.protocol_canister_id
                    || transfer_tuple.spender_subaccount.is_some()
                    || transfer_tuple.destination.owner != row.protocol_canister_id
                    || transfer_tuple.destination.subaccount.is_some()
                    || transfer_tuple.source.owner != row.stability_pool
                    || transfer_tuple.source.subaccount.is_some()
                    || transfer_tuple.parent_absorb_id != Some(row.absorb_id)
                    || transfer_tuple.op_nonce == 0
                    || transfer_tuple.memo.as_slice() != rumi_protocol_backend::management::nonce_to_memo(transfer_tuple.op_nonce).0.as_slice()
                    || transfer_tuple.created_at_time_ns != rumi_protocol_backend::management::nonce_to_created_at_time(transfer_tuple.op_nonce)
                    || transfer_tuple.fee_e8s.is_some_and(|fee| fee != *observed_transfer_fee)
                    || *transfer_block_index != proof.block_index
                    || proof.ledger_kind != rumi_protocol_backend::icrc3_proof::SpProofLedger::ThreePoolTransferDefault
                    || proof.vault_id_memo != row.vault_id
                    || result.vault_id != row.vault_id
                    || !result.success
                    || (result.block_index != 0 && result.block_index != proof.block_index)
                    || result.liquidated_debt == 0
                    || result.liquidated_debt > row.debt_covered_e8s
                    || expected_collateral_type != row.collateral_type
                    // The row price is the admission snapshot. Oracle values
                    // can refresh while approval/pull/backend awaits are in
                    // flight, so terminal accounting pins the authenticated
                    // backend execution price instead of requiring stale
                    // snapshot equality.
                    || result.collateral_price_e8s == 0
                    || payout.ledger != row.collateral_type
                    || payout.collateral_type != row.collateral_type
                    || payout.source.owner != row.protocol_canister_id
                    || payout.source.subaccount.is_some()
                    || payout.destination.owner != row.stability_pool
                    || payout.destination.subaccount.is_some()
                    || payout.op_nonce == 0
                    || payout.memo.as_slice() != rumi_protocol_backend::management::nonce_to_memo(payout.op_nonce).0.as_slice()
                    || payout.created_at_time_ns != rumi_protocol_backend::management::nonce_to_created_at_time(payout.op_nonce)
                    || payout.gross_amount_e8s != result.collateral_received
                    || payout.net_amount_e8s == 0
                    || payout.net_amount_e8s.checked_add(payout.fee_e8s) != Some(payout.gross_amount_e8s)
                { return Err(StabilityPoolError::SystemBusy); }
                let consumed = u64::try_from(
                    (row.three_usd_amount as u128)
                        .checked_mul(result.liquidated_debt as u128)
                        .ok_or(StabilityPoolError::SystemBusy)?
                        / row.debt_covered_e8s as u128,
                )
                .map_err(|_| StabilityPoolError::SystemBusy)?;
                let refund_gross = row
                    .three_usd_amount
                    .checked_sub(consumed)
                    .ok_or(StabilityPoolError::SystemBusy)?;
                validate_three_usd_refund(
                    proportional_refund.as_ref(),
                    refund_gross,
                    row.protocol_canister_id,
                    row.stability_pool,
                )?;
                let refund_amount = proportional_refund
                    .as_ref()
                    .map(|receipt| receipt.tuple.amount_e8s)
                    .unwrap_or(0);
                (consumed, refund_amount, payout.net_amount_e8s)
            }
            SpThreeUsdTerminalEvidence::FailedAfterTransfer {
                backend_vault_id,
                backend_absorb_id,
                request,
                transfer_tuple,
                transfer_block_index,
                observed_transfer_fee,
                proof,
                error,
                full_refund,
            } => {
                if error.chars().count() > 512
                    || *backend_vault_id != row.vault_id
                    || *backend_absorb_id != row.absorb_id
                    || request.ledger != row.ledger
                    || request.icusd_debt_covered_e8s != row.debt_covered_e8s
                    || request.three_usd_amount_e8s != row.three_usd_amount
                    || transfer_tuple.amount_e8s != row.three_usd_amount
                    || transfer_tuple.spender_owner != row.protocol_canister_id
                    || transfer_tuple.spender_subaccount.is_some()
                    || transfer_tuple.destination.owner != row.protocol_canister_id
                    || transfer_tuple.destination.subaccount.is_some()
                    || transfer_tuple.source.owner != row.stability_pool
                    || transfer_tuple.source.subaccount.is_some()
                    || transfer_tuple.parent_absorb_id != Some(row.absorb_id)
                    || transfer_tuple.op_nonce == 0
                    || transfer_tuple.memo.as_slice() != rumi_protocol_backend::management::nonce_to_memo(transfer_tuple.op_nonce).0.as_slice()
                    || transfer_tuple.created_at_time_ns != rumi_protocol_backend::management::nonce_to_created_at_time(transfer_tuple.op_nonce)
                    || transfer_tuple.fee_e8s.is_some_and(|fee| fee != *observed_transfer_fee)
                    || *transfer_block_index != proof.block_index
                    || proof.ledger_kind != rumi_protocol_backend::icrc3_proof::SpProofLedger::ThreePoolTransferDefault
                    || proof.vault_id_memo != row.vault_id
                { return Err(StabilityPoolError::SystemBusy); }
                validate_three_usd_refund(
                    Some(full_refund),
                    row.three_usd_amount,
                    row.protocol_canister_id,
                    row.stability_pool,
                )?;
                (0, full_refund.tuple.amount_e8s, 0)
            }
        };
        let approval_fee = row.approval.fee;
        let approval_fee_debits = three_usd_snapshot_allocations(&row, approval_fee, false)?;
        let principal_debits = three_usd_snapshot_allocations(&row, principal_consumed, true)?;
        let collateral_weights: Vec<(Principal, u64)> = principal_debits
            .iter()
            .filter_map(|(owner, amount)| (*amount > 0).then_some((*owner, *amount)))
            .collect();
        let collateral_credits = if collateral_received == 0 {
            BTreeMap::new()
        } else {
            exact_weight_allocations(&collateral_weights, collateral_received)?
        };
        let mut combined = BTreeMap::<Principal, u64>::new();
        for allocation in [&approval_fee_debits, &principal_debits] {
            for (owner, amount) in allocation {
                let entry = combined.entry(*owner).or_default();
                *entry = entry
                    .checked_add(*amount)
                    .ok_or(StabilityPoolError::SystemBusy)?;
            }
        }
        for (owner, debit) in &combined {
            if row
                .depositor_snapshot
                .get(owner)
                .is_none_or(|snapshot| snapshot.balance < *debit)
            {
                return Err(StabilityPoolError::InsufficientPoolBalance);
            }
        }
        let total_stable_debit = combined
            .values()
            .try_fold(0u64, |sum, amount| sum.checked_add(*amount))
            .ok_or(StabilityPoolError::SystemBusy)?;
        let refund_net = refund_amount_received;
        let expected_expense = if matches!(
            &evidence,
            SpThreeUsdTerminalEvidence::PreTransferRejected { .. }
        ) {
            approval_fee
        } else {
            row.three_usd_amount
                .checked_add(approval_fee)
                .and_then(|value| value.checked_sub(refund_net))
                .ok_or(StabilityPoolError::SystemBusy)?
        };
        if total_stable_debit != expected_expense
            || collateral_credits
                .values()
                .try_fold(0u64, |sum, value| sum.checked_add(*value))
                != Some(collateral_received)
        {
            return Err(StabilityPoolError::SystemBusy);
        }
        let plan = SpThreeUsdAllocationPlan {
            principal_debits,
            approval_fee_debits,
            collateral_credits,
            principal_consumed,
            refund_amount_received,
            total_stable_debit,
            collateral_received,
        };
        let pending = self
            .pending_sp_three_usd_absorbs
            .as_mut()
            .and_then(|rows| rows.get_mut(&absorb_id))
            .ok_or(StabilityPoolError::SystemBusy)?;
        if pending != &row {
            return Err(StabilityPoolError::SystemBusy);
        }
        if pending
            .terminal
            .as_ref()
            .is_some_and(|existing| existing != &evidence)
            || pending
                .allocation
                .as_ref()
                .is_some_and(|existing| existing != &plan)
        {
            return Err(StabilityPoolError::SystemBusy);
        }
        pending.terminal = Some(evidence);
        pending.allocation = Some(plan.clone());
        pending.phase = SpThreeUsdAbsorbPhase::TerminalProven;
        Ok(plan)
    }

    /// Atomically applies a previously planned terminal allocation after
    /// rechecking the full pinned position snapshot and pool aggregates.
    pub fn apply_sp_three_usd_terminal(
        &mut self,
        absorb_id: u64,
    ) -> Result<(), StabilityPoolError> {
        let Some(row) = self
            .pending_sp_three_usd_absorbs
            .as_ref()
            .and_then(|rows| rows.get(&absorb_id))
            .cloned()
        else {
            return if self
                .completed_sp_three_usd_absorbs
                .as_ref()
                .is_some_and(|rows| {
                    rows.get(&absorb_id)
                        .is_some_and(|row| row.phase == SpThreeUsdAbsorbPhase::Complete)
                }) {
                Ok(())
            } else {
                Err(StabilityPoolError::SystemBusy)
            };
        };
        let plan = row
            .allocation
            .clone()
            .ok_or(StabilityPoolError::SystemBusy)?;
        if row.terminal.is_none() {
            return Err(StabilityPoolError::SystemBusy);
        }
        if row.phase == SpThreeUsdAbsorbPhase::Complete {
            return Ok(());
        }
        if row.phase != SpThreeUsdAbsorbPhase::TerminalProven
            || self.ensure_stablecoin_aggregate_matches_positions(row.ledger)?
                != row.aggregate_balance
            || self.deposits.len() != row.depositor_snapshot.len()
        {
            return Err(StabilityPoolError::SystemBusy);
        }
        for (owner, snapshot) in &row.depositor_snapshot {
            let position = self
                .deposits
                .get(owner)
                .ok_or(StabilityPoolError::SystemBusy)?;
            if position
                .stablecoin_balances
                .get(&row.ledger)
                .copied()
                .unwrap_or(0)
                != snapshot.balance
                || self.position_opted_in_for(position, &row.collateral_type)
                    != snapshot.collateral_opted_in
            {
                return Err(StabilityPoolError::SystemBusy);
            }
        }
        let new_total = row
            .aggregate_balance
            .checked_sub(plan.total_stable_debit)
            .ok_or(StabilityPoolError::InsufficientPoolBalance)?;
        let execution_price_e8s = match &row.terminal {
            Some(SpThreeUsdTerminalEvidence::Absorbed { result, .. }) => {
                if result.collateral_price_e8s == 0
                    || result.collateral_type.parse::<Principal>().ok() != Some(row.collateral_type)
                {
                    return Err(StabilityPoolError::SystemBusy);
                }
                Some(result.collateral_price_e8s)
            }
            _ => None,
        };
        let absorbed = execution_price_e8s.is_some();
        let expected_liquidation_count = if absorbed {
            self.total_liquidations_executed
                .checked_add(1)
                .ok_or(StabilityPoolError::SystemBusy)?
        } else {
            self.total_liquidations_executed
        };
        let completed = self
            .completed_sp_three_usd_absorbs
            .as_ref()
            .ok_or(StabilityPoolError::SystemBusy)?;
        let evicted_id = if completed.len() >= MAX_COMPLETED_SP_THREE_USD_ABSORBS {
            Some(
                *completed
                    .keys()
                    .next()
                    .ok_or(StabilityPoolError::SystemBusy)?,
            )
        } else {
            None
        };
        let next_floor = if let Some(id) = evicted_id {
            Some(id.checked_add(1).ok_or(StabilityPoolError::SystemBusy)?)
        } else {
            None
        };
        if self
            .pending_sp_three_usd_absorbs
            .as_ref()
            .is_none_or(|rows| !rows.contains_key(&absorb_id))
            || self.completed_sp_three_usd_absorb_floor.is_none()
        {
            return Err(StabilityPoolError::SystemBusy);
        }
        let mut combined = BTreeMap::<Principal, u64>::new();
        for allocation in [&plan.approval_fee_debits, &plan.principal_debits] {
            for (owner, amount) in allocation {
                let entry = combined.entry(*owner).or_default();
                *entry = entry
                    .checked_add(*amount)
                    .ok_or(StabilityPoolError::SystemBusy)?;
            }
        }
        for (owner, debit) in &combined {
            let current = self
                .deposits
                .get(owner)
                .and_then(|pos| pos.stablecoin_balances.get(&row.ledger))
                .copied()
                .ok_or(StabilityPoolError::SystemBusy)?;
            if current < *debit {
                return Err(StabilityPoolError::InsufficientPoolBalance);
            }
        }
        for (owner, credit) in &plan.collateral_credits {
            let existing = self
                .deposits
                .get(owner)
                .and_then(|pos| pos.collateral_gains.get(&row.collateral_type))
                .copied()
                .unwrap_or(0);
            existing
                .checked_add(*credit)
                .ok_or(StabilityPoolError::SystemBusy)?;
        }
        for (owner, debit) in combined {
            let position = self
                .deposits
                .get_mut(&owner)
                .expect("snapshot owner prevalidated");
            let balance = position
                .stablecoin_balances
                .get_mut(&row.ledger)
                .expect("balance prevalidated");
            *balance -= debit;
            if *balance == 0 {
                position.stablecoin_balances.remove(&row.ledger);
            }
        }
        for (owner, credit) in &plan.collateral_credits {
            let position = self
                .deposits
                .get_mut(owner)
                .expect("credit owner prevalidated");
            let entry = position
                .collateral_gains
                .entry(row.collateral_type)
                .or_default();
            *entry += *credit;
        }
        self.total_stablecoin_balances.insert(row.ledger, new_total);
        if absorbed {
            self.record_liquidation_in_history(PoolLiquidationRecord {
                vault_id: row.vault_id,
                timestamp: row.started_at_ns,
                stables_consumed: BTreeMap::from([(row.ledger, plan.principal_consumed)]),
                collateral_gained: plan.collateral_received,
                collateral_type: row.collateral_type,
                depositors_count: plan
                    .principal_debits
                    .values()
                    .filter(|amount| **amount > 0)
                    .count() as u64,
                collateral_price_e8s: execution_price_e8s,
            });
            debug_assert_eq!(self.total_liquidations_executed, expected_liquidation_count);
        }
        let completed = self
            .completed_sp_three_usd_absorbs
            .as_mut()
            .expect("completed map prevalidated");
        if let Some(id) = evicted_id {
            completed.remove(&id);
        }
        let mut completed_row = row.clone();
        completed_row.phase = SpThreeUsdAbsorbPhase::Complete;
        completed.insert(absorb_id, completed_row);
        if let Some(floor) = next_floor {
            self.completed_sp_three_usd_absorb_floor = Some(floor);
        }
        self.pending_sp_three_usd_absorbs
            .as_mut()
            .expect("pending map prevalidated")
            .remove(&absorb_id);
        Ok(())
    }

    pub fn recover_interrupted_sp_liquidation_v2_approvals(&mut self) -> usize {
        let mut recovered = 0;
        if let Some(rows) = self.pending_sp_liquidations_v2.as_mut() {
            for row in rows.values_mut() {
                if row.approval_dispatch_in_flight {
                    row.approval_dispatch_in_flight = false;
                    row.approval_ambiguous_seen = true;
                    row.last_error =
                        Some("upgrade interrupted approval; exact receipt required".into());
                    recovered += 1;
                }
            }
        }
        recovered
    }

    pub fn allocate_outbound_payout_timestamp(
        &mut self,
        now_ns: u64,
    ) -> Result<u64, StabilityPoolError> {
        let timestamp = match self.last_outbound_payout_created_at_ns {
            Some(previous) if now_ns <= previous => previous
                .checked_add(1)
                .ok_or(StabilityPoolError::SystemBusy)?,
            _ => now_ns,
        };
        self.last_outbound_payout_created_at_ns = Some(timestamp);
        Ok(timestamp)
    }

    pub fn prepare_sp_liquidation_v2(
        &mut self,
        vault_id: u64,
        collateral_type: Principal,
        collateral_price_e8s: u64,
        token_ledger: Principal,
        amount: u64,
        token: SpLiquidationToken,
        approval: SpLiquidationV2ApprovalTuple,
    ) -> Result<PendingSpLiquidationV2, StabilityPoolError> {
        self.ensure_stablecoin_aggregate_matches_positions(token_ledger)?;
        if self.in_flight_liquidations.contains(&vault_id) {
            return Err(StabilityPoolError::SystemBusy);
        }
        let pending = self
            .pending_sp_liquidations_v2
            .as_ref()
            .ok_or(StabilityPoolError::SystemBusy)?;
        let has_unacknowledged_completion = self
            .completed_sp_liquidations_v2
            .as_ref()
            .is_none_or(|completed| completed.values().any(|row| !row.backend_acknowledged));
        if !pending.is_empty()
            || has_unacknowledged_completion
            || pending.len() >= MAX_PENDING_SP_LIQUIDATIONS_V2
            || self
                .completed_sp_liquidations_v2
                .as_ref()
                .is_none_or(|completed| completed.len() >= MAX_COMPLETED_SP_LIQUIDATIONS_V2)
        {
            return Err(StabilityPoolError::SystemBusy);
        }
        let request_id = self
            .next_sp_liquidation_request_id
            .ok_or(StabilityPoolError::SystemBusy)?;
        let next = request_id
            .checked_add(1)
            .ok_or(StabilityPoolError::SystemBusy)?;
        let row = PendingSpLiquidationV2 {
            request: SpLiquidationV2Intent {
                request_id,
                vault_id,
                amount,
                token,
            },
            backend_request: None,
            stablecoin_ledger: token_ledger,
            collateral_type,
            collateral_price_e8s,
            approval,
            approval_dispatch_in_flight: false,
            approval_ambiguous_seen: false,
            approval_proven_no_effect: false,
            approval_candidate_block_index: None,
            approval_receipt_block_index: None,
            phase: SpLiquidationV2LocalPhase::ApprovalPending,
            ambiguous_seen: false,
            stable_pull_receipt: None,
            stable_pull_tuple: None,
            stable_pull_candidate_block_index: None,
            payout_tuple: None,
            payout_candidate_block_index: None,
            payout_receipt: None,
            payout_supersession_generation: Some(0),
            payout_supersession_predecessor: None,
            payout_supersession_replacement: None,
            payout_supersession_evidence: None,
            result: None,
            stable_debit_applied: false,
            pending_collateral_allocations: BTreeMap::new(),
            approval_fee_debits: BTreeMap::new(),
            stable_pull_fee_debits: BTreeMap::new(),
            stable_principal_debits: BTreeMap::new(),
            stable_refund_tuple: None,
            stable_refund_candidate_block_index: None,
            stable_refund_receipt: None,
            stable_refund_applied: false,
            backend_acknowledged: false,
            last_error: None,
        };
        self.next_sp_liquidation_request_id = Some(next);
        self.pending_sp_liquidations_v2
            .as_mut()
            .ok_or(StabilityPoolError::SystemBusy)?
            .insert(request_id, row.clone());
        Ok(row)
    }

    pub fn pending_sp_liquidation_v2(&self, request_id: u64) -> Option<PendingSpLiquidationV2> {
        self.pending_sp_liquidations_v2
            .as_ref()?
            .get(&request_id)
            .cloned()
    }

    pub fn pending_sp_liquidation_v2_for_vault(
        &self,
        vault_id: u64,
    ) -> Option<PendingSpLiquidationV2> {
        self.pending_sp_liquidations_v2
            .as_ref()?
            .values()
            .find(|row| row.request.vault_id == vault_id)
            .cloned()
    }

    pub fn sp_liquidation_v2_row(&self, request_id: u64) -> Option<PendingSpLiquidationV2> {
        self.pending_sp_liquidation_v2(request_id).or_else(|| {
            self.completed_sp_liquidations_v2
                .as_ref()?
                .get(&request_id)
                .cloned()
        })
    }

    pub fn sp_liquidation_v2_id_is_stale(&self, request_id: u64) -> bool {
        self.completed_sp_liquidation_request_floor
            .is_some_and(|floor| request_id < floor)
    }

    /// Bounded round-robin recovery selection. The persistent cursor ensures a
    /// large or repeatedly failing low-ID prefix cannot starve later rows.
    pub fn take_sp_liquidation_v2_recovery_batch(&mut self, limit: usize) -> Vec<u64> {
        if limit == 0 {
            return Vec::new();
        }
        let pending = self
            .pending_sp_liquidations_v2
            .as_ref()
            .map(|rows| rows.keys().copied().collect::<Vec<_>>())
            .unwrap_or_default();
        let completed = self
            .completed_sp_liquidations_v2
            .as_ref()
            .map(|rows| {
                rows.iter()
                    .filter_map(|(id, row)| (!row.backend_acknowledged).then_some(*id))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let keys: Vec<u64> = pending
            .into_iter()
            .chain(completed)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if keys.is_empty() {
            return Vec::new();
        }
        let cursor = self.sp_liquidation_v2_recovery_cursor;
        let mut selected: Vec<u64> = keys
            .iter()
            .copied()
            .filter(|id| cursor.map_or(true, |after| *id > after))
            .take(limit)
            .collect();
        if selected.len() < limit {
            selected.extend(
                keys.iter()
                    .copied()
                    .filter(|id| cursor.is_some_and(|after| *id <= after))
                    .take(limit - selected.len()),
            );
        }
        if let Some(last) = selected.last().copied() {
            self.sp_liquidation_v2_recovery_cursor = Some(last);
        }
        selected
    }

    /// Select only rows whose exact approval receipt was already verified.
    /// The timer may repeat the pinned backend identity and query its status,
    /// but must never guess an approval block after an ambiguous ledger reply.
    pub fn take_sp_three_usd_recovery_batch(&mut self, limit: usize) -> Vec<u64> {
        if limit == 0 {
            return Vec::new();
        }
        let keys = self
            .pending_sp_three_usd_absorbs
            .as_ref()
            .map(|rows| {
                rows.iter()
                    .filter_map(|(id, row)| {
                        (row.approval_receipt_block_index.is_some()
                            && matches!(
                                row.phase,
                                SpThreeUsdAbsorbPhase::ApprovalProven
                                    | SpThreeUsdAbsorbPhase::BackendPending
                                    | SpThreeUsdAbsorbPhase::Held
                            ))
                        .then_some(*id)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if keys.is_empty() {
            return Vec::new();
        }
        let cursor = self.sp_three_usd_recovery_cursor;
        let mut selected: Vec<u64> = keys
            .iter()
            .copied()
            .filter(|id| cursor.map_or(true, |after| *id > after))
            .take(limit)
            .collect();
        if selected.len() < limit {
            selected.extend(
                keys.iter()
                    .copied()
                    .filter(|id| cursor.is_some_and(|after| *id <= after))
                    .take(limit - selected.len()),
            );
        }
        if let Some(last) = selected.last().copied() {
            self.sp_three_usd_recovery_cursor = Some(last);
        }
        selected
    }

    pub fn update_pending_sp_liquidation_v2(
        &mut self,
        row: PendingSpLiquidationV2,
    ) -> Result<(), StabilityPoolError> {
        let pending = self
            .pending_sp_liquidations_v2
            .as_mut()
            .ok_or(StabilityPoolError::SystemBusy)?;
        let saved = pending
            .get_mut(&row.request.request_id)
            .ok_or(StabilityPoolError::SystemBusy)?;
        if saved.request != row.request
            || saved.approval != row.approval
            || saved
                .backend_request
                .as_ref()
                .is_some_and(|request| row.backend_request.as_ref() != Some(request))
            || (saved.backend_request.is_none()
                && row.backend_request.is_some()
                && (!row.approval.fee_accounted
                    || row.backend_request.as_ref().is_none_or(|request| {
                        request.request_id != row.request.request_id
                            || request.vault_id != row.request.vault_id
                            || request.amount != row.request.amount
                            || request.token != row.request.token
                            || request.approval.block_index
                                != row.approval_receipt_block_index.unwrap_or(0)
                    })))
        {
            return Err(StabilityPoolError::SystemBusy);
        }
        *saved = row;
        Ok(())
    }

    pub fn complete_sp_liquidation_v2(
        &mut self,
        request_id: u64,
    ) -> Result<PendingSpLiquidationV2, StabilityPoolError> {
        if self
            .completed_sp_liquidations_v2
            .as_ref()
            .is_none_or(|completed| completed.len() >= MAX_COMPLETED_SP_LIQUIDATIONS_V2)
        {
            return Err(StabilityPoolError::SystemBusy);
        }
        let mut row = self
            .pending_sp_liquidations_v2
            .as_mut()
            .ok_or(StabilityPoolError::SystemBusy)?
            .remove(&request_id)
            .ok_or(StabilityPoolError::SystemBusy)?;
        if row.phase != SpLiquidationV2LocalPhase::Rejected {
            row.phase = SpLiquidationV2LocalPhase::Complete;
        }
        let completed = self
            .completed_sp_liquidations_v2
            .as_mut()
            .ok_or(StabilityPoolError::SystemBusy)?;
        completed.insert(request_id, row.clone());
        Ok(row)
    }

    /// Retire a terminal local row only after the backend acknowledges the
    /// same request. Advance the replay floor solely across a contiguous
    /// acknowledged prefix; later acknowledgements remain tombstoned while an
    /// earlier request is unresolved.
    pub fn acknowledge_sp_liquidation_v2(
        &mut self,
        request_id: u64,
    ) -> Result<(), StabilityPoolError> {
        let floor = self.completed_sp_liquidation_request_floor.unwrap_or(0);
        if request_id < floor {
            return Ok(());
        }
        let row = self
            .completed_sp_liquidations_v2
            .as_mut()
            .and_then(|completed| completed.get_mut(&request_id))
            .ok_or(StabilityPoolError::SystemBusy)?;
        row.backend_acknowledged = true;

        let completed = self
            .completed_sp_liquidations_v2
            .as_mut()
            .ok_or(StabilityPoolError::SystemBusy)?;
        let mut next_floor = floor;
        loop {
            let Some(row) = completed.get(&next_floor) else {
                break;
            };
            if !row.backend_acknowledged {
                break;
            }
            completed.remove(&next_floor);
            let Some(next) = next_floor.checked_add(1) else {
                break;
            };
            next_floor = next;
        }
        self.completed_sp_liquidation_request_floor = Some(next_floor);
        Ok(())
    }

    pub fn account_sp_liquidation_v2_approval_fee(
        &mut self,
        request_id: u64,
        receipt: SpLiquidationApprovalReceipt,
    ) -> Result<(), StabilityPoolError> {
        let row = self
            .pending_sp_liquidations_v2
            .as_ref()
            .and_then(|rows| rows.get(&request_id))
            .ok_or(StabilityPoolError::SystemBusy)?;
        if row.approval.fee_accounted {
            return if row
                .backend_request
                .as_ref()
                .is_some_and(|request| request.approval == receipt)
            {
                Ok(())
            } else {
                Err(StabilityPoolError::SystemBusy)
            };
        }
        let expected_tuple = SpLiquidationApprovalTuple {
            ledger: row.approval.ledger,
            owner: row.approval.owner.clone(),
            spender: row.approval.spender.clone(),
            allowance_raw: row.approval.allowance_raw,
            fee_raw: row.approval.fee_raw,
            memo: row.approval.memo.clone(),
            created_at_time_ns: row.approval.created_at_time_ns,
            expires_at_ns: row.approval.expires_at_ns,
        };
        // ICRC-3 block indexes are Nat values and zero is a valid first block.
        // Presence is represented by Option in the journal, so do not reserve
        // zero as a sentinel here.
        if receipt.tuple != expected_tuple {
            return Err(StabilityPoolError::SystemBusy);
        }
        let ledger = row.stablecoin_ledger;
        let fee = row.approval.fee_raw;
        if self
            .total_stablecoin_balances
            .get(&ledger)
            .copied()
            .unwrap_or(0)
            < fee
        {
            return Err(StabilityPoolError::InsufficientPoolBalance);
        }
        let fee_debits = self.deduct_exact_pool_fee(ledger, fee)?;
        let row = self
            .pending_sp_liquidations_v2
            .as_mut()
            .and_then(|rows| rows.get_mut(&request_id))
            .expect("pending liquidation row was validated before exact fee debit");
        row.approval.fee_accounted = true;
        row.approval_fee_debits = fee_debits;
        row.approval_receipt_block_index = Some(receipt.block_index);
        row.backend_request = Some(SpLiquidationV2Request {
            request_id: row.request.request_id,
            vault_id: row.request.vault_id,
            amount: row.request.amount,
            token: row.request.token,
            approval: receipt,
        });
        row.approval_dispatch_in_flight = false;
        row.phase = SpLiquidationV2LocalPhase::BackendPending;
        Ok(())
    }

    pub fn pending_sp_legacy_approval_fee(
        &self,
        vault_id: u64,
        ledger: Principal,
    ) -> Option<PendingSpLegacyApprovalFee> {
        self.pending_sp_legacy_approval_fees
            .as_ref()
            .and_then(|rows| rows.get(&(vault_id, ledger)).cloned())
    }

    pub fn pending_sp_legacy_approval_fee_keys(&self, limit: usize) -> Vec<(u64, Principal)> {
        self.pending_sp_legacy_approval_fees
            .as_ref()
            .map(|rows| rows.keys().take(limit).copied().collect())
            .unwrap_or_default()
    }

    pub fn begin_sp_legacy_approval_fee(
        &mut self,
        row: PendingSpLegacyApprovalFee,
    ) -> Result<PendingSpLegacyApprovalFee, StabilityPoolError> {
        let key = (row.vault_id, row.approval.ledger);
        let rows = self
            .pending_sp_legacy_approval_fees
            .get_or_insert_with(BTreeMap::new);
        if let Some(saved) = rows.get(&key) {
            return if saved.vault_id == row.vault_id && saved.approval == row.approval {
                Ok(saved.clone())
            } else {
                Err(StabilityPoolError::SystemBusy)
            };
        }
        // Keep one unresolved legacy approval globally. This prevents later
        // approvals from progressing while a prior fee debit is uncertain.
        if !rows.is_empty() || rows.len() >= 128 {
            return Err(StabilityPoolError::SystemBusy);
        }
        rows.insert(key, row.clone());
        Ok(row)
    }

    pub fn mark_sp_legacy_approval_dispatch(
        &mut self,
        vault_id: u64,
        ledger: Principal,
        dispatch_in_flight: bool,
        ambiguous: bool,
    ) -> Result<(), StabilityPoolError> {
        let row = self
            .pending_sp_legacy_approval_fees
            .as_mut()
            .and_then(|rows| rows.get_mut(&(vault_id, ledger)))
            .ok_or(StabilityPoolError::SystemBusy)?;
        // If a previous dispatch was still in flight when this attempt starts,
        // the prior result was lost and the exact tuple is now ambiguous.
        if dispatch_in_flight && row.dispatch_in_flight {
            row.ambiguous_seen = true;
        }
        row.dispatch_in_flight = dispatch_in_flight;
        row.ambiguous_seen |= ambiguous;
        Ok(())
    }

    pub fn clear_sp_legacy_approval_fee_after_no_effect(
        &mut self,
        vault_id: u64,
        ledger: Principal,
    ) -> Result<(), StabilityPoolError> {
        let rows = self
            .pending_sp_legacy_approval_fees
            .as_mut()
            .ok_or(StabilityPoolError::SystemBusy)?;
        let row = rows
            .get(&(vault_id, ledger))
            .ok_or(StabilityPoolError::SystemBusy)?;
        if row.dispatch_in_flight || row.ambiguous_seen {
            return Err(StabilityPoolError::SystemBusy);
        }
        rows.remove(&(vault_id, ledger));
        Ok(())
    }

    pub fn account_sp_legacy_approval_fee(
        &mut self,
        vault_id: u64,
        receipt: SpLiquidationApprovalReceipt,
    ) -> Result<(), StabilityPoolError> {
        let key = (vault_id, receipt.tuple.ledger);
        let row = self
            .pending_sp_legacy_approval_fees
            .as_ref()
            .and_then(|rows| rows.get(&key))
            .cloned()
            .ok_or(StabilityPoolError::SystemBusy)?;
        if row.vault_id != vault_id
            || row.approval != receipt.tuple
            || (!row.dispatch_in_flight && !row.ambiguous_seen)
        {
            return Err(StabilityPoolError::SystemBusy);
        }
        let ledger = row.approval.ledger;
        let fee = row.approval.fee_raw;
        self.deduct_exact_pool_fee(ledger, fee)?;
        self.pending_sp_legacy_approval_fees
            .as_mut()
            .ok_or(StabilityPoolError::SystemBusy)?
            .remove(&key);
        Ok(())
    }

    pub fn mark_sp_liquidation_v2_approval_dispatch(
        &mut self,
        request_id: u64,
        dispatch_in_flight: bool,
        ambiguous: bool,
        candidate_block_index: Option<u64>,
    ) -> Result<(), StabilityPoolError> {
        let row = self
            .pending_sp_liquidations_v2
            .as_mut()
            .and_then(|rows| rows.get_mut(&request_id))
            .ok_or(StabilityPoolError::SystemBusy)?;
        // Candidate block indexes are untrusted until the public caller has
        // independently fetched and verified the exact ICRC-3 approval tuple.
        // This state mutator is only for dispatch/ambiguity flags; do not let a
        // caller accidentally persist an unverified candidate here.
        if row.approval.fee_accounted || candidate_block_index.is_some() {
            return Err(StabilityPoolError::SystemBusy);
        }
        row.approval_dispatch_in_flight = dispatch_in_flight;
        row.approval_ambiguous_seen |= ambiguous;
        if dispatch_in_flight {
            row.approval_proven_no_effect = false;
        }
        if ambiguous {
            row.last_error = Some("approval outcome unknown; exact receipt required".into());
        } else if !dispatch_in_flight {
            row.last_error = None;
        }
        Ok(())
    }

    pub fn mark_sp_liquidation_v2_approval_no_effect(
        &mut self,
        request_id: u64,
        error: String,
    ) -> Result<(), StabilityPoolError> {
        let row = self
            .pending_sp_liquidations_v2
            .as_mut()
            .and_then(|rows| rows.get_mut(&request_id))
            .ok_or(StabilityPoolError::SystemBusy)?;
        if !row.approval_dispatch_in_flight
            || row.approval_ambiguous_seen
            || row.approval.fee_accounted
            || row.backend_request.is_some()
        {
            return Err(StabilityPoolError::SystemBusy);
        }
        row.approval_dispatch_in_flight = false;
        row.approval_proven_no_effect = true;
        row.last_error = Some(error.chars().take(512).collect());
        Ok(())
    }

    /// Reprice an approval only after its first dispatch returned a typed
    /// no-effect result. Once any outcome is ambiguous, the exact tuple is
    /// immutable and the request ID remains fenced for reconciliation.
    pub fn reprice_sp_liquidation_v2_approval_after_no_effect(
        &mut self,
        request_id: u64,
        approval: SpLiquidationV2ApprovalTuple,
    ) -> Result<(), StabilityPoolError> {
        let row = self
            .pending_sp_liquidations_v2
            .as_mut()
            .and_then(|rows| rows.get_mut(&request_id))
            .ok_or(StabilityPoolError::SystemBusy)?;
        if row.approval_dispatch_in_flight
            || row.approval_ambiguous_seen
            || !row.approval_proven_no_effect
            || row.approval.fee_accounted
            || row.backend_request.is_some()
            || approval.ledger != row.stablecoin_ledger
            || approval.owner != row.approval.owner
            || approval.spender != row.approval.spender
        {
            return Err(StabilityPoolError::SystemBusy);
        }
        row.approval = approval;
        row.approval_proven_no_effect = false;
        row.approval_candidate_block_index = None;
        row.approval_receipt_block_index = None;
        row.last_error = None;
        Ok(())
    }

    pub fn record_sp_liquidation_v2_backend_receipts(
        &mut self,
        request_id: u64,
        stable_pull_receipt: SpLiquidationStablePullReceipt,
        payout_tuple: SpLiquidationPayoutTuple,
        payout_candidate_block_index: Option<u64>,
        result: SpLiquidationV2SuccessWithFee,
    ) -> Result<(), StabilityPoolError> {
        let row = self
            .pending_sp_liquidations_v2
            .as_mut()
            .and_then(|rows| rows.get_mut(&request_id))
            .ok_or(StabilityPoolError::SystemBusy)?;
        if !row.approval.fee_accounted
            || row
                .stable_pull_receipt
                .as_ref()
                .is_some_and(|saved| saved != &stable_pull_receipt)
            || row
                .payout_tuple
                .as_ref()
                .is_some_and(|saved| saved != &payout_tuple)
            || row.result.as_ref().is_some_and(|saved| saved != &result)
        {
            return Err(StabilityPoolError::SystemBusy);
        }
        row.stable_pull_tuple = Some(stable_pull_receipt.tuple.clone());
        row.stable_pull_candidate_block_index = Some(stable_pull_receipt.block_index);
        row.stable_pull_receipt = Some(stable_pull_receipt);
        row.payout_tuple = Some(payout_tuple);
        row.payout_candidate_block_index = payout_candidate_block_index;
        row.result = Some(result);
        row.phase = SpLiquidationV2LocalPhase::CollateralPending;
        Ok(())
    }

    /// Persist the single backend-authorized collateral payout successor before
    /// acknowledging it cross-canister. Legacy rows lacking the new generation
    /// field decode as `None` and are intentionally ineligible.
    pub fn adopt_sp_liquidation_v2_payout_supersession(
        &mut self,
        request_id: u64,
        stable_pull_receipt: &SpLiquidationStablePullReceipt,
        predecessor: SpLiquidationPayoutTuple,
        replacement: SpLiquidationPayoutTuple,
        result: &SpLiquidationV2SuccessWithFee,
        evidence: SpLiquidationPayoutNoEffectEvidence,
        generation: u32,
    ) -> Result<(), StabilityPoolError> {
        let row = self
            .pending_sp_liquidation_v2(request_id)
            .ok_or(StabilityPoolError::SystemBusy)?;

        // A lost accept reply can replay this exact transition. No other
        // generation or tuple pair is accepted.
        if row.payout_supersession_generation == Some(1)
            && row.payout_supersession_predecessor.as_ref() == Some(&predecessor)
            && row.payout_supersession_replacement.as_ref() == Some(&replacement)
            && row.payout_supersession_evidence.as_ref() == Some(&evidence)
            && row.payout_tuple.as_ref() == Some(&replacement)
        {
            return if row.stable_pull_receipt.as_ref() == Some(stable_pull_receipt)
                && row.result.as_ref().is_none_or(|saved| saved == result)
            {
                Ok(())
            } else {
                Err(StabilityPoolError::SystemBusy)
            };
        }

        let gross = predecessor.gross_amount_raw;
        let replacement_total = replacement
            .net_amount_raw
            .checked_add(replacement.fee_raw)
            .ok_or(StabilityPoolError::SystemBusy)?;
        if generation != 1
            || row.payout_supersession_generation != Some(0)
            || row.payout_supersession_predecessor.is_some()
            || row.payout_supersession_replacement.is_some()
            || row.payout_supersession_evidence.is_some()
            || row.payout_candidate_block_index.is_some()
            || row.payout_receipt.is_some()
            || !row.approval.fee_accounted
            || row
                .payout_tuple
                .as_ref()
                .is_some_and(|saved| saved != &predecessor)
            || row
                .stable_pull_receipt
                .as_ref()
                .is_some_and(|saved| saved != stable_pull_receipt)
            || row.result.as_ref().is_some_and(|saved| saved != result)
            || stable_pull_receipt.tuple.ledger != row.stablecoin_ledger
            || stable_pull_receipt.tuple.from != row.approval.owner
            || stable_pull_receipt.tuple.spender != row.approval.spender
            || stable_pull_receipt.tuple.to != stable_pull_receipt.tuple.spender
            || stable_pull_receipt.tuple.amount_raw == 0
            || stable_pull_receipt.tuple.amount_raw > row.request.amount
            || row
                .stable_pull_tuple
                .as_ref()
                .is_some_and(|saved| saved != &stable_pull_receipt.tuple)
            || row
                .stable_pull_candidate_block_index
                .is_some_and(|saved| saved != stable_pull_receipt.block_index)
            || result.collateral_amount_received != Some(gross)
            || result.xrp_claim_id.is_some()
            || row.collateral_type != predecessor.collateral_type
            || replacement.ledger != predecessor.ledger
            || replacement.source != predecessor.source
            || replacement.destination != predecessor.destination
            || replacement.collateral_type != predecessor.collateral_type
            || replacement.gross_amount_raw != gross
            || replacement.net_amount_raw == 0
            || replacement_total != gross
            || replacement == predecessor
            || replacement.op_nonce <= predecessor.op_nonce
            || replacement.memo
                != rumi_protocol_backend::management::nonce_to_memo(replacement.op_nonce)
                    .0
                    .to_vec()
            || replacement.created_at_time_ns
                != rumi_protocol_backend::management::nonce_to_created_at_time(replacement.op_nonce)
        {
            return Err(StabilityPoolError::SystemBusy);
        }

        match &evidence {
            SpLiquidationPayoutNoEffectEvidence::BadFee { expected_fee_raw }
                if replacement.fee_raw == *expected_fee_raw => {}
            SpLiquidationPayoutNoEffectEvidence::InsufficientFunds { .. }
                if replacement.fee_raw == predecessor.fee_raw
                    && replacement.net_amount_raw == predecessor.net_amount_raw => {}
            _ => return Err(StabilityPoolError::SystemBusy),
        }

        // Prepare every fallible accounting operation before committing the
        // tuple/evidence transition. First observation applies the debit using
        // the successor net; an already-debited predecessor keeps its stable
        // debits and recomputes only the collateral liability from saved shares.
        let already_debited = row.stable_debit_applied;
        let rebased_allocations = if already_debited {
            if row.stable_pull_receipt.as_ref() != Some(stable_pull_receipt)
                || row.payout_tuple.as_ref() != Some(&predecessor)
                || row.result.as_ref() != Some(result)
                || row.stable_principal_debits.is_empty()
                || row
                    .pending_collateral_allocations
                    .values()
                    .try_fold(0u64, |sum, amount| sum.checked_add(*amount))
                    != Some(predecessor.net_amount_raw)
            {
                return Err(StabilityPoolError::SystemBusy);
            }
            let weights: Vec<(Principal, u64)> = row
                .stable_principal_debits
                .iter()
                .filter_map(|(owner, amount)| (*amount > 0).then_some((*owner, *amount)))
                .collect();
            let allocations = exact_weight_allocations(&weights, replacement.net_amount_raw)?;
            if allocations
                .values()
                .try_fold(0u64, |sum, amount| sum.checked_add(*amount))
                != Some(replacement.net_amount_raw)
            {
                return Err(StabilityPoolError::SystemBusy);
            }
            Some(allocations)
        } else {
            None
        };

        if !already_debited {
            // This method precomputes all allocations and checks before its
            // synchronous commit; no fallible work follows a successful call.
            self.apply_sp_liquidation_v2_stable_receipt(
                request_id,
                stable_pull_receipt.clone(),
                replacement.clone(),
                result.clone(),
            )?;
        } else {
            let row = self
                .pending_sp_liquidations_v2
                .as_mut()
                .and_then(|rows| rows.get_mut(&request_id))
                .expect("row was validated before synchronous commit");
            row.pending_collateral_allocations = rebased_allocations
                .expect("already-debited branch prepared collateral allocations");
            row.payout_tuple = Some(replacement.clone());
            row.phase = SpLiquidationV2LocalPhase::StableDebited;
        }
        let row = self
            .pending_sp_liquidations_v2
            .as_mut()
            .and_then(|rows| rows.get_mut(&request_id))
            .expect("row was validated before synchronous commit");
        row.payout_supersession_generation = Some(generation);
        row.payout_supersession_predecessor = Some(predecessor);
        row.payout_supersession_replacement = Some(replacement);
        row.payout_supersession_evidence = Some(evidence);
        Ok(())
    }

    pub fn record_sp_liquidation_v2_stable_pull_candidate(
        &mut self,
        request_id: u64,
        tuple: SpLiquidationStablePullTuple,
        candidate_block_index: Option<u64>,
    ) -> Result<(), StabilityPoolError> {
        let row = self
            .pending_sp_liquidations_v2
            .as_mut()
            .and_then(|rows| rows.get_mut(&request_id))
            .ok_or(StabilityPoolError::SystemBusy)?;
        if tuple.ledger != row.stablecoin_ledger
            || tuple.from != row.approval.owner
            || tuple.spender != row.approval.spender
            || tuple.to != tuple.spender
            || tuple.amount_raw > row.request.amount
            || tuple
                .amount_raw
                .checked_add(tuple.fee_raw)
                .is_none_or(|total| total > row.approval.allowance_raw)
            || row
                .stable_pull_tuple
                .as_ref()
                .is_some_and(|saved| saved != &tuple)
            || row.stable_pull_candidate_block_index.is_some_and(|saved| {
                candidate_block_index.is_some_and(|candidate| candidate != saved)
            })
        {
            return Err(StabilityPoolError::SystemBusy);
        }
        row.stable_pull_tuple = Some(tuple);
        if let Some(index) = candidate_block_index {
            row.stable_pull_candidate_block_index = Some(index);
        }
        Ok(())
    }

    pub fn mark_sp_liquidation_v2_error(
        &mut self,
        request_id: u64,
        error: String,
    ) -> Result<(), StabilityPoolError> {
        let row = self
            .pending_sp_liquidations_v2
            .as_mut()
            .and_then(|rows| rows.get_mut(&request_id))
            .ok_or(StabilityPoolError::SystemBusy)?;
        row.last_error = Some(error.chars().take(512).collect());
        row.ambiguous_seen = true;
        Ok(())
    }

    pub fn record_sp_liquidation_v2_payout_receipt(
        &mut self,
        request_id: u64,
        receipt: SpLiquidationPayoutReceipt,
    ) -> Result<(), StabilityPoolError> {
        let row = self
            .pending_sp_liquidations_v2
            .as_mut()
            .and_then(|rows| rows.get_mut(&request_id))
            .ok_or(StabilityPoolError::SystemBusy)?;
        if row.payout_tuple.as_ref() != Some(&receipt.tuple)
            || row
                .payout_candidate_block_index
                .is_some_and(|index| index != receipt.block_index)
            || row
                .payout_receipt
                .as_ref()
                .is_some_and(|saved| saved != &receipt)
        {
            return Err(StabilityPoolError::SystemBusy);
        }
        row.payout_candidate_block_index = Some(receipt.block_index);
        row.payout_receipt = Some(receipt);
        Ok(())
    }

    pub fn record_sp_liquidation_v2_refund_tuple(
        &mut self,
        request_id: u64,
        tuple: SpLiquidationStableRefundTuple,
        candidate_block_index: Option<u64>,
    ) -> Result<(), StabilityPoolError> {
        let row = self
            .pending_sp_liquidations_v2
            .as_mut()
            .and_then(|rows| rows.get_mut(&request_id))
            .ok_or(StabilityPoolError::SystemBusy)?;
        let stable_receipt = row.stable_pull_receipt.as_ref();
        let expected_principal = stable_receipt.map_or(0, |receipt| receipt.tuple.amount_raw);
        let expected_pull_fee = stable_receipt.map_or(0, |receipt| receipt.tuple.fee_raw);
        let expected_approval_fee = row.approval.fee_raw;
        let expected_total = expected_principal
            .checked_add(expected_pull_fee)
            .and_then(|value| value.checked_add(expected_approval_fee))
            .ok_or(StabilityPoolError::SystemBusy)?;
        if tuple.ledger != row.stablecoin_ledger
            || tuple.principal_refund_raw != expected_principal
            || tuple.pull_fee_refund_raw != expected_pull_fee
            || tuple.approval_fee_refund_raw != expected_approval_fee
            || tuple.amount_raw != expected_total
            || row
                .stable_refund_tuple
                .as_ref()
                .is_some_and(|saved| saved != &tuple)
        {
            return Err(StabilityPoolError::SystemBusy);
        }
        row.stable_refund_tuple = Some(tuple);
        row.stable_refund_candidate_block_index = candidate_block_index;
        Ok(())
    }

    /// Restore the exact depositor debits only after the direct ICRC-3 refund
    /// tuple has been independently verified. Allocations are immutable
    /// snapshots from the original debits; current balances are never used to
    /// recompute who receives reimbursement.
    pub fn apply_sp_liquidation_v2_refund_receipt(
        &mut self,
        request_id: u64,
        receipt: SpLiquidationStableRefundReceipt,
    ) -> Result<(), StabilityPoolError> {
        if let Some(completed) = self
            .completed_sp_liquidations_v2
            .as_ref()
            .and_then(|rows| rows.get(&request_id))
        {
            return if completed.stable_refund_applied
                && completed.stable_refund_receipt.as_ref() == Some(&receipt)
            {
                Ok(())
            } else {
                Err(StabilityPoolError::SystemBusy)
            };
        }
        let row = self
            .pending_sp_liquidation_v2(request_id)
            .ok_or(StabilityPoolError::SystemBusy)?;
        if row.stable_refund_applied {
            return if row.stable_refund_receipt.as_ref() == Some(&receipt) {
                Ok(())
            } else {
                Err(StabilityPoolError::SystemBusy)
            };
        }
        let tuple = row
            .stable_refund_tuple
            .as_ref()
            .ok_or(StabilityPoolError::SystemBusy)?;
        if receipt.tuple != *tuple
            || row
                .stable_refund_candidate_block_index
                .is_some_and(|index| index != receipt.block_index)
            || self
                .completed_sp_liquidations_v2
                .as_ref()
                .is_none_or(|rows| rows.len() >= MAX_COMPLETED_SP_LIQUIDATIONS_V2)
            || self
                .pending_sp_liquidations_v2
                .as_ref()
                .is_none_or(|rows| !rows.contains_key(&request_id))
        {
            return Err(StabilityPoolError::SystemBusy);
        }
        let approval_total = row
            .approval_fee_debits
            .values()
            .try_fold(0u64, |sum, value| sum.checked_add(*value));
        let pull_fee_total = row
            .stable_pull_fee_debits
            .values()
            .try_fold(0u64, |sum, value| sum.checked_add(*value));
        let principal_total = row
            .stable_principal_debits
            .values()
            .try_fold(0u64, |sum, value| sum.checked_add(*value));
        if approval_total != Some(tuple.approval_fee_refund_raw)
            || pull_fee_total != Some(tuple.pull_fee_refund_raw)
            || principal_total != Some(tuple.principal_refund_raw)
        {
            return Err(StabilityPoolError::SystemBusy);
        }
        let tracked = self.ensure_stablecoin_aggregate_matches_positions(row.stablecoin_ledger)?;
        let new_total = tracked
            .checked_add(tuple.amount_raw)
            .ok_or(StabilityPoolError::SystemBusy)?;
        let mut restore = BTreeMap::<Principal, u64>::new();
        for allocations in [
            &row.approval_fee_debits,
            &row.stable_pull_fee_debits,
            &row.stable_principal_debits,
        ] {
            for (owner, amount) in allocations {
                let current = restore.get(owner).copied().unwrap_or(0);
                restore.insert(
                    *owner,
                    current
                        .checked_add(*amount)
                        .ok_or(StabilityPoolError::SystemBusy)?,
                );
            }
        }
        let mut updates = Vec::with_capacity(restore.len());
        for (owner, amount) in restore {
            let current = self
                .deposits
                .get(&owner)
                .and_then(|position| {
                    position
                        .stablecoin_balances
                        .get(&row.stablecoin_ledger)
                        .copied()
                })
                .unwrap_or(0);
            updates.push((
                owner,
                current
                    .checked_add(amount)
                    .ok_or(StabilityPoolError::SystemBusy)?,
            ));
        }
        let ledger = row.stablecoin_ledger;
        for (owner, balance) in updates {
            self.deposits
                .entry(owner)
                .or_insert_with(|| DepositPosition::new(0))
                .stablecoin_balances
                .insert(ledger, balance);
        }
        self.total_stablecoin_balances.insert(ledger, new_total);
        let pending = self
            .pending_sp_liquidations_v2
            .as_mut()
            .expect("pending refund row was prevalidated");
        let mut completed_row = pending
            .remove(&request_id)
            .expect("pending refund row was prevalidated");
        completed_row.stable_refund_receipt = Some(receipt);
        completed_row.stable_refund_applied = true;
        completed_row.phase = SpLiquidationV2LocalPhase::Rejected;
        self.completed_sp_liquidations_v2
            .as_mut()
            .expect("completed map was prevalidated")
            .insert(request_id, completed_row);
        Ok(())
    }

    /// V2 ledger fees are real pool debits. Unlike the legacy helper, this
    /// allocates every raw fee unit across depositor positions and the tracked
    /// aggregate, assigning rounding remainder deterministically.
    fn deduct_exact_pool_fee(
        &mut self,
        token_ledger: Principal,
        fee: u64,
    ) -> Result<BTreeMap<Principal, u64>, StabilityPoolError> {
        self.ensure_stablecoin_aggregate_matches_positions(token_ledger)?;
        let tracked = self
            .total_stablecoin_balances
            .get(&token_ledger)
            .copied()
            .unwrap_or(0);
        let new_total = tracked
            .checked_sub(fee)
            .ok_or(StabilityPoolError::InsufficientPoolBalance)?;
        let balances: Vec<(Principal, u64)> = self
            .deposits
            .iter()
            .filter_map(|(owner, pos)| {
                pos.stablecoin_balances
                    .get(&token_ledger)
                    .copied()
                    .filter(|balance| *balance > 0)
                    .map(|balance| (*owner, balance))
            })
            .collect();
        let allocations = exact_proportional_debit_allocations(&balances, fee)?;
        for (owner, debit) in &allocations {
            let balance = self
                .deposits
                .get(owner)
                .and_then(|pos| pos.stablecoin_balances.get(&token_ledger).copied())
                .ok_or(StabilityPoolError::SystemBusy)?;
            if balance < *debit {
                return Err(StabilityPoolError::InsufficientPoolBalance);
            }
        }

        for (owner, debit) in &allocations {
            let pos = self
                .deposits
                .get_mut(&owner)
                .expect("fee allocation owner was prevalidated");
            let balance = pos
                .stablecoin_balances
                .get_mut(&token_ledger)
                .expect("fee allocation balance was prevalidated");
            *balance -= *debit;
            if *balance == 0 {
                pos.stablecoin_balances.remove(&token_ledger);
            }
        }
        self.total_stablecoin_balances
            .insert(token_ledger, new_total);
        Ok(allocations)
    }

    fn ensure_stablecoin_aggregate_matches_positions(
        &self,
        token_ledger: Principal,
    ) -> Result<u64, StabilityPoolError> {
        let positions_total = self.deposits.values().try_fold(0u64, |sum, pos| {
            sum.checked_add(
                pos.stablecoin_balances
                    .get(&token_ledger)
                    .copied()
                    .unwrap_or(0),
            )
            .ok_or(StabilityPoolError::SystemBusy)
        })?;
        let tracked = self
            .total_stablecoin_balances
            .get(&token_ledger)
            .copied()
            .unwrap_or(0);
        if positions_total != tracked {
            return Err(StabilityPoolError::SystemBusy);
        }
        Ok(tracked)
    }

    /// Apply the stable pull exactly once and retain per-depositor collateral
    /// allocations as a liability until the payout block is independently
    /// verified by the SP.
    pub fn apply_sp_liquidation_v2_stable_receipt(
        &mut self,
        request_id: u64,
        stable_receipt: SpLiquidationStablePullReceipt,
        payout_tuple: SpLiquidationPayoutTuple,
        result: SpLiquidationV2SuccessWithFee,
    ) -> Result<(), StabilityPoolError> {
        let row = self
            .pending_sp_liquidation_v2(request_id)
            .ok_or(StabilityPoolError::SystemBusy)?;
        if row.stable_debit_applied {
            if row.stable_pull_receipt.as_ref() != Some(&stable_receipt)
                || row.payout_tuple.as_ref() != Some(&payout_tuple)
                || row.result.as_ref() != Some(&result)
            {
                return Err(StabilityPoolError::SystemBusy);
            }
            return Ok(());
        }
        let transfer_total = payout_tuple
            .net_amount_raw
            .checked_add(payout_tuple.fee_raw)
            .ok_or(StabilityPoolError::SystemBusy)?;
        if !row.approval.fee_accounted
            || stable_receipt.tuple.ledger != row.stablecoin_ledger
            || stable_receipt.tuple.to != stable_receipt.tuple.spender
            || stable_receipt.tuple.amount_raw > row.request.amount
            || row
                .stable_pull_tuple
                .as_ref()
                .is_some_and(|tuple| tuple != &stable_receipt.tuple)
            || row
                .stable_pull_candidate_block_index
                .is_some_and(|index| index != stable_receipt.block_index)
            || payout_tuple.collateral_type != row.collateral_type
            || payout_tuple.net_amount_raw == 0
            || transfer_total != payout_tuple.gross_amount_raw
            || result.collateral_amount_received != Some(payout_tuple.gross_amount_raw)
        {
            return Err(StabilityPoolError::SystemBusy);
        }

        let stable_ledger = row.stablecoin_ledger;
        let stable_amount = stable_receipt.tuple.amount_raw;
        let stable_fee = stable_receipt.tuple.fee_raw;
        if stable_amount == 0
            || stable_receipt.tuple.from != row.approval.owner
            || stable_receipt.tuple.spender != row.approval.spender
        {
            return Err(StabilityPoolError::SystemBusy);
        }
        let tracked = self.ensure_stablecoin_aggregate_matches_positions(stable_ledger)?;
        let total_expense = stable_amount
            .checked_add(stable_fee)
            .ok_or(StabilityPoolError::SystemBusy)?;
        if tracked < total_expense {
            return Err(StabilityPoolError::InsufficientPoolBalance);
        }
        let all_balances: Vec<(Principal, u64)> = self
            .deposits
            .iter()
            .filter_map(|(owner, pos)| {
                pos.stablecoin_balances
                    .get(&stable_ledger)
                    .copied()
                    .filter(|balance| *balance > 0)
                    .map(|balance| (*owner, balance))
            })
            .collect();
        let fee_allocations = exact_proportional_debit_allocations(&all_balances, stable_fee)?;
        let mut post_fee_balances = BTreeMap::new();
        for (owner, balance) in &all_balances {
            let fee_share = fee_allocations.get(owner).copied().unwrap_or(0);
            post_fee_balances.insert(
                *owner,
                balance
                    .checked_sub(fee_share)
                    .ok_or(StabilityPoolError::SystemBusy)?,
            );
        }
        let eligible_balances: Vec<(Principal, u64)> = post_fee_balances
            .iter()
            .filter_map(|(owner, balance)| {
                let opted_in = self
                    .deposits
                    .get(owner)
                    .is_some_and(|pos| self.position_opted_in_for(pos, &row.collateral_type));
                (opted_in && *balance > 0).then_some((*owner, *balance))
            })
            .collect();
        let principal_allocations =
            exact_proportional_debit_allocations(&eligible_balances, stable_amount)?;
        let mut combined_debits = fee_allocations.clone();
        for (owner, amount) in &principal_allocations {
            let combined = combined_debits
                .get(owner)
                .copied()
                .unwrap_or(0)
                .checked_add(*amount)
                .ok_or(StabilityPoolError::SystemBusy)?;
            combined_debits.insert(*owner, combined);
        }
        for (owner, debit) in &combined_debits {
            let original_balance = self
                .deposits
                .get(owner)
                .and_then(|pos| pos.stablecoin_balances.get(&stable_ledger).copied())
                .ok_or(StabilityPoolError::SystemBusy)?;
            if original_balance < *debit {
                return Err(StabilityPoolError::InsufficientPoolBalance);
            }
        }
        let allocation_weights: Vec<(Principal, u64)> = principal_allocations
            .iter()
            .filter_map(|(owner, amount)| (*amount > 0).then_some((*owner, *amount)))
            .collect();
        let collateral_allocations =
            exact_weight_allocations(&allocation_weights, payout_tuple.net_amount_raw)?;
        if collateral_allocations
            .values()
            .try_fold(0u64, |sum, value| sum.checked_add(*value))
            != Some(payout_tuple.net_amount_raw)
        {
            return Err(StabilityPoolError::SystemBusy);
        }
        let new_total = tracked
            .checked_sub(total_expense)
            .ok_or(StabilityPoolError::InsufficientPoolBalance)?;

        // All validation and exact allocations are complete. From here through
        // journal update there are no awaits or recoverable errors.
        for (owner, debit) in combined_debits {
            let pos = self
                .deposits
                .get_mut(&owner)
                .expect("planned liquidation debit owner was prevalidated");
            let balance = pos
                .stablecoin_balances
                .get_mut(&stable_ledger)
                .expect("planned liquidation balance was prevalidated");
            *balance -= debit;
            if *balance == 0 {
                pos.stablecoin_balances.remove(&stable_ledger);
            }
        }
        self.total_stablecoin_balances
            .insert(stable_ledger, new_total);
        let saved = self
            .pending_sp_liquidations_v2
            .as_mut()
            .and_then(|rows| rows.get_mut(&request_id))
            .expect("pending liquidation row was prevalidated before synchronous commit");
        saved.stable_pull_receipt = Some(stable_receipt);
        saved.payout_tuple = Some(payout_tuple);
        saved.result = Some(result);
        saved.stable_debit_applied = true;
        saved.pending_collateral_allocations = collateral_allocations;
        saved.stable_pull_fee_debits = fee_allocations;
        saved.stable_principal_debits = principal_allocations;
        saved.phase = SpLiquidationV2LocalPhase::StableDebited;
        Ok(())
    }

    /// Account a positively proved stable pull when the backend has entered a
    /// refund path instead of producing a collateral payout. Exact share maps
    /// are persisted so a later protocol-paid refund restores the same owners.
    pub fn apply_sp_liquidation_v2_refundable_stable_pull(
        &mut self,
        request_id: u64,
        receipt: SpLiquidationStablePullReceipt,
    ) -> Result<(), StabilityPoolError> {
        let row = self
            .pending_sp_liquidation_v2(request_id)
            .ok_or(StabilityPoolError::SystemBusy)?;
        if row.stable_debit_applied {
            return if row.stable_pull_receipt.as_ref() == Some(&receipt) {
                Ok(())
            } else {
                Err(StabilityPoolError::SystemBusy)
            };
        }
        if !row.approval.fee_accounted
            || row.backend_request.is_none()
            || receipt.tuple.ledger != row.stablecoin_ledger
            || receipt.tuple.to != receipt.tuple.spender
            || receipt.tuple.amount_raw > row.request.amount
            || row
                .stable_pull_tuple
                .as_ref()
                .is_some_and(|tuple| tuple != &receipt.tuple)
            || row
                .stable_pull_candidate_block_index
                .is_some_and(|index| index != receipt.block_index)
            || receipt.tuple.from != row.approval.owner
            || receipt.tuple.spender != row.approval.spender
            || receipt.tuple.amount_raw == 0
        {
            return Err(StabilityPoolError::SystemBusy);
        }
        let tracked = self.ensure_stablecoin_aggregate_matches_positions(row.stablecoin_ledger)?;
        let total_expense = receipt
            .tuple
            .amount_raw
            .checked_add(receipt.tuple.fee_raw)
            .ok_or(StabilityPoolError::SystemBusy)?;
        if tracked < total_expense {
            return Err(StabilityPoolError::InsufficientPoolBalance);
        }
        let all_balances: Vec<(Principal, u64)> = self
            .deposits
            .iter()
            .filter_map(|(owner, position)| {
                position
                    .stablecoin_balances
                    .get(&row.stablecoin_ledger)
                    .copied()
                    .filter(|balance| *balance > 0)
                    .map(|balance| (*owner, balance))
            })
            .collect();
        let fee_debits =
            exact_proportional_debit_allocations(&all_balances, receipt.tuple.fee_raw)?;
        let remaining: Vec<(Principal, u64)> = all_balances
            .iter()
            .filter_map(|(owner, balance)| {
                let fee = fee_debits.get(owner).copied().unwrap_or(0);
                let eligible = self.deposits.get(owner).is_some_and(|position| {
                    self.position_opted_in_for(position, &row.collateral_type)
                });
                let after_fee = balance - fee;
                (eligible && after_fee > 0).then_some((*owner, after_fee))
            })
            .collect();
        let principal_debits =
            exact_proportional_debit_allocations(&remaining, receipt.tuple.amount_raw)?;
        let mut combined = fee_debits.clone();
        for (owner, amount) in &principal_debits {
            let total = combined
                .get(owner)
                .copied()
                .unwrap_or(0)
                .checked_add(*amount)
                .ok_or(StabilityPoolError::SystemBusy)?;
            combined.insert(*owner, total);
        }
        let new_total = tracked
            .checked_sub(total_expense)
            .ok_or(StabilityPoolError::SystemBusy)?;
        for (owner, debit) in &combined {
            let balance = self
                .deposits
                .get(owner)
                .and_then(|position| {
                    position
                        .stablecoin_balances
                        .get(&row.stablecoin_ledger)
                        .copied()
                })
                .ok_or(StabilityPoolError::SystemBusy)?;
            if balance < *debit {
                return Err(StabilityPoolError::InsufficientPoolBalance);
            }
        }
        // All arithmetic and target lookups are validated before mutation.
        for (owner, debit) in combined {
            let position = self
                .deposits
                .get_mut(&owner)
                .expect("debit owner was prevalidated");
            let balance = position
                .stablecoin_balances
                .get_mut(&row.stablecoin_ledger)
                .expect("debit balance was prevalidated");
            *balance -= debit;
            if *balance == 0 {
                position.stablecoin_balances.remove(&row.stablecoin_ledger);
            }
        }
        self.total_stablecoin_balances
            .insert(row.stablecoin_ledger, new_total);
        let saved = self
            .pending_sp_liquidations_v2
            .as_mut()
            .and_then(|rows| rows.get_mut(&request_id))
            .expect("pending request was prevalidated before synchronous commit");
        saved.stable_pull_receipt = Some(receipt);
        saved.stable_pull_fee_debits = fee_debits;
        saved.stable_principal_debits = principal_debits;
        saved.stable_debit_applied = true;
        saved.phase = SpLiquidationV2LocalPhase::StableDebited;
        Ok(())
    }

    pub fn finalize_sp_liquidation_v2_payout(
        &mut self,
        request_id: u64,
        timestamp: u64,
    ) -> Result<PendingSpLiquidationV2, StabilityPoolError> {
        if let Some(completed) = self
            .completed_sp_liquidations_v2
            .as_ref()
            .and_then(|rows| rows.get(&request_id))
        {
            return Ok(completed.clone());
        }
        let row = self
            .pending_sp_liquidation_v2(request_id)
            .ok_or(StabilityPoolError::SystemBusy)?;
        if !row.stable_debit_applied || row.pending_collateral_allocations.is_empty() {
            return Err(StabilityPoolError::SystemBusy);
        }
        if self
            .completed_sp_liquidations_v2
            .as_ref()
            .is_none_or(|rows| rows.len() >= MAX_COMPLETED_SP_LIQUIDATIONS_V2)
        {
            return Err(StabilityPoolError::SystemBusy);
        }
        let stable_receipt = row
            .stable_pull_receipt
            .as_ref()
            .ok_or(StabilityPoolError::SystemBusy)?;
        let payout = row
            .payout_tuple
            .as_ref()
            .ok_or(StabilityPoolError::SystemBusy)?;
        if row.payout_receipt.as_ref().map(|receipt| &receipt.tuple) != Some(payout) {
            return Err(StabilityPoolError::SystemBusy);
        }
        if row
            .pending_collateral_allocations
            .values()
            .try_fold(0u64, |sum, amount| sum.checked_add(*amount))
            != Some(payout.net_amount_raw)
        {
            return Err(StabilityPoolError::SystemBusy);
        }
        let mut gain_updates = Vec::new();
        for (owner, amount) in &row.pending_collateral_allocations {
            let existing = self
                .deposits
                .get(owner)
                .and_then(|pos| pos.collateral_gains.get(&row.collateral_type).copied())
                .unwrap_or(0);
            let updated = existing
                .checked_add(*amount)
                .ok_or(StabilityPoolError::SystemBusy)?;
            gain_updates.push((*owner, updated));
        }
        let mut stables_consumed = BTreeMap::new();
        stables_consumed.insert(row.stablecoin_ledger, stable_receipt.tuple.amount_raw);
        // Move the pending row to its completed tombstone before materializing
        // claims. Capacity and arithmetic were preflighted above, so no
        // recoverable exit can occur after this transition. An IC trap still
        // rolls the whole message back atomically.
        let completed = self.complete_sp_liquidation_v2(request_id)?;
        for (owner, updated) in gain_updates {
            let pos = self
                .deposits
                .entry(owner)
                .or_insert_with(|| DepositPosition::new(timestamp));
            pos.collateral_gains.insert(row.collateral_type, updated);
        }
        self.record_liquidation_in_history(PoolLiquidationRecord {
            vault_id: row.request.vault_id,
            timestamp,
            stables_consumed,
            collateral_gained: payout.net_amount_raw,
            collateral_type: row.collateral_type,
            depositors_count: row.pending_collateral_allocations.len() as u64,
            collateral_price_e8s: Some(row.collateral_price_e8s),
        });
        self.push_event_at(
            self.protocol_canister_id,
            PoolEventType::LiquidationExecuted {
                vault_id: row.request.vault_id,
                stables_consumed_e8s: stable_receipt.tuple.amount_raw,
                collateral_gained: payout.net_amount_raw,
                collateral_type: row.collateral_type,
                success: true,
            },
            timestamp,
        );
        Ok(completed)
    }

    pub fn prepare_collateral_payout(
        &mut self,
        caller: Principal,
        ledger: Principal,
        fee: u64,
        created_at_time_ns: u64,
        memo: Vec<u8>,
    ) -> Result<Option<PendingOutboundPayout>, StabilityPoolError> {
        if self
            .pending_outbound_payouts
            .as_ref()
            .ok_or(StabilityPoolError::SystemBusy)?
            .contains_key(&(caller, ledger))
        {
            return Err(StabilityPoolError::SystemBusy);
        }
        if self
            .pending_outbound_payouts
            .as_ref()
            .map_or(true, |p| p.len() >= MAX_PENDING_OUTBOUND_PAYOUTS)
        {
            return Err(StabilityPoolError::SystemBusy);
        }
        let gains = self
            .deposits
            .get(&caller)
            .and_then(|pos| pos.collateral_gains.get(&ledger).copied())
            .unwrap_or(0);
        if gains == 0 {
            return Ok(None);
        }
        if gains <= fee {
            return Err(StabilityPoolError::AmountTooLow {
                minimum_e8s: fee.saturating_add(1),
            });
        }
        let payout = PendingOutboundPayout {
            kind: OutboundPayoutKind::CollateralClaim,
            request_amount: gains,
            gross_amount: gains,
            transfer_amount: gains - fee,
            transfer_fee: fee,
            transfer_created_at_time_ns: created_at_time_ns,
            transfer_memo: memo,
            dispatch_in_flight: true,
            ambiguous_seen: false,
            last_error: None,
        };
        if let Some(pos) = self.deposits.get_mut(&caller) {
            if let Some(saved_gains) = pos.collateral_gains.get_mut(&ledger) {
                *saved_gains = saved_gains.saturating_sub(gains);
                if *saved_gains == 0 {
                    pos.collateral_gains.remove(&ledger);
                }
            }
        }
        self.pending_outbound_payouts
            .as_mut()
            .ok_or(StabilityPoolError::SystemBusy)?
            .insert((caller, ledger), payout.clone());
        Ok(Some(payout))
    }

    pub fn begin_outbound_payout_retry(
        &mut self,
        caller: Principal,
        ledger: Principal,
    ) -> Result<PendingOutboundPayout, StabilityPoolError> {
        let payouts = self
            .pending_outbound_payouts
            .as_mut()
            .ok_or(StabilityPoolError::SystemBusy)?;
        let payout = payouts
            .get_mut(&(caller, ledger))
            .ok_or(StabilityPoolError::SystemBusy)?;
        if payout.dispatch_in_flight {
            return Err(StabilityPoolError::SystemBusy);
        }
        payout.dispatch_in_flight = true;
        Ok(payout.clone())
    }

    pub fn recover_interrupted_outbound_payout(&mut self, caller: Principal, ledger: Principal) {
        if let Some(payout) = self
            .pending_outbound_payouts
            .as_mut()
            .and_then(|payouts| payouts.get_mut(&(caller, ledger)))
        {
            if payout.dispatch_in_flight {
                payout.dispatch_in_flight = false;
                payout.ambiguous_seen = true;
                payout.last_error =
                    Some("dispatch continuation ended without a recorded result".into());
            }
        }
    }

    pub fn mark_outbound_payout_ambiguous(
        &mut self,
        caller: Principal,
        ledger: Principal,
        reason: String,
    ) {
        if let Some(payout) = self
            .pending_outbound_payouts
            .as_mut()
            .and_then(|payouts| payouts.get_mut(&(caller, ledger)))
        {
            payout.dispatch_in_flight = false;
            payout.ambiguous_seen = true;
            payout.last_error = Some(reason);
        }
    }

    /// Resolve a successful or Duplicate receipt exactly once.
    pub fn complete_outbound_payout(
        &mut self,
        caller: Principal,
        ledger: Principal,
        now_ns: u64,
    ) -> bool {
        let Some(payout) = self
            .pending_outbound_payouts
            .as_mut()
            .and_then(|payouts| payouts.remove(&(caller, ledger)))
        else {
            return false;
        };
        let (collateral_ledger, amount) = match payout.kind {
            OutboundPayoutKind::Withdraw => (None, payout.gross_amount),
            OutboundPayoutKind::CollateralClaim => (Some(ledger), payout.transfer_amount),
        };
        if let Some(collateral_ledger) = collateral_ledger {
            if let Some(pos) = self.deposits.get_mut(&caller) {
                *pos.total_claimed_gains
                    .entry(collateral_ledger)
                    .or_insert(0) += payout.gross_amount;
            }
            self.push_event_at(
                caller,
                PoolEventType::ClaimCollateral {
                    collateral_ledger,
                    amount,
                },
                now_ns,
            );
        } else {
            self.push_event_at(
                caller,
                PoolEventType::Withdraw {
                    token_ledger: ledger,
                    amount,
                },
                now_ns,
            );
        }
        true
    }

    /// A typed ledger rejection only restores funds when no earlier attempt
    /// had an ambiguous outcome. The row removal makes restoration idempotent.
    pub fn reject_outbound_payout_without_effect(
        &mut self,
        caller: Principal,
        ledger: Principal,
        reason: String,
        now_ns: u64,
    ) -> Result<bool, StabilityPoolError> {
        let payout = {
            let payouts = self
                .pending_outbound_payouts
                .as_mut()
                .ok_or(StabilityPoolError::SystemBusy)?;
            let Some(payout) = payouts.get(&(caller, ledger)).cloned() else {
                return Ok(false);
            };
            if payout.ambiguous_seen {
                if let Some(saved) = payouts.get_mut(&(caller, ledger)) {
                    saved.dispatch_in_flight = false;
                    saved.last_error =
                        Some(format!("typed rejection after prior ambiguity: {reason}"));
                }
                return Ok(false);
            }
            payouts.remove(&(caller, ledger));
            payout
        };
        match payout.kind {
            OutboundPayoutKind::Withdraw => {
                self.add_deposit_at(caller, ledger, payout.gross_amount, now_ns)
            }
            OutboundPayoutKind::CollateralClaim => {
                // A concurrent withdrawal on another ledger may have emptied
                // and removed the position while this claim was unresolved.
                // Recreate it so a definite first-attempt no-effect result
                // cannot destroy the reserved collateral gain.
                let pos = self
                    .deposits
                    .entry(caller)
                    .or_insert_with(|| DepositPosition::new(now_ns));
                *pos.collateral_gains.entry(ledger).or_insert(0) += payout.gross_amount;
            }
        }
        Ok(true)
    }

    pub fn add_deposit(&mut self, user: Principal, token_ledger: Principal, amount: u64) {
        self.add_deposit_at(user, token_ledger, amount, ic_cdk::api::time());
    }

    fn add_deposit_at(
        &mut self,
        user: Principal,
        token_ledger: Principal,
        amount: u64,
        now_ns: u64,
    ) {
        let position = self
            .deposits
            .entry(user)
            .or_insert_with(|| DepositPosition::new(now_ns));
        *position
            .stablecoin_balances
            .entry(token_ledger)
            .or_insert(0) += amount;
        *self
            .total_stablecoin_balances
            .entry(token_ledger)
            .or_insert(0) += amount;
    }

    /// Distribute icUSD interest revenue to eligible icUSD-holding depositors.
    /// Called by the backend after minting interest to the pool canister.
    ///
    /// Interest is distributed pro-rata based on each depositor's **icUSD balance
    /// only** (computed via `DepositPosition::icusd_value`). Depositors who hold
    /// only 3USD, ckUSDC, or ckUSDT do not earn from this stream (they still
    /// absorb liquidations pro-rata via the separate `compute_token_draw` path,
    /// but the protocol's borrowing-interest yield goes to icUSD holders only).
    ///
    /// When `collateral_type` is provided, depositors who have opted out of that
    /// collateral are excluded from the distribution (they should not earn
    /// interest from vaults backed by collateral they've opted out of).
    ///
    /// If no eligible depositor holds icUSD when this is called, the function
    /// records no credit. The mint remains in the canister's ICRC-1 balance and
    /// requires an explicit operator reconciliation policy; assigning it to a
    /// later depositor would create an unjustified windfall.
    pub fn distribute_interest_revenue(
        &mut self,
        token_ledger: Principal,
        amount: u64,
        collateral_type: Option<Principal>,
    ) {
        if amount == 0 {
            return;
        }

        let decimals = self
            .stablecoin_registry
            .get(&token_ledger)
            .map(|c| c.decimals)
            .unwrap_or(8);

        // Only icUSD-denominated balances earn the interest stream.
        // 3USD, ckUSDC, ckUSDT depositors still participate in liquidations
        // pro-rata but no longer earn the interest distribution.
        let holders: Vec<(Principal, u64)> = self
            .deposits
            .iter()
            .filter_map(|(p, pos)| {
                let icusd_value = pos.icusd_value(&self.stablecoin_registry);
                if icusd_value == 0 {
                    return None;
                }
                // If we know the collateral source, skip opted-out depositors
                if let Some(ct) = &collateral_type {
                    if !self.position_opted_in_for(pos, ct) {
                        return None;
                    }
                }
                Some((*p, icusd_value))
            })
            .collect();

        let eligible_total: u64 = holders.iter().map(|(_, b)| *b).sum();
        if eligible_total == 0 {
            log!(
                INFO,
                "WARN distribute_interest_revenue: {} of token {} received but no eligible icUSD depositor exists; explicit reconciliation required",
                amount,
                token_ledger,
            );
            return;
        }

        let mut distributed: u64 = 0;
        let mut first_eligible: Option<Principal> = None;

        for (principal, balance) in &holders {
            if first_eligible.is_none() {
                first_eligible = Some(*principal);
            }
            let credit = (amount as u128 * *balance as u128 / eligible_total as u128) as u64;
            if credit > 0 {
                if let Some(pos) = self.deposits.get_mut(principal) {
                    *pos.stablecoin_balances.entry(token_ledger).or_insert(0) += credit;
                    *pos.total_interest_earned_e8s.get_or_insert(0) +=
                        normalize_to_e8s(credit, decimals);
                }
                distributed += credit;
            }
        }

        // Assign rounding dust to first eligible depositor
        let dust = amount.saturating_sub(distributed);
        if dust > 0 {
            if let Some(first) = first_eligible {
                if let Some(pos) = self.deposits.get_mut(&first) {
                    *pos.stablecoin_balances.entry(token_ledger).or_insert(0) += dust;
                    *pos.total_interest_earned_e8s.get_or_insert(0) +=
                        normalize_to_e8s(dust, decimals);
                }
            }
        }

        // Update aggregate totals
        *self
            .total_stablecoin_balances
            .entry(token_ledger)
            .or_insert(0) += amount;
        *self.total_interest_received_e8s.get_or_insert(0) += normalize_to_e8s(amount, decimals);
    }

    /// Check every balance touched by an interest allocation before the
    /// non-fallible legacy distribution helper mutates any of them.
    pub fn validate_interest_distribution_overflow(
        &self,
        token_ledger: Principal,
        amount: u64,
        collateral_type: Option<Principal>,
    ) -> Result<(), StabilityPoolError> {
        if amount == 0 {
            return Ok(());
        }
        let decimals = self
            .stablecoin_registry
            .get(&token_ledger)
            .map(|config| config.decimals)
            .ok_or(StabilityPoolError::SystemBusy)?;
        let holders: Vec<(Principal, u64)> = self
            .deposits
            .iter()
            .filter_map(|(principal, pos)| {
                let weight = pos.icusd_value(&self.stablecoin_registry);
                (weight > 0
                    && collateral_type
                        .as_ref()
                        .is_none_or(|ct| self.position_opted_in_for(pos, ct)))
                .then_some((*principal, weight))
            })
            .collect();
        let eligible_total = holders.iter().try_fold(0u64, |total, (_, weight)| {
            total
                .checked_add(*weight)
                .ok_or(StabilityPoolError::SystemBusy)
        })?;
        if eligible_total == 0 {
            return Err(StabilityPoolError::SystemBusy);
        }
        let mut credits = BTreeMap::<Principal, u64>::new();
        let mut distributed = 0u64;
        for (principal, weight) in &holders {
            let credit = ((amount as u128) * (*weight as u128) / (eligible_total as u128)) as u64;
            distributed = distributed
                .checked_add(credit)
                .ok_or(StabilityPoolError::SystemBusy)?;
            credits.insert(*principal, credit);
        }
        let dust = amount
            .checked_sub(distributed)
            .ok_or(StabilityPoolError::SystemBusy)?;
        if dust > 0 {
            let first = holders
                .first()
                .map(|(principal, _)| *principal)
                .ok_or(StabilityPoolError::SystemBusy)?;
            let credit = credits
                .get(&first)
                .copied()
                .unwrap_or(0)
                .checked_add(dust)
                .ok_or(StabilityPoolError::SystemBusy)?;
            credits.insert(first, credit);
        }
        for (principal, credit) in credits {
            let pos = self
                .deposits
                .get(&principal)
                .ok_or(StabilityPoolError::SystemBusy)?;
            pos.stablecoin_balances
                .get(&token_ledger)
                .copied()
                .unwrap_or(0)
                .checked_add(credit)
                .ok_or(StabilityPoolError::SystemBusy)?;
            pos.total_interest_earned_e8s
                .unwrap_or(0)
                .checked_add(normalize_to_e8s(credit, decimals))
                .ok_or(StabilityPoolError::SystemBusy)?;
        }
        self.total_stablecoin_balances
            .get(&token_ledger)
            .copied()
            .unwrap_or(0)
            .checked_add(amount)
            .ok_or(StabilityPoolError::SystemBusy)?;
        self.total_interest_received_e8s
            .unwrap_or(0)
            .checked_add(normalize_to_e8s(amount, decimals))
            .ok_or(StabilityPoolError::SystemBusy)?;
        Ok(())
    }

    pub fn try_distribute_interest_revenue(
        &mut self,
        token_ledger: Principal,
        amount: u64,
        collateral_type: Option<Principal>,
    ) -> Result<(), StabilityPoolError> {
        self.validate_interest_distribution_overflow(token_ledger, amount, collateral_type)?;
        self.distribute_interest_revenue(token_ledger, amount, collateral_type);
        Ok(())
    }

    /// True when at least one icUSD depositor is eligible for interest from the
    /// supplied source collateral. This is intentionally the same predicate as
    /// `distribute_interest_revenue`, so an unallocated payment is routed to
    /// treasury only when no payout recipient exists at receipt time.
    pub fn has_eligible_interest_recipient(&self, collateral_type: Option<&Principal>) -> bool {
        self.deposits.values().any(|pos| {
            pos.icusd_value(&self.stablecoin_registry) > 0
                && collateral_type
                    .map(|ct| self.position_opted_in_for(pos, ct))
                    .unwrap_or(true)
        })
    }

    pub fn interest_mint_receipt_status(
        &self,
        source_mint_block: u64,
        payload: &InterestMintReceiptPayload,
    ) -> InterestMintReceiptStatus {
        if self.unallocated_interest_mint_index.is_none() {
            return InterestMintReceiptStatus::OutsideReplayWindow;
        }
        let forward_batch = self
            .unallocated_interest_mint_index
            .as_ref()
            .and_then(|index| index.get(&source_mint_block).copied());
        if let Some(accepted) = self
            .processed_interest_mint_payloads
            .as_ref()
            .and_then(|payloads| payloads.get(&source_mint_block))
        {
            if accepted != payload {
                return InterestMintReceiptStatus::PayloadMismatch;
            }
            return forward_batch
                .map(InterestMintReceiptStatus::PendingForward)
                .unwrap_or(InterestMintReceiptStatus::Duplicate);
        }
        // Pre-binding snapshots have only the block number. A replay cannot
        // prove it is the same ledger, amount, or collateral route.
        if forward_batch.is_some()
            || self
                .processed_interest_mint_blocks
                .as_ref()
                .is_some_and(|blocks| blocks.contains(&source_mint_block))
        {
            return InterestMintReceiptStatus::OutsideReplayWindow;
        }
        if let Some(high) = self.processed_interest_mint_block_high_watermark {
            let floor = high.saturating_sub(MAX_PROCESSED_INTEREST_MINT_BLOCKS as u64 - 1);
            if source_mint_block < floor {
                return InterestMintReceiptStatus::OutsideReplayWindow;
            }
        }
        InterestMintReceiptStatus::New
    }

    /// Atomically records a newly accepted source mint receipt. Caller must
    /// persist this in the same state mutation as applying the distribution.
    pub fn record_interest_mint_receipt(
        &mut self,
        source_mint_block: u64,
        payload: InterestMintReceiptPayload,
    ) -> InterestMintReceiptStatus {
        match self.interest_mint_receipt_status(source_mint_block, &payload) {
            InterestMintReceiptStatus::New => {}
            status => return status,
        }
        self.insert_interest_mint_receipt(source_mint_block, payload);
        InterestMintReceiptStatus::New
    }

    fn insert_interest_mint_receipt(
        &mut self,
        source_mint_block: u64,
        payload: InterestMintReceiptPayload,
    ) {
        let blocks = self
            .processed_interest_mint_blocks
            .get_or_insert_with(BTreeSet::new);
        blocks.insert(source_mint_block);
        self.processed_interest_mint_payloads
            .get_or_insert_with(BTreeMap::new)
            .insert(source_mint_block, payload);
        let high = self
            .processed_interest_mint_block_high_watermark
            .map_or(source_mint_block, |previous| {
                previous.max(source_mint_block)
            });
        self.processed_interest_mint_block_high_watermark = Some(high);
        let floor = high.saturating_sub(MAX_PROCESSED_INTEREST_MINT_BLOCKS as u64 - 1);
        while blocks.first().is_some_and(|oldest| *oldest < floor) {
            if let Some(oldest) = blocks.first().copied() {
                blocks.remove(&oldest);
                if let Some(payloads) = self.processed_interest_mint_payloads.as_mut() {
                    payloads.remove(&oldest);
                }
            }
        }
    }

    pub fn process_withdrawal(
        &mut self,
        user: Principal,
        token_ledger: Principal,
        amount: u64,
    ) -> Result<(), StabilityPoolError> {
        let position = self
            .deposits
            .get_mut(&user)
            .ok_or(StabilityPoolError::NoPositionFound)?;

        let balance = position
            .stablecoin_balances
            .get(&token_ledger)
            .copied()
            .unwrap_or(0);
        if balance < amount {
            return Err(StabilityPoolError::InsufficientBalance {
                token: token_ledger,
                required: amount,
                available: balance,
            });
        }

        // Safe subtraction: unwrap is justified for per-user balance (we just checked it exists
        // with balance >= amount above), but use saturating_sub for aggregate to be defensive.
        *position.stablecoin_balances.get_mut(&token_ledger).unwrap() -= amount;
        if let Some(total) = self.total_stablecoin_balances.get_mut(&token_ledger) {
            *total = total.saturating_sub(amount);
        }

        // Clean up zero balances
        if position.stablecoin_balances.get(&token_ledger) == Some(&0) {
            position.stablecoin_balances.remove(&token_ledger);
        }
        if position.is_empty() {
            self.deposits.remove(&user);
        }
        Ok(())
    }

    /// Proportionally debit a token balance across all depositors who hold it.
    ///
    /// **Not used by the liquidation flow.** The SP-001 fix (audit 2026-04-22-28e9896)
    /// removed this helper from `execute_single_liquidation`'s orchestration because
    /// it caused a double-deduction against `process_liquidation_gains_at`.
    ///
    /// Retained as an emergency operator tool for scenarios where token balances
    /// have been destroyed outside of a normal liquidation flow (e.g., a ledger
    /// migration quirk or an external reconciliation). `correct_balance` is the
    /// per-depositor-targeted analogue for surgical corrections.
    pub fn deduct_burned_lp_from_balances(&mut self, token_ledger: Principal, burned_amount: u64) {
        let total = self
            .total_stablecoin_balances
            .get(&token_ledger)
            .copied()
            .unwrap_or(0);
        if total == 0 || burned_amount == 0 {
            return;
        }
        let actual_deduct = burned_amount.min(total);

        // Distribute proportionally across depositors
        let depositors: Vec<(Principal, u64)> = self
            .deposits
            .iter()
            .filter_map(|(p, pos)| {
                let bal = pos
                    .stablecoin_balances
                    .get(&token_ledger)
                    .copied()
                    .unwrap_or(0);
                if bal > 0 {
                    Some((*p, bal))
                } else {
                    None
                }
            })
            .collect();

        let mut total_deducted = 0u64;
        for (principal, user_bal) in &depositors {
            let user_share = (*user_bal as u128 * actual_deduct as u128 / total as u128) as u64;
            let user_share = user_share.min(*user_bal);
            if let Some(pos) = self.deposits.get_mut(principal) {
                if let Some(bal) = pos.stablecoin_balances.get_mut(&token_ledger) {
                    *bal = bal.saturating_sub(user_share);
                }
            }
            total_deducted += user_share;
        }

        // Assign rounding dust to largest holder to prevent aggregate/individual drift
        let dust = actual_deduct.saturating_sub(total_deducted);
        if dust > 0 {
            if let Some(largest_p) = depositors
                .iter()
                .max_by_key(|(_, bal)| *bal)
                .map(|(p, _)| *p)
            {
                if let Some(pos) = self.deposits.get_mut(&largest_p) {
                    if let Some(bal) = pos.stablecoin_balances.get_mut(&token_ledger) {
                        *bal = bal.saturating_sub(dust);
                    }
                }
                total_deducted += dust;
            }
        }

        if let Some(agg) = self.total_stablecoin_balances.get_mut(&token_ledger) {
            *agg = agg.saturating_sub(total_deducted);
        }
    }

    /// Inverse of `deduct_burned_lp_from_balances`: proportionally credit a token
    /// balance back to depositors.
    ///
    /// **Not used by the liquidation flow.** After the SP-001 fix removed the
    /// pre-deduct pattern, there are no rollback sites that need this function.
    /// Retained as the symmetric operator tool alongside `deduct_burned_lp_from_balances`.
    pub fn credit_tokens_to_pool(&mut self, token_ledger: Principal, amount: u64) {
        if amount == 0 {
            return;
        }
        // Add back to aggregate
        *self
            .total_stablecoin_balances
            .entry(token_ledger)
            .or_insert(0) += amount;

        // Distribute proportionally across depositors who hold this token
        let holders: Vec<(Principal, u64)> = self
            .deposits
            .iter()
            .filter_map(|(p, pos)| {
                let bal = pos
                    .stablecoin_balances
                    .get(&token_ledger)
                    .copied()
                    .unwrap_or(0);
                if bal > 0 {
                    Some((*p, bal))
                } else {
                    None
                }
            })
            .collect();

        if holders.is_empty() {
            // Edge case: no holders, credit to first depositor
            if let Some((first_p, _)) = self.deposits.iter().next() {
                let first_p = *first_p;
                if let Some(pos) = self.deposits.get_mut(&first_p) {
                    *pos.stablecoin_balances.entry(token_ledger).or_insert(0) += amount;
                }
            }
            return;
        }

        let holder_total: u64 = holders.iter().map(|(_, b)| *b).sum();
        let mut credited = 0u64;
        for (principal, bal) in &holders {
            let share = (amount as u128 * *bal as u128 / holder_total as u128) as u64;
            if let Some(pos) = self.deposits.get_mut(principal) {
                *pos.stablecoin_balances.entry(token_ledger).or_insert(0) += share;
            }
            credited += share;
        }

        // Assign dust to first holder
        let dust = amount.saturating_sub(credited);
        if dust > 0 {
            if let Some((first_p, _)) = holders.first() {
                if let Some(pos) = self.deposits.get_mut(first_p) {
                    *pos.stablecoin_balances.entry(token_ledger).or_insert(0) += dust;
                }
            }
        }
    }

    // ─── Collateral Gains ───

    pub fn get_collateral_gains(&self, user: &Principal) -> BTreeMap<Principal, u64> {
        self.deposits
            .get(user)
            .map(|p| p.collateral_gains.clone())
            .unwrap_or_default()
    }

    pub fn mark_gains_claimed(
        &mut self,
        user: &Principal,
        collateral_ledger: &Principal,
        amount: u64,
    ) {
        if let Some(position) = self.deposits.get_mut(user) {
            if let Some(gains) = position.collateral_gains.get_mut(collateral_ledger) {
                *gains = gains.saturating_sub(amount);
                if *gains == 0 {
                    position.collateral_gains.remove(collateral_ledger);
                }
            }
            *position
                .total_claimed_gains
                .entry(*collateral_ledger)
                .or_insert(0) += amount;
        }
    }

    pub fn mark_cfx_claimed(&mut self, user: &Principal, chain_sentinel: &Principal, amount: u128) {
        if let Some(position) = self.deposits.get_mut(user) {
            if let Some(claims) = position.cfx_claims.as_mut() {
                if let Some(gains) = claims.get_mut(chain_sentinel) {
                    *gains = gains.saturating_sub(amount);
                    if *gains == 0 {
                        claims.remove(chain_sentinel);
                    }
                }
            }
        }
    }

    pub fn record_chain_claim_source(
        &mut self,
        chain_sentinel: Principal,
        claim_id: u64,
        amount_native: u128,
    ) {
        if amount_native == 0 {
            return;
        }
        let sources = self
            .chain_claim_sources
            .get_or_insert_with(BTreeMap::new)
            .entry(chain_sentinel)
            .or_default();
        if let Some(existing) = sources.iter_mut().find(|s| s.claim_id == claim_id) {
            existing.remaining_native = existing.remaining_native.saturating_add(amount_native);
        } else {
            sources.push(ChainClaimSource {
                claim_id,
                remaining_native: amount_native,
            });
        }
    }

    pub fn pending_chain_absorb_count(&self) -> usize {
        self.pending_chain_absorbs
            .as_ref()
            .map(|m| m.len())
            .unwrap_or(0)
    }

    pub fn has_pending_chain_absorbs(&self) -> bool {
        self.pending_chain_absorb_count() > 0
    }

    pub fn pending_native_xrp_absorb_count(&self) -> usize {
        self.pending_native_xrp_absorbs
            .as_ref()
            .map(|m| m.len())
            .unwrap_or(0)
    }

    pub fn has_pending_native_xrp_absorbs(&self) -> bool {
        self.pending_native_xrp_absorb_count() > 0
    }

    pub fn has_pending_pool_absorbs(&self) -> bool {
        self.has_pending_chain_absorbs() || self.has_pending_native_xrp_absorbs()
    }

    pub fn pending_chain_absorb_status(&self, vault_id: u64) -> Option<ChainSpAbsorbIntentStatus> {
        self.pending_chain_absorbs
            .as_ref()
            .and_then(|m| m.get(&vault_id))
            .map(|intent| intent.status)
    }

    pub fn pending_chain_absorbs(&self) -> Vec<ChainSpAbsorbIntent> {
        self.pending_chain_absorbs
            .as_ref()
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default()
    }

    pub fn completed_chain_absorbs(&self, limit: usize) -> Vec<ChainSpAbsorbCompletion> {
        self.completed_chain_absorbs
            .as_ref()
            .map(|m| m.values().rev().take(limit).cloned().collect())
            .unwrap_or_default()
    }

    pub fn get_pending_chain_absorb(&self, vault_id: u64) -> Option<ChainSpAbsorbIntent> {
        self.pending_chain_absorbs
            .as_ref()
            .and_then(|m| m.get(&vault_id).cloned())
    }

    pub fn put_pending_chain_absorb(
        &mut self,
        intent: ChainSpAbsorbIntent,
    ) -> Result<(), StabilityPoolError> {
        let pending = self.pending_chain_absorbs.get_or_insert_with(BTreeMap::new);
        if !pending.contains_key(&intent.vault_id) && pending.len() >= MAX_PENDING_CHAIN_ABSORBS {
            return Err(StabilityPoolError::SystemBusy);
        }
        pending.insert(intent.vault_id, intent);
        Ok(())
    }

    pub fn take_pending_chain_absorb(&mut self, vault_id: u64) -> Option<ChainSpAbsorbIntent> {
        self.pending_chain_absorbs
            .as_mut()
            .and_then(|m| m.remove(&vault_id))
    }

    pub fn pending_native_xrp_absorbs(&self) -> Vec<NativeXrpAbsorbIntent> {
        self.pending_native_xrp_absorbs
            .as_ref()
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default()
    }

    pub fn get_pending_native_xrp_absorb(&self, vault_id: u64) -> Option<NativeXrpAbsorbIntent> {
        self.pending_native_xrp_absorbs
            .as_ref()
            .and_then(|m| m.get(&vault_id).cloned())
    }

    pub fn put_pending_native_xrp_absorb(
        &mut self,
        intent: NativeXrpAbsorbIntent,
    ) -> Result<(), StabilityPoolError> {
        let pending = self
            .pending_native_xrp_absorbs
            .get_or_insert_with(BTreeMap::new);
        if !pending.contains_key(&intent.vault_id)
            && pending.len() >= MAX_PENDING_NATIVE_XRP_ABSORBS
        {
            return Err(StabilityPoolError::SystemBusy);
        }
        pending.insert(intent.vault_id, intent);
        Ok(())
    }

    pub fn take_pending_native_xrp_absorb(
        &mut self,
        vault_id: u64,
    ) -> Option<NativeXrpAbsorbIntent> {
        self.pending_native_xrp_absorbs
            .as_mut()
            .and_then(|m| m.remove(&vault_id))
    }

    pub fn record_completed_chain_absorb(&mut self, completion: ChainSpAbsorbCompletion) {
        let completed = self
            .completed_chain_absorbs
            .get_or_insert_with(BTreeMap::new);
        completed.insert(completion.vault_id, completion);
        while completed.len() > MAX_COMPLETED_CHAIN_ABSORBS {
            let Some(oldest_vault_id) = completed.keys().next().copied() else {
                break;
            };
            completed.remove(&oldest_vault_id);
        }
    }

    pub fn completed_chain_absorb(&self, vault_id: u64) -> Option<ChainSpAbsorbCompletion> {
        self.completed_chain_absorbs
            .as_ref()
            .and_then(|m| m.get(&vault_id).cloned())
    }

    pub fn completed_cfx_claim_payout_recovery(
        &self,
        key: &CfxClaimPayoutRecoveryKey,
    ) -> Option<CfxClaimPayoutRecoveryRecord> {
        self.completed_cfx_claim_payout_recoveries
            .as_ref()
            .and_then(|m| m.get(key).cloned())
    }

    pub fn completed_cfx_claim_payout_recovery_was_evicted(
        &self,
        key: &CfxClaimPayoutRecoveryKey,
    ) -> bool {
        self.completed_cfx_claim_payout_recovery_floor
            .as_ref()
            .and_then(|m| m.get(&key.chain_sentinel))
            .map(|floor| key.op_id <= *floor)
            .unwrap_or(false)
    }

    pub fn record_completed_cfx_claim_payout_recovery(
        &mut self,
        record: CfxClaimPayoutRecoveryRecord,
    ) {
        let completed = self
            .completed_cfx_claim_payout_recoveries
            .get_or_insert_with(BTreeMap::new);
        completed.insert(record.key.clone(), record);
        while completed.len() > MAX_COMPLETED_CFX_CLAIM_PAYOUT_RECOVERIES {
            let Some(oldest_key) = completed
                .iter()
                .min_by_key(|(key, record)| (record.recovered_at_ns, (*key).clone()))
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some(evicted) = completed.remove(&oldest_key) {
                let floors = self
                    .completed_cfx_claim_payout_recovery_floor
                    .get_or_insert_with(BTreeMap::new);
                let floor = floors.entry(evicted.key.chain_sentinel).or_insert(0);
                *floor = (*floor).max(evicted.key.op_id);
            }
        }
    }

    pub fn chain_absorb_auto_config(&self) -> ChainAbsorbAutoConfig {
        self.chain_absorb_auto_config.clone().unwrap_or_default()
    }

    pub fn set_chain_absorb_auto_config(
        &mut self,
        mut config: ChainAbsorbAutoConfig,
    ) -> Result<(), StabilityPoolError> {
        if config.interval_seconds < MIN_CHAIN_ABSORB_AUTO_INTERVAL_SECONDS {
            return Err(StabilityPoolError::LiquidationFailed {
                vault_id: 0,
                reason: format!(
                    "chain absorb auto interval must be at least {} seconds",
                    MIN_CHAIN_ABSORB_AUTO_INTERVAL_SECONDS
                ),
            });
        }
        if config.max_scan_per_chain == 0 {
            return Err(StabilityPoolError::LiquidationFailed {
                vault_id: 0,
                reason: "chain absorb auto max_scan_per_chain must be greater than 0".to_string(),
            });
        }
        config.max_scan_per_chain = config
            .max_scan_per_chain
            .min(MAX_CHAIN_ABSORB_AUTO_SCAN_PER_CHAIN);
        self.chain_absorb_auto_config = Some(config);
        Ok(())
    }

    pub fn chain_absorb_auto_last_tick(&self) -> Option<ChainAbsorbAutoTickRecord> {
        self.chain_absorb_auto_last_tick.clone()
    }

    pub fn record_chain_absorb_auto_tick(&mut self, tick: ChainAbsorbAutoTickRecord) {
        self.chain_absorb_auto_last_tick = Some(tick);
    }

    pub fn chain_absorb_auto_due(&self, now_ns: u64) -> bool {
        let config = self.chain_absorb_auto_config();
        if !config.enabled {
            return false;
        }
        let Some(last) = &self.chain_absorb_auto_last_tick else {
            return true;
        };
        let elapsed_ns = now_ns.saturating_sub(last.completed_at_ns);
        elapsed_ns >= config.interval_seconds.saturating_mul(1_000_000_000)
    }

    // ─── Opt-in / Opt-out ───

    pub fn opt_out_collateral(
        &mut self,
        user: &Principal,
        collateral_type: Principal,
    ) -> Result<(), StabilityPoolError> {
        if self.collateral_requires_payout_address(&collateral_type) {
            let position = self
                .deposits
                .get_mut(user)
                .ok_or(StabilityPoolError::NoPositionFound)?;
            let removed = position
                .native_payout_addresses
                .get_or_insert_with(BTreeMap::new)
                .remove(&collateral_type);
            position
                .native_payout_destination_tags
                .get_or_insert_with(BTreeMap::new)
                .remove(&collateral_type);
            if removed.is_none() {
                return Err(StabilityPoolError::AlreadyOptedOut {
                    collateral: collateral_type,
                });
            }
            return Ok(());
        }
        if self.is_chain_collateral_sentinel(&collateral_type) {
            return self.opt_out_cfx(user, collateral_type);
        }
        let position = self
            .deposits
            .get_mut(user)
            .ok_or(StabilityPoolError::NoPositionFound)?;
        if !position.opted_out_collateral.insert(collateral_type) {
            return Err(StabilityPoolError::AlreadyOptedOut {
                collateral: collateral_type,
            });
        }
        Ok(())
    }

    pub fn opt_in_collateral(
        &mut self,
        user: &Principal,
        collateral_type: Principal,
    ) -> Result<(), StabilityPoolError> {
        // Sunset collaterals (BOB, EXE, ...) are in wind-down. Existing
        // receiving positions may leave through `opt_out_collateral`, but
        // neither a new nor former participant may create fresh exposure.
        if sunset_collaterals().contains(&collateral_type) {
            return Err(StabilityPoolError::TokenNotActive {
                ledger: collateral_type,
            });
        }
        if self.collateral_requires_payout_address(&collateral_type) {
            return Err(StabilityPoolError::PayoutAddressRequired {
                collateral: collateral_type,
            });
        }
        if self.is_chain_collateral_sentinel(&collateral_type) {
            return self.opt_in_cfx(user, collateral_type);
        }
        let position = self
            .deposits
            .get_mut(user)
            .ok_or(StabilityPoolError::NoPositionFound)?;
        if !position.opted_out_collateral.remove(&collateral_type) {
            return Err(StabilityPoolError::AlreadyOptedIn {
                collateral: collateral_type,
            });
        }
        Ok(())
    }

    pub fn opt_in_cfx(
        &mut self,
        user: &Principal,
        sentinel: Principal,
    ) -> Result<(), StabilityPoolError> {
        if !self.is_chain_collateral_sentinel(&sentinel) {
            return Err(StabilityPoolError::CollateralNotFound { ledger: sentinel });
        }
        let position = self
            .deposits
            .get_mut(user)
            .ok_or(StabilityPoolError::NoPositionFound)?;
        let opted_in = position
            .opted_in_chain_collateral
            .get_or_insert_with(BTreeSet::new);
        if !opted_in.insert(sentinel) {
            return Err(StabilityPoolError::AlreadyOptedIn {
                collateral: sentinel,
            });
        }
        Ok(())
    }

    pub fn opt_out_cfx(
        &mut self,
        user: &Principal,
        sentinel: Principal,
    ) -> Result<(), StabilityPoolError> {
        if !self.is_chain_collateral_sentinel(&sentinel) {
            return Err(StabilityPoolError::CollateralNotFound { ledger: sentinel });
        }
        let position = self
            .deposits
            .get_mut(user)
            .ok_or(StabilityPoolError::NoPositionFound)?;
        let opted_in = position
            .opted_in_chain_collateral
            .get_or_insert_with(BTreeSet::new);
        if !opted_in.remove(&sentinel) {
            return Err(StabilityPoolError::AlreadyOptedOut {
                collateral: sentinel,
            });
        }
        Ok(())
    }

    pub fn opt_in_native_collateral(
        &mut self,
        user: &Principal,
        collateral_type: Principal,
        payout_address: String,
    ) -> Result<(), StabilityPoolError> {
        self.opt_in_native_collateral_with_tag(user, collateral_type, payout_address, None)
    }

    pub fn opt_in_native_collateral_with_tag(
        &mut self,
        user: &Principal,
        collateral_type: Principal,
        payout_address: String,
        destination_tag: Option<u32>,
    ) -> Result<(), StabilityPoolError> {
        if !self.collateral_requires_payout_address(&collateral_type) {
            return self.opt_in_collateral(user, collateral_type);
        }

        let address = payout_address.trim().to_string();
        rumi_protocol_backend::chains::xrp::address::account_id_from_classic_address(&address)
            .map_err(|reason| StabilityPoolError::InvalidPayoutAddress { reason })?;

        let position = self
            .deposits
            .get_mut(user)
            .ok_or(StabilityPoolError::NoPositionFound)?;
        position.opted_out_collateral.remove(&collateral_type);
        position
            .native_payout_addresses
            .get_or_insert_with(BTreeMap::new)
            .insert(collateral_type, address);
        let tags = position
            .native_payout_destination_tags
            .get_or_insert_with(BTreeMap::new);
        match destination_tag {
            Some(tag) => {
                tags.insert(collateral_type, tag);
            }
            None => {
                tags.remove(&collateral_type);
            }
        }
        Ok(())
    }

    pub fn build_native_xrp_payout_allocations(
        &self,
        collateral_type: Principal,
        stables_consumed: &BTreeMap<Principal, u64>,
        collateral_received_drops: u64,
    ) -> Result<Vec<NativeXrpPayoutAllocation>, StabilityPoolError> {
        if !self.collateral_requires_payout_address(&collateral_type) {
            return Err(StabilityPoolError::PayoutAddressRequired {
                collateral: collateral_type,
            });
        }
        if collateral_received_drops == 0 {
            return Err(StabilityPoolError::LiquidationFailed {
                vault_id: 0,
                reason: "native XRP allocation has zero drops".to_string(),
            });
        }

        let mut eligible_principals: Vec<Principal> = self
            .deposits
            .iter()
            .filter(|(_, pos)| self.position_opted_in_for(pos, &collateral_type))
            .filter(|(_, pos)| {
                stables_consumed
                    .keys()
                    .any(|token| pos.stablecoin_balances.get(token).copied().unwrap_or(0) > 0)
            })
            .map(|(principal, _)| *principal)
            .collect();
        eligible_principals.sort_by(|a, b| a.as_slice().cmp(b.as_slice()));
        if eligible_principals.is_empty() {
            return Err(StabilityPoolError::InsufficientPoolBalance);
        }

        let mut per_token_opted_in_totals: BTreeMap<Principal, u64> = BTreeMap::new();
        for token_ledger in stables_consumed.keys() {
            let total: u64 = eligible_principals
                .iter()
                .filter_map(|principal| self.deposits.get(principal))
                .map(|pos| {
                    pos.stablecoin_balances
                        .get(token_ledger)
                        .copied()
                        .unwrap_or(0)
                })
                .sum();
            per_token_opted_in_totals.insert(*token_ledger, total);
        }

        let vps = self.virtual_prices().clone();
        let registry_snapshot: BTreeMap<Principal, (u8, bool)> = stables_consumed
            .keys()
            .filter_map(|ledger| {
                self.stablecoin_registry
                    .get(ledger)
                    .map(|c| (*ledger, (c.decimals, c.is_lp_token.unwrap_or(false))))
            })
            .collect();
        let total_consumed_e8s: u64 = stables_consumed
            .iter()
            .map(|(ledger, &amount)| {
                let (decimals, is_lp) =
                    registry_snapshot.get(ledger).copied().unwrap_or((8, false));
                if is_lp {
                    vps.get(ledger)
                        .map(|&vp| lp_to_usd_e8s(amount, vp))
                        .unwrap_or(0)
                } else {
                    normalize_to_e8s(amount, decimals)
                }
            })
            .sum();
        if total_consumed_e8s == 0 {
            return Err(StabilityPoolError::InsufficientPoolBalance);
        }

        struct Candidate {
            allocation: NativeXrpPayoutAllocation,
            consumed_e8s: u64,
        }

        let mut candidates: Vec<Candidate> = Vec::new();
        for principal in eligible_principals {
            let Some(position) = self.deposits.get(&principal) else {
                continue;
            };
            let payout_address = position
                .native_payout_addresses
                .as_ref()
                .and_then(|addresses| addresses.get(&collateral_type))
                .cloned()
                .unwrap_or_default();
            if payout_address.trim().is_empty() {
                return Err(StabilityPoolError::PayoutAddressRequired {
                    collateral: collateral_type,
                });
            }

            let mut user_consumed_e8s: u64 = 0;
            for (token_ledger, &total_consumed) in stables_consumed {
                let total_opted_in = per_token_opted_in_totals
                    .get(token_ledger)
                    .copied()
                    .unwrap_or(0);
                if total_opted_in == 0 {
                    continue;
                }
                let user_balance = position
                    .stablecoin_balances
                    .get(token_ledger)
                    .copied()
                    .unwrap_or(0);
                if user_balance == 0 {
                    continue;
                }

                let user_share_native =
                    (total_consumed as u128 * user_balance as u128 / total_opted_in as u128) as u64;
                let user_share_native = user_share_native.min(user_balance);
                let (decimals, is_lp) = registry_snapshot
                    .get(token_ledger)
                    .copied()
                    .unwrap_or((8, false));
                let share_e8s = if is_lp {
                    vps.get(token_ledger)
                        .map(|&vp| lp_to_usd_e8s(user_share_native, vp))
                        .unwrap_or(0)
                } else {
                    normalize_to_e8s(user_share_native, decimals)
                };
                user_consumed_e8s = user_consumed_e8s.saturating_add(share_e8s);
            }

            candidates.push(Candidate {
                allocation: NativeXrpPayoutAllocation {
                    claimant: principal,
                    payout_address,
                    destination_tag: position
                        .native_payout_destination_tags
                        .as_ref()
                        .and_then(|tags| tags.get(&collateral_type).copied()),
                    drops: 0,
                },
                consumed_e8s: user_consumed_e8s,
            });
        }
        if candidates.is_empty() {
            return Err(StabilityPoolError::InsufficientPoolBalance);
        }

        let consumed_floor_total: u64 = candidates
            .iter()
            .map(|candidate| candidate.consumed_e8s)
            .sum();
        let consumed_dust = total_consumed_e8s.saturating_sub(consumed_floor_total);
        if consumed_dust > 0 {
            candidates[0].consumed_e8s = candidates[0].consumed_e8s.saturating_add(consumed_dust);
        }

        let mut allocations = Vec::new();
        let mut total_allocated: u64 = 0;
        for candidate in &candidates {
            let drops = (collateral_received_drops as u128 * candidate.consumed_e8s as u128
                / total_consumed_e8s as u128) as u64;
            if drops > 0 {
                let mut allocation = candidate.allocation.clone();
                allocation.drops = drops;
                total_allocated = total_allocated.saturating_add(drops);
                allocations.push(allocation);
            }
        }

        let dust = collateral_received_drops.saturating_sub(total_allocated);
        if dust > 0 {
            if let Some(first) = allocations.first_mut() {
                first.drops = first.drops.saturating_add(dust);
            } else if let Some(candidate) = candidates.first() {
                let mut allocation = candidate.allocation.clone();
                allocation.drops = dust;
                allocations.push(allocation);
            }
        }

        if allocations.len() > MAX_XRP_SP_PAYOUT_ALLOCATIONS {
            return Err(StabilityPoolError::LiquidationFailed {
                vault_id: 0,
                reason: format!(
                    "native XRP payout fanout {} exceeds max {}",
                    allocations.len(),
                    MAX_XRP_SP_PAYOUT_ALLOCATIONS
                ),
            });
        }
        Ok(allocations)
    }

    // ─── Pending Refunds (audit IC-S-001) ───

    /// Record tokens the pool owes `user` after a failed `deposit_as_3usd`
    /// refund so they can be recovered via `claim_pending_refund`. The payout
    /// preserves this full principal; a separately funded reserve pays fees.
    /// Returns the refund id. `now` is passed explicitly so the bookkeeping is
    /// testable without the IC runtime.
    pub fn record_pending_refund(
        &mut self,
        user: Principal,
        token_ledger: Principal,
        amount: u64,
        reason: String,
        now: u64,
    ) -> u64 {
        let refunds = self.pending_refunds.get_or_insert_with(BTreeMap::new);
        let id = self.next_pending_refund_id.unwrap_or(0);
        self.next_pending_refund_id = Some(id + 1);
        refunds.insert(
            id,
            PendingRefund {
                id,
                user,
                token_ledger,
                amount,
                reason,
                created_at: now,
                transfer_attempted: Some(false),
                transfer_created_at_time_ns: None,
                transfer_fee: None,
                transfer_memo: None,
                transfer_attempt_no: Some(0),
                transfer_too_old_rejected: None,
                transfer_history_scan_cursor: None,
                transfer_history_scan_tip: None,
                protocol_fee_reserved: None,
            },
        );
        id
    }

    /// Called on upgrade because newly added optional fields decode as `None`
    /// from older stable snapshots. It intentionally does not modify pending
    /// refund rows, whose absent attempt identity must remain held for evidence.
    pub fn normalize_pending_refund_fee_state(&mut self) {
        self.pending_refund_fee_reserves
            .get_or_insert_with(BTreeMap::new);
        self.pending_refund_fee_funding_blocks
            .get_or_insert_with(BTreeSet::new);
    }

    /// Credit fee capacity only once for an exact ledger funding receipt.
    pub fn credit_pending_refund_fee_reserve(
        &mut self,
        ledger: Principal,
        block_index: u64,
        amount: u64,
    ) -> Result<(), &'static str> {
        let receipts = self
            .pending_refund_fee_funding_blocks
            .as_mut()
            .ok_or("refund fee funding history is unavailable")?;
        if !receipts.insert((ledger, block_index)) {
            return Err("refund fee funding block was already credited");
        }
        let reserves = self
            .pending_refund_fee_reserves
            .as_mut()
            .ok_or("refund fee reserve state is unavailable")?;
        let current = reserves.get(&ledger).copied().unwrap_or(0);
        let Some(next) = current.checked_add(amount) else {
            receipts.remove(&(ledger, block_index));
            return Err("refund fee reserve balance overflow");
        };
        reserves.insert(ledger, next);
        Ok(())
    }

    /// Persist the exact full-principal transfer identity and reserve its fee
    /// before any ledger call. A missing dispatch marker is legacy ambiguity.
    pub fn prepare_pending_refund_transfer(
        &mut self,
        id: u64,
        fee: u64,
        created_at_time_ns: u64,
        memo: Vec<u8>,
    ) -> Result<PendingRefund, &'static str> {
        let refund = self
            .pending_refunds
            .as_mut()
            .and_then(|refunds| refunds.get_mut(&id))
            .ok_or("pending refund not found")?;
        match refund.transfer_attempted {
            None => return Err("legacy refund has unknown transfer history and remains held"),
            Some(true) => {
                if refund.transfer_created_at_time_ns.is_some()
                    && refund.transfer_fee.is_some()
                    && refund.transfer_memo.is_some()
                    && refund.protocol_fee_reserved == refund.transfer_fee
                {
                    return Ok(refund.clone());
                }
                return Err("pending refund transfer journal is incomplete and remains held");
            }
            Some(false) => {}
        }
        if refund.transfer_created_at_time_ns.is_some()
            || refund.transfer_fee.is_some()
            || refund.transfer_memo.is_some()
            || refund.protocol_fee_reserved.is_some()
        {
            return Err("unattempted refund unexpectedly contains a transfer identity");
        }
        let reserves = self
            .pending_refund_fee_reserves
            .as_mut()
            .ok_or("refund fee reserve state is unavailable")?;
        let available = reserves.get(&refund.token_ledger).copied().unwrap_or(0);
        if available < fee {
            return Err("protocol-funded refund fee reserve is insufficient");
        }
        reserves.insert(refund.token_ledger, available - fee);
        refund.transfer_created_at_time_ns = Some(created_at_time_ns);
        refund.transfer_fee = Some(fee);
        refund.transfer_memo = Some(memo);
        refund.protocol_fee_reserved = Some(fee);
        refund.transfer_attempted = Some(true);
        Ok(refund.clone())
    }

    /// Refresh only the fee after an ICRC-1 `BadFee`, which proves the old
    /// transfer had no effect. The immutable timestamp and memo remain fixed;
    /// fee capacity is reconciled before a replacement attempt is persisted.
    pub fn refresh_pending_refund_fee_after_bad_fee(
        &mut self,
        id: u64,
        corrected_fee: u64,
    ) -> Result<PendingRefund, &'static str> {
        let refunds = self
            .pending_refunds
            .as_mut()
            .ok_or("pending refund state is unavailable")?;
        let refund = refunds.get_mut(&id).ok_or("pending refund not found")?;
        if refund.transfer_attempted != Some(true) {
            return Err("refund fee refresh requires a journaled prior attempt");
        }
        let old_fee = refund
            .transfer_fee
            .ok_or("refund transfer fee is missing from its journal")?;
        if refund.transfer_created_at_time_ns.is_none()
            || refund.transfer_memo.is_none()
            || refund.protocol_fee_reserved != Some(old_fee)
        {
            return Err("pending refund transfer journal is incomplete and remains held");
        }
        let reserves = self
            .pending_refund_fee_reserves
            .as_mut()
            .ok_or("refund fee reserve state is unavailable")?;
        let available = reserves.get(&refund.token_ledger).copied().unwrap_or(0);
        let capacity = available
            .checked_add(old_fee)
            .ok_or("refund fee reserve balance overflow")?;
        if capacity < corrected_fee {
            return Err("protocol-funded refund fee reserve is insufficient for corrected fee");
        }
        reserves.insert(refund.token_ledger, capacity - corrected_fee);
        refund.transfer_fee = Some(corrected_fee);
        refund.protocol_fee_reserved = Some(corrected_fee);
        Ok(refund.clone())
    }

    pub fn complete_pending_refund(&mut self, id: u64) -> Option<PendingRefund> {
        self.pending_refunds
            .as_mut()
            .and_then(|refunds| refunds.remove(&id))
    }

    /// Record a typed TooOld response for the exact persisted attempt. This
    /// keeps the tuple fixed and starts no-effect reconciliation from block 0.
    pub fn mark_pending_refund_too_old(&mut self, id: u64) -> Result<PendingRefund, &'static str> {
        let refund = self
            .pending_refunds
            .as_mut()
            .and_then(|refunds| refunds.get_mut(&id))
            .ok_or("pending refund not found")?;
        if refund.transfer_attempted != Some(true)
            || refund.transfer_created_at_time_ns.is_none()
            || refund.transfer_fee.is_none()
            || refund.transfer_memo.is_none()
            || refund.protocol_fee_reserved != refund.transfer_fee
        {
            return Err("TooOld recovery requires the complete exact refund journal");
        }
        refund.transfer_too_old_rejected = Some(true);
        refund.transfer_history_scan_cursor = None;
        refund.transfer_history_scan_tip = None;
        Ok(refund.clone())
    }

    pub fn start_pending_refund_history_scan(
        &mut self,
        id: u64,
        log_length: u64,
    ) -> Result<PendingRefund, &'static str> {
        let refund = self
            .pending_refunds
            .as_mut()
            .and_then(|refunds| refunds.get_mut(&id))
            .ok_or("pending refund not found")?;
        if refund.transfer_too_old_rejected != Some(true) {
            return Err("history scan requires a typed TooOld response for this refund");
        }
        match (
            refund.transfer_history_scan_cursor,
            refund.transfer_history_scan_tip,
        ) {
            (None, None) => {
                refund.transfer_history_scan_cursor = Some(0);
                refund.transfer_history_scan_tip = Some(log_length);
            }
            (Some(_), Some(existing_tip)) if existing_tip == log_length => {}
            (Some(_), Some(_)) => {
                return Err("refund history scan tip changed; restart required");
            }
            _ => return Err("refund history scan journal is incomplete"),
        }
        Ok(refund.clone())
    }

    pub fn advance_pending_refund_history_scan(
        &mut self,
        id: u64,
        expected_cursor: u64,
        log_length: u64,
        next_cursor: u64,
    ) -> Result<PendingRefund, &'static str> {
        let refund = self
            .pending_refunds
            .as_mut()
            .and_then(|refunds| refunds.get_mut(&id))
            .ok_or("pending refund not found")?;
        if refund.transfer_too_old_rejected != Some(true)
            || refund.transfer_history_scan_cursor != Some(expected_cursor)
            || refund.transfer_history_scan_tip != Some(log_length)
            || next_cursor < expected_cursor
            || next_cursor > log_length
        {
            return Err("refund history scan state changed or range is invalid");
        }
        refund.transfer_history_scan_cursor = Some(next_cursor);
        Ok(refund.clone())
    }

    /// Release the reserved fee and permit a new exact identity only after the
    /// complete pinned-ledger history prefix has been scanned without a match.
    pub fn rotate_pending_refund_after_no_effect(
        &mut self,
        id: u64,
    ) -> Result<PendingRefund, &'static str> {
        const MAX_ATTEMPTS: u32 = 5;
        let refund = self
            .pending_refunds
            .as_mut()
            .and_then(|refunds| refunds.get_mut(&id))
            .ok_or("pending refund not found")?;
        if refund.transfer_attempted != Some(true)
            || refund.transfer_too_old_rejected != Some(true)
            || refund.transfer_history_scan_cursor != refund.transfer_history_scan_tip
            || refund.transfer_history_scan_tip.is_none()
            || refund.transfer_fee.is_none()
            || refund.protocol_fee_reserved != refund.transfer_fee
        {
            return Err("fresh refund identity requires a complete no-effect history proof");
        }
        let next_attempt = refund
            .transfer_attempt_no
            .unwrap_or(0)
            .checked_add(1)
            .filter(|attempt| *attempt < MAX_ATTEMPTS)
            .ok_or("pending refund retry limit reached")?;
        let fee = refund.transfer_fee.expect("checked above");
        let reserves = self
            .pending_refund_fee_reserves
            .as_mut()
            .ok_or("refund fee reserve state is unavailable")?;
        let available = reserves.get(&refund.token_ledger).copied().unwrap_or(0);
        let restored = available
            .checked_add(fee)
            .ok_or("refund fee reserve balance overflow")?;
        reserves.insert(refund.token_ledger, restored);
        refund.transfer_attempted = Some(false);
        refund.transfer_created_at_time_ns = None;
        refund.transfer_fee = None;
        refund.transfer_memo = None;
        refund.transfer_attempt_no = Some(next_attempt);
        refund.transfer_too_old_rejected = None;
        refund.transfer_history_scan_cursor = None;
        refund.transfer_history_scan_tip = None;
        refund.protocol_fee_reserved = None;
        Ok(refund.clone())
    }

    pub fn pending_refunds_for(&self, user: &Principal) -> Vec<PendingRefund> {
        self.pending_refunds
            .as_ref()
            .map(|m| m.values().filter(|r| r.user == *user).cloned().collect())
            .unwrap_or_default()
    }

    pub fn record_native_xrp_pending_payout(
        &mut self,
        user: Principal,
        payout: NativeXrpPendingPayout,
    ) -> Result<(), StabilityPoolError> {
        let position = self
            .deposits
            .get_mut(&user)
            .ok_or(StabilityPoolError::NoPositionFound)?;
        position
            .pending_native_xrp_payouts
            .get_or_insert_with(BTreeMap::new)
            .insert(payout.claim_id, payout);
        Ok(())
    }

    /// Every user's pending native-XRP payouts, ordered by claim id (claim ids
    /// are allocated monotonically, so this is also oldest-first). Drives the
    /// auto-settlement sweep.
    pub fn all_native_xrp_pending_payouts(&self) -> Vec<(Principal, NativeXrpPendingPayout)> {
        let mut all: Vec<(Principal, NativeXrpPendingPayout)> = self
            .deposits
            .iter()
            .flat_map(|(user, pos)| {
                pos.pending_native_xrp_payouts
                    .iter()
                    .flat_map(|payouts| payouts.values())
                    .map(|p| (*user, p.clone()))
            })
            .collect();
        all.sort_by_key(|(_, p)| p.claim_id);
        all
    }

    pub fn native_xrp_pending_payouts_for(&self, user: &Principal) -> Vec<NativeXrpPendingPayout> {
        self.deposits
            .get(user)
            .and_then(|pos| pos.pending_native_xrp_payouts.as_ref())
            .map(|payouts| payouts.values().cloned().collect())
            .unwrap_or_default()
    }

    pub fn native_xrp_pending_payout_for(
        &self,
        user: &Principal,
        claim_id: u64,
    ) -> Option<NativeXrpPendingPayout> {
        self.deposits
            .get(user)
            .and_then(|pos| pos.pending_native_xrp_payouts.as_ref())
            .and_then(|payouts| payouts.get(&claim_id))
            .cloned()
    }

    pub fn ack_native_xrp_payout_settled(
        &mut self,
        user: &Principal,
        claim_id: u64,
    ) -> Result<(), StabilityPoolError> {
        let removed = self
            .deposits
            .get_mut(user)
            .and_then(|pos| pos.pending_native_xrp_payouts.as_mut())
            .and_then(|payouts| payouts.remove(&claim_id));
        if removed.is_none() {
            return Err(StabilityPoolError::RefundClaimNotFound);
        }
        Ok(())
    }

    // ─── Effective Pool Computation ───

    /// Compute total opted-in stablecoin value (e8s) for a given collateral type.
    pub fn effective_pool_for_collateral(&self, collateral_type: &Principal) -> u64 {
        let vps = self.virtual_prices();
        self.deposits
            .values()
            .filter(|pos| self.position_opted_in_for(pos, collateral_type))
            .map(|pos| pos.total_usd_value(&self.stablecoin_registry, vps))
            .sum()
    }

    pub fn available_stablecoin_for_collateral(
        &self,
        token_ledger: Principal,
        collateral_type: &Principal,
    ) -> Result<u64, StabilityPoolError> {
        let available = self
            .deposits
            .iter()
            .filter(|(_, position)| self.position_opted_in_for(position, collateral_type))
            .try_fold(0u64, |sum, (_, position)| {
                sum.checked_add(
                    position
                        .stablecoin_balances
                        .get(&token_ledger)
                        .copied()
                        .unwrap_or(0),
                )
                .ok_or(StabilityPoolError::SystemBusy)
            })?;
        Ok(available)
    }

    pub fn icusd_ledger(&self) -> Option<Principal> {
        self.stablecoin_registry
            .iter()
            .find(|(_, config)| config.symbol == "icUSD")
            .map(|(ledger, _)| *ledger)
    }

    /// Compute opted-in icUSD coverage only. Chain-native liquidations use
    /// this instead of the mixed-token draw so Inc 4 burns only IC-native icUSD.
    pub fn effective_icusd_pool_for_collateral(&self, collateral_type: &Principal) -> u64 {
        self.deposits
            .values()
            .filter(|pos| self.position_opted_in_for(pos, collateral_type))
            .map(|pos| pos.icusd_value(&self.stablecoin_registry))
            .sum()
    }

    // ─── Liquidation Processing ───

    /// Compute the stablecoin draw for a liquidation of a given debt amount (e8s).
    /// Returns a map of token_ledger -> amount to consume (in native decimals).
    ///
    /// For small debts (< 1 icUSD / 100_000_000 e8s), uses a single token — whichever
    /// has the highest balance — to avoid splitting into amounts too small for the backend.
    /// For larger debts, follows priority ordering with proportional splits.
    pub fn compute_token_draw(
        &self,
        debt_e8s: u64,
        collateral_type: &Principal,
    ) -> BTreeMap<Principal, u64> {
        let vps = self.virtual_prices();

        // Gather all available tokens with their e8s-equivalent balances
        // Tuple: (ledger, available_native, decimals, is_lp, available_e8s)
        let mut all_tokens: Vec<(Principal, u64, u8, bool, u64)> = Vec::new();

        for (ledger, config) in &self.stablecoin_registry {
            let available_native: u64 = self
                .deposits
                .values()
                .filter(|pos| self.position_opted_in_for(pos, collateral_type))
                .map(|pos| pos.stablecoin_balances.get(ledger).copied().unwrap_or(0))
                .sum();
            if available_native > 0 {
                let is_lp = config.is_lp_token.unwrap_or(false);
                let available_e8s = if is_lp {
                    vps.get(ledger)
                        .map(|&vp| lp_to_usd_e8s(available_native, vp))
                        .unwrap_or(0)
                } else {
                    normalize_to_e8s(available_native, config.decimals)
                };
                if available_e8s > 0 {
                    all_tokens.push((
                        *ledger,
                        available_native,
                        config.decimals,
                        is_lp,
                        available_e8s,
                    ));
                }
            }
        }

        if all_tokens.is_empty() {
            return BTreeMap::new();
        }

        // Small debt optimization: use the single token with the highest balance.
        // This avoids splitting into amounts that all fall below the backend minimum.
        const SMALL_DEBT_THRESHOLD: u64 = 100_000_000; // 1 icUSD
        if debt_e8s < SMALL_DEBT_THRESHOLD {
            let best = all_tokens
                .iter()
                .max_by_key(|(_, _, _, _, e8s)| *e8s)
                .unwrap(); // safe: all_tokens is non-empty

            let (ledger, available_native, decimals, is_lp, available_e8s) = *best;
            let draw_e8s = debt_e8s.min(available_e8s);
            let draw_native = if is_lp {
                vps.get(&ledger)
                    .map(|&vp| usd_e8s_to_lp(draw_e8s, vp))
                    .unwrap_or(0)
            } else {
                normalize_from_e8s(draw_e8s, decimals)
            };

            let mut result = BTreeMap::new();
            if draw_native > 0 {
                result.insert(ledger, draw_native.min(available_native));
            }
            return result;
        }

        // Normal path: priority-based proportional draw
        let mut result = BTreeMap::new();
        let mut remaining_e8s = debt_e8s;

        // Group by priority
        let mut priority_buckets: BTreeMap<u8, Vec<(Principal, u64, u8, bool)>> = BTreeMap::new();
        for (ledger, config) in &self.stablecoin_registry {
            let available_native: u64 = self
                .deposits
                .values()
                .filter(|pos| self.position_opted_in_for(pos, collateral_type))
                .map(|pos| pos.stablecoin_balances.get(ledger).copied().unwrap_or(0))
                .sum();
            if available_native > 0 {
                let is_lp = config.is_lp_token.unwrap_or(false);
                priority_buckets.entry(config.priority).or_default().push((
                    *ledger,
                    available_native,
                    config.decimals,
                    is_lp,
                ));
            }
        }

        // Process from highest priority first
        let mut priorities: Vec<u8> = priority_buckets.keys().copied().collect();
        priorities.sort_by(|a, b| b.cmp(a)); // descending

        for priority in priorities {
            if remaining_e8s == 0 {
                break;
            }
            let tokens = priority_buckets.get(&priority).unwrap();

            let total_available_e8s: u64 = tokens
                .iter()
                .map(|(ledger, amount, decimals, is_lp)| {
                    if *is_lp {
                        vps.get(ledger)
                            .map(|&vp| lp_to_usd_e8s(*amount, vp))
                            .unwrap_or(0)
                    } else {
                        normalize_to_e8s(*amount, *decimals)
                    }
                })
                .sum();

            if total_available_e8s == 0 {
                continue;
            }

            let draw_e8s = remaining_e8s.min(total_available_e8s);

            for (ledger, available_native, decimals, is_lp) in tokens {
                let token_available_e8s = if *is_lp {
                    vps.get(ledger)
                        .map(|&vp| lp_to_usd_e8s(*available_native, vp))
                        .unwrap_or(0)
                } else {
                    normalize_to_e8s(*available_native, *decimals)
                };
                if token_available_e8s == 0 {
                    continue;
                }
                let token_draw_e8s = (draw_e8s as u128 * token_available_e8s as u128
                    / total_available_e8s as u128) as u64;
                let token_draw_native = if *is_lp {
                    vps.get(ledger)
                        .map(|&vp| usd_e8s_to_lp(token_draw_e8s, vp))
                        .unwrap_or(0)
                } else {
                    normalize_from_e8s(token_draw_e8s, *decimals)
                };
                if token_draw_native > 0 {
                    result.insert(*ledger, token_draw_native.min(*available_native));
                }
            }

            remaining_e8s -= draw_e8s;
        }

        result
    }

    /// Compute an Inc 4 chain-vault draw. Returns at most one token, icUSD, in
    /// e8s. ckStables and 3USD are intentionally excluded from this path.
    pub fn compute_icusd_chain_draw(
        &self,
        debt_e8s: u64,
        chain_sentinel: &Principal,
    ) -> BTreeMap<Principal, u64> {
        let mut result = BTreeMap::new();
        if debt_e8s == 0 || !self.is_chain_collateral_sentinel(chain_sentinel) {
            return result;
        }
        let Some(icusd_ledger) = self.icusd_ledger() else {
            return result;
        };
        let draw_e8s = debt_e8s.min(self.effective_icusd_pool_for_collateral(chain_sentinel));
        if draw_e8s > 0 {
            result.insert(icusd_ledger, draw_e8s);
        }
        result
    }

    /// After a successful liquidation, reduce depositor balances and distribute collateral gains.
    /// `stables_consumed` is a map of token_ledger -> total amount consumed (native decimals).
    /// `collateral_gained` is the collateral received by the pool (native decimals).
    /// Only opted-in depositors for `collateral_type` participate.
    pub fn process_liquidation_gains(
        &mut self,
        vault_id: u64,
        collateral_type: Principal,
        stables_consumed: &BTreeMap<Principal, u64>,
        collateral_gained: u64,
        collateral_price_e8s: u64,
    ) -> Result<(), StabilityPoolError> {
        self.process_liquidation_gains_at(
            vault_id,
            collateral_type,
            stables_consumed,
            collateral_gained,
            collateral_price_e8s,
            ic_cdk::api::time(),
        )
    }

    /// Precompute exact stablecoin debits for an opted-in liquidation cohort.
    /// The returned allocations sum to each ledger pull exactly; any mismatch
    /// is rejected before a depositor balance or aggregate is changed.
    fn exact_liquidation_debit_allocations(
        &self,
        opted_in_principals: &[Principal],
        stables_consumed: &BTreeMap<Principal, u64>,
    ) -> Result<BTreeMap<Principal, BTreeMap<Principal, u64>>, StabilityPoolError> {
        let mut by_token = BTreeMap::new();
        for (token_ledger, amount) in stables_consumed {
            self.ensure_stablecoin_aggregate_matches_positions(*token_ledger)?;
            let balances: Vec<(Principal, u64)> = opted_in_principals
                .iter()
                .filter_map(|owner| {
                    self.deposits
                        .get(owner)
                        .and_then(|pos| pos.stablecoin_balances.get(token_ledger))
                        .copied()
                        .filter(|balance| *balance > 0)
                        .map(|balance| (*owner, balance))
                })
                .collect();
            by_token.insert(
                *token_ledger,
                exact_proportional_debit_allocations(&balances, *amount)?,
            );
        }
        Ok(by_token)
    }

    pub fn can_process_liquidation_debits(
        &self,
        collateral_type: Principal,
        stables_consumed: &BTreeMap<Principal, u64>,
    ) -> bool {
        let opted_in: Vec<Principal> = self
            .deposits
            .iter()
            .filter(|(_, pos)| self.position_opted_in_for(pos, &collateral_type))
            .map(|(owner, _)| *owner)
            .collect();
        self.exact_liquidation_debit_allocations(&opted_in, stables_consumed)
            .is_ok()
    }

    pub fn can_process_chain_liquidation_debits(
        &self,
        chain_sentinel: Principal,
        stables_consumed: &BTreeMap<Principal, u64>,
    ) -> bool {
        if !self.is_chain_collateral_sentinel(&chain_sentinel) {
            return false;
        }
        let opted_in: Vec<Principal> = self
            .deposits
            .iter()
            .filter(|(_, pos)| pos.is_opted_in_for_chain(&chain_sentinel))
            .map(|(owner, _)| *owner)
            .collect();
        self.exact_liquidation_debit_allocations(&opted_in, stables_consumed)
            .is_ok()
    }

    pub fn process_chain_liquidation_gains(
        &mut self,
        vault_id: u64,
        chain_sentinel: Principal,
        stables_consumed: &BTreeMap<Principal, u64>,
        cfx_gained_native: u128,
        collateral_price_e8s: u64,
    ) -> Result<(), StabilityPoolError> {
        self.process_chain_liquidation_gains_at(
            vault_id,
            chain_sentinel,
            stables_consumed,
            cfx_gained_native,
            collateral_price_e8s,
            ic_cdk::api::time(),
        )
    }

    /// Append an audit record for a completed liquidation and advance
    /// `total_liquidations_executed`.
    ///
    /// Every path that absorbs a vault must go through this, so the counter and
    /// `liquidation_history` can never disagree. They previously did: the
    /// native-XRP and chain-collateral paths bumped the counter inline but
    /// never pushed a record, so a real absorb advanced the count while leaving
    /// the Earn page's history list unchanged.
    fn record_liquidation_in_history(&mut self, record: PoolLiquidationRecord) {
        self.liquidation_history.push(record);
        self.total_liquidations_executed += 1;

        // Cap history to prevent unbounded memory growth
        if self.liquidation_history.len() > MAX_LIQUIDATION_HISTORY {
            let excess = self.liquidation_history.len() - MAX_LIQUIDATION_HISTORY;
            self.liquidation_history.drain(..excess);
        }
    }

    pub fn process_native_xrp_absorb_success_at(
        &mut self,
        vault_id: u64,
        collateral_type: Principal,
        stables_consumed: &BTreeMap<Principal, u64>,
        collateral_received_drops: u64,
        payout_claims: &[XrpSpPayoutClaim],
        timestamp: u64,
    ) -> Result<(), StabilityPoolError> {
        if !self.collateral_requires_payout_address(&collateral_type) {
            return Err(StabilityPoolError::PayoutAddressRequired {
                collateral: collateral_type,
            });
        }
        if collateral_received_drops == 0 || payout_claims.is_empty() {
            return Err(StabilityPoolError::LiquidationFailed {
                vault_id,
                reason: "native XRP absorb has no payout allocations".to_string(),
            });
        }
        if payout_claims.len() > MAX_XRP_SP_PAYOUT_ALLOCATIONS {
            return Err(StabilityPoolError::LiquidationFailed {
                vault_id,
                reason: format!(
                    "native XRP payout fanout {} exceeds max {}",
                    payout_claims.len(),
                    MAX_XRP_SP_PAYOUT_ALLOCATIONS
                ),
            });
        }

        let mut payout_sum = 0u64;
        let mut seen_claim_ids = BTreeSet::new();
        let mut seen_claimants = BTreeSet::new();
        for claim in payout_claims {
            if claim.drops == 0 || claim.payout_address.trim().is_empty() {
                return Err(StabilityPoolError::LiquidationFailed {
                    vault_id,
                    reason: "native XRP payout claim has invalid amount or address".to_string(),
                });
            }
            if !seen_claim_ids.insert(claim.claim_id) || !seen_claimants.insert(claim.claimant) {
                return Err(StabilityPoolError::LiquidationFailed {
                    vault_id,
                    reason: "native XRP payout claims contain duplicate ids or claimants"
                        .to_string(),
                });
            }
            let opted_in = self
                .deposits
                .get(&claim.claimant)
                .map(|pos| self.position_opted_in_for(pos, &collateral_type))
                .unwrap_or(false);
            if !opted_in {
                return Err(StabilityPoolError::LiquidationFailed {
                    vault_id,
                    reason: "backend returned native XRP payout for non-opted-in depositor"
                        .to_string(),
                });
            }
            payout_sum = payout_sum.saturating_add(claim.drops);
        }
        if payout_sum != collateral_received_drops {
            return Err(StabilityPoolError::LiquidationFailed {
                vault_id,
                reason: "backend native XRP payout sum does not match preflight collateral"
                    .to_string(),
            });
        }

        let mut opted_in_principals: Vec<Principal> = self
            .deposits
            .iter()
            .filter(|(_, pos)| self.position_opted_in_for(pos, &collateral_type))
            .filter(|(_, pos)| {
                stables_consumed
                    .keys()
                    .any(|token| pos.stablecoin_balances.get(token).copied().unwrap_or(0) > 0)
            })
            .map(|(principal, _)| *principal)
            .collect();
        opted_in_principals.sort_by(|a, b| a.as_slice().cmp(b.as_slice()));
        if opted_in_principals.is_empty() {
            return Err(StabilityPoolError::InsufficientPoolBalance);
        }

        let mut per_token_opted_in_totals: BTreeMap<Principal, u64> = BTreeMap::new();
        for token_ledger in stables_consumed.keys() {
            let total: u64 = opted_in_principals
                .iter()
                .filter_map(|principal| self.deposits.get(principal))
                .map(|pos| {
                    pos.stablecoin_balances
                        .get(token_ledger)
                        .copied()
                        .unwrap_or(0)
                })
                .sum();
            per_token_opted_in_totals.insert(*token_ledger, total);
        }

        let vps = self.virtual_prices().clone();
        let registry_snapshot: BTreeMap<Principal, (u8, bool)> = stables_consumed
            .keys()
            .filter_map(|ledger| {
                self.stablecoin_registry
                    .get(ledger)
                    .map(|c| (*ledger, (c.decimals, c.is_lp_token.unwrap_or(false))))
            })
            .collect();
        let total_consumed_e8s: u64 = stables_consumed
            .iter()
            .map(|(ledger, &amount)| {
                let (decimals, is_lp) =
                    registry_snapshot.get(ledger).copied().unwrap_or((8, false));
                if is_lp {
                    vps.get(ledger)
                        .map(|&vp| lp_to_usd_e8s(amount, vp))
                        .unwrap_or(0)
                } else {
                    normalize_to_e8s(amount, decimals)
                }
            })
            .sum();
        if total_consumed_e8s == 0 {
            return Err(StabilityPoolError::InsufficientPoolBalance);
        }

        let mut actual_deductions_per_token: BTreeMap<Principal, u64> = BTreeMap::new();
        for principal in &opted_in_principals {
            if let Some(position) = self.deposits.get_mut(principal) {
                for (token_ledger, &total_consumed) in stables_consumed {
                    let total_opted_in = per_token_opted_in_totals
                        .get(token_ledger)
                        .copied()
                        .unwrap_or(0);
                    if total_opted_in == 0 {
                        continue;
                    }
                    let user_balance = position
                        .stablecoin_balances
                        .get(token_ledger)
                        .copied()
                        .unwrap_or(0);
                    if user_balance == 0 {
                        continue;
                    }

                    let user_share_native = (total_consumed as u128 * user_balance as u128
                        / total_opted_in as u128)
                        as u64;
                    let user_share_native = user_share_native.min(user_balance);
                    if let Some(balance) = position.stablecoin_balances.get_mut(token_ledger) {
                        *balance = balance.saturating_sub(user_share_native);
                    }
                    *actual_deductions_per_token
                        .entry(*token_ledger)
                        .or_insert(0) += user_share_native;
                }
            }
        }

        for (token_ledger, &total_consumed) in stables_consumed {
            let actual_deducted = actual_deductions_per_token
                .get(token_ledger)
                .copied()
                .unwrap_or(0);
            let mut remaining = total_consumed.saturating_sub(actual_deducted);
            if remaining == 0 {
                continue;
            }

            for principal in &opted_in_principals {
                if remaining == 0 {
                    break;
                }
                let Some(position) = self.deposits.get_mut(principal) else {
                    continue;
                };
                let Some(balance) = position.stablecoin_balances.get_mut(token_ledger) else {
                    continue;
                };
                if *balance == 0 {
                    continue;
                }
                let extra = remaining.min(*balance);
                *balance = balance.saturating_sub(extra);
                *actual_deductions_per_token
                    .entry(*token_ledger)
                    .or_insert(0) += extra;
                remaining -= extra;
            }

            if remaining > 0 {
                return Err(StabilityPoolError::LiquidationFailed {
                    vault_id,
                    reason: "native XRP absorb could not deduct full burned stablecoin amount"
                        .to_string(),
                });
            }
        }

        for (token_ledger, actual_deducted) in &actual_deductions_per_token {
            if let Some(total) = self.total_stablecoin_balances.get_mut(token_ledger) {
                *total = total.saturating_sub(*actual_deducted);
            }
        }

        for claim in payout_claims {
            self.record_native_xrp_pending_payout(
                claim.claimant,
                NativeXrpPendingPayout {
                    claim_id: claim.claim_id,
                    collateral_type,
                    collateral_price_e8s: 0,
                    vault_id,
                    drops: claim.drops,
                    payout_address: claim.payout_address.clone(),
                    destination_tag: claim.destination_tag,
                    created_at_ns: timestamp,
                },
            )?;
        }

        // `collateral_gained` is the seized amount in drops (XRP is 6-decimal);
        // consumers must format it with the collateral's own decimals.
        // `collateral_price_e8s` is None: the pool never sees an XRP price on
        // this path (the backend does the sizing), and the field is optional
        // precisely so records can omit it.
        self.record_liquidation_in_history(PoolLiquidationRecord {
            vault_id,
            timestamp,
            stables_consumed: stables_consumed.clone(),
            collateral_gained: collateral_received_drops,
            collateral_type,
            depositors_count: payout_claims.len() as u64,
            collateral_price_e8s: None,
        });
        self.deposits.retain(|_, pos| !pos.is_empty());
        debug_assert!(
            self.validate_state().is_ok(),
            "stability pool aggregate/per-depositor invariant violated after \
             process_native_xrp_absorb_success_at"
        );
        Ok(())
    }

    /// Core liquidation gain processing logic with explicit timestamp (testable without IC runtime).
    pub fn process_liquidation_gains_at(
        &mut self,
        vault_id: u64,
        collateral_type: Principal,
        stables_consumed: &BTreeMap<Principal, u64>,
        collateral_gained: u64,
        collateral_price_e8s: u64,
        timestamp: u64,
    ) -> Result<(), StabilityPoolError> {
        if self.collateral_requires_payout_address(&collateral_type) {
            log!(
                INFO,
                "Rejecting generic liquidation gain processing for native collateral {} on vault {}; \
                 native XRP requires backend XrpClaim-backed pending payout records",
                collateral_type,
                vault_id
            );
            return Err(StabilityPoolError::PayoutAddressRequired {
                collateral: collateral_type,
            });
        }

        // Phase 1: Compute each opted-in depositor's share of the consumed stables (in e8s)
        let opted_in_principals: Vec<Principal> = self
            .deposits
            .iter()
            .filter(|(_, pos)| self.position_opted_in_for(pos, &collateral_type))
            .map(|(p, _)| *p)
            .collect();
        if opted_in_principals.is_empty() {
            return Err(StabilityPoolError::InsufficientPoolBalance);
        }
        let debit_allocations =
            self.exact_liquidation_debit_allocations(&opted_in_principals, stables_consumed)?;

        // Phase 2: Compute total e8s consumed to determine collateral distribution shares.
        // LP tokens are valued at virtual price, not face value.
        // Clone virtual prices and registry info upfront to avoid borrow conflicts with Phase 3.
        let vps = self.virtual_prices().clone();
        let registry_snapshot: BTreeMap<Principal, (u8, bool)> = stables_consumed
            .keys()
            .filter_map(|ledger| {
                self.stablecoin_registry
                    .get(ledger)
                    .map(|c| (*ledger, (c.decimals, c.is_lp_token.unwrap_or(false))))
            })
            .collect();
        let total_consumed_e8s: u64 = stables_consumed
            .iter()
            .map(|(ledger, &amount)| {
                let (decimals, is_lp) =
                    registry_snapshot.get(ledger).copied().unwrap_or((8, false));
                if is_lp {
                    vps.get(ledger)
                        .map(|&vp| lp_to_usd_e8s(amount, vp))
                        .unwrap_or(0)
                } else {
                    normalize_to_e8s(amount, decimals)
                }
            })
            .sum();

        if total_consumed_e8s == 0 {
            return Err(StabilityPoolError::InsufficientPoolBalance);
        }

        // Phase 3: For each opted-in depositor, reduce their token balances and add collateral gains.
        let mut actual_deductions_per_token: BTreeMap<Principal, u64> = BTreeMap::new();
        let mut total_collateral_distributed: u64 = 0;

        for principal in &opted_in_principals {
            let mut user_consumed_e8s: u64 = 0;

            if let Some(position) = self.deposits.get_mut(principal) {
                for (token_ledger, allocations) in &debit_allocations {
                    let user_share_native = allocations.get(principal).copied().unwrap_or(0);
                    if user_share_native == 0 {
                        continue;
                    }
                    if let Some(bal) = position.stablecoin_balances.get_mut(token_ledger) {
                        *bal -= user_share_native;
                        if *bal == 0 {
                            position.stablecoin_balances.remove(token_ledger);
                        }
                    }
                    *actual_deductions_per_token
                        .entry(*token_ledger)
                        .or_insert(0) += user_share_native;

                    // Track consumed value in e8s for collateral distribution.
                    // LP tokens valued at virtual price, not face value.
                    let (decimals, is_lp) = registry_snapshot
                        .get(token_ledger)
                        .copied()
                        .unwrap_or((8, false));
                    let share_e8s = if is_lp {
                        vps.get(token_ledger)
                            .map(|&vp| lp_to_usd_e8s(user_share_native, vp))
                            .unwrap_or(0)
                    } else {
                        normalize_to_e8s(user_share_native, decimals)
                    };
                    user_consumed_e8s += share_e8s;
                }

                // Distribute collateral proportional to e8s consumed
                if user_consumed_e8s > 0 {
                    let user_collateral = (collateral_gained as u128 * user_consumed_e8s as u128
                        / total_consumed_e8s as u128)
                        as u64;
                    *position
                        .collateral_gains
                        .entry(collateral_type)
                        .or_insert(0) += user_collateral;
                    total_collateral_distributed += user_collateral;
                }
            }
        }

        // Phase 3b: Assign collateral rounding dust to first opted-in depositor
        let collateral_dust = collateral_gained.saturating_sub(total_collateral_distributed);
        if collateral_dust > 0 {
            if let Some(first) = opted_in_principals.first() {
                if let Some(pos) = self.deposits.get_mut(first) {
                    *pos.collateral_gains.entry(collateral_type).or_insert(0) += collateral_dust;
                }
            }
        }

        // Phase 4: Update aggregate totals using ACTUAL deductions (not stables_consumed)
        // to prevent rounding dust drift that would cause validate_state() to fail.
        for (token_ledger, &actual_deducted) in &actual_deductions_per_token {
            if let Some(total) = self.total_stablecoin_balances.get_mut(token_ledger) {
                *total = total.saturating_sub(actual_deducted);
            }
        }

        // Phase 5: Record in history
        self.record_liquidation_in_history(PoolLiquidationRecord {
            vault_id,
            timestamp,
            stables_consumed: stables_consumed.clone(),
            collateral_gained,
            collateral_type,
            depositors_count: opted_in_principals.len() as u64,
            collateral_price_e8s: Some(collateral_price_e8s),
        });

        // Phase 6: Clean up empty positions
        self.deposits.retain(|_, pos| !pos.is_empty());

        // SP-001 regression fence: per-depositor balances must sum to the
        // aggregate total after the full gains pass. Violations indicate a
        // double-deduction or divergent-update bug (debug builds only — the
        // field-level assertions above are already proportional-sound).
        debug_assert!(
            self.validate_state().is_ok(),
            "stability pool aggregate/per-depositor invariant violated after \
             process_liquidation_gains_at (likely regression of SP-001)"
        );
        Ok(())
    }

    /// CFX/native-chain sibling of `process_liquidation_gains_at`. Stablecoin
    /// draw and rounding are identical, but collateral claims are u128 wei and
    /// live in `DepositPosition::cfx_claims` instead of the u64 ICRC gains map.
    pub fn process_chain_liquidation_gains_at(
        &mut self,
        _vault_id: u64,
        chain_sentinel: Principal,
        stables_consumed: &BTreeMap<Principal, u64>,
        cfx_gained_native: u128,
        _collateral_price_e8s: u64,
        _timestamp: u64,
    ) -> Result<(), StabilityPoolError> {
        if !self.is_chain_collateral_sentinel(&chain_sentinel) || cfx_gained_native == 0 {
            return Err(StabilityPoolError::LiquidationFailed {
                vault_id: _vault_id,
                reason: "chain liquidation gains have an invalid sentinel or zero collateral"
                    .into(),
            });
        }

        let opted_in_principals: Vec<Principal> = self
            .deposits
            .iter()
            .filter(|(_, pos)| pos.is_opted_in_for_chain(&chain_sentinel))
            .map(|(p, _)| *p)
            .collect();
        if opted_in_principals.is_empty() {
            return Err(StabilityPoolError::InsufficientPoolBalance);
        }
        let debit_allocations =
            self.exact_liquidation_debit_allocations(&opted_in_principals, stables_consumed)?;

        let vps = self.virtual_prices().clone();
        let registry_snapshot: BTreeMap<Principal, (u8, bool)> = stables_consumed
            .keys()
            .filter_map(|ledger| {
                self.stablecoin_registry
                    .get(ledger)
                    .map(|c| (*ledger, (c.decimals, c.is_lp_token.unwrap_or(false))))
            })
            .collect();
        let total_consumed_e8s: u64 = stables_consumed
            .iter()
            .map(|(ledger, &amount)| {
                let (decimals, is_lp) =
                    registry_snapshot.get(ledger).copied().unwrap_or((8, false));
                if is_lp {
                    vps.get(ledger)
                        .map(|&vp| lp_to_usd_e8s(amount, vp))
                        .unwrap_or(0)
                } else {
                    normalize_to_e8s(amount, decimals)
                }
            })
            .sum();
        if total_consumed_e8s == 0 {
            return Err(StabilityPoolError::InsufficientPoolBalance);
        }

        let mut actual_deductions_per_token: BTreeMap<Principal, u64> = BTreeMap::new();
        let mut total_cfx_distributed: u128 = 0;

        for principal in &opted_in_principals {
            let mut user_consumed_e8s: u64 = 0;

            if let Some(position) = self.deposits.get_mut(principal) {
                for (token_ledger, allocations) in &debit_allocations {
                    let user_share_native = allocations.get(principal).copied().unwrap_or(0);
                    if user_share_native == 0 {
                        continue;
                    }
                    if let Some(bal) = position.stablecoin_balances.get_mut(token_ledger) {
                        *bal -= user_share_native;
                        if *bal == 0 {
                            position.stablecoin_balances.remove(token_ledger);
                        }
                    }
                    *actual_deductions_per_token
                        .entry(*token_ledger)
                        .or_insert(0) += user_share_native;

                    let (decimals, is_lp) = registry_snapshot
                        .get(token_ledger)
                        .copied()
                        .unwrap_or((8, false));
                    let share_e8s = if is_lp {
                        vps.get(token_ledger)
                            .map(|&vp| lp_to_usd_e8s(user_share_native, vp))
                            .unwrap_or(0)
                    } else {
                        normalize_to_e8s(user_share_native, decimals)
                    };
                    user_consumed_e8s = user_consumed_e8s.saturating_add(share_e8s);
                }

                if user_consumed_e8s > 0 {
                    let user_cfx = cfx_gained_native.saturating_mul(user_consumed_e8s as u128)
                        / total_consumed_e8s as u128;
                    let claims = position.cfx_claims.get_or_insert_with(BTreeMap::new);
                    let entry = claims.entry(chain_sentinel).or_insert(0);
                    *entry = entry.saturating_add(user_cfx);
                    total_cfx_distributed = total_cfx_distributed.saturating_add(user_cfx);
                }
            }
        }

        let cfx_dust = cfx_gained_native.saturating_sub(total_cfx_distributed);
        if cfx_dust > 0 {
            if let Some(first) = opted_in_principals.first() {
                if let Some(pos) = self.deposits.get_mut(first) {
                    let claims = pos.cfx_claims.get_or_insert_with(BTreeMap::new);
                    let entry = claims.entry(chain_sentinel).or_insert(0);
                    *entry = entry.saturating_add(cfx_dust);
                }
            }
        }

        for (token_ledger, &actual_deducted) in &actual_deductions_per_token {
            if let Some(total) = self.total_stablecoin_balances.get_mut(token_ledger) {
                *total = total.saturating_sub(actual_deducted);
            }
        }

        self.total_liquidations_executed += 1;
        self.deposits.retain(|_, pos| !pos.is_empty());
        debug_assert!(
            self.validate_state().is_ok(),
            "stability pool aggregate/per-depositor invariant violated after \
             process_chain_liquidation_gains_at"
        );
        Ok(())
    }

    // ─── Query Helpers ───

    pub fn get_pool_status(&self) -> StabilityPoolStatus {
        let vps = self.virtual_prices();
        let total_e8s: u64 = self
            .total_stablecoin_balances
            .iter()
            .map(|(ledger, &amount)| {
                let config = self.stablecoin_registry.get(ledger);
                if config
                    .map(|c| c.is_lp_token.unwrap_or(false))
                    .unwrap_or(false)
                {
                    vps.get(ledger)
                        .map(|&vp| lp_to_usd_e8s(amount, vp))
                        .unwrap_or(0)
                } else {
                    let decimals = config.map(|c| c.decimals).unwrap_or(8);
                    normalize_to_e8s(amount, decimals)
                }
            })
            .sum();

        let total_collateral_gains: BTreeMap<Principal, u64> = {
            let mut gains = BTreeMap::new();
            for record in &self.liquidation_history {
                *gains.entry(record.collateral_type).or_insert(0) += record.collateral_gained;
            }
            gains
        };

        StabilityPoolStatus {
            total_deposits_e8s: total_e8s,
            total_depositors: self.deposits.len() as u64,
            total_liquidations_executed: self.total_liquidations_executed,
            stablecoin_balances: self.total_stablecoin_balances.clone(),
            collateral_gains: total_collateral_gains,
            stablecoin_registry: self.stablecoin_registry.values().cloned().collect(),
            collateral_registry: self.collateral_registry.values().cloned().collect(),
            emergency_paused: self.configuration.emergency_pause,
            total_interest_received_e8s: self.total_interest_received_e8s.unwrap_or(0),
            eligible_icusd_per_collateral: self.eligible_icusd_per_collateral(),
            eligible_usd_per_collateral: Some(self.eligible_usd_per_collateral()),
        }
    }

    /// For each collateral type, compute the total icUSD balance (e8s) held by
    /// depositors who are opted in to that collateral type.
    ///
    /// This is the denominator for the borrowing-interest APY. Interest is
    /// distributed only to icUSD balances; including 3USD or ck-stable
    /// deposits here would understate the APY without changing payouts.
    fn eligible_icusd_per_collateral(&self) -> Vec<(Principal, u64)> {
        self.collateral_registry
            .keys()
            .map(|ct| {
                let eligible: u64 = self
                    .deposits
                    .values()
                    .filter(|pos| self.position_opted_in_for(pos, ct))
                    .map(|pos| pos.icusd_value(&self.stablecoin_registry))
                    .sum();
                (*ct, eligible)
            })
            .collect()
    }

    /// Per-collateral liquidation capacity across every accepted stablecoin.
    /// This must remain distinct from the icUSD-only interest denominator.
    fn eligible_usd_per_collateral(&self) -> Vec<(Principal, u64)> {
        let virtual_prices = self.virtual_prices();
        self.collateral_registry
            .keys()
            .map(|ct| {
                let eligible: u64 = self
                    .deposits
                    .values()
                    .filter(|pos| self.position_opted_in_for(pos, ct))
                    .map(|pos| pos.total_usd_value(&self.stablecoin_registry, &virtual_prices))
                    .sum();
                (*ct, eligible)
            })
            .collect()
    }

    pub fn get_user_position(&self, user: &Principal) -> Option<UserStabilityPosition> {
        self.deposits.get(user).map(|pos| UserStabilityPosition {
            stablecoin_balances: pos.stablecoin_balances.clone(),
            collateral_gains: pos.collateral_gains.clone(),
            cfx_claims: pos.cfx_claims.clone(),
            opted_out_collateral: pos.opted_out_collateral.iter().cloned().collect(),
            eligible_interest_collateral: Some(
                self.collateral_registry
                    .keys()
                    .filter(|collateral| self.position_opted_in_for(pos, collateral))
                    .copied()
                    .collect(),
            ),
            native_payout_addresses: Some(pos.native_payout_addresses.clone().unwrap_or_default()),
            native_payout_destination_tags: Some(
                pos.native_payout_destination_tags
                    .clone()
                    .unwrap_or_default(),
            ),
            deposit_timestamp: pos.deposit_timestamp,
            total_claimed_gains: pos.total_claimed_gains.clone(),
            total_usd_value_e8s: pos
                .total_usd_value(&self.stablecoin_registry, self.virtual_prices()),
            total_interest_earned_e8s: pos.total_interest_earned_e8s.unwrap_or(0),
        })
    }

    // ─── Fee Accounting ───

    /// Deduct a ledger fee (e.g. approve fee) proportionally from all depositors
    /// who hold `token_ledger`, then adjust the aggregate total to match.
    pub fn can_deduct_fee_from_pool(&self, token_ledger: Principal, fee: u64) -> bool {
        self.ensure_stablecoin_aggregate_matches_positions(token_ledger)
            .ok()
            .and_then(|total| total.checked_sub(fee))
            .is_some()
    }

    /// Deduct a ledger fee exactly across all holders. A malformed or
    /// insufficient pool is left unchanged so callers can stop before the
    /// corresponding ledger operation.
    pub fn deduct_fee_from_pool(
        &mut self,
        token_ledger: Principal,
        fee: u64,
    ) -> Result<(), StabilityPoolError> {
        self.deduct_exact_pool_fee(token_ledger, fee).map(|_| ())
    }

    // ─── Admin Balance Correction ───

    /// Set a depositor's balance for a specific token to `correct_amount`,
    /// adjusting the aggregate total accordingly.  Used to fix phantom balances
    /// that exist in state but not on the actual ledger.
    pub fn correct_balance(
        &mut self,
        user: Principal,
        token_ledger: Principal,
        correct_amount: u64,
    ) -> String {
        let old_amount = self
            .deposits
            .get(&user)
            .and_then(|pos| pos.stablecoin_balances.get(&token_ledger).copied())
            .unwrap_or(0);

        if old_amount == correct_amount {
            return format!(
                "No change needed: user {} balance for {} is already {}",
                user, token_ledger, correct_amount
            );
        }

        let diff = old_amount as i128 - correct_amount as i128;

        if let Some(pos) = self.deposits.get_mut(&user) {
            if correct_amount == 0 {
                pos.stablecoin_balances.remove(&token_ledger);
            } else {
                pos.stablecoin_balances.insert(token_ledger, correct_amount);
            }
            if pos.is_empty() {
                self.deposits.remove(&user);
            }
        }

        // Adjust aggregate total
        if let Some(total) = self.total_stablecoin_balances.get_mut(&token_ledger) {
            if diff > 0 {
                *total = total.saturating_sub(diff as u64);
            } else {
                *total = total.saturating_add((-diff) as u64);
            }
        }

        format!(
            "Corrected {} balance for {}: {} -> {}",
            token_ledger, user, old_amount, correct_amount
        )
    }

    /// Set a depositor's collateral gain for a specific collateral type to `correct_amount`.
    /// Used to fix drift between tracked gains and actual ledger balance (e.g., transfer fee dust).
    pub fn correct_collateral_gain(
        &mut self,
        user: Principal,
        collateral_ledger: Principal,
        correct_amount: u64,
    ) -> String {
        let old_amount = self
            .deposits
            .get(&user)
            .and_then(|pos| pos.collateral_gains.get(&collateral_ledger).copied())
            .unwrap_or(0);

        if old_amount == correct_amount {
            return format!(
                "No change needed: user {} gain for {} is already {}",
                user, collateral_ledger, correct_amount
            );
        }

        if let Some(pos) = self.deposits.get_mut(&user) {
            if correct_amount == 0 {
                pos.collateral_gains.remove(&collateral_ledger);
            } else {
                pos.collateral_gains
                    .insert(collateral_ledger, correct_amount);
            }
        }

        format!(
            "Corrected {} collateral gain for {}: {} -> {}",
            collateral_ledger, user, old_amount, correct_amount
        )
    }

    // ─── State Validation ───

    pub fn validate_state(&self) -> Result<(), String> {
        if let Some(payouts) = self.pending_outbound_payouts.as_ref() {
            for ((caller, ledger), payout) in payouts {
                let total = payout
                    .transfer_amount
                    .checked_add(payout.transfer_fee)
                    .ok_or_else(|| {
                        format!(
                            "Outbound payout amount overflow for {} / {}",
                            caller, ledger
                        )
                    })?;
                if payout.gross_amount == 0 || total != payout.gross_amount {
                    return Err(format!(
                        "Invalid outbound payout tuple for {} / {}",
                        caller, ledger
                    ));
                }
            }
        }
        for (ledger, &tracked_total) in &self.total_stablecoin_balances {
            let computed_total: u64 = self
                .deposits
                .values()
                .map(|pos| pos.stablecoin_balances.get(ledger).copied().unwrap_or(0))
                .sum();
            if computed_total != tracked_total {
                return Err(format!(
                    "Stablecoin total mismatch for {}: tracked={}, computed={}",
                    ledger, tracked_total, computed_total
                ));
            }
        }
        Ok(())
    }
}

// ─── Thread-local state + accessors ───

thread_local! {
    static STATE: RefCell<StabilityPoolState> = RefCell::new(StabilityPoolState::default());
}

pub fn mutate_state<F, R>(f: F) -> R
where
    F: FnOnce(&mut StabilityPoolState) -> R,
{
    STATE.with(|s| f(&mut s.borrow_mut()))
}

pub fn read_state<F, R>(f: F) -> R
where
    F: FnOnce(&StabilityPoolState) -> R,
{
    STATE.with(|s| f(&s.borrow()))
}

pub fn replace_state(state: StabilityPoolState) {
    STATE.with(|s| {
        *s.borrow_mut() = state;
    });
}

/// Serialize state to stable memory (called from pre_upgrade).
//
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
        let bytes = Encode!(&*state).expect("Failed to encode stability pool state");
        let len = bytes.len() as u64;

        // Only grow if current stable memory is insufficient.
        // Pages are 64 KiB each and never shrink, so avoid redundant grows.
        let needed_pages = (len + 8 + 65535) / 65536;
        let current_pages = ic_cdk::api::stable::stable64_size();
        if needed_pages > current_pages {
            ic_cdk::api::stable::stable64_grow(needed_pages - current_pages)
                .expect("Failed to grow stable memory");
        }

        // Write length prefix (8 bytes) then data
        ic_cdk::api::stable::stable64_write(0, &len.to_le_bytes());
        ic_cdk::api::stable::stable64_write(8, &bytes);
    });
}

/// Pre-IC-S-001 snapshot of `StabilityPoolState` (before the `pending_refunds`
/// and `next_pending_refund_id` fields). The new fields are `opt`, so the
/// current decoder already accepts old bytes; this snapshot is the mandated
/// versioned fallback (UPG-001 chain) in case that ever changes.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct StabilityPoolStateV1 {
    pub deposits: BTreeMap<Principal, DepositPosition>,
    pub total_stablecoin_balances: BTreeMap<Principal, u64>,
    pub stablecoin_registry: BTreeMap<Principal, StablecoinConfig>,
    pub collateral_registry: BTreeMap<Principal, CollateralInfo>,
    pub protocol_canister_id: Principal,
    pub configuration: PoolConfiguration,
    pub liquidation_history: Vec<PoolLiquidationRecord>,
    pub in_flight_liquidations: BTreeSet<u64>,
    pub total_liquidations_executed: u64,
    pub pool_creation_timestamp: u64,
    #[serde(default)]
    pub total_interest_received_e8s: Option<u64>,
    #[serde(default)]
    pub token_consecutive_failures: Option<BTreeMap<Principal, u32>>,
    #[serde(default)]
    pub cached_virtual_prices: Option<BTreeMap<Principal, u128>>,
    #[serde(default)]
    pub protocol_reserve_address: Option<Principal>,
    pub is_initialized: bool,
    #[serde(default)]
    pub pool_events: Option<Vec<PoolEvent>>,
    #[serde(default)]
    pub next_event_id: Option<u64>,
}

impl From<StabilityPoolStateV1> for StabilityPoolState {
    fn from(v1: StabilityPoolStateV1) -> Self {
        Self {
            deposits: v1.deposits,
            total_stablecoin_balances: v1.total_stablecoin_balances,
            stablecoin_registry: v1.stablecoin_registry,
            collateral_registry: v1.collateral_registry,
            chain_collateral_sentinels: Some(BTreeSet::new()),
            chain_claim_sources: Some(BTreeMap::new()),
            pending_chain_absorbs: Some(BTreeMap::new()),
            completed_chain_absorbs: Some(BTreeMap::new()),
            pending_native_xrp_absorbs: Some(BTreeMap::new()),
            chain_absorb_auto_config: Some(ChainAbsorbAutoConfig::default()),
            chain_absorb_auto_last_tick: None,
            completed_cfx_claim_payout_recoveries: Some(BTreeMap::new()),
            completed_cfx_claim_payout_recovery_floor: Some(BTreeMap::new()),
            protocol_canister_id: v1.protocol_canister_id,
            configuration: v1.configuration,
            liquidation_history: v1.liquidation_history,
            in_flight_liquidations: v1.in_flight_liquidations,
            total_liquidations_executed: v1.total_liquidations_executed,
            pool_creation_timestamp: v1.pool_creation_timestamp,
            total_interest_received_e8s: v1.total_interest_received_e8s,
            token_consecutive_failures: v1.token_consecutive_failures,
            cached_virtual_prices: v1.cached_virtual_prices,
            protocol_reserve_address: v1.protocol_reserve_address,
            interest_treasury: None,
            unallocated_interest_forward_batches: Some(BTreeMap::new()),
            next_unallocated_interest_forward_batch_id: Some(0),
            unallocated_interest_mint_index: Some(BTreeMap::new()),
            is_initialized: v1.is_initialized,
            pool_events: v1.pool_events,
            next_event_id: v1.next_event_id,
            pending_refunds: Some(BTreeMap::new()),
            next_pending_refund_id: Some(0),
            last_deposit_transfer_created_at: None,
            pending_deposit_intents: Some(BTreeMap::new()),
            pending_outbound_payouts: Some(BTreeMap::new()),
            last_outbound_payout_created_at_ns: None,
            next_sp_liquidation_request_id: Some(0),
            pending_sp_liquidations_v2: Some(BTreeMap::new()),
            sp_liquidation_v2_recovery_cursor: None,
            completed_sp_liquidations_v2: Some(BTreeMap::new()),
            completed_sp_liquidation_request_floor: Some(0),
            pending_sp_legacy_approval_fees: Some(BTreeMap::new()),
            next_sp_three_usd_absorb_id: Some(1),
            pending_sp_three_usd_absorbs: Some(BTreeMap::new()),
            sp_three_usd_recovery_cursor: None,
            completed_sp_three_usd_absorbs: Some(BTreeMap::new()),
            completed_sp_three_usd_absorb_floor: Some(1),
            processed_interest_mint_blocks: Some(BTreeSet::new()),
            processed_interest_mint_payloads: Some(BTreeMap::new()),
            processed_interest_mint_block_high_watermark: None,
            pending_refund_fee_reserves: Some(BTreeMap::new()),
            pending_refund_fee_funding_blocks: Some(BTreeSet::new()),
        }
    }
}

/// Try to deserialize a stability pool state snapshot, walking known schema
/// versions in order. Returns `None` if no version successfully decodes.
///
/// Adding a new schema (UPG-001 multi-version fallback): when a non-additive
/// change ships, copy the current `StabilityPoolState` definition as
/// `StabilityPoolStateVN` (with the appropriate `From<StabilityPoolStateVN>
/// for StabilityPoolState` conversion) and add a fallback branch below before
/// the existing ones. Keep at least the previous 2 to 3 versions.
pub fn try_decode_state(bytes: &[u8]) -> Option<StabilityPoolState> {
    // v-current.
    if let Ok(state) = Decode!(bytes, StabilityPoolState) {
        if !current_financial_journals_match_wire(bytes, &state) {
            return None;
        }
        return Some(state);
    }
    // A legacy Candid record is a structural supertype of newer records: it
    // can decode while silently ignoring fields it does not know. Only allow
    // the V1 migration when the wire record has none of the financial journal
    // fields introduced after V1. This also catches a current snapshot whose
    // nested journal type no longer decodes under the current Rust schema.
    if !legacy_fallback_has_no_new_journals(bytes) {
        return None;
    }
    // v1: pre-IC-S-001 (no pending_refunds / next_pending_refund_id).
    if let Ok(prev) = Decode!(bytes, StabilityPoolStateV1) {
        return Some(prev.into());
    }
    None
}

fn legacy_fallback_has_no_new_journals(bytes: &[u8]) -> bool {
    const NEW_FINANCIAL_JOURNALS: &[&str] = &[
        "pending_chain_absorbs",
        "pending_native_xrp_absorbs",
        "completed_cfx_claim_payout_recoveries",
        "pending_refunds",
        "pending_outbound_payouts",
        "pending_sp_liquidations_v2",
        "pending_sp_three_usd_absorbs",
        "unallocated_interest_forward_batches",
    ];
    let Some(args) = parse_state_record(bytes) else {
        return false;
    };
    let Some(fields) = state_record_fields(&args) else {
        return false;
    };
    !fields.iter().any(|field| {
        NEW_FINANCIAL_JOURNALS
            .iter()
            .any(|name| field.id.get_id() == candid::idl_hash(name))
    })
}

fn current_financial_journals_match_wire(bytes: &[u8], state: &StabilityPoolState) -> bool {
    let Some(args) = parse_state_record(bytes) else {
        return false;
    };
    let Some(fields) = state_record_fields(&args) else {
        return false;
    };
    [
        (
            "pending_chain_absorbs",
            state.pending_chain_absorbs.is_some(),
        ),
        (
            "pending_native_xrp_absorbs",
            state.pending_native_xrp_absorbs.is_some(),
        ),
        (
            "completed_cfx_claim_payout_recoveries",
            state.completed_cfx_claim_payout_recoveries.is_some(),
        ),
        ("pending_refunds", state.pending_refunds.is_some()),
        (
            "pending_outbound_payouts",
            state.pending_outbound_payouts.is_some(),
        ),
        (
            "pending_sp_liquidations_v2",
            state.pending_sp_liquidations_v2.is_some(),
        ),
        (
            "pending_sp_three_usd_absorbs",
            state.pending_sp_three_usd_absorbs.is_some(),
        ),
        (
            "unallocated_interest_forward_batches",
            state.unallocated_interest_forward_batches.is_some(),
        ),
    ]
    .iter()
    .all(|(name, decoded_has_value)| {
        let field = fields
            .iter()
            .find(|field| field.id.get_id() == candid::idl_hash(name));
        match field {
            None => true, // A genuine historical snapshot omitted the field.
            Some(field) => match &field.val {
                candid::IDLValue::None => !decoded_has_value,
                candid::IDLValue::Opt(_) => *decoded_has_value,
                _ => false,
            },
        }
    })
}

fn parse_state_record(bytes: &[u8]) -> Option<candid::IDLArgs> {
    candid::IDLArgs::from_bytes(bytes).ok()
}

fn state_record_fields(args: &candid::IDLArgs) -> Option<&[candid::types::value::IDLField]> {
    match args.args.as_slice() {
        [candid::IDLValue::Record(fields)] => Some(fields),
        _ => None,
    }
}

/// Restore state from stable memory (called from post_upgrade).
///
/// UPG-001 fix: rather than trapping on decode failure (which bricks the
/// canister until a hotfix wasm with a compatible decoder is shipped), walk
/// the known-version fallback chain via `try_decode_state`. If every known
/// version fails, TRAP (audit 2026-06-05, UPG-101).
///
/// The previous behavior wiped to empty state, which ZEROES every depositor's
/// position — the most destructive possible outcome for a stability pool and
/// exactly the 2026-05-18 silent-state-wipe incident class. Trapping instead
/// keeps the canister on its old wasm with stable memory intact, so an operator
/// can ship a wasm with a matching `StabilityPoolStateVN` snapshot and recover
/// every position. This matches the backend (UPG-001) and the other satellites,
/// which all trap-not-wipe on an undecodable snapshot.
pub fn load_from_stable_memory() {
    let mut len_bytes = [0u8; 8];
    ic_cdk::api::stable::stable64_read(0, &mut len_bytes);
    let len = u64::from_le_bytes(len_bytes) as usize;

    if len == 0 {
        return; // No saved state, fresh start.
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
        "CRITICAL UPG-001/UPG-101: stability pool snapshot decode failed for all known schema \
         versions. snapshot_len={} bytes, first_{}_bytes_hex={}. \
         Trapping to preserve on-chain state (old wasm + stable memory stay intact) rather than \
         wiping every depositor position. Ship a wasm with a matching StabilityPoolStateVN \
         snapshot to recover.",
        bytes.len(),
        preview_len,
        preview_hex
    );
    ic_cdk::trap(
        "stability_pool post_upgrade: stable state did not decode under any known schema version; \
         refusing to wipe depositor positions — see CRITICAL log",
    );
}

// ──────────────────────────────────────────────────────────────
// Unit tests
// ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    // Deterministic test principals
    fn user_a() -> Principal {
        Principal::from_slice(&[1])
    }
    fn user_b() -> Principal {
        Principal::from_slice(&[2])
    }
    fn user_c() -> Principal {
        Principal::from_slice(&[3])
    }
    fn icusd_ledger() -> Principal {
        Principal::from_slice(&[10])
    }
    fn ckusdt_ledger() -> Principal {
        Principal::from_slice(&[11])
    }
    fn ckusdc_ledger() -> Principal {
        Principal::from_slice(&[12])
    }
    fn icp_ledger() -> Principal {
        Principal::from_slice(&[20])
    }
    fn ckbtc_ledger() -> Principal {
        Principal::from_slice(&[21])
    }
    fn cfx_sentinel() -> Principal {
        Principal::from_slice(&[30])
    }
    fn xrp_ledger() -> Principal {
        rumi_protocol_backend::state::xrp_collateral_principal()
    }
    fn valid_xrp_address() -> String {
        "rUn84CUYbNjRoTQ6mSW7BVJPSVJNLb1QLo".to_string()
    }

    fn prepare_v2_request(state: &mut StabilityPoolState, vault_id: u64) -> u64 {
        let protocol = state.protocol_canister_id;
        state
            .prepare_sp_liquidation_v2(
                vault_id,
                icp_ledger(),
                100_000_000,
                icusd_ledger(),
                1_000_000,
                SpLiquidationToken::IcUsd,
                SpLiquidationV2ApprovalTuple {
                    ledger: icusd_ledger(),
                    owner: icrc_ledger_types::icrc1::account::Account {
                        owner: Principal::from_slice(&[40]),
                        subaccount: None,
                    },
                    spender: icrc_ledger_types::icrc1::account::Account {
                        owner: protocol,
                        subaccount: None,
                    },
                    allowance_raw: 2_000_000,
                    fee_raw: 10_000,
                    memo: vec![7],
                    created_at_time_ns: 11,
                    expires_at_ns: 22,
                    fee_accounted: false,
                },
            )
            .expect("request is prepared")
            .request
            .request_id
    }

    /// Build a test state with:
    /// - icUSD (8 decimals, priority 1)
    /// - ckUSDT (6 decimals, priority 2)
    /// - ckUSDC (6 decimals, priority 2)
    /// - ICP collateral (8 decimals, Active)
    /// - ckBTC collateral (8 decimals, Active)
    fn test_state() -> StabilityPoolState {
        let mut state = StabilityPoolState::default();

        state.register_stablecoin(StablecoinConfig {
            ledger_id: icusd_ledger(),
            symbol: "icUSD".to_string(),
            decimals: 8,
            priority: 1,
            is_active: true,
            transfer_fee: Some(100_000),
            is_lp_token: None,
            underlying_pool: None,
        });
        state.register_stablecoin(StablecoinConfig {
            ledger_id: ckusdt_ledger(),
            symbol: "ckUSDT".to_string(),
            decimals: 6,
            priority: 2,
            is_active: true,
            transfer_fee: Some(10_000),
            is_lp_token: None,
            underlying_pool: None,
        });
        state.register_stablecoin(StablecoinConfig {
            ledger_id: ckusdc_ledger(),
            symbol: "ckUSDC".to_string(),
            decimals: 6,
            priority: 2,
            is_active: true,
            transfer_fee: Some(10_000),
            is_lp_token: None,
            underlying_pool: None,
        });

        state.register_collateral(CollateralInfo {
            ledger_id: icp_ledger(),
            symbol: "ICP".to_string(),
            decimals: 8,
            status: CollateralStatus::Active,
        });
        state.register_collateral(CollateralInfo {
            ledger_id: ckbtc_ledger(),
            symbol: "ckBTC".to_string(),
            decimals: 8,
            status: CollateralStatus::Active,
        });
        state.register_collateral(CollateralInfo {
            ledger_id: xrp_ledger(),
            symbol: "XRP".to_string(),
            decimals: 6,
            status: CollateralStatus::Active,
        });

        state
    }

    #[test]
    fn same_round_identical_deposit_callbacks_credit_one_transfer_once() {
        let mut state = StabilityPoolState::default();
        let amount = 25_000_000;
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 0);
        let first_timestamp = state
            .begin_deposit_intent(user_a(), icusd_ledger(), amount, 1_000)
            .unwrap();
        let retry_timestamp = state
            .begin_deposit_intent(user_a(), icusd_ledger(), amount, 1_000)
            .unwrap();

        assert_eq!(retry_timestamp, first_timestamp);
        assert_eq!(
            state.pending_deposit_intents.as_ref().unwrap()[&user_a()].in_flight_attempts,
            Some(2),
        );
        assert!(!state.pending_deposit_intents.as_ref().unwrap()[&user_a()].ambiguous_seen);
        assert!(state.record_deposit_receipt(user_a(), icusd_ledger(), amount, first_timestamp, 7,));
        assert_eq!(
            state.pending_deposit_intents.as_ref().unwrap()[&user_a()].in_flight_attempts,
            Some(1),
            "one ledger response must finish only its own dispatch",
        );
        assert!(!state.clear_deposit_intent_after_no_effect(
            user_a(),
            icusd_ledger(),
            amount,
            retry_timestamp,
        ));
        assert_eq!(
            state.pending_deposit_intents.as_ref().unwrap()[&user_a()].transfer_block_index,
            Some(7),
            "a no-effect callback cannot erase a receipt already proven by its peer",
        );
        assert!(state.complete_deposit_intent(
            user_a(),
            icusd_ledger(),
            amount,
            first_timestamp,
            2_000,
        ));
        assert!(!state.complete_deposit_intent(
            user_a(),
            icusd_ledger(),
            amount,
            retry_timestamp,
            2_001,
        ));
        assert_eq!(
            state.deposits[&user_a()].stablecoin_balances[&icusd_ledger()],
            amount,
        );
        assert_eq!(state.total_stablecoin_balances[&icusd_ledger()], amount);
        assert_eq!(state.pool_events.as_ref().unwrap().len(), 1);

        // Once the receipt is finalized, another identical request in the
        // same IC time round receives a fresh transfer identity.
        let next_timestamp = state
            .begin_deposit_intent(user_a(), icusd_ledger(), amount, 1_000)
            .unwrap();
        assert!(next_timestamp > first_timestamp);
    }

    #[test]
    fn too_old_deposit_identity_stays_fenced_until_fixed_tip_absence_proof_completes() {
        let mut state = StabilityPoolState::default();
        let caller = user_a();
        let ledger = icusd_ledger();
        let amount = 25_000_000;
        let original = state
            .begin_deposit_intent(caller, ledger, amount, 1_000)
            .unwrap();
        state
            .begin_deposit_intent(caller, ledger, amount, 1_000)
            .unwrap();

        assert!(state.mark_deposit_intent_too_old(caller, ledger, amount, original));
        assert_eq!(
            state.pending_deposit_intents.as_ref().unwrap()[&caller].in_flight_attempts,
            Some(1),
            "the other exact-identity dispatch must still be accounted for"
        );
        assert!(state.start_deposit_history_scan(caller, 130, 0).is_err());
        state.mark_deposit_intent_ambiguous(caller, ledger, amount, original);
        assert_eq!(
            state.pending_deposit_intents.as_ref().unwrap()[&caller].too_old_rejected,
            Some(true)
        );
        assert!(state
            .begin_deposit_intent(caller, ledger, amount, 2_000)
            .is_err());
        let scan = state.claim_deposit_history_scan(caller, 1_000).unwrap();
        let generation = scan.reconciliation_generation.unwrap();
        assert!(state.claim_deposit_history_scan(caller, 1_000).is_err());
        assert!(state
            .start_deposit_history_scan(caller, 130, generation)
            .is_ok());
        assert!(state
            .advance_deposit_history_scan(caller, 0, 130, 64, 1_000, generation)
            .is_ok());
        assert!(state.rotate_deposit_after_no_effect(caller, 2_000).is_err());

        let scan = state
            .claim_deposit_history_scan(caller, 5_000_001_000)
            .unwrap();
        let generation = scan.reconciliation_generation.unwrap();
        assert!(state
            .advance_deposit_history_scan(caller, 64, 130, 130, 5_000_001_000, generation)
            .is_ok());
        let rotated = state
            .rotate_deposit_after_no_effect(caller, 2_000)
            .expect("only a complete fixed-tip scan may rotate identity");
        assert!(rotated.transfer_created_at_time_ns > original);
        assert_eq!(rotated.amount, amount);
        assert_eq!(rotated.token_ledger, ledger);
        assert_eq!(rotated.attempt_no, Some(1));
        assert_eq!(rotated.in_flight_attempts, Some(0));
        assert_eq!(rotated.too_old_rejected, None);
        assert_eq!(rotated.history_scan_cursor, None);
        assert_eq!(rotated.history_scan_tip, None);
        assert_eq!(
            state.begin_deposit_intent(caller, ledger, amount, 2_001),
            Ok(rotated.transfer_created_at_time_ns),
            "the next dispatch uses the one newly authorized identity"
        );
    }

    #[test]
    fn reconciled_deposit_receipt_does_not_consume_a_dispatch_or_credit_twice() {
        let mut state = StabilityPoolState::default();
        let caller = user_a();
        let ledger = icusd_ledger();
        let amount = 25_000_000;
        let timestamp = state
            .begin_deposit_intent(caller, ledger, amount, 1_000)
            .unwrap();
        assert_eq!(
            state.pending_deposit_intents.as_ref().unwrap()[&caller].in_flight_attempts,
            Some(1)
        );
        let claim = state
            .claim_pending_deposit_reconciliation(caller, 1_000)
            .unwrap();
        assert!(state
            .claim_pending_deposit_reconciliation(caller, 1_000)
            .is_err());
        assert!(state.complete_reconciled_deposit(
            caller,
            ledger,
            amount,
            timestamp,
            7,
            2_000,
            claim.reconciliation_generation.unwrap(),
        ));
        assert!(
            !state.record_deposit_receipt(caller, ledger, amount, timestamp, 7),
            "a late callback is fenced by the completed intent's removal"
        );
        assert!(!state.complete_deposit_intent(caller, ledger, amount, timestamp, 2_001));
        assert_eq!(state.total_stablecoin_balances[&ledger], amount);
        assert_eq!(state.pool_events.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn stale_reconciliation_generation_cannot_release_or_commit_new_lease() {
        let mut state = StabilityPoolState::default();
        let caller = user_a();
        let ledger = icusd_ledger();
        let amount = 25_000_000;
        let timestamp = state
            .begin_deposit_intent(caller, ledger, amount, 1_000)
            .unwrap();
        let old_claim = state
            .claim_pending_deposit_reconciliation(caller, 1_000)
            .unwrap();
        let old_generation = old_claim.reconciliation_generation.unwrap();

        let reclaimed = state
            .claim_pending_deposit_reconciliation(caller, 900_000_001_000)
            .unwrap();
        let reclaimed_generation = reclaimed.reconciliation_generation.unwrap();
        assert!(reclaimed_generation > old_generation);
        state.release_deposit_history_scan(caller, old_generation, 900_000_002_000);
        assert_eq!(
            state.pending_deposit_intents.as_ref().unwrap()[&caller].reconciliation_in_progress,
            Some(true),
            "an expired future cannot release the reclaimed lease"
        );

        state.reconcile_pending_deposit_attempts_after_upgrade();
        let new_claim = state
            .claim_pending_deposit_reconciliation(caller, 900_000_003_000)
            .unwrap();
        let new_generation = new_claim.reconciliation_generation.unwrap();
        assert!(new_generation > reclaimed_generation);
        state.release_deposit_history_scan(caller, reclaimed_generation, 900_000_004_000);
        assert_eq!(
            state.pending_deposit_intents.as_ref().unwrap()[&caller].reconciliation_in_progress,
            Some(true),
            "a stale future cannot release the newer lease"
        );
        assert!(!state.complete_reconciled_deposit(
            caller,
            ledger,
            amount,
            timestamp,
            7,
            900_000_005_000,
            old_generation,
        ));
        state.release_deposit_history_scan(caller, new_generation, 900_000_006_000);
        assert!(
            state
                .claim_pending_deposit_reconciliation(caller, 900_000_006_000)
                .is_err(),
            "wrong-proof cooldown remains active"
        );
        assert!(state
            .claim_pending_deposit_reconciliation(caller, 905_000_006_000)
            .is_ok());
    }

    #[test]
    fn concurrent_definitive_deposit_failures_clear_only_after_both_replies() {
        let mut state = StabilityPoolState::default();
        let caller = user_a();
        let ledger = icusd_ledger();
        let amount = 25_000_000;
        let first_timestamp = state
            .begin_deposit_intent(caller, ledger, amount, 1_000)
            .unwrap();
        let second_timestamp = state
            .begin_deposit_intent(caller, ledger, amount, 1_000)
            .unwrap();
        assert_eq!(first_timestamp, second_timestamp);

        assert!(!state.clear_deposit_intent_after_no_effect(
            caller,
            ledger,
            amount,
            first_timestamp,
        ));
        assert_eq!(
            state.pending_deposit_intents.as_ref().unwrap()[&caller].in_flight_attempts,
            Some(1),
            "first definitive reply cannot clear while its peer is outstanding",
        );
        assert!(state
            .begin_deposit_intent(caller, ledger, amount + 1, 1_000)
            .is_err());

        assert!(state.clear_deposit_intent_after_no_effect(
            caller,
            ledger,
            amount,
            second_timestamp,
        ));
        assert!(state.pending_deposit_intents.as_ref().unwrap().is_empty());
        let next_timestamp = state
            .begin_deposit_intent(caller, ledger, amount + 1, 1_000)
            .expect("a fresh request is admitted after all attempts proved no effect");
        assert!(next_timestamp > first_timestamp);
    }

    #[test]
    fn ambiguous_and_definitive_deposit_replies_keep_exact_intent_for_receipt_retry() {
        let mut state = StabilityPoolState::default();
        let caller = user_a();
        let ledger = icusd_ledger();
        let amount = 25_000_000;
        let timestamp = state
            .begin_deposit_intent(caller, ledger, amount, 1_000)
            .unwrap();
        assert_eq!(
            state.begin_deposit_intent(caller, ledger, amount, 1_000),
            Ok(timestamp),
        );

        state.mark_deposit_intent_ambiguous(caller, ledger, amount, timestamp);
        assert!(!state.clear_deposit_intent_after_no_effect(caller, ledger, amount, timestamp,));
        let retained = &state.pending_deposit_intents.as_ref().unwrap()[&caller];
        assert_eq!(retained.in_flight_attempts, Some(0));
        assert!(retained.ambiguous_seen);

        assert!(state
            .begin_deposit_intent(caller, ledger, amount + 1, 1_000)
            .is_err());
        assert_eq!(
            state.begin_deposit_intent(caller, ledger, amount, 2_000),
            Ok(timestamp),
            "an exact retry must reuse the original ledger identity",
        );
        assert!(state.record_deposit_receipt(caller, ledger, amount, timestamp, 17));
        assert!(state.complete_deposit_intent(caller, ledger, amount, timestamp, 3_000));
        assert!(!state.complete_deposit_intent(caller, ledger, amount, timestamp, 3_001));
        assert_eq!(state.deposits[&caller].stablecoin_balances[&ledger], amount);
        assert_eq!(state.total_stablecoin_balances[&ledger], amount);
        assert_eq!(state.pool_events.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn upgrade_marks_unresolved_and_pre_counter_deposit_intents_ambiguous() {
        #[derive(CandidType)]
        struct PendingDepositIntentBeforeAttemptCounter {
            token_ledger: Principal,
            amount: u64,
            transfer_created_at_time_ns: u64,
            transfer_block_index: Option<u64>,
            ambiguous_seen: bool,
        }

        let old = PendingDepositIntentBeforeAttemptCounter {
            token_ledger: icusd_ledger(),
            amount: 25_000_000,
            transfer_created_at_time_ns: 1_000,
            transfer_block_index: None,
            ambiguous_seen: false,
        };
        let bytes = Encode!(&old).unwrap();
        let decoded: PendingDepositIntent =
            Decode!(&bytes, PendingDepositIntent).expect("pre-counter intent still decodes");
        assert_eq!(decoded.in_flight_attempts, None);

        let mut state = StabilityPoolState::default();
        state
            .pending_deposit_intents
            .as_mut()
            .unwrap()
            .insert(user_a(), decoded);
        // Also model a candidate snapshot taken while a tracked dispatch was
        // awaiting its ledger reply.
        state
            .begin_deposit_intent(user_b(), icusd_ledger(), 25_000_000, 1_000)
            .unwrap();
        state.reconcile_pending_deposit_attempts_after_upgrade();

        for caller in [user_a(), user_b()] {
            let intent = &state.pending_deposit_intents.as_ref().unwrap()[&caller];
            assert_eq!(intent.in_flight_attempts, Some(0));
            assert!(intent.ambiguous_seen);
        }
        assert!(!state.clear_deposit_intent_after_no_effect(
            user_a(),
            icusd_ledger(),
            25_000_000,
            1_000,
        ));
        assert!(state
            .pending_deposit_intents
            .as_ref()
            .unwrap()
            .contains_key(&user_a()));
    }

    #[test]
    fn ordinary_and_convenience_deposits_share_timestamp_allocator() {
        let mut state = StabilityPoolState::default();
        let caller = user_a();
        let ledger = icusd_ledger();
        let ordinary = state
            .begin_deposit_intent(caller, ledger, 25_000_000, 1_000)
            .unwrap();
        let convenience = state.reserve_deposit_transfer_timestamp(1_000).unwrap();
        let next_ordinary = state
            .begin_deposit_intent(user_b(), ledger, 25_000_000, 1_000)
            .unwrap();

        assert!(ordinary < convenience);
        assert!(convenience < next_ordinary);
        assert_eq!(state.last_deposit_transfer_created_at, Some(next_ordinary));
    }

    #[test]
    fn test_known_stablecoin_fee_normalization_repairs_legacy_ckstable_values() {
        let mut state = StabilityPoolState::default();

        state.register_stablecoin(StablecoinConfig {
            ledger_id: ckusdc_ledger(),
            symbol: "ckUSDC".to_string(),
            decimals: 6,
            priority: 2,
            is_active: true,
            transfer_fee: Some(10),
            is_lp_token: None,
            underlying_pool: None,
        });
        state.register_stablecoin(StablecoinConfig {
            ledger_id: ckusdt_ledger(),
            symbol: "ckUSDT".to_string(),
            decimals: 6,
            priority: 2,
            is_active: true,
            transfer_fee: None,
            is_lp_token: None,
            underlying_pool: None,
        });
        state.register_stablecoin(StablecoinConfig {
            ledger_id: Principal::from_slice(&[13]),
            symbol: "3USD".to_string(),
            decimals: 8,
            priority: 0,
            is_active: true,
            transfer_fee: None,
            is_lp_token: Some(true),
            underlying_pool: None,
        });

        assert_eq!(
            state
                .stablecoin_registry
                .get(&ckusdc_ledger())
                .and_then(|c| c.transfer_fee),
            Some(10_000)
        );
        assert_eq!(
            state
                .stablecoin_registry
                .get(&ckusdt_ledger())
                .and_then(|c| c.transfer_fee),
            Some(10_000)
        );
        assert_eq!(
            state
                .stablecoin_registry
                .get(&Principal::from_slice(&[13]))
                .and_then(|c| c.transfer_fee),
            Some(0)
        );
    }

    /// Helper: directly add a deposit without ic_cdk::api::time().
    fn add_deposit_direct(
        state: &mut StabilityPoolState,
        user: Principal,
        token: Principal,
        amount: u64,
    ) {
        let position = state
            .deposits
            .entry(user)
            .or_insert_with(|| DepositPosition::new(0));
        *position.stablecoin_balances.entry(token).or_insert(0) += amount;
        *state.total_stablecoin_balances.entry(token).or_insert(0) += amount;
    }

    // ─── Test: Deposit and Withdrawal ───

    #[test]
    fn test_deposit_and_withdrawal() {
        let mut state = test_state();

        // Add deposits for user_a
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_000_000); // 1 icUSD
        add_deposit_direct(&mut state, user_a(), ckusdt_ledger(), 2_000_000); // 2 ckUSDT

        // Verify balances
        let pos = state.deposits.get(&user_a()).unwrap();
        assert_eq!(
            pos.stablecoin_balances.get(&icusd_ledger()),
            Some(&100_000_000)
        );
        assert_eq!(
            pos.stablecoin_balances.get(&ckusdt_ledger()),
            Some(&2_000_000)
        );

        // Verify aggregate totals
        assert_eq!(
            state.total_stablecoin_balances.get(&icusd_ledger()),
            Some(&100_000_000)
        );
        assert_eq!(
            state.total_stablecoin_balances.get(&ckusdt_ledger()),
            Some(&2_000_000)
        );

        // Partial withdrawal
        state
            .process_withdrawal(user_a(), icusd_ledger(), 30_000_000)
            .unwrap();
        let pos = state.deposits.get(&user_a()).unwrap();
        assert_eq!(
            pos.stablecoin_balances.get(&icusd_ledger()),
            Some(&70_000_000)
        );
        assert_eq!(
            state.total_stablecoin_balances.get(&icusd_ledger()),
            Some(&70_000_000)
        );

        // Full withdrawal of ckUSDT -- zero-balance entry should be cleaned up
        state
            .process_withdrawal(user_a(), ckusdt_ledger(), 2_000_000)
            .unwrap();
        let pos = state.deposits.get(&user_a()).unwrap();
        assert_eq!(pos.stablecoin_balances.get(&ckusdt_ledger()), None);

        // Full withdrawal of remaining icUSD -- empty position should be removed
        state
            .process_withdrawal(user_a(), icusd_ledger(), 70_000_000)
            .unwrap();
        assert!(
            state.deposits.get(&user_a()).is_none(),
            "Empty position should be removed"
        );

        // Attempt to withdraw from nonexistent position
        let err = state
            .process_withdrawal(user_a(), icusd_ledger(), 1)
            .unwrap_err();
        assert!(matches!(err, StabilityPoolError::NoPositionFound));
    }

    #[test]
    fn test_withdrawal_insufficient_balance() {
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 50_000_000);

        let err = state
            .process_withdrawal(user_a(), icusd_ledger(), 100_000_000)
            .unwrap_err();
        match err {
            StabilityPoolError::InsufficientBalance {
                token,
                required,
                available,
            } => {
                assert_eq!(token, icusd_ledger());
                assert_eq!(required, 100_000_000);
                assert_eq!(available, 50_000_000);
            }
            _ => panic!("Expected InsufficientBalance error"),
        }
    }

    // ─── Test: Token Draw — Single Priority ───

    #[test]
    fn test_token_draw_single_priority() {
        let mut state = test_state();

        // Only ckstables at priority 2: 60 ckUSDT + 40 ckUSDC = 100 USD total
        add_deposit_direct(&mut state, user_a(), ckusdt_ledger(), 60_000_000); // 60 ckUSDT (6 dec)
        add_deposit_direct(&mut state, user_b(), ckusdc_ledger(), 40_000_000); // 40 ckUSDC (6 dec)

        // Draw 50 USD (50_00000000 e8s) worth
        let draw = state.compute_token_draw(50_00000000, &icp_ledger());

        // Proportional: ckUSDT has 60% of the pool, ckUSDC has 40%
        // ckUSDT draw: 50 * 60/100 = 30 USD = 30_000_000 native (6 dec)
        // ckUSDC draw: 50 * 40/100 = 20 USD = 20_000_000 native (6 dec)
        let usdt_draw = draw.get(&ckusdt_ledger()).copied().unwrap_or(0);
        let usdc_draw = draw.get(&ckusdc_ledger()).copied().unwrap_or(0);

        assert_eq!(usdt_draw, 30_000_000, "ckUSDT should contribute 30 USD");
        assert_eq!(usdc_draw, 20_000_000, "ckUSDC should contribute 20 USD");
    }

    // ─── Test: Token Draw — Mixed Priorities ───

    #[test]
    fn test_token_draw_mixed_priorities() {
        let mut state = test_state();

        // ckUSDT (priority 2): 100 USD
        // icUSD (priority 1): 200 USD
        add_deposit_direct(&mut state, user_a(), ckusdt_ledger(), 100_000_000); // 100 ckUSDT
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 200_00000000); // 200 icUSD

        // Draw 80 USD -- should come entirely from ckUSDT (higher priority)
        let draw = state.compute_token_draw(80_00000000, &icp_ledger());

        let usdt_draw = draw.get(&ckusdt_ledger()).copied().unwrap_or(0);
        let icusd_draw = draw.get(&icusd_ledger()).copied().unwrap_or(0);

        assert_eq!(
            usdt_draw, 80_000_000,
            "All 80 USD should come from ckUSDT (priority 2)"
        );
        assert_eq!(icusd_draw, 0, "icUSD (priority 1) should not be touched");
    }

    // ─── Test: Token Draw — Insufficient ckStables ───

    #[test]
    fn test_token_draw_insufficient_ckstables() {
        let mut state = test_state();

        // ckUSDT (priority 2): 30 USD
        // ckUSDC (priority 2): 20 USD
        // icUSD (priority 1): 200 USD
        add_deposit_direct(&mut state, user_a(), ckusdt_ledger(), 30_000_000); // 30 ckUSDT
        add_deposit_direct(&mut state, user_a(), ckusdc_ledger(), 20_000_000); // 20 ckUSDC
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 200_00000000); // 200 icUSD

        // Draw 80 USD -- ckstables can only cover 50, remainder from icUSD
        let draw = state.compute_token_draw(80_00000000, &icp_ledger());

        let usdt_draw = draw.get(&ckusdt_ledger()).copied().unwrap_or(0);
        let usdc_draw = draw.get(&ckusdc_ledger()).copied().unwrap_or(0);
        let icusd_draw = draw.get(&icusd_ledger()).copied().unwrap_or(0);

        // ckstables (priority 2) consumed first: 30 ckUSDT + 20 ckUSDC = 50 USD
        assert_eq!(usdt_draw, 30_000_000, "All ckUSDT consumed");
        assert_eq!(usdc_draw, 20_000_000, "All ckUSDC consumed");

        // Remaining 30 USD comes from icUSD (priority 1)
        assert_eq!(icusd_draw, 30_00000000, "icUSD covers remaining 30 USD");
    }

    // ─── Test: Liquidation Gains Distribution ───

    #[test]
    fn test_liquidation_gains_distribution() {
        let mut state = test_state();

        // 3 depositors with different icUSD balances:
        // user_a: 50 USD, user_b: 30 USD, user_c: 20 USD = 100 USD total
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 50_00000000);
        add_deposit_direct(&mut state, user_b(), icusd_ledger(), 30_00000000);
        add_deposit_direct(&mut state, user_c(), icusd_ledger(), 20_00000000);

        // Liquidation: 10 USD of debt absorbed, 5 ICP collateral gained
        let mut stables_consumed = BTreeMap::new();
        stables_consumed.insert(icusd_ledger(), 10_00000000); // 10 icUSD consumed

        state
            .process_liquidation_gains_at(
                1, // vault_id
                icp_ledger(),
                &stables_consumed,
                5_00000000,    // 5 ICP
                7_50000000,    // collateral price $7.50
                1_000_000_000, // timestamp
            )
            .unwrap();

        // Check proportional reduction of icUSD balances:
        // user_a consumed: 10 * (50/100) = 5 icUSD -> remaining: 45
        // user_b consumed: 10 * (30/100) = 3 icUSD -> remaining: 27
        // user_c consumed: 10 * (20/100) = 2 icUSD -> remaining: 18
        let pos_a = state.deposits.get(&user_a()).unwrap();
        let pos_b = state.deposits.get(&user_b()).unwrap();
        let pos_c = state.deposits.get(&user_c()).unwrap();

        assert_eq!(
            pos_a
                .stablecoin_balances
                .get(&icusd_ledger())
                .copied()
                .unwrap_or(0),
            45_00000000
        );
        assert_eq!(
            pos_b
                .stablecoin_balances
                .get(&icusd_ledger())
                .copied()
                .unwrap_or(0),
            27_00000000
        );
        assert_eq!(
            pos_c
                .stablecoin_balances
                .get(&icusd_ledger())
                .copied()
                .unwrap_or(0),
            18_00000000
        );

        // Check proportional collateral gains:
        // user_a gain: 5 ICP * (5/10) = 2.5 ICP = 2_50000000
        // user_b gain: 5 ICP * (3/10) = 1.5 ICP = 1_50000000
        // user_c gain: 5 ICP * (2/10) = 1.0 ICP = 1_00000000
        assert_eq!(
            pos_a
                .collateral_gains
                .get(&icp_ledger())
                .copied()
                .unwrap_or(0),
            2_50000000
        );
        assert_eq!(
            pos_b
                .collateral_gains
                .get(&icp_ledger())
                .copied()
                .unwrap_or(0),
            1_50000000
        );
        assert_eq!(
            pos_c
                .collateral_gains
                .get(&icp_ledger())
                .copied()
                .unwrap_or(0),
            1_00000000
        );

        // Verify aggregate total was reduced
        assert_eq!(
            state
                .total_stablecoin_balances
                .get(&icusd_ledger())
                .copied()
                .unwrap_or(0),
            90_00000000, // 100 - 10 = 90
        );

        // Verify liquidation history recorded
        assert_eq!(state.liquidation_history.len(), 1);
        assert_eq!(state.liquidation_history[0].vault_id, 1);
        assert_eq!(state.liquidation_history[0].depositors_count, 3);
        assert_eq!(state.total_liquidations_executed, 1);
    }

    #[test]
    fn legacy_liquidation_rounding_keeps_ledger_and_pool_balances_exact() {
        let mut state = test_state();
        for (owner, icusd, ckusdc) in [(user_a(), 1, 2), (user_b(), 2, 3), (user_c(), 3, 4)] {
            state.add_deposit_at(owner, icusd_ledger(), icusd, 0);
            state.add_deposit_at(owner, ckusdc_ledger(), ckusdc, 0);
        }
        for (owner, amount) in [(user_a(), 4), (user_b(), 5), (user_c(), 6)] {
            state.add_deposit_at(owner, ckusdt_ledger(), amount, 0);
        }

        // The ledger takes all five fee units. The legacy proportional floors
        // previously debited only three units across these three positions.
        state.deduct_fee_from_pool(icusd_ledger(), 5).unwrap();
        assert_eq!(
            state.total_stablecoin_balances.get(&icusd_ledger()),
            Some(&1)
        );
        assert_eq!(
            state
                .deposits
                .values()
                .map(|pos| pos
                    .stablecoin_balances
                    .get(&icusd_ledger())
                    .copied()
                    .unwrap_or(0))
                .sum::<u64>(),
            1,
        );

        // The backend pulls all eight units. Independent per-user floors used
        // to leave two phantom units in both depositor balances and aggregate.
        let collateral = icp_ledger();
        let consumed = BTreeMap::from([(ckusdc_ledger(), 8), (ckusdt_ledger(), 11)]);
        state
            .process_liquidation_gains_at(77, collateral, &consumed, 10, 1_000_000_000, 1)
            .unwrap();
        assert_eq!(
            state.total_stablecoin_balances.get(&ckusdc_ledger()),
            Some(&1)
        );
        assert_eq!(
            state
                .deposits
                .values()
                .map(|pos| pos
                    .stablecoin_balances
                    .get(&ckusdc_ledger())
                    .copied()
                    .unwrap_or(0))
                .sum::<u64>(),
            1,
        );
        assert_eq!(
            state.total_stablecoin_balances.get(&ckusdt_ledger()),
            Some(&4)
        );
        assert_eq!(
            state
                .deposits
                .values()
                .map(|pos| pos
                    .stablecoin_balances
                    .get(&ckusdt_ledger())
                    .copied()
                    .unwrap_or(0))
                .sum::<u64>(),
            4,
        );

        // The dedicated chain absorb has the same exact-debit requirement.
        state.register_chain_collateral_sentinel(cfx_sentinel());
        for owner in [user_a(), user_b(), user_c()] {
            state.opt_in_cfx(&owner, cfx_sentinel()).unwrap();
        }
        let chain_consumed = BTreeMap::from([(icusd_ledger(), 1)]);
        assert!(state.can_process_chain_liquidation_debits(cfx_sentinel(), &chain_consumed));
        state
            .process_chain_liquidation_gains_at(
                78,
                cfx_sentinel(),
                &chain_consumed,
                10,
                1_000_000_000,
                2,
            )
            .unwrap();
        assert_eq!(
            state.total_stablecoin_balances.get(&icusd_ledger()),
            Some(&0)
        );
        assert!(state
            .deposits
            .values()
            .all(|pos| !pos.stablecoin_balances.contains_key(&icusd_ledger())));
    }

    #[test]
    fn legacy_liquidation_books_the_fresh_fee_after_a_ledger_fee_change() {
        let mut state = test_state();
        for owner in [user_a(), user_b(), user_c()] {
            state.add_deposit_at(owner, icusd_ledger(), 20, 0);
        }

        // The process-local fee cache could still say 10 while the ledger has
        // changed to 13. Booking that stale amount after a successful approve
        // would leave the books at 50 while the ledger holds 47.
        let stale_cached_fee = 10;
        let current_ledger_fee = 13;
        assert!(state.can_deduct_fee_from_pool(icusd_ledger(), current_ledger_fee));
        state
            .deduct_fee_from_pool(icusd_ledger(), current_ledger_fee)
            .unwrap();

        assert_eq!(60 - current_ledger_fee, 47);
        assert_eq!(
            state.total_stablecoin_balances.get(&icusd_ledger()),
            Some(&47)
        );
        assert_eq!(
            state
                .deposits
                .values()
                .map(|position| position
                    .stablecoin_balances
                    .get(&icusd_ledger())
                    .copied()
                    .unwrap_or(0))
                .sum::<u64>(),
            47,
        );
        assert_ne!(60 - stale_cached_fee, 47);
    }

    #[test]
    fn legacy_approval_lost_reply_retries_exact_identity_and_books_once() {
        let mut state = test_state();
        for owner in [user_a(), user_b(), user_c()] {
            state.add_deposit_at(owner, icusd_ledger(), 20, 0);
        }
        let vault_id = 801;
        let created_at_time_ns = 123_456;
        let tuple = SpLiquidationApprovalTuple {
            ledger: icusd_ledger(),
            owner: icrc_ledger_types::icrc1::account::Account {
                owner: Principal::from_slice(&[99]),
                subaccount: None,
            },
            spender: icrc_ledger_types::icrc1::account::Account {
                owner: Principal::from_slice(&[100]),
                subaccount: None,
            },
            allowance_raw: 40,
            fee_raw: 13,
            memo: b"sp-legacy-approval/801/123456".to_vec(),
            created_at_time_ns,
            expires_at_ns: created_at_time_ns + 300_000_000_000,
        };
        state
            .begin_sp_legacy_approval_fee(PendingSpLegacyApprovalFee {
                vault_id,
                approval: tuple.clone(),
                dispatch_in_flight: false,
                ambiguous_seen: false,
            })
            .unwrap();
        state
            .mark_sp_legacy_approval_dispatch(vault_id, icusd_ledger(), true, false)
            .unwrap();

        // Upgrade while the first approve is awaiting its reply. The saved
        // in-flight state forces the retry to preserve the identical tuple.
        let bytes = Encode!(&state).unwrap();
        let mut restored = Decode!(&bytes, StabilityPoolState).unwrap();
        let saved = restored
            .pending_sp_legacy_approval_fee(vault_id, icusd_ledger())
            .unwrap();
        assert_eq!(saved.approval, tuple);
        restored
            .mark_sp_legacy_approval_dispatch(vault_id, icusd_ledger(), true, false)
            .unwrap();
        assert!(
            restored
                .pending_sp_legacy_approval_fee(vault_id, icusd_ledger())
                .unwrap()
                .ambiguous_seen
        );

        // A later error cannot erase an outcome that may have committed. The
        // exact Duplicate block receipt then allows one exact fee debit.
        assert!(restored
            .clear_sp_legacy_approval_fee_after_no_effect(vault_id, icusd_ledger())
            .is_err());
        let receipt = SpLiquidationApprovalReceipt {
            block_index: 44,
            tuple: saved.approval,
        };
        restored
            .account_sp_legacy_approval_fee(vault_id, receipt.clone())
            .unwrap();
        assert_eq!(
            restored.total_stablecoin_balances.get(&icusd_ledger()),
            Some(&47)
        );
        assert!(restored
            .pending_sp_legacy_approval_fee(vault_id, icusd_ledger())
            .is_none());
        assert!(restored
            .account_sp_legacy_approval_fee(vault_id, receipt)
            .is_err());
        assert_eq!(
            restored.total_stablecoin_balances.get(&icusd_ledger()),
            Some(&47)
        );
    }

    #[test]
    fn legacy_liquidation_overdraw_fails_before_mutating_any_pool_state() {
        let mut state = test_state();
        state.register_chain_collateral_sentinel(cfx_sentinel());
        for (owner, amount) in [(user_a(), 1), (user_b(), 2), (user_c(), 3)] {
            state.add_deposit_at(owner, icusd_ledger(), amount, 0);
            state.opt_in_cfx(&owner, cfx_sentinel()).unwrap();
        }
        let before_balances = state
            .deposits
            .iter()
            .map(|(owner, pos)| (*owner, pos.stablecoin_balances.clone()))
            .collect::<BTreeMap<_, _>>();
        let before_totals = state.total_stablecoin_balances.clone();
        let before_history = state.liquidation_history.clone();
        let overdraw = BTreeMap::from([(icusd_ledger(), 7)]);

        assert!(state
            .process_liquidation_gains_at(79, icp_ledger(), &overdraw, 10, 1_000_000_000, 3)
            .is_err());
        assert!(
            state
                .process_chain_liquidation_gains_at(
                    80,
                    cfx_sentinel(),
                    &overdraw,
                    10,
                    1_000_000_000,
                    4,
                )
                .is_err()
        );
        assert_eq!(state.total_stablecoin_balances, before_totals);
        assert_eq!(state.liquidation_history, before_history);
        assert_eq!(
            state
                .deposits
                .iter()
                .map(|(owner, pos)| (*owner, pos.stablecoin_balances.clone()))
                .collect::<BTreeMap<_, _>>(),
            before_balances,
        );
        assert!(state
            .deposits
            .values()
            .all(|pos| pos.collateral_gains.is_empty()
                && pos
                    .cfx_claims
                    .as_ref()
                    .map(BTreeMap::is_empty)
                    .unwrap_or(true)));
    }

    // ─── Test: Opt-out Filtering ───

    #[test]
    fn test_opt_out_filtering() {
        let mut state = test_state();

        // user_a: 60 icUSD, user_b: 40 icUSD
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 60_00000000);
        add_deposit_direct(&mut state, user_b(), icusd_ledger(), 40_00000000);

        // user_b opts out of ICP collateral
        state.opt_out_collateral(&user_b(), icp_ledger()).unwrap();

        // Effective pool for ICP should only include user_a
        let effective = state.effective_pool_for_collateral(&icp_ledger());
        assert_eq!(
            effective, 60_00000000,
            "Only user_a's 60 icUSD should be in effective pool"
        );

        // Effective pool for ckBTC should include both (user_b only opted out of ICP)
        let effective_btc = state.effective_pool_for_collateral(&ckbtc_ledger());
        assert_eq!(
            effective_btc, 100_00000000,
            "Both users should be in ckBTC pool"
        );

        // Token draw for ICP should only draw from user_a's balance
        let draw = state.compute_token_draw(30_00000000, &icp_ledger());
        assert_eq!(draw.get(&icusd_ledger()).copied().unwrap_or(0), 30_00000000);

        // Liquidation gains: only user_a participates for ICP
        let mut stables_consumed = BTreeMap::new();
        stables_consumed.insert(icusd_ledger(), 20_00000000);

        state
            .process_liquidation_gains_at(
                2,
                icp_ledger(),
                &stables_consumed,
                10_00000000,
                7_50000000,
                2_000_000_000,
            )
            .unwrap();

        // user_a should lose all 20 icUSD (only opted-in depositor)
        let pos_a = state.deposits.get(&user_a()).unwrap();
        assert_eq!(
            pos_a
                .stablecoin_balances
                .get(&icusd_ledger())
                .copied()
                .unwrap_or(0),
            40_00000000
        );
        assert_eq!(
            pos_a
                .collateral_gains
                .get(&icp_ledger())
                .copied()
                .unwrap_or(0),
            10_00000000
        );

        // user_b should be completely untouched
        let pos_b = state.deposits.get(&user_b()).unwrap();
        assert_eq!(
            pos_b
                .stablecoin_balances
                .get(&icusd_ledger())
                .copied()
                .unwrap_or(0),
            40_00000000
        );
        assert_eq!(
            pos_b
                .collateral_gains
                .get(&icp_ledger())
                .copied()
                .unwrap_or(0),
            0
        );
    }

    #[test]
    fn test_opt_out_duplicate_errors() {
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 10_00000000);

        // First opt-out succeeds
        state.opt_out_collateral(&user_a(), icp_ledger()).unwrap();

        // Second opt-out of same collateral should fail
        let err = state
            .opt_out_collateral(&user_a(), icp_ledger())
            .unwrap_err();
        assert!(matches!(err, StabilityPoolError::AlreadyOptedOut { .. }));

        // Opt back in
        state.opt_in_collateral(&user_a(), icp_ledger()).unwrap();

        // Double opt-in should fail
        let err = state
            .opt_in_collateral(&user_a(), icp_ledger())
            .unwrap_err();
        assert!(matches!(err, StabilityPoolError::AlreadyOptedIn { .. }));
    }

    #[test]
    fn legacy_bob_participant_can_exit_but_cannot_reenter() {
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 10_00000000);
        let bob = crate::types::bob_collateral();

        state
            .deposits
            .get_mut(&user_a())
            .unwrap()
            .opted_out_collateral
            .remove(&bob);
        assert_eq!(state.effective_pool_for_collateral(&bob), 10_00000000);

        state.opt_out_collateral(&user_a(), bob).unwrap();
        assert_eq!(state.effective_pool_for_collateral(&bob), 0);

        let err = state.opt_in_collateral(&user_a(), bob).unwrap_err();
        assert!(matches!(err, StabilityPoolError::TokenNotActive { ledger } if ledger == bob));
    }

    #[test]
    fn legacy_exe_participant_can_exit_but_cannot_reenter() {
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 10_00000000);
        let exe = crate::types::exe_collateral();

        state
            .deposits
            .get_mut(&user_a())
            .unwrap()
            .opted_out_collateral
            .remove(&exe);
        assert_eq!(state.effective_pool_for_collateral(&exe), 10_00000000);

        state.opt_out_collateral(&user_a(), exe).unwrap();
        assert_eq!(state.effective_pool_for_collateral(&exe), 0);

        let err = state.opt_in_collateral(&user_a(), exe).unwrap_err();
        assert!(matches!(err, StabilityPoolError::TokenNotActive { ledger } if ledger == exe));
    }

    #[test]
    fn non_bob_collateral_opt_out_and_reentry_are_unchanged() {
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 10_00000000);

        state.opt_out_collateral(&user_a(), icp_ledger()).unwrap();
        state.opt_in_collateral(&user_a(), icp_ledger()).unwrap();

        assert!(state.deposits[&user_a()].is_opted_in(&icp_ledger()));
    }

    #[test]
    fn test_opt_no_position_errors() {
        let mut state = test_state();

        // Opt-out on nonexistent position
        let err = state
            .opt_out_collateral(&user_a(), icp_ledger())
            .unwrap_err();
        assert!(matches!(err, StabilityPoolError::NoPositionFound));

        // Opt-in on nonexistent position
        let err = state
            .opt_in_collateral(&user_a(), icp_ledger())
            .unwrap_err();
        assert!(matches!(err, StabilityPoolError::NoPositionFound));
    }

    #[test]
    fn xrp_requires_payout_address_before_participating() {
        let mut state = test_state();

        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        add_deposit_direct(&mut state, user_b(), icusd_ledger(), 50_00000000);

        assert_eq!(
            state.effective_pool_for_collateral(&xrp_ledger()),
            0,
            "XRP must be opt-in only; default depositors cannot absorb XRP liquidations"
        );
        assert!(
            state
                .compute_token_draw(10_00000000, &xrp_ledger())
                .is_empty(),
            "XRP liquidations must not draw from users without a payout address"
        );

        let err = state
            .opt_in_collateral(&user_a(), xrp_ledger())
            .unwrap_err();
        assert!(matches!(
            err,
            StabilityPoolError::PayoutAddressRequired { .. }
        ));

        state
            .opt_in_native_collateral(&user_a(), xrp_ledger(), valid_xrp_address())
            .unwrap();

        assert_eq!(
            state.native_payout_address(&user_a(), &xrp_ledger()),
            Some(valid_xrp_address()),
        );
        assert_eq!(
            state.effective_pool_for_collateral(&xrp_ledger()),
            100_00000000,
            "only the depositor with an XRP payout address is eligible"
        );

        let draw = state.compute_token_draw(25_00000000, &xrp_ledger());
        assert_eq!(draw.get(&icusd_ledger()).copied().unwrap_or(0), 25_00000000);
    }

    #[test]
    fn opt_in_native_collateral_with_tag_stores_address_and_tag() {
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 10_00000000);

        state
            .opt_in_native_collateral_with_tag(
                &user_a(),
                xrp_ledger(),
                valid_xrp_address(),
                Some(4_294_967_295),
            )
            .unwrap();

        assert_eq!(
            state.native_payout_address(&user_a(), &xrp_ledger()),
            Some(valid_xrp_address()),
        );
        assert_eq!(
            state.native_payout_destination_tag(&user_a(), &xrp_ledger()),
            Some(4_294_967_295),
        );
    }

    #[test]
    fn opt_in_native_collateral_address_only_wrapper_clears_tag() {
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 10_00000000);

        state
            .opt_in_native_collateral_with_tag(
                &user_a(),
                xrp_ledger(),
                valid_xrp_address(),
                Some(777),
            )
            .unwrap();
        state
            .opt_in_native_collateral(&user_a(), xrp_ledger(), valid_xrp_address())
            .unwrap();

        assert_eq!(
            state.native_payout_address(&user_a(), &xrp_ledger()),
            Some(valid_xrp_address()),
        );
        assert_eq!(
            state.native_payout_destination_tag(&user_a(), &xrp_ledger()),
            None,
            "the legacy address-only wrapper must not leave a stale destination tag",
        );
    }

    #[test]
    fn get_user_position_exposes_native_payout_destination_tags() {
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 10_00000000);

        state
            .opt_in_native_collateral_with_tag(
                &user_a(),
                xrp_ledger(),
                valid_xrp_address(),
                Some(123_456),
            )
            .unwrap();

        let position = state.get_user_position(&user_a()).unwrap();
        assert_eq!(
            position
                .native_payout_destination_tags
                .unwrap_or_default()
                .get(&xrp_ledger())
                .copied(),
            Some(123_456),
        );
    }

    #[test]
    fn xrp_sp_absorb_does_not_credit_icrc_collateral_gains() {
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        state
            .opt_in_native_collateral_with_tag(
                &user_a(),
                xrp_ledger(),
                valid_xrp_address(),
                Some(99),
            )
            .unwrap();

        let mut consumed = BTreeMap::new();
        consumed.insert(icusd_ledger(), 20_00000000);
        assert!(state
            .process_liquidation_gains_at(
                144,
                xrp_ledger(),
                &consumed,
                5_000_000,
                50_00000000,
                3_000_000_000,
            )
            .is_err());

        let pos_a = state.deposits.get(&user_a()).unwrap();
        assert_eq!(
            pos_a
                .stablecoin_balances
                .get(&icusd_ledger())
                .copied()
                .unwrap_or(0),
            100_00000000,
            "unsafe generic XRP absorption must not deduct SP balances",
        );
        assert_eq!(
            pos_a
                .collateral_gains
                .get(&xrp_ledger())
                .copied()
                .unwrap_or(0),
            0,
            "native XRP must never enter ICRC collateral_gains",
        );
    }

    #[test]
    fn xrp_interest_only_goes_to_address_opted_depositors() {
        let mut state = test_state();

        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        add_deposit_direct(&mut state, user_b(), icusd_ledger(), 50_00000000);

        state.distribute_interest_revenue(icusd_ledger(), 9_00000000, Some(xrp_ledger()));
        assert_eq!(
            state
                .deposits
                .get(&user_a())
                .unwrap()
                .stablecoin_balances
                .get(&icusd_ledger())
                .copied()
                .unwrap_or(0),
            100_00000000,
            "XRP interest must not be distributed until a depositor provides a payout address"
        );
        assert_eq!(
            state
                .deposits
                .get(&user_b())
                .unwrap()
                .stablecoin_balances
                .get(&icusd_ledger())
                .copied()
                .unwrap_or(0),
            50_00000000,
        );

        state
            .opt_in_native_collateral(&user_a(), xrp_ledger(), valid_xrp_address())
            .unwrap();
        state.distribute_interest_revenue(icusd_ledger(), 9_00000000, Some(xrp_ledger()));

        let pos_a = state.deposits.get(&user_a()).unwrap();
        let pos_b = state.deposits.get(&user_b()).unwrap();
        assert_eq!(
            pos_a
                .stablecoin_balances
                .get(&icusd_ledger())
                .copied()
                .unwrap_or(0),
            109_00000000
        );
        assert_eq!(
            pos_b
                .stablecoin_balances
                .get(&icusd_ledger())
                .copied()
                .unwrap_or(0),
            50_00000000
        );
        assert_eq!(pos_a.total_interest_earned_e8s.unwrap_or(0), 9_00000000);
        assert_eq!(pos_b.total_interest_earned_e8s.unwrap_or(0), 0);
    }

    #[test]
    fn xrp_opt_in_rejects_invalid_payout_address() {
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 10_00000000);

        let err = state
            .opt_in_native_collateral(&user_a(), xrp_ledger(), "not-an-xrpl-address".to_string())
            .unwrap_err();
        assert!(matches!(
            err,
            StabilityPoolError::InvalidPayoutAddress { .. }
        ));
        assert_eq!(state.native_payout_address(&user_a(), &xrp_ledger()), None);
        assert_eq!(state.effective_pool_for_collateral(&xrp_ledger()), 0);
    }

    #[test]
    fn xrp_opt_out_clears_payout_address_and_tag() {
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 10_00000000);

        state
            .opt_in_native_collateral_with_tag(
                &user_a(),
                xrp_ledger(),
                valid_xrp_address(),
                Some(321),
            )
            .unwrap();
        state.opt_out_collateral(&user_a(), xrp_ledger()).unwrap();

        assert_eq!(state.native_payout_address(&user_a(), &xrp_ledger()), None);
        assert_eq!(
            state.native_payout_destination_tag(&user_a(), &xrp_ledger()),
            None,
        );
        assert_eq!(state.effective_pool_for_collateral(&xrp_ledger()), 0);
    }

    #[test]
    fn claim_collateral_rejects_native_xrp_before_ledger_call() {
        let state = test_state();
        let err = state
            .ensure_icrc_claimable_collateral(&xrp_ledger())
            .unwrap_err();
        assert!(matches!(
            err,
            StabilityPoolError::PayoutAddressRequired { collateral }
                if collateral == xrp_ledger()
        ));
    }

    #[test]
    fn claim_all_collateral_skips_native_xrp() {
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 10_00000000);
        let position = state.deposits.get_mut(&user_a()).unwrap();
        position.collateral_gains.insert(icp_ledger(), 500);
        position.collateral_gains.insert(xrp_ledger(), 700);

        let claimable = state.get_claimable_icrc_collateral_gains(&user_a());
        assert_eq!(claimable.get(&icp_ledger()).copied(), Some(500));
        assert!(
            !claimable.contains_key(&xrp_ledger()),
            "claim_all_collateral must skip native XRP instead of routing it through ICRC",
        );
    }

    #[test]
    fn allocation_builder_rejects_over_500_native_xrp_payouts_before_burn() {
        let mut state = test_state();
        for i in 0..501u16 {
            let principal = Principal::from_slice(&i.to_be_bytes());
            add_deposit_direct(&mut state, principal, icusd_ledger(), 1_00000000);
            state
                .opt_in_native_collateral_with_tag(
                    &principal,
                    xrp_ledger(),
                    valid_xrp_address(),
                    None,
                )
                .unwrap();
        }
        let before_total = state
            .total_stablecoin_balances
            .get(&icusd_ledger())
            .copied();
        let mut consumed = BTreeMap::new();
        consumed.insert(icusd_ledger(), 501_00000000);

        let err = state
            .build_native_xrp_payout_allocations(xrp_ledger(), &consumed, 501)
            .unwrap_err();

        assert!(matches!(err, StabilityPoolError::LiquidationFailed { .. }));
        assert_eq!(
            state
                .total_stablecoin_balances
                .get(&icusd_ledger())
                .copied(),
            before_total,
            "allocation fanout rejection must happen before any burn/accounting mutation",
        );
    }

    #[test]
    fn allocation_builder_assigns_dust_to_first_sorted_eligible_depositor() {
        let mut state = test_state();
        add_deposit_direct(&mut state, user_b(), icusd_ledger(), 1);
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 1);
        state
            .opt_in_native_collateral_with_tag(
                &user_b(),
                xrp_ledger(),
                valid_xrp_address(),
                Some(2),
            )
            .unwrap();
        state
            .opt_in_native_collateral_with_tag(
                &user_a(),
                xrp_ledger(),
                valid_xrp_address(),
                Some(1),
            )
            .unwrap();
        let mut consumed = BTreeMap::new();
        consumed.insert(icusd_ledger(), 2);

        let allocations = state
            .build_native_xrp_payout_allocations(xrp_ledger(), &consumed, 1)
            .unwrap();

        assert_eq!(allocations.len(), 1);
        assert_eq!(allocations[0].claimant, user_a());
        assert_eq!(allocations[0].drops, 1);
        assert_eq!(allocations[0].destination_tag, Some(1));
    }

    #[test]
    fn native_xrp_absorb_deducts_full_burned_amount_when_shares_round_down() {
        let mut state = test_state();
        add_deposit_direct(&mut state, user_b(), icusd_ledger(), 1);
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 1);
        state
            .opt_in_native_collateral_with_tag(
                &user_b(),
                xrp_ledger(),
                valid_xrp_address(),
                Some(2),
            )
            .unwrap();
        state
            .opt_in_native_collateral_with_tag(
                &user_a(),
                xrp_ledger(),
                valid_xrp_address(),
                Some(1),
            )
            .unwrap();
        let mut consumed = BTreeMap::new();
        consumed.insert(icusd_ledger(), 1);
        let payout_claims = vec![XrpSpPayoutClaim {
            claimant: user_a(),
            claim_id: 77,
            payout_address: valid_xrp_address(),
            destination_tag: Some(1),
            drops: 1,
        }];

        state
            .process_native_xrp_absorb_success_at(
                42,
                xrp_ledger(),
                &consumed,
                1,
                &payout_claims,
                123,
            )
            .unwrap();

        assert_eq!(
            state
                .deposits
                .get(&user_a())
                .and_then(|pos| pos.stablecoin_balances.get(&icusd_ledger()).copied()),
            Some(0),
        );
        assert_eq!(
            state
                .deposits
                .get(&user_b())
                .and_then(|pos| pos.stablecoin_balances.get(&icusd_ledger()).copied()),
            Some(1),
        );
        assert_eq!(
            state
                .total_stablecoin_balances
                .get(&icusd_ledger())
                .copied(),
            Some(1),
            "aggregate SP balance must drop by the full burned icUSD amount",
        );
        assert_eq!(state.native_xrp_pending_payouts_for(&user_a()).len(), 1);
    }

    #[test]
    #[ignore = "KNOWN GAP: PoolLiquidationRecord.collateral_gained is u64, which cannot hold \
                18-decimal wei (u64 max is only ~18.4 CFX). Fixing this needs a record-shape \
                decision (widen the field, or normalize to e8s and teach every consumer that \
                chain rows mean something different) — see the test body."]
    fn chain_absorb_appends_a_liquidation_history_record() {
        // Same defect the native-XRP path had: `process_chain_liquidation_gains_at`
        // advances `total_liquidations_executed` without pushing a
        // `PoolLiquidationRecord`, so a chain absorb inflates the count while
        // leaving the Earn page's history list unchanged.
        //
        // Deliberately NOT fixed alongside XRP, because it cannot be fixed
        // honestly with the current record shape. Chain claims are `u128` wei
        // (`DepositPosition::cfx_claims`) while `collateral_gained` is `u64`:
        // 2^64-1 wei is ~18.4 CFX, so any realistic absorb saturates. Writing a
        // saturated value would put a silently wrong number in the UI, which is
        // worse than the current absence. Normalizing to e8s would make the same
        // field mean different things per collateral — exactly the kind of
        // implicit contract that produced this class of bug.
        //
        // Low urgency: chain liquidation is disabled-by-default and staging-only.
        // Un-ignore this once the record shape is settled.
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 1_00000000);
        state.opt_in_cfx(&user_a(), cfx_sentinel()).unwrap();
        let mut stables_consumed = BTreeMap::new();
        stables_consumed.insert(icusd_ledger(), 1_00000000);

        let history_before = state.liquidation_history.len();
        state
            .process_chain_liquidation_gains_at(
                99,
                cfx_sentinel(),
                &stables_consumed,
                20_000 * 1_000_000_000_000_000_000u128,
                5_000_000,
                123,
            )
            .unwrap();

        assert_eq!(
            state.liquidation_history.len(),
            history_before + 1,
            "chain absorb must append a history record, in lockstep with the counter"
        );
        let record = state.liquidation_history.last().expect("history record");
        assert_eq!(record.vault_id, 99);
        assert_eq!(record.collateral_type, cfx_sentinel());
    }

    #[test]
    fn native_xrp_absorb_appends_a_liquidation_history_record() {
        // The XRP absorb path bumps `total_liquidations_executed` but used to
        // skip `liquidation_history` entirely (only the generic ICRC path wrote
        // records). Live consequence: the Earn page's Liquidation History
        // showed no row for a real XRP absorb while the counter climbed, so the
        // count and the list disagreed. The record must be written by the same
        // call that bumps the counter.
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_000_000);
        state
            .opt_in_native_collateral_with_tag(&user_a(), xrp_ledger(), valid_xrp_address(), None)
            .unwrap();
        let mut consumed = BTreeMap::new();
        consumed.insert(icusd_ledger(), 60_000_000);
        let payout_claims = vec![XrpSpPayoutClaim {
            claimant: user_a(),
            claim_id: 9,
            payout_address: valid_xrp_address(),
            destination_tag: None,
            drops: 1_796_552,
        }];

        let history_before = state.liquidation_history.len();
        let counter_before = state.total_liquidations_executed;

        state
            .process_native_xrp_absorb_success_at(
                195,
                xrp_ledger(),
                &consumed,
                1_796_552,
                &payout_claims,
                4_242,
            )
            .unwrap();

        assert_eq!(
            state.total_liquidations_executed,
            counter_before + 1,
            "counter must still advance"
        );
        assert_eq!(
            state.liquidation_history.len(),
            history_before + 1,
            "history must advance in lockstep with the counter"
        );
        let record = state.liquidation_history.last().expect("history record");
        assert_eq!(record.vault_id, 195);
        assert_eq!(record.collateral_type, xrp_ledger());
        assert_eq!(
            record.collateral_gained, 1_796_552,
            "collateral_gained is the seized drops"
        );
        assert_eq!(record.stables_consumed, consumed);
        assert_eq!(
            record.depositors_count, 1,
            "depositors_count is the number of payout claimants"
        );
        assert_eq!(record.timestamp, 4_242);
    }

    #[test]
    fn pending_native_xrp_payout_storage_and_ack_are_caller_scoped() {
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 10_00000000);
        add_deposit_direct(&mut state, user_b(), icusd_ledger(), 10_00000000);
        let payout = NativeXrpPendingPayout {
            claim_id: 42,
            collateral_type: xrp_ledger(),
            collateral_price_e8s: 0,
            vault_id: 144,
            drops: 123_456,
            payout_address: valid_xrp_address(),
            destination_tag: Some(55),
            created_at_ns: 999,
        };

        state
            .record_native_xrp_pending_payout(user_a(), payout.clone())
            .unwrap();

        assert_eq!(
            state.native_xrp_pending_payouts_for(&user_a()),
            vec![payout.clone()],
        );
        assert_eq!(
            state.native_xrp_pending_payout_for(&user_a(), 42),
            Some(payout.clone()),
        );
        assert_eq!(state.native_xrp_pending_payout_for(&user_b(), 42), None);
        assert!(state.native_xrp_pending_payouts_for(&user_b()).is_empty());
        assert!(matches!(
            state.ack_native_xrp_payout_settled(&user_b(), 42),
            Err(StabilityPoolError::RefundClaimNotFound)
        ));
        assert_eq!(
            state.native_xrp_pending_payouts_for(&user_a()).len(),
            1,
            "wrong caller must not remove another user's pending XRP payout",
        );

        state.ack_native_xrp_payout_settled(&user_a(), 42).unwrap();
        assert!(state.native_xrp_pending_payouts_for(&user_a()).is_empty());
    }

    #[test]
    fn chain_sentinel_named_xrp_routes_to_chain_optin_not_payout_address() {
        // Regression (review MEDIUM-1): a chain-registered collateral that happens
        // to carry the "XRP" symbol must NOT be treated as native XRP. It must use
        // the CFX-style sentinel opt-in, not the payout-address branch. Gating
        // `collateral_requires_payout_address` on principal identity (not symbol)
        // preserves this so a CFX-style opt-in depositor keeps absorbing it.
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 10_00000000);

        let sentinel = state
            .register_chain_collateral(9999, "XRP".to_string(), 6)
            .unwrap();
        assert_ne!(sentinel, xrp_ledger());
        assert!(!state.collateral_requires_payout_address(&sentinel));

        // Opts in via the chain (CFX) path; the payout-address endpoint is wrong here.
        state.opt_in_cfx(&user_a(), sentinel).unwrap();
        let pos = state.deposits.get(&user_a()).unwrap();
        assert!(state.position_opted_in_for(pos, &sentinel));
        assert_eq!(state.native_payout_address(&user_a(), &sentinel), None);
    }

    // ─── Test: Normalize E8s Conversions ───

    #[test]
    fn test_normalize_e8s_conversions() {
        // 8-decimal token (e.g. icUSD, ICP): identity
        assert_eq!(normalize_to_e8s(100_000_000, 8), 100_000_000);
        assert_eq!(normalize_from_e8s(100_000_000, 8), 100_000_000);

        // 6-decimal token (e.g. ckUSDT, ckUSDC): multiply/divide by 100
        // 1.0 ckUSDT (1_000_000 native) = 1_00000000 e8s
        assert_eq!(normalize_to_e8s(1_000_000, 6), 100_000_000);
        assert_eq!(normalize_from_e8s(100_000_000, 6), 1_000_000);

        // 50.5 ckUSDT = 50_500_000 native -> 5_050_000_000 e8s
        assert_eq!(normalize_to_e8s(50_500_000, 6), 5_050_000_000);
        assert_eq!(normalize_from_e8s(5_050_000_000, 6), 50_500_000);

        // Edge case: zero
        assert_eq!(normalize_to_e8s(0, 6), 0);
        assert_eq!(normalize_to_e8s(0, 8), 0);
        assert_eq!(normalize_from_e8s(0, 6), 0);
        assert_eq!(normalize_from_e8s(0, 8), 0);

        // Edge case: 1 unit of 6-decimal token
        assert_eq!(normalize_to_e8s(1, 6), 100); // 0.000001 USD = 0.00000100 e8s
        assert_eq!(normalize_from_e8s(100, 6), 1);

        // Round-trip for 6-decimal
        let original = 12_345_678u64;
        assert_eq!(
            normalize_from_e8s(normalize_to_e8s(original, 6), 6),
            original
        );

        // Round-trip for 8-decimal
        let original = 98_765_432u64;
        assert_eq!(
            normalize_from_e8s(normalize_to_e8s(original, 8), 8),
            original
        );

        // Hypothetical 12-decimal token (greater than 8): e.g. 1.0 = 1_000_000_000_000
        // normalize_to_e8s: divide by 10^4 = 10_000
        assert_eq!(normalize_to_e8s(1_000_000_000_000, 12), 100_000_000);
        assert_eq!(normalize_from_e8s(100_000_000, 12), 1_000_000_000_000);

        // Truncation: 12-decimal with sub-e8s precision (loses fractional)
        // 5_000 units at 12 decimals = 5_000 / 10_000 = 0 e8s (truncated)
        assert_eq!(normalize_to_e8s(5_000, 12), 0);
    }

    // ─── Test: State Validation ───

    #[test]
    fn test_state_validation() {
        let mut state = test_state();

        // Valid state: consistent totals
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        add_deposit_direct(&mut state, user_b(), icusd_ledger(), 50_00000000);
        assert!(state.validate_state().is_ok());

        // Corrupt the tracked total to create a mismatch
        *state
            .total_stablecoin_balances
            .get_mut(&icusd_ledger())
            .unwrap() = 999;
        let err = state.validate_state();
        assert!(err.is_err());
        let msg = err.unwrap_err();
        assert!(
            msg.contains("mismatch"),
            "Error should mention mismatch: {}",
            msg
        );
        assert!(
            msg.contains("tracked=999"),
            "Error should show tracked value: {}",
            msg
        );

        // Fix the corruption
        *state
            .total_stablecoin_balances
            .get_mut(&icusd_ledger())
            .unwrap() = 150_00000000;
        assert!(state.validate_state().is_ok());
    }

    #[test]
    fn test_state_validation_empty_state() {
        let state = test_state();
        // Empty state with zero totals should pass
        assert!(state.validate_state().is_ok());
    }

    #[test]
    fn unallocated_interest_receipt_deduplicates_and_batches_fee_dust() {
        let mut state = test_state();
        state.interest_treasury = Some(Principal::from_slice(&[91]));
        let first = state
            .queue_unallocated_interest_forward_at(44, icusd_ledger(), 50, 1)
            .expect("first receipt queues");
        let duplicate = state
            .queue_unallocated_interest_forward_at(44, icusd_ledger(), 50, 2)
            .expect("duplicate receipt is idempotent");
        assert_eq!(duplicate, first);

        // Simulate a 100-unit fee: the first receipt remains durable dust.
        let dust = state
            .prepare_unallocated_interest_forward_at(first, 100, 3)
            .expect("fee persists before any transfer await");
        assert_eq!(dust.gross_amount, 50);
        assert_eq!(dust.fee, Some(100));
        let same_batch = state
            .queue_unallocated_interest_forward_at(45, icusd_ledger(), 75, 4)
            .expect("a later receipt carries dust over the fee threshold");
        assert_eq!(same_batch, first);
        let combined = state
            .unallocated_interest_forward_batch(first)
            .expect("batch remains durable");
        assert_eq!(combined.gross_amount, 125);
        assert_eq!(combined.source_mint_blocks, vec![44, 45]);
    }

    #[test]
    fn chain_absorb_auto_fields_decode_disabled_when_missing() {
        let mut current = StabilityPoolState::default();
        current
            .register_chain_collateral(1030, "CFX".to_string(), 18)
            .unwrap();

        #[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
        struct PreInc9State {
            deposits: BTreeMap<Principal, DepositPosition>,
            total_stablecoin_balances: BTreeMap<Principal, u64>,
            stablecoin_registry: BTreeMap<Principal, StablecoinConfig>,
            collateral_registry: BTreeMap<Principal, CollateralInfo>,
            chain_collateral_sentinels: Option<BTreeSet<Principal>>,
            chain_claim_sources: Option<BTreeMap<Principal, Vec<ChainClaimSource>>>,
            pending_chain_absorbs: Option<BTreeMap<u64, ChainSpAbsorbIntent>>,
            completed_chain_absorbs: Option<BTreeMap<u64, ChainSpAbsorbCompletion>>,
            protocol_canister_id: Principal,
            configuration: PoolConfiguration,
            liquidation_history: Vec<PoolLiquidationRecord>,
            in_flight_liquidations: BTreeSet<u64>,
            total_liquidations_executed: u64,
            pool_creation_timestamp: u64,
            total_interest_received_e8s: Option<u64>,
            token_consecutive_failures: Option<BTreeMap<Principal, u32>>,
            cached_virtual_prices: Option<BTreeMap<Principal, u128>>,
            protocol_reserve_address: Option<Principal>,
            is_initialized: bool,
            pool_events: Option<Vec<PoolEvent>>,
            next_event_id: Option<u64>,
            pending_refunds: Option<BTreeMap<u64, PendingRefund>>,
            next_pending_refund_id: Option<u64>,
        }

        let old = PreInc9State {
            deposits: current.deposits.clone(),
            total_stablecoin_balances: current.total_stablecoin_balances.clone(),
            stablecoin_registry: current.stablecoin_registry.clone(),
            collateral_registry: current.collateral_registry.clone(),
            chain_collateral_sentinels: current.chain_collateral_sentinels.clone(),
            chain_claim_sources: current.chain_claim_sources.clone(),
            pending_chain_absorbs: current.pending_chain_absorbs.clone(),
            completed_chain_absorbs: current.completed_chain_absorbs.clone(),
            protocol_canister_id: current.protocol_canister_id,
            configuration: current.configuration.clone(),
            liquidation_history: current.liquidation_history.clone(),
            in_flight_liquidations: current.in_flight_liquidations.clone(),
            total_liquidations_executed: current.total_liquidations_executed,
            pool_creation_timestamp: current.pool_creation_timestamp,
            total_interest_received_e8s: current.total_interest_received_e8s,
            token_consecutive_failures: current.token_consecutive_failures.clone(),
            cached_virtual_prices: current.cached_virtual_prices.clone(),
            protocol_reserve_address: current.protocol_reserve_address,
            is_initialized: current.is_initialized,
            pool_events: current.pool_events.clone(),
            next_event_id: current.next_event_id,
            pending_refunds: current.pending_refunds.clone(),
            next_pending_refund_id: current.next_pending_refund_id,
        };

        let bytes = Encode!(&old).unwrap();
        let decoded = Decode!(&bytes, StabilityPoolState).unwrap();
        let config = decoded.chain_absorb_auto_config();

        assert!(!config.enabled);
        assert_eq!(
            config.interval_seconds,
            DEFAULT_CHAIN_ABSORB_AUTO_INTERVAL_SECONDS
        );
        assert_eq!(
            config.max_scan_per_chain,
            DEFAULT_CHAIN_ABSORB_AUTO_MAX_SCAN_PER_CHAIN
        );
        assert!(decoded.chain_absorb_auto_last_tick().is_none());
        assert!(decoded.pending_outbound_payouts.is_none());
    }

    #[test]
    fn chain_absorb_auto_config_validation_rejects_saturating_timer_values() {
        let mut state = StabilityPoolState::default();
        let too_fast = ChainAbsorbAutoConfig {
            enabled: true,
            interval_seconds: MIN_CHAIN_ABSORB_AUTO_INTERVAL_SECONDS - 1,
            max_scan_per_chain: 1,
        };
        assert!(matches!(
            state.set_chain_absorb_auto_config(too_fast),
            Err(StabilityPoolError::LiquidationFailed { .. })
        ));

        let no_scan = ChainAbsorbAutoConfig {
            enabled: true,
            interval_seconds: DEFAULT_CHAIN_ABSORB_AUTO_INTERVAL_SECONDS,
            max_scan_per_chain: 0,
        };
        assert!(matches!(
            state.set_chain_absorb_auto_config(no_scan),
            Err(StabilityPoolError::LiquidationFailed { .. })
        ));

        let accepted = ChainAbsorbAutoConfig {
            enabled: true,
            interval_seconds: MIN_CHAIN_ABSORB_AUTO_INTERVAL_SECONDS,
            max_scan_per_chain: 500,
        };
        state
            .set_chain_absorb_auto_config(accepted.clone())
            .unwrap();
        assert_eq!(state.chain_absorb_auto_config(), accepted);
    }

    #[test]
    fn chain_absorb_auto_due_requires_enabled_and_elapsed_interval() {
        let mut state = StabilityPoolState::default();
        assert!(!state.chain_absorb_auto_due(1_000_000_000_000));

        state
            .set_chain_absorb_auto_config(ChainAbsorbAutoConfig {
                enabled: true,
                interval_seconds: 300,
                max_scan_per_chain: 1,
            })
            .unwrap();
        assert!(state.chain_absorb_auto_due(1_000_000_000_000));

        state.record_chain_absorb_auto_tick(ChainAbsorbAutoTickRecord {
            started_at_ns: 1_000_000_000_000,
            completed_at_ns: 1_001_000_000_000,
            attempted_vault_id: None,
            candidates_scanned: 0,
            absorbed: None,
            error: None,
            skipped_reason: Some("no eligible candidates".to_string()),
        });

        assert!(!state.chain_absorb_auto_due(1_100_000_000_000));
        assert!(state.chain_absorb_auto_due(1_301_000_000_000));
    }

    // ─── Test: DepositPosition helpers ───

    #[test]
    fn test_deposit_position_total_usd_value() {
        let state = test_state();

        let mut pos = DepositPosition::new(0);
        // 10 icUSD (8 dec) + 5 ckUSDT (6 dec) = 15 USD in e8s
        pos.stablecoin_balances.insert(icusd_ledger(), 10_00000000);
        pos.stablecoin_balances.insert(ckusdt_ledger(), 5_000_000);

        let total = pos.total_usd_value(&state.stablecoin_registry, state.virtual_prices());
        assert_eq!(total, 15_00000000, "10 icUSD + 5 ckUSDT = 15 USD in e8s");
    }

    #[test]
    fn test_deposit_position_is_empty() {
        let mut pos = DepositPosition::new(0);
        assert!(pos.is_empty());

        pos.stablecoin_balances.insert(icusd_ledger(), 100);
        assert!(!pos.is_empty());

        // Zero balance present but value is 0
        pos.stablecoin_balances.insert(icusd_ledger(), 0);
        assert!(pos.is_empty());

        // Has collateral gains -> not empty
        pos.collateral_gains.insert(icp_ledger(), 100);
        assert!(!pos.is_empty());

        pos.collateral_gains.clear();
        pos.cfx_claims
            .get_or_insert_with(BTreeMap::new)
            .insert(cfx_sentinel(), 1_000_000_000_000_000_000);
        assert!(
            !pos.is_empty(),
            "u128 CFX claims must prevent position cleanup"
        );
    }

    // ─── Test: Mark gains claimed ───

    #[test]
    fn test_mark_gains_claimed() {
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);

        // Manually add collateral gains
        state
            .deposits
            .get_mut(&user_a())
            .unwrap()
            .collateral_gains
            .insert(icp_ledger(), 5_00000000);

        // Partially claim
        state.mark_gains_claimed(&user_a(), &icp_ledger(), 2_00000000);
        let pos = state.deposits.get(&user_a()).unwrap();
        assert_eq!(
            pos.collateral_gains
                .get(&icp_ledger())
                .copied()
                .unwrap_or(0),
            3_00000000
        );
        assert_eq!(
            pos.total_claimed_gains
                .get(&icp_ledger())
                .copied()
                .unwrap_or(0),
            2_00000000
        );

        // Claim the rest
        state.mark_gains_claimed(&user_a(), &icp_ledger(), 3_00000000);
        let pos = state.deposits.get(&user_a()).unwrap();
        assert_eq!(
            pos.collateral_gains.get(&icp_ledger()),
            None,
            "Zero gains should be cleaned up"
        );
        assert_eq!(
            pos.total_claimed_gains
                .get(&icp_ledger())
                .copied()
                .unwrap_or(0),
            5_00000000
        );
    }

    #[test]
    fn outbound_payout_first_typed_rejection_restores_once_and_retry_keeps_exact_tuple() {
        let caller = user_a();
        let ledger = icusd_ledger();
        let mut state = test_state();
        state.add_deposit_at(caller, ledger, 100, 0);
        state.process_withdrawal(caller, ledger, 40).unwrap();
        let payout = PendingOutboundPayout {
            kind: OutboundPayoutKind::Withdraw,
            request_amount: 40,
            gross_amount: 40,
            transfer_amount: 30,
            transfer_fee: 10,
            transfer_created_at_time_ns: 77,
            transfer_memo: b"stable-identity".to_vec(),
            dispatch_in_flight: true,
            ambiguous_seen: false,
            last_error: None,
        };
        state
            .pending_outbound_payouts
            .as_mut()
            .unwrap()
            .insert((caller, ledger), payout.clone());

        // A first typed no-effect result safely restores once.
        assert!(state
            .reject_outbound_payout_without_effect(caller, ledger, "BadFee".into(), 1)
            .unwrap());
        assert!(!state
            .reject_outbound_payout_without_effect(caller, ledger, "BadFee".into(), 1)
            .unwrap());
        assert_eq!(state.deposits[&caller].stablecoin_balances[&ledger], 100);

        // A pending retry returns the same immutable ledger identity.
        state.process_withdrawal(caller, ledger, 40).unwrap();
        state
            .pending_outbound_payouts
            .as_mut()
            .unwrap()
            .insert((caller, ledger), payout.clone());
        let before_upgrade = state.pending_outbound_payout(&caller, &ledger).unwrap();
        state.reconcile_pending_outbound_payouts_after_upgrade();
        let after_upgrade = state.begin_outbound_payout_retry(caller, ledger).unwrap();
        assert_eq!(after_upgrade.kind, before_upgrade.kind);
        assert_eq!(after_upgrade.gross_amount, before_upgrade.gross_amount);
        assert_eq!(
            after_upgrade.transfer_amount,
            before_upgrade.transfer_amount
        );
        assert_eq!(after_upgrade.transfer_fee, before_upgrade.transfer_fee);
        assert_eq!(
            after_upgrade.transfer_created_at_time_ns,
            before_upgrade.transfer_created_at_time_ns
        );
        assert_eq!(after_upgrade.transfer_memo, before_upgrade.transfer_memo);
        assert!(after_upgrade.ambiguous_seen);
    }

    #[test]
    fn collateral_claim_reservation_is_durable_and_ambiguous_rejection_does_not_restore() {
        let caller = user_a();
        let ledger = icp_ledger();
        let mut state = test_state();
        add_deposit_direct(&mut state, caller, icusd_ledger(), 1_000_000);
        state
            .deposits
            .get_mut(&caller)
            .unwrap()
            .collateral_gains
            .insert(ledger, 500);
        let payout = state
            .prepare_collateral_payout(caller, ledger, 10, 99, b"claim-id".to_vec())
            .unwrap()
            .expect("claim row prepared");
        assert_eq!(payout.gross_amount, 500);
        assert_eq!(payout.transfer_amount, 490);
        assert_eq!(state.deposits[&caller].collateral_gains.get(&ledger), None);
        assert_eq!(
            state.deposits[&caller].total_claimed_gains.get(&ledger),
            None
        );

        state.reconcile_pending_outbound_payouts_after_upgrade();
        assert!(!state
            .reject_outbound_payout_without_effect(caller, ledger, "TooOld".into(), 100)
            .unwrap());
        assert_eq!(state.deposits[&caller].collateral_gains.get(&ledger), None);
        assert_eq!(
            state.deposits[&caller].total_claimed_gains.get(&ledger),
            None
        );
        assert_eq!(
            state.pending_outbound_payout(&caller, &ledger),
            Some(PendingOutboundPayout {
                dispatch_in_flight: false,
                ambiguous_seen: true,
                last_error: Some("typed rejection after prior ambiguity: TooOld".into()),
                ..payout
            })
        );
    }

    #[test]
    fn collateral_claim_no_effect_restores_gain_after_other_ledger_withdraw_removes_position() {
        let caller = user_a();
        let stable_ledger = icusd_ledger();
        let collateral_ledger = icp_ledger();
        let mut state = test_state();
        state.add_deposit_at(caller, stable_ledger, 100, 0);
        state
            .deposits
            .get_mut(&caller)
            .unwrap()
            .collateral_gains
            .insert(collateral_ledger, 500);
        state
            .prepare_collateral_payout(caller, collateral_ledger, 10, 99, b"claim".to_vec())
            .unwrap()
            .expect("claim row prepared");

        // A claim on ledger A and a full withdrawal on ledger B can overlap.
        // The withdrawal removes the now-empty position before the claim reply.
        state
            .process_withdrawal(caller, stable_ledger, 100)
            .unwrap();
        assert!(!state.deposits.contains_key(&caller));
        assert!(state
            .reject_outbound_payout_without_effect(caller, collateral_ledger, "BadFee".into(), 100)
            .unwrap());
        assert_eq!(
            state
                .deposits
                .get(&caller)
                .and_then(|pos| pos.collateral_gains.get(&collateral_ledger).copied()),
            Some(500),
        );
    }

    #[test]
    fn completed_collateral_payout_is_counted_once() {
        let caller = user_a();
        let ledger = icp_ledger();
        let mut state = test_state();
        add_deposit_direct(&mut state, caller, icusd_ledger(), 1_000_000);
        state
            .deposits
            .get_mut(&caller)
            .unwrap()
            .collateral_gains
            .insert(ledger, 500);
        state
            .prepare_collateral_payout(caller, ledger, 10, 99, b"claim-id".to_vec())
            .unwrap();
        assert!(state.complete_outbound_payout(caller, ledger, 100));
        assert!(!state.complete_outbound_payout(caller, ledger, 101));
        assert_eq!(
            state.deposits[&caller].total_claimed_gains.get(&ledger),
            Some(&500)
        );
    }

    #[test]
    fn pending_outbound_status_is_owner_scoped_and_preserves_exact_tuple() {
        let caller = user_a();
        let other = user_b();
        let ledger = icusd_ledger();
        let mut state = test_state();
        let payout = PendingOutboundPayout {
            kind: OutboundPayoutKind::Withdraw,
            request_amount: 40,
            gross_amount: 40,
            transfer_amount: 30,
            transfer_fee: 10,
            transfer_created_at_time_ns: 77,
            transfer_memo: b"stable-identity".to_vec(),
            dispatch_in_flight: false,
            ambiguous_seen: true,
            last_error: Some("unknown response".into()),
        };
        state
            .pending_outbound_payouts
            .as_mut()
            .unwrap()
            .insert((caller, ledger), payout);

        let status = state.pending_outbound_payouts_for(&caller);
        assert_eq!(status.len(), 1);
        assert_eq!(status[0].ledger, ledger);
        assert_eq!(status[0].transfer_amount, 30);
        assert_eq!(status[0].transfer_fee, 10);
        assert_eq!(status[0].transfer_created_at_time_ns, 77);
        assert_eq!(status[0].transfer_memo, b"stable-identity".to_vec());
        assert!(status[0].ambiguous_seen);
        assert!(state.pending_outbound_payouts_for(&other).is_empty());
    }

    // ─── Test: Effective pool computation ───

    #[test]
    fn test_effective_pool_for_collateral() {
        let mut state = test_state();

        // user_a: 50 icUSD + 20 ckUSDT = 70 USD
        // user_b: 30 icUSD = 30 USD
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 50_00000000);
        add_deposit_direct(&mut state, user_a(), ckusdt_ledger(), 20_000_000); // 20 ckUSDT (6 dec)
        add_deposit_direct(&mut state, user_b(), icusd_ledger(), 30_00000000);

        // All opted in -> total = 100 USD e8s
        assert_eq!(
            state.effective_pool_for_collateral(&icp_ledger()),
            100_00000000
        );

        // user_a opts out of ICP -> only user_b remains
        state.opt_out_collateral(&user_a(), icp_ledger()).unwrap();
        assert_eq!(
            state.effective_pool_for_collateral(&icp_ledger()),
            30_00000000
        );

        // ckBTC effective pool still has everyone
        assert_eq!(
            state.effective_pool_for_collateral(&ckbtc_ledger()),
            100_00000000
        );
    }

    #[test]
    fn cfx_sentinel_requires_explicit_opt_in_without_breaking_default_collateral() {
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        add_deposit_direct(&mut state, user_b(), icusd_ledger(), 50_00000000);
        state.register_chain_collateral_sentinel(cfx_sentinel());

        assert_eq!(
            state.effective_pool_for_collateral(&icp_ledger()),
            150_00000000,
            "non-chain collateral stays default-in",
        );
        assert_eq!(
            state.effective_pool_for_collateral(&cfx_sentinel()),
            0,
            "chain sentinel starts default-out",
        );
        assert!(state
            .compute_token_draw(10_00000000, &cfx_sentinel())
            .is_empty());

        state
            .opt_in_cfx(&user_a(), cfx_sentinel())
            .expect("user A opts into CFX");
        assert_eq!(
            state.effective_pool_for_collateral(&cfx_sentinel()),
            100_00000000
        );
        let draw = state.compute_token_draw(10_00000000, &cfx_sentinel());
        assert_eq!(draw.get(&icusd_ledger()).copied(), Some(10_00000000));

        state
            .opt_out_cfx(&user_a(), cfx_sentinel())
            .expect("user A opts back out");
        assert_eq!(state.effective_pool_for_collateral(&cfx_sentinel()), 0);
    }

    #[test]
    fn chain_collateral_sentinel_registration_is_stable_and_validation_safe() {
        let mut state = test_state();
        let sentinel = chain_collateral_sentinel(1030);
        assert_eq!(sentinel, chain_collateral_sentinel(1030));
        assert_ne!(sentinel, chain_collateral_sentinel(71));
        assert_ne!(sentinel, icusd_ledger());
        assert_ne!(sentinel, icp_ledger());

        let registered = state
            .register_chain_collateral(1030, "CFX".to_string(), 18)
            .expect("register CFX sentinel");
        assert_eq!(registered, sentinel);
        assert!(state.is_chain_collateral_sentinel(&sentinel));
        let info = state
            .collateral_registry
            .get(&sentinel)
            .expect("sentinel registered as collateral");
        assert_eq!(info.symbol, "CFX");
        assert_eq!(info.decimals, 18);
        assert!(matches!(info.status, CollateralStatus::Active));
        assert_eq!(state.effective_pool_for_collateral(&sentinel), 0);
        assert!(
            state.validate_state().is_ok(),
            "sentinel is metadata, not a ledger aggregate"
        );
    }

    #[test]
    fn chain_liquidation_gains_credit_u128_cfx_claims_with_dust() {
        const E18: u128 = 1_000_000_000_000_000_000;
        let mut state = test_state();
        state.register_chain_collateral_sentinel(cfx_sentinel());
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 1_00000000);
        add_deposit_direct(&mut state, user_b(), icusd_ledger(), 1_00000000);
        state.opt_in_cfx(&user_a(), cfx_sentinel()).unwrap();
        state.opt_in_cfx(&user_b(), cfx_sentinel()).unwrap();
        let mut stables_consumed = BTreeMap::new();
        stables_consumed.insert(icusd_ledger(), 2_00000000);

        state
            .process_chain_liquidation_gains_at(
                99,
                cfx_sentinel(),
                &stables_consumed,
                20_000 * E18 + 1,
                5_000_000,
                123,
            )
            .unwrap();

        let claim_a = state
            .deposits
            .get(&user_a())
            .unwrap()
            .cfx_claims
            .as_ref()
            .unwrap()
            .get(&cfx_sentinel())
            .copied()
            .unwrap_or(0);
        let claim_b = state
            .deposits
            .get(&user_b())
            .unwrap()
            .cfx_claims
            .as_ref()
            .unwrap()
            .get(&cfx_sentinel())
            .copied()
            .unwrap_or(0);
        assert_eq!(
            claim_a,
            10_000 * E18 + 1,
            "first opted-in depositor receives wei dust"
        );
        assert_eq!(claim_b, 10_000 * E18);
        assert!(
            claim_a > u64::MAX as u128,
            "CFX claim must not truncate to u64"
        );
        assert_eq!(
            state
                .total_stablecoin_balances
                .get(&icusd_ledger())
                .copied(),
            Some(0)
        );
        assert!(
            state.deposits.contains_key(&user_a()),
            "CFX claim keeps drained position alive"
        );

        state.mark_cfx_claimed(&user_a(), &cfx_sentinel(), 3 * E18);
        let after_partial = state
            .deposits
            .get(&user_a())
            .unwrap()
            .cfx_claims
            .as_ref()
            .unwrap()
            .get(&cfx_sentinel())
            .copied()
            .unwrap_or(0);
        assert_eq!(after_partial, 9_997 * E18 + 1);
        state.mark_cfx_claimed(&user_a(), &cfx_sentinel(), 9_997 * E18 + 1);
        assert!(state
            .deposits
            .get(&user_a())
            .unwrap()
            .cfx_claims
            .as_ref()
            .unwrap()
            .get(&cfx_sentinel())
            .is_none());
    }

    #[test]
    fn chain_icusd_draw_ignores_ckstables_and_lp_tokens() {
        let mut state = test_state_with_3usd();
        state.register_chain_collateral_sentinel(cfx_sentinel());

        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 40_00000000);
        add_deposit_direct(&mut state, user_a(), ckusdc_ledger(), 500_000_000);
        add_deposit_direct(&mut state, user_a(), three_usd_ledger(), 500_00000000);
        add_deposit_direct(&mut state, user_b(), icusd_ledger(), 100_00000000);
        state.opt_in_cfx(&user_a(), cfx_sentinel()).unwrap();

        assert_eq!(
            state.effective_icusd_pool_for_collateral(&cfx_sentinel()),
            40_00000000,
            "only opted-in icUSD counts toward chain absorb coverage",
        );

        let draw = state.compute_icusd_chain_draw(70_00000000, &cfx_sentinel());
        assert_eq!(draw.len(), 1, "chain draw should contain only icUSD");
        assert_eq!(
            draw.get(&icusd_ledger()).copied(),
            Some(40_00000000),
            "chain draw caps to opted-in icUSD, not total stable value",
        );
        assert!(
            !draw.contains_key(&ckusdc_ledger()),
            "ckUSDC must not be burned for Inc 4 chain absorb"
        );
        assert!(
            !draw.contains_key(&three_usd_ledger()),
            "3USD LP must not be burned for Inc 4 chain absorb"
        );

        state.opt_in_cfx(&user_b(), cfx_sentinel()).unwrap();
        let full_draw = state.compute_icusd_chain_draw(70_00000000, &cfx_sentinel());
        assert_eq!(
            full_draw.get(&icusd_ledger()).copied(),
            Some(70_00000000),
            "additional opted-in icUSD can cover the requested debt",
        );
    }

    // ─── Test: Multi-token liquidation with mixed decimals ───

    #[test]
    fn test_liquidation_multi_token_mixed_decimals() {
        let mut state = test_state();

        // user_a: 100 icUSD (8 dec) + 50 ckUSDT (6 dec)
        // user_b: 50 ckUSDC (6 dec)
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        add_deposit_direct(&mut state, user_a(), ckusdt_ledger(), 50_000_000); // 50 ckUSDT
        add_deposit_direct(&mut state, user_b(), ckusdc_ledger(), 50_000_000); // 50 ckUSDC

        // Total pool: 100 + 50 + 50 = 200 USD

        // Liquidation consumes some of each token
        let mut stables_consumed = BTreeMap::new();
        stables_consumed.insert(ckusdt_ledger(), 20_000_000); // 20 ckUSDT consumed
        stables_consumed.insert(ckusdc_ledger(), 20_000_000); // 20 ckUSDC consumed
                                                              // total consumed = 40 USD

        state
            .process_liquidation_gains_at(
                10,
                icp_ledger(),
                &stables_consumed,
                20_00000000,
                7_50000000,
                3_000_000_000,
            )
            .unwrap();

        // user_a has all the ckUSDT, so consumes all 20 ckUSDT
        let pos_a = state.deposits.get(&user_a()).unwrap();
        assert_eq!(
            pos_a
                .stablecoin_balances
                .get(&ckusdt_ledger())
                .copied()
                .unwrap_or(0),
            30_000_000
        ); // 50 - 20
           // user_a's icUSD should be untouched (not consumed)
        assert_eq!(
            pos_a
                .stablecoin_balances
                .get(&icusd_ledger())
                .copied()
                .unwrap_or(0),
            100_00000000
        );

        // user_b has all the ckUSDC, so consumes all 20 ckUSDC
        let pos_b = state.deposits.get(&user_b()).unwrap();
        assert_eq!(
            pos_b
                .stablecoin_balances
                .get(&ckusdc_ledger())
                .copied()
                .unwrap_or(0),
            30_000_000
        ); // 50 - 20

        // Collateral distribution: each consumed 20 USD worth out of 40 total = 50% each
        // user_a: 50% of 20 ICP = 10 ICP
        // user_b: 50% of 20 ICP = 10 ICP
        assert_eq!(
            pos_a
                .collateral_gains
                .get(&icp_ledger())
                .copied()
                .unwrap_or(0),
            10_00000000
        );
        assert_eq!(
            pos_b
                .collateral_gains
                .get(&icp_ledger())
                .copied()
                .unwrap_or(0),
            10_00000000
        );
    }

    // ─── Test: Liquidation cleans up fully consumed positions ───

    #[test]
    fn test_liquidation_cleans_empty_positions() {
        let mut state = test_state();

        // user_a: 100 icUSD (will be fully consumed)
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);

        let mut stables_consumed = BTreeMap::new();
        stables_consumed.insert(icusd_ledger(), 100_00000000); // consume all 100 icUSD

        state
            .process_liquidation_gains_at(
                5,
                icp_ledger(),
                &stables_consumed,
                50_00000000,
                7_50000000,
                4_000_000_000,
            )
            .unwrap();

        // user_a's stablecoin balance is zero, but they have collateral gains
        // so position should NOT be removed
        let pos = state.deposits.get(&user_a());
        assert!(
            pos.is_some(),
            "Position with collateral gains should not be cleaned up"
        );
        let pos = pos.unwrap();
        assert_eq!(
            pos.stablecoin_balances
                .get(&icusd_ledger())
                .copied()
                .unwrap_or(0),
            0
        );
        assert_eq!(
            pos.collateral_gains
                .get(&icp_ledger())
                .copied()
                .unwrap_or(0),
            50_00000000
        );
    }

    // ─── Test: Token draw with no available balance ───

    #[test]
    fn test_token_draw_empty_pool() {
        let state = test_state();

        // No deposits -> empty draw
        let draw = state.compute_token_draw(100_00000000, &icp_ledger());
        assert!(draw.is_empty(), "Draw from empty pool should be empty");
    }

    // ─── Test: Register stablecoin initializes aggregate tracking ───

    #[test]
    fn test_register_stablecoin_initializes_totals() {
        let state = test_state();

        // Registration should initialize zero balances in total tracking
        assert_eq!(
            state.total_stablecoin_balances.get(&icusd_ledger()),
            Some(&0)
        );
        assert_eq!(
            state.total_stablecoin_balances.get(&ckusdt_ledger()),
            Some(&0)
        );
        assert_eq!(
            state.total_stablecoin_balances.get(&ckusdc_ledger()),
            Some(&0)
        );
    }

    // ─── Test: Pool status query ───

    #[test]
    fn test_get_pool_status() {
        let mut state = test_state();

        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        add_deposit_direct(&mut state, user_b(), ckusdt_ledger(), 50_000_000); // 50 ckUSDT

        let status = state.get_pool_status();
        // 100 icUSD + 50 ckUSDT = 150 USD in e8s
        assert_eq!(status.total_deposits_e8s, 150_00000000);
        assert_eq!(status.total_depositors, 2);
        assert_eq!(status.total_liquidations_executed, 0);
        assert!(!status.emergency_paused);
    }

    // ─── Test: User position query ───

    #[test]
    fn test_get_user_position() {
        let mut state = test_state();

        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        add_deposit_direct(&mut state, user_a(), ckusdt_ledger(), 25_000_000);
        state
            .deposits
            .get_mut(&user_a())
            .unwrap()
            .cfx_claims
            .get_or_insert_with(BTreeMap::new)
            .insert(cfx_sentinel(), 1_000_000_000_000_000_000);

        let pos = state.get_user_position(&user_a()).unwrap();
        assert_eq!(pos.total_usd_value_e8s, 125_00000000); // 100 + 25
        assert_eq!(
            pos.stablecoin_balances.get(&icusd_ledger()),
            Some(&100_00000000)
        );
        assert_eq!(
            pos.stablecoin_balances.get(&ckusdt_ledger()),
            Some(&25_000_000)
        );
        assert_eq!(
            pos.cfx_claims
                .as_ref()
                .and_then(|claims| claims.get(&cfx_sentinel()))
                .copied(),
            Some(1_000_000_000_000_000_000),
            "user position exposes restored CFX claims for auditability",
        );

        add_deposit_direct(&mut state, user_b(), icusd_ledger(), 1_00000000);
        let pos_b = state.get_user_position(&user_b()).unwrap();
        assert!(
            pos_b.cfx_claims.clone().unwrap_or_default().is_empty(),
            "normal depositors without chain claims expose an empty optional map",
        );

        // Nonexistent user
        assert!(state.get_user_position(&user_c()).is_none());
    }

    // ─── Test: Multiple deposits accumulate ───

    #[test]
    fn test_multiple_deposits_accumulate() {
        let mut state = test_state();

        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 50_00000000);
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 30_00000000);
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 20_00000000);

        let pos = state.deposits.get(&user_a()).unwrap();
        assert_eq!(
            pos.stablecoin_balances.get(&icusd_ledger()),
            Some(&100_00000000)
        );
        assert_eq!(
            state.total_stablecoin_balances.get(&icusd_ledger()),
            Some(&100_00000000)
        );
    }

    // ─── Test: Interest Distribution ───

    #[test]
    fn test_distribute_interest_single_depositor() {
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);

        state.distribute_interest_revenue(icusd_ledger(), 5_00000000, None);

        let pos = state.deposits.get(&user_a()).unwrap();
        assert_eq!(pos.stablecoin_balances[&icusd_ledger()], 105_00000000);
        assert_eq!(pos.total_interest_earned_e8s, Some(5_00000000)); // icUSD is 8 decimals = e8s
        assert_eq!(state.total_interest_received_e8s, Some(5_00000000));
        assert_eq!(
            state.total_stablecoin_balances[&icusd_ledger()],
            105_00000000
        );
    }

    #[test]
    fn test_distribute_interest_proportional() {
        let mut state = test_state();
        // A has 75%, B has 25%
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 75_00000000);
        add_deposit_direct(&mut state, user_b(), icusd_ledger(), 25_00000000);

        state.distribute_interest_revenue(icusd_ledger(), 10_00000000, None);

        let a = state.deposits.get(&user_a()).unwrap();
        let b = state.deposits.get(&user_b()).unwrap();
        // A gets 7.5, B gets 2.5
        assert_eq!(a.stablecoin_balances[&icusd_ledger()], 82_50000000);
        assert_eq!(b.stablecoin_balances[&icusd_ledger()], 27_50000000);
        // Total should be exactly original + interest
        assert_eq!(
            state.total_stablecoin_balances[&icusd_ledger()],
            110_00000000
        );
    }

    #[test]
    fn test_distribute_interest_zero_total_noop() {
        let mut state = test_state();
        // No depositors for icUSD
        state.distribute_interest_revenue(icusd_ledger(), 5_00000000, None);
        assert_eq!(state.total_interest_received_e8s, Some(0));
    }

    #[test]
    fn test_distribute_interest_dust_handling() {
        let mut state = test_state();
        // 3 depositors with equal balances, interest = 10 (not divisible by 3)
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100);
        add_deposit_direct(&mut state, user_b(), icusd_ledger(), 100);
        add_deposit_direct(
            &mut state,
            Principal::from_slice(&[99]),
            icusd_ledger(),
            100,
        );

        state.distribute_interest_revenue(icusd_ledger(), 10, None);

        // Each gets floor(10 * 100/300) = 3. Dust = 10 - 9 = 1 goes to first depositor.
        let total: u64 = state
            .deposits
            .values()
            .map(|p| {
                p.stablecoin_balances
                    .get(&icusd_ledger())
                    .copied()
                    .unwrap_or(0)
            })
            .sum();
        assert_eq!(total, 310, "All interest must be accounted for");
        assert_eq!(state.total_stablecoin_balances[&icusd_ledger()], 310);
    }

    #[test]
    fn test_distribute_interest_cross_stablecoin() {
        // Under the icUSD-only interest rule, a ckUSDT-only depositor earns
        // no interest. They still participate in liquidations pro-rata
        // (separate code path) but are excluded from the interest stream.
        let mut state = test_state(); // Already has ckUSDT registered (6 decimals, priority 2)

        // A deposits 50 icUSD, B deposits 50 ckUSDT (both worth $50)
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 50_00000000);
        add_deposit_direct(&mut state, user_b(), ckusdt_ledger(), 50_000_000); // 50 * 10^6

        // Distribute 10 icUSD interest
        state.distribute_interest_revenue(icusd_ledger(), 10_00000000, None);

        let a = state.deposits.get(&user_a()).unwrap();
        let b = state.deposits.get(&user_b()).unwrap();
        let a_interest = a.stablecoin_balances[&icusd_ledger()] - 50_00000000;
        let b_interest = b
            .stablecoin_balances
            .get(&icusd_ledger())
            .copied()
            .unwrap_or(0);
        // icUSD-only rule: A (the icUSD depositor) gets the full 10 icUSD,
        // B (the ckUSDT depositor) gets nothing.
        assert_eq!(
            a_interest, 10_00000000,
            "icUSD depositor should receive the full 10 icUSD interest"
        );
        assert_eq!(
            b_interest, 0,
            "ckUSDT depositor should earn no interest under icUSD-only rule"
        );
        assert_eq!(
            b.stablecoin_balances[&ckusdt_ledger()],
            50_000_000,
            "B: ckUSDT unchanged"
        );
        assert_eq!(
            state.total_stablecoin_balances[&icusd_ledger()],
            60_00000000
        );
    }

    #[test]
    fn test_distribute_interest_3usd_lp_depositor() {
        // Under the icUSD-only interest rule, a 3USD LP depositor earns
        // no interest. They still participate in liquidations pro-rata
        // (separate code path) but are excluded from the interest stream.
        let mut state = test_state_with_3usd();

        // A deposits 100 icUSD ($100), B deposits 100 3USD (worth ~$104.92 at vp=1.0492)
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        add_deposit_direct(&mut state, user_b(), three_usd_ledger(), 100_00000000);

        // Distribute 20 icUSD interest
        state.distribute_interest_revenue(icusd_ledger(), 20_00000000, None);

        let a = state.deposits.get(&user_a()).unwrap();
        let b = state.deposits.get(&user_b()).unwrap();
        let a_interest = a.stablecoin_balances[&icusd_ledger()] - 100_00000000;
        let b_interest = b
            .stablecoin_balances
            .get(&icusd_ledger())
            .copied()
            .unwrap_or(0);
        // icUSD-only rule: A (the icUSD depositor) takes the entire 20 icUSD,
        // B (the 3USD LP depositor) gets nothing.
        assert_eq!(
            a_interest, 20_00000000,
            "icUSD depositor should receive the full 20 icUSD interest"
        );
        assert_eq!(
            b_interest, 0,
            "3USD depositor should earn no interest under icUSD-only rule"
        );
        assert_eq!(
            b.stablecoin_balances[&three_usd_ledger()],
            100_00000000,
            "B: 3USD position unchanged"
        );
    }

    #[test]
    fn test_distribute_interest_icusd_only() {
        // Two depositors, both opted in for ICP collateral interest:
        //   - user_a: 100 icUSD
        //   - user_b: 100 3USD (LP token, virtual_price = 1.0492)
        // Interest of 10 icUSD is distributed.
        // Expected (under icUSD-only rule): user_a gets 10 icUSD, user_b gets 0.
        let mut state = test_state_with_3usd();

        // Deposit 100 icUSD for user_a, 100 3USD for user_b (both opted in for ICP by default)
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        add_deposit_direct(&mut state, user_b(), three_usd_ledger(), 100_00000000);

        state.distribute_interest_revenue(icusd_ledger(), 10_00000000, Some(icp_ledger()));

        let alice_icusd = state
            .deposits
            .get(&user_a())
            .unwrap()
            .stablecoin_balances
            .get(&icusd_ledger())
            .copied()
            .unwrap_or(0);
        let bob_icusd = state
            .deposits
            .get(&user_b())
            .unwrap()
            .stablecoin_balances
            .get(&icusd_ledger())
            .copied()
            .unwrap_or(0);

        assert_eq!(
            alice_icusd,
            100_00000000 + 10_00000000,
            "icUSD depositor should receive the full 10 icUSD interest"
        );
        assert_eq!(
            bob_icusd, 0,
            "3USD depositor should receive no icUSD interest"
        );
    }

    #[test]
    fn test_distribute_interest_mixed_position() {
        // Tests the most common real-world scenario: a depositor with both icUSD
        // and 3USD. Only the icUSD slice should count toward their interest share.
        //
        // Setup:
        //   - user_a: 100 icUSD only
        //   - user_b: 100 icUSD + 100 3USD (mixed position)
        // Distribute: 20 icUSD interest, no collateral_type filter
        // Expected (pro-rata on icUSD-only): each gets 10 icUSD
        //   - user_a: 100 → 110
        //   - user_b's icUSD: 100 → 110 (user_b's 3USD unchanged at 100)
        let mut state = test_state_with_3usd();

        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        add_deposit_direct(&mut state, user_b(), icusd_ledger(), 100_00000000);
        add_deposit_direct(&mut state, user_b(), three_usd_ledger(), 100_00000000);

        state.distribute_interest_revenue(icusd_ledger(), 20_00000000, None);

        let alice_icusd = state
            .deposits
            .get(&user_a())
            .unwrap()
            .stablecoin_balances
            .get(&icusd_ledger())
            .copied()
            .unwrap_or(0);
        let bob_icusd = state
            .deposits
            .get(&user_b())
            .unwrap()
            .stablecoin_balances
            .get(&icusd_ledger())
            .copied()
            .unwrap_or(0);
        let bob_three_usd = state
            .deposits
            .get(&user_b())
            .unwrap()
            .stablecoin_balances
            .get(&three_usd_ledger())
            .copied()
            .unwrap_or(0);

        assert_eq!(
            alice_icusd, 110_00000000,
            "user_a (100 icUSD) earns 10 icUSD on equal-icUSD share"
        );
        assert_eq!(
            bob_icusd, 110_00000000,
            "user_b's icUSD slice (100) earns 10 icUSD (same as user_a)"
        );
        assert_eq!(
            bob_three_usd, 100_00000000,
            "user_b's 3USD balance is not credited and not touched"
        );
    }

    #[test]
    fn test_eligible_icusd_per_collateral_excludes_non_icusd_deposits() {
        let mut state = test_state_with_3usd();

        // Interest is paid only against icUSD. Mixed and non-icUSD positions
        // must not dilute the per-collateral APY denominator.
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        add_deposit_direct(&mut state, user_b(), icusd_ledger(), 100_00000000);
        add_deposit_direct(&mut state, user_b(), three_usd_ledger(), 100_00000000);
        add_deposit_direct(&mut state, user_c(), ckusdt_ledger(), 100_000_000);

        let status = state.get_pool_status();
        let eligible_icp = status
            .eligible_icusd_per_collateral
            .iter()
            .find(|(collateral, _)| *collateral == icp_ledger())
            .map(|(_, amount)| *amount)
            .expect("ICP collateral should be registered");

        assert_eq!(
            eligible_icp, 200_00000000,
            "only the two 100 icUSD balances should determine the interest APY"
        );

        let liquidation_capacity_icp = status
            .eligible_usd_per_collateral
            .as_ref()
            .expect("current canister must report all-token liquidation capacity")
            .iter()
            .find(|(collateral, _)| *collateral == icp_ledger())
            .map(|(_, amount)| *amount)
            .expect("ICP collateral should be registered");
        assert_eq!(
            liquidation_capacity_icp, 404_92000000,
            "liquidation capacity must still include 3USD at its virtual price and ckUSDT"
        );
    }

    #[test]
    fn test_distribute_interest_no_icusd_depositors_requires_reconciliation() {
        // Automatic reassignment of this mint to a later depositor would create
        // a windfall with no defined entitlement, so it remains uncredited for
        // explicit operator reconciliation.
        let mut state = test_state_with_3usd();

        // Both depositors hold only 3USD — no icUSD anywhere
        add_deposit_direct(&mut state, user_a(), three_usd_ledger(), 100_00000000);
        add_deposit_direct(&mut state, user_b(), three_usd_ledger(), 100_00000000);

        let total_received_before = state.total_interest_received_e8s.unwrap_or(0);
        let stable_balance_before = state
            .total_stablecoin_balances
            .get(&icusd_ledger())
            .copied()
            .unwrap_or(0);

        state.distribute_interest_revenue(icusd_ledger(), 10_00000000, Some(icp_ledger()));

        // No depositor or aggregate is credited.
        assert_eq!(
            state.total_interest_received_e8s.unwrap_or(0),
            total_received_before,
        );
        assert_eq!(
            state
                .total_stablecoin_balances
                .get(&icusd_ledger())
                .copied()
                .unwrap_or(0),
            stable_balance_before,
            "unallocated interest is not withdrawable without reconciliation"
        );

        // Neither depositor's balances changed
        for user in [user_a(), user_b()] {
            let pos = state.deposits.get(&user).unwrap();
            assert_eq!(
                pos.stablecoin_balances
                    .get(&icusd_ledger())
                    .copied()
                    .unwrap_or(0),
                0,
                "depositor without icUSD should not be credited"
            );
            assert_eq!(
                pos.stablecoin_balances
                    .get(&three_usd_ledger())
                    .copied()
                    .unwrap_or(0),
                100_00000000,
                "3USD balance unchanged"
            );
        }
    }

    #[test]
    fn interest_excludes_icusd_depositor_opted_out_of_source_collateral() {
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        add_deposit_direct(&mut state, user_b(), icusd_ledger(), 100_00000000);
        state.opt_out_collateral(&user_a(), icp_ledger()).unwrap();

        state.distribute_interest_revenue(icusd_ledger(), 20_00000000, Some(icp_ledger()));

        assert_eq!(
            state.deposits[&user_a()].stablecoin_balances[&icusd_ledger()],
            100_00000000,
            "a depositor opted out of ICP receives none of the ICP-vault interest"
        );
        assert_eq!(
            state.deposits[&user_b()].stablecoin_balances[&icusd_ledger()],
            120_00000000,
            "the opted-in icUSD depositor receives the full ICP-vault interest"
        );
    }

    #[test]
    fn interest_mint_receipt_prevents_duplicate_distribution_and_bounds_replay() {
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        let payload = InterestMintReceiptPayload {
            token_ledger: icusd_ledger(),
            amount: 10_00000000,
            collateral_type: None,
        };
        let apply_notification = |state: &mut StabilityPoolState, block| {
            if state.record_interest_mint_receipt(block, payload.clone())
                == InterestMintReceiptStatus::New
            {
                state
                    .try_distribute_interest_revenue(
                        payload.token_ledger,
                        payload.amount,
                        payload.collateral_type,
                    )
                    .expect("safe interest credit");
            }
        };

        apply_notification(&mut state, 55);
        apply_notification(&mut state, 55);
        assert_eq!(
            state.deposits[&user_a()].stablecoin_balances[&icusd_ledger()],
            110_00000000
        );
        assert_eq!(state.total_interest_received_e8s, Some(10_00000000));

        let bytes = Encode!(&state).expect("encode state");
        let mut restored = try_decode_state(&bytes).expect("decode state after upgrade");
        assert_eq!(
            restored.interest_mint_receipt_status(55, &payload),
            InterestMintReceiptStatus::Duplicate
        );
        let mut changed = payload.clone();
        changed.amount += 1;
        assert_eq!(
            restored.interest_mint_receipt_status(55, &changed),
            InterestMintReceiptStatus::PayloadMismatch
        );
        changed = payload.clone();
        changed.collateral_type = Some(icp_ledger());
        assert_eq!(
            restored.interest_mint_receipt_status(55, &changed),
            InterestMintReceiptStatus::PayloadMismatch
        );
        changed = payload.clone();
        changed.token_ledger = Principal::from_slice(&[99]);
        assert_eq!(
            restored.interest_mint_receipt_status(55, &changed),
            InterestMintReceiptStatus::PayloadMismatch
        );
        apply_notification(&mut restored, 55);
        assert_eq!(
            restored.deposits[&user_a()].stablecoin_balances[&icusd_ledger()],
            110_00000000
        );
        assert_eq!(restored.total_interest_received_e8s, Some(10_00000000));

        let high = 55 + MAX_PROCESSED_INTEREST_MINT_BLOCKS as u64;
        assert_eq!(
            restored.record_interest_mint_receipt(high, payload.clone()),
            InterestMintReceiptStatus::New
        );
        assert_eq!(
            restored.interest_mint_receipt_status(55, &payload),
            InterestMintReceiptStatus::OutsideReplayWindow
        );
    }

    #[test]
    fn interest_distribution_rejects_overflow_before_any_credit() {
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100);
        state
            .deposits
            .get_mut(&user_a())
            .unwrap()
            .total_interest_earned_e8s = Some(u64::MAX);
        let balances_before = state.deposits[&user_a()].stablecoin_balances.clone();
        let aggregate_before = state.total_stablecoin_balances.clone();
        assert!(matches!(
            state.try_distribute_interest_revenue(icusd_ledger(), 1, None),
            Err(StabilityPoolError::SystemBusy),
        ));
        assert_eq!(
            state.deposits[&user_a()].stablecoin_balances,
            balances_before
        );
        assert_eq!(state.total_stablecoin_balances, aggregate_before);
        assert_eq!(state.total_interest_received_e8s, Some(0));
    }

    #[test]
    fn legacy_block_only_interest_receipt_fails_closed_after_upgrade() {
        let mut state = test_state();
        state.processed_interest_mint_blocks = Some([55].into_iter().collect());
        state.processed_interest_mint_payloads = None;
        state.processed_interest_mint_block_high_watermark = Some(55);
        let payload = InterestMintReceiptPayload {
            token_ledger: icusd_ledger(),
            amount: 10,
            collateral_type: None,
        };
        assert_eq!(
            state.interest_mint_receipt_status(55, &payload),
            InterestMintReceiptStatus::OutsideReplayWindow,
        );
    }

    #[test]
    fn user_interest_eligibility_matches_native_opt_in_policy() {
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);

        let before = state.get_user_position(&user_a()).unwrap();
        assert!(
            !before
                .eligible_interest_collateral
                .as_ref()
                .expect("current canister reports exact eligibility")
                .contains(&xrp_ledger()),
            "XRP must be absent until a payout address opts the depositor in"
        );

        state
            .opt_in_native_collateral(&user_a(), xrp_ledger(), valid_xrp_address())
            .unwrap();
        let after = state.get_user_position(&user_a()).unwrap();
        assert!(
            after
                .eligible_interest_collateral
                .as_ref()
                .expect("current canister reports exact eligibility")
                .contains(&xrp_ledger()),
            "the UI-facing eligibility set must include XRP after native opt-in"
        );
    }

    // ─── Test: Rounding dust doesn't drift aggregate totals ───

    #[test]
    fn test_liquidation_no_rounding_drift() {
        let mut state = test_state();

        // Create 3 depositors with balances that produce rounding dust:
        // 3_333_333, 3_333_333, 3_333_334 = 10_000_000 total (in e8s icUSD)
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 3_333_333);
        add_deposit_direct(&mut state, user_b(), icusd_ledger(), 3_333_333);
        add_deposit_direct(&mut state, user_c(), icusd_ledger(), 3_333_334);

        // Total: 10_000_000
        assert!(state.validate_state().is_ok());

        // Consume 1_000_000 — proportional split produces rounding:
        // user_a: 1_000_000 * 3_333_333 / 10_000_000 = 333_333 (truncated from 333_333.3)
        // user_b: 333_333
        // user_c: 1_000_000 * 3_333_334 / 10_000_000 = 333_333 (truncated from 333_333.4)
        // Sum of shares: 999_999 (less than 1_000_000!)
        let mut consumed = BTreeMap::new();
        consumed.insert(icusd_ledger(), 1_000_000);

        state
            .process_liquidation_gains_at(
                1,
                icp_ledger(),
                &consumed,
                500_000,
                7_50000000,
                1_000_000_000,
            )
            .unwrap();

        // The critical assertion: aggregate should match sum of individual balances
        // even when rounding dust occurs. validate_state() checks this.
        assert!(
            state.validate_state().is_ok(),
            "State must remain consistent after rounding"
        );

        // Verify individual balances sum to aggregate
        let sum: u64 = state
            .deposits
            .values()
            .map(|p| {
                p.stablecoin_balances
                    .get(&icusd_ledger())
                    .copied()
                    .unwrap_or(0)
            })
            .sum();
        let tracked = state
            .total_stablecoin_balances
            .get(&icusd_ledger())
            .copied()
            .unwrap_or(0);
        assert_eq!(
            sum, tracked,
            "Sum of individual balances must equal aggregate"
        );
    }

    // ─── 3USD / LP Token Tests ───

    fn three_usd_ledger() -> Principal {
        Principal::from_slice(&[30])
    }
    fn three_pool_canister() -> Principal {
        Principal::from_slice(&[31])
    }

    /// Build a test state with 3USD LP token registered.
    fn test_state_with_3usd() -> StabilityPoolState {
        let mut state = test_state();
        state.register_stablecoin(StablecoinConfig {
            ledger_id: three_usd_ledger(),
            symbol: "3USD".to_string(),
            decimals: 8,
            priority: 0, // consumed last
            is_active: true,
            transfer_fee: Some(0),
            is_lp_token: Some(true),
            underlying_pool: Some(three_pool_canister()),
        });
        // Set virtual price to ~1.0492 (scaled 1e18)
        state
            .cached_virtual_prices
            .get_or_insert_with(BTreeMap::new)
            .insert(three_usd_ledger(), 1_049_200_000_000_000_000u128);
        state
    }

    fn prepare_test_three_usd_absorb() -> (StabilityPoolState, u64, Principal, Principal) {
        let mut state = test_state_with_3usd();
        let pool = Principal::from_slice(&[41]);
        let backend = Principal::from_slice(&[42]);
        let collateral = icp_ledger();
        state.protocol_canister_id = backend;
        add_deposit_direct(&mut state, user_a(), three_usd_ledger(), 700);
        add_deposit_direct(&mut state, user_b(), three_usd_ledger(), 300);
        state
            .deposits
            .get_mut(&user_b())
            .unwrap()
            .opted_out_collateral
            .insert(collateral);
        let row = state
            .prepare_sp_three_usd_absorb(
                77,
                pool,
                three_usd_ledger(),
                collateral,
                123_000_000,
                456_000_000,
                600,
                600,
                1_000_000_000_000_000_000,
                SpThreeUsdApprovalIntent {
                    ledger: three_usd_ledger(),
                    allowance: 600,
                    fee: 3,
                    memo: vec![7; 16],
                    created_at_time_ns: 100,
                    expires_at_ns: 200,
                },
            )
            .unwrap();
        assert_eq!(row.absorb_id, 1);
        (state, row.absorb_id, pool, backend)
    }

    fn test_three_usd_absorbed_evidence(
        absorb_id: u64,
        pool: Principal,
        backend: Principal,
        ledger: Principal,
        collateral: Principal,
    ) -> SpThreeUsdTerminalEvidence {
        use icrc_ledger_types::icrc1::account::Account;
        let nonce = (100u128 << 64) | 11;
        let tuple = rumi_protocol_backend::state::ThreeUsdReserveIngressTuple {
            spender_owner: backend,
            spender_subaccount: None,
            source: Account {
                owner: pool,
                subaccount: None,
            },
            destination: Account {
                owner: backend,
                subaccount: None,
            },
            amount_e8s: 600,
            fee_e8s: None,
            memo: rumi_protocol_backend::management::nonce_to_memo(nonce)
                .0
                .as_ref()
                .try_into()
                .unwrap(),
            created_at_time_ns: rumi_protocol_backend::management::nonce_to_created_at_time(nonce),
            op_nonce: nonce,
            parent_absorb_id: Some(absorb_id),
        };
        let result = rumi_protocol_backend::state::ThreeUsdReserveIngressResult {
            success: true,
            vault_id: 77,
            liquidated_debt: 360,
            collateral_received: 105,
            collateral_type: collateral.to_string(),
            block_index: 10,
            fee: 0,
            collateral_price_e8s: 123_000_000,
        };
        let refund_nonce = (100u128 << 64) | 12;
        let refund = rumi_protocol_backend::state::ThreeUsdReserveIngressRefundReceipt {
            block_index: 12,
            tuple: rumi_protocol_backend::state::ThreeUsdRefundTransferTuple {
                source_owner: backend,
                source_subaccount: None,
                destination: Account {
                    owner: pool,
                    subaccount: None,
                },
                amount_e8s: 240,
                fee_e8s: 2,
                memo: rumi_protocol_backend::management::nonce_to_memo(refund_nonce)
                    .0
                    .as_ref()
                    .try_into()
                    .unwrap(),
                created_at_time_ns: rumi_protocol_backend::management::nonce_to_created_at_time(
                    refund_nonce,
                ),
            },
        };
        let payout_nonce = (100u128 << 64) | 13;
        let payout = rumi_protocol_backend::state::ThreeUsdReserveIngressPayoutReceipt {
            block_index: 13,
            tuple: rumi_protocol_backend::state::ThreeUsdReserveIngressPayoutTuple {
                op_nonce: payout_nonce,
                ledger: collateral,
                proof_kind: rumi_protocol_backend::state::PayoutProofKind::Icrc3,
                source: Account {
                    owner: backend,
                    subaccount: None,
                },
                destination: Account {
                    owner: pool,
                    subaccount: None,
                },
                gross_amount_e8s: 105,
                net_amount_e8s: 100,
                fee_e8s: 5,
                memo: rumi_protocol_backend::management::nonce_to_memo(payout_nonce)
                    .0
                    .as_ref()
                    .try_into()
                    .unwrap(),
                created_at_time_ns: rumi_protocol_backend::management::nonce_to_created_at_time(
                    payout_nonce,
                ),
                collateral_type: collateral,
            },
        };
        SpThreeUsdTerminalEvidence::Absorbed {
            backend_vault_id: 77,
            backend_absorb_id: absorb_id,
            request: rumi_protocol_backend::state::ThreeUsdReserveIngressRequest {
                icusd_debt_covered_e8s: 600,
                three_usd_amount_e8s: 600,
                ledger,
            },
            transfer_tuple: tuple,
            transfer_block_index: 10,
            observed_transfer_fee: 5,
            proof: rumi_protocol_backend::icrc3_proof::SpWritedownProof {
                block_index: 10,
                ledger_kind:
                    rumi_protocol_backend::icrc3_proof::SpProofLedger::ThreePoolTransferDefault,
                vault_id_memo: 77,
            },
            result,
            proportional_refund: Some(refund),
            payout_receipt: payout,
        }
    }

    fn start_test_three_usd_backend(state: &mut StabilityPoolState, absorb_id: u64) {
        state
            .mark_sp_three_usd_approval_dispatch(absorb_id)
            .unwrap();
        state
            .record_sp_three_usd_approval_receipt(absorb_id, 5, 3)
            .unwrap();
        state.mark_sp_three_usd_backend_dispatch(absorb_id).unwrap();
    }

    #[test]
    fn three_usd_absorb_pins_plan_and_commits_exactly_once() {
        let (mut state, absorb_id, pool, backend) = prepare_test_three_usd_absorb();
        start_test_three_usd_backend(&mut state, absorb_id);
        let mut evidence = test_three_usd_absorbed_evidence(
            absorb_id,
            pool,
            backend,
            three_usd_ledger(),
            icp_ledger(),
        );
        if let SpThreeUsdTerminalEvidence::Absorbed { result, .. } = &mut evidence {
            // Oracle refresh between admission and backend commit is normal;
            // terminal history must use the authenticated execution quote.
            result.collateral_price_e8s = 130_000_000;
        }
        let plan = state
            .plan_sp_three_usd_terminal(absorb_id, evidence)
            .unwrap();
        assert_eq!(plan.principal_consumed, 360);
        assert_eq!(plan.refund_amount_received, 240);
        assert_eq!(plan.total_stable_debit, 363); // pool pays principal plus approval fee; backend pays pull/refund fees
        assert_eq!(plan.principal_debits, BTreeMap::from([(user_a(), 360)]));
        assert_eq!(plan.collateral_credits, BTreeMap::from([(user_a(), 100)]));
        assert_eq!(
            state.deposits[&user_a()].stablecoin_balances[&three_usd_ledger()],
            700
        );
        assert_eq!(state.total_stablecoin_balances[&three_usd_ledger()], 1_000);
        state.apply_sp_three_usd_terminal(absorb_id).unwrap();
        assert_eq!(
            state.deposits[&user_a()].stablecoin_balances[&three_usd_ledger()],
            338
        );
        assert_eq!(
            state.deposits[&user_b()].stablecoin_balances[&three_usd_ledger()],
            299
        );
        assert_eq!(
            state.deposits[&user_a()].collateral_gains[&icp_ledger()],
            100
        );
        assert_eq!(
            state
                .liquidation_history
                .last()
                .unwrap()
                .collateral_price_e8s,
            Some(130_000_000)
        );
        assert_eq!(state.total_stablecoin_balances[&three_usd_ledger()], 637);
        assert!(state
            .pending_sp_three_usd_absorbs
            .as_ref()
            .unwrap()
            .is_empty());
        assert_eq!(
            state.completed_sp_three_usd_absorbs.as_ref().unwrap()[&absorb_id].phase,
            SpThreeUsdAbsorbPhase::Complete
        );
        let after = state.total_stablecoin_balances[&three_usd_ledger()];
        state.apply_sp_three_usd_terminal(absorb_id).unwrap();
        assert_eq!(state.total_stablecoin_balances[&three_usd_ledger()], after);
    }

    #[test]
    fn three_usd_absorb_accepts_pinned_native_icp_payout_adapter() {
        let (mut state, absorb_id, pool, backend) = prepare_test_three_usd_absorb();
        start_test_three_usd_backend(&mut state, absorb_id);
        let mut evidence = test_three_usd_absorbed_evidence(
            absorb_id,
            pool,
            backend,
            three_usd_ledger(),
            icp_ledger(),
        );
        let SpThreeUsdTerminalEvidence::Absorbed { payout_receipt, .. } = &mut evidence else {
            unreachable!();
        };
        payout_receipt.tuple.proof_kind = rumi_protocol_backend::state::PayoutProofKind::NativeIcp;

        let plan = state
            .plan_sp_three_usd_terminal(absorb_id, evidence)
            .unwrap();
        assert_eq!(plan.collateral_received, 100);
        assert_eq!(plan.collateral_credits, BTreeMap::from([(user_a(), 100)]));
    }

    #[test]
    fn three_usd_absorb_accepts_legacy_zero_result_block_sentinel() {
        let (mut state, absorb_id, pool, backend) = prepare_test_three_usd_absorb();
        start_test_three_usd_backend(&mut state, absorb_id);
        let mut evidence = test_three_usd_absorbed_evidence(
            absorb_id,
            pool,
            backend,
            three_usd_ledger(),
            icp_ledger(),
        );
        let SpThreeUsdTerminalEvidence::Absorbed { result, .. } = &mut evidence else {
            unreachable!();
        };
        result.block_index = 0;

        let plan = state
            .plan_sp_three_usd_terminal(absorb_id, evidence)
            .unwrap();
        assert_eq!(plan.collateral_received, 100);
    }

    #[test]
    fn three_usd_absorb_rejects_zero_execution_price_and_wrong_collateral() {
        let (mut state, absorb_id, pool, backend) = prepare_test_three_usd_absorb();
        start_test_three_usd_backend(&mut state, absorb_id);
        let mut evidence = test_three_usd_absorbed_evidence(
            absorb_id,
            pool,
            backend,
            three_usd_ledger(),
            icp_ledger(),
        );
        if let SpThreeUsdTerminalEvidence::Absorbed { result, .. } = &mut evidence {
            result.collateral_price_e8s = 0;
        }
        assert!(state
            .plan_sp_three_usd_terminal(absorb_id, evidence)
            .is_err());

        let (mut state, absorb_id, pool, backend) = prepare_test_three_usd_absorb();
        start_test_three_usd_backend(&mut state, absorb_id);
        let mut evidence = test_three_usd_absorbed_evidence(
            absorb_id,
            pool,
            backend,
            three_usd_ledger(),
            icp_ledger(),
        );
        if let SpThreeUsdTerminalEvidence::Absorbed { result, .. } = &mut evidence {
            result.collateral_type = Principal::from_slice(&[99]).to_string();
        }
        assert!(state
            .plan_sp_three_usd_terminal(absorb_id, evidence)
            .is_err());
    }

    #[test]
    fn three_usd_absorb_mismatch_and_snapshot_drift_fail_without_partial_commit() {
        let (mut state, absorb_id, pool, backend) = prepare_test_three_usd_absorb();
        start_test_three_usd_backend(&mut state, absorb_id);
        let mut wrong = test_three_usd_absorbed_evidence(
            absorb_id,
            pool,
            backend,
            three_usd_ledger(),
            icp_ledger(),
        );
        if let SpThreeUsdTerminalEvidence::Absorbed {
            backend_vault_id, ..
        } = &mut wrong
        {
            *backend_vault_id = 78;
        }
        assert!(state.plan_sp_three_usd_terminal(absorb_id, wrong).is_err());
        let evidence = test_three_usd_absorbed_evidence(
            absorb_id,
            pool,
            backend,
            three_usd_ledger(),
            icp_ledger(),
        );
        state
            .plan_sp_three_usd_terminal(absorb_id, evidence)
            .unwrap();
        state
            .deposits
            .get_mut(&user_b())
            .unwrap()
            .opted_out_collateral
            .remove(&icp_ledger());
        let before_alice = state.deposits[&user_a()].stablecoin_balances[&three_usd_ledger()];
        let before_total = state.total_stablecoin_balances[&three_usd_ledger()];
        assert!(state.apply_sp_three_usd_terminal(absorb_id).is_err());
        assert_eq!(
            state.deposits[&user_a()].stablecoin_balances[&three_usd_ledger()],
            before_alice
        );
        assert_eq!(
            state.total_stablecoin_balances[&three_usd_ledger()],
            before_total
        );
        assert!(state
            .pending_sp_three_usd_absorbs
            .as_ref()
            .unwrap()
            .contains_key(&absorb_id));
    }

    #[test]
    fn three_usd_absorb_full_refund_only_debits_proven_pool_fees() {
        let (mut state, absorb_id, pool, backend) = prepare_test_three_usd_absorb();
        start_test_three_usd_backend(&mut state, absorb_id);
        let absorbed = test_three_usd_absorbed_evidence(
            absorb_id,
            pool,
            backend,
            three_usd_ledger(),
            icp_ledger(),
        );
        let SpThreeUsdTerminalEvidence::Absorbed {
            backend_vault_id,
            backend_absorb_id,
            request,
            transfer_tuple,
            transfer_block_index,
            observed_transfer_fee,
            proof,
            ..
        } = absorbed
        else {
            unreachable!()
        };
        let refund_nonce = (100u128 << 64) | 14;
        let evidence = SpThreeUsdTerminalEvidence::FailedAfterTransfer {
            backend_vault_id,
            backend_absorb_id,
            request,
            transfer_tuple,
            transfer_block_index,
            observed_transfer_fee,
            proof,
            error: "vault changed before writedown".into(),
            full_refund: rumi_protocol_backend::state::ThreeUsdReserveIngressRefundReceipt {
                block_index: 14,
                tuple: rumi_protocol_backend::state::ThreeUsdRefundTransferTuple {
                    source_owner: backend,
                    source_subaccount: None,
                    destination: icrc_ledger_types::icrc1::account::Account {
                        owner: pool,
                        subaccount: None,
                    },
                    amount_e8s: 600,
                    fee_e8s: 7,
                    memo: rumi_protocol_backend::management::nonce_to_memo(refund_nonce)
                        .0
                        .as_ref()
                        .try_into()
                        .unwrap(),
                    created_at_time_ns: rumi_protocol_backend::management::nonce_to_created_at_time(
                        refund_nonce,
                    ),
                },
            },
        };
        let plan = state
            .plan_sp_three_usd_terminal(absorb_id, evidence)
            .unwrap();
        assert_eq!(plan.principal_consumed, 0);
        assert!(plan.collateral_credits.is_empty());
        assert_eq!(plan.refund_amount_received, 600);
        assert_eq!(plan.total_stable_debit, 3);
        state.apply_sp_three_usd_terminal(absorb_id).unwrap();
        assert_eq!(
            state.deposits[&user_a()].stablecoin_balances[&three_usd_ledger()],
            698
        );
        assert_eq!(
            state.deposits[&user_b()].stablecoin_balances[&three_usd_ledger()],
            299
        );
        assert_eq!(state.total_stablecoin_balances[&three_usd_ledger()], 997);
        assert_eq!(state.total_liquidations_executed, 0);
    }

    #[test]
    fn three_usd_absorb_migration_defaults_and_pending_roundtrip_are_safe() {
        let mut old = StabilityPoolState::default();
        old.next_sp_three_usd_absorb_id = None;
        old.pending_sp_three_usd_absorbs = None;
        old.completed_sp_three_usd_absorbs = None;
        old.completed_sp_three_usd_absorb_floor = None;
        old.initialize_sp_three_usd_absorb_journal();
        assert_eq!(old.next_sp_three_usd_absorb_id, Some(1));
        assert!(old
            .pending_sp_three_usd_absorbs
            .as_ref()
            .unwrap()
            .is_empty());
        let (pending, id, _, _) = prepare_test_three_usd_absorb();
        let encoded = Encode!(&pending).unwrap();
        let decoded = Decode!(&encoded, StabilityPoolState).unwrap();
        assert_eq!(
            decoded.pending_sp_three_usd_absorbs.unwrap()[&id].vault_id,
            77
        );
    }

    #[test]
    fn three_usd_recovery_batch_excludes_ambiguous_approval_and_is_bounded() {
        let (mut state, absorb_id, _, _) = prepare_test_three_usd_absorb();
        let mut second = state.pending_sp_three_usd_absorbs.as_ref().unwrap()[&absorb_id].clone();
        second.absorb_id = absorb_id + 1;
        second.vault_id += 1;
        state
            .pending_sp_three_usd_absorbs
            .as_mut()
            .unwrap()
            .insert(second.absorb_id, second);
        assert!(state.take_sp_three_usd_recovery_batch(2).is_empty());
        state
            .mark_sp_three_usd_approval_dispatch(absorb_id)
            .unwrap();
        // An attempted approval without its exact receipt remains controller-only.
        assert!(state.take_sp_three_usd_recovery_batch(2).is_empty());
        state
            .record_sp_three_usd_approval_receipt(absorb_id, 5, 3)
            .unwrap();
        assert_eq!(state.take_sp_three_usd_recovery_batch(1), vec![absorb_id]);
        state
            .mark_sp_three_usd_approval_dispatch(absorb_id + 1)
            .unwrap();
        state
            .record_sp_three_usd_approval_receipt(absorb_id + 1, 6, 3)
            .unwrap();
        assert_eq!(
            state.take_sp_three_usd_recovery_batch(1),
            vec![absorb_id + 1]
        );
        assert!(state.take_sp_three_usd_recovery_batch(0).is_empty());
    }

    #[test]
    fn three_usd_absorb_admission_snapshots_while_its_vault_is_in_flight() {
        let (mut state, _, pool, _) = prepare_test_three_usd_absorb();
        state.pending_sp_three_usd_absorbs.as_mut().unwrap().clear();
        state.in_flight_liquidations.insert(77);
        let approval = SpThreeUsdApprovalIntent {
            ledger: three_usd_ledger(),
            allowance: 600,
            fee: 3,
            memo: vec![8; 16],
            created_at_time_ns: 101,
            expires_at_ns: 201,
        };

        let row = state
            .prepare_sp_three_usd_absorb(
                77,
                pool,
                three_usd_ledger(),
                icp_ledger(),
                123_000_000,
                101,
                600,
                600,
                1_000_000_000_000_000_000,
                approval,
            )
            .unwrap();
        assert_eq!(row.phase, SpThreeUsdAbsorbPhase::ApprovalPending);
        assert_eq!(row.depositor_snapshot[&user_a()].balance, 700);
        assert!(row.depositor_snapshot[&user_a()].collateral_opted_in);
        assert_eq!(row.depositor_snapshot[&user_b()].balance, 300);
        assert!(!row.depositor_snapshot[&user_b()].collateral_opted_in);
        assert!(!row.approval_dispatch_may_have_happened);
    }

    #[test]
    fn three_usd_absorb_admission_fails_before_dispatch_if_fee_and_principal_overdraw_one_position()
    {
        let (mut state, _, pool, _) = prepare_test_three_usd_absorb();
        state.pending_sp_three_usd_absorbs.as_mut().unwrap().clear();
        state
            .deposits
            .get_mut(&user_a())
            .unwrap()
            .stablecoin_balances
            .insert(three_usd_ledger(), 600);
        state
            .total_stablecoin_balances
            .insert(three_usd_ledger(), 900);
        let approval = SpThreeUsdApprovalIntent {
            ledger: three_usd_ledger(),
            allowance: 600,
            fee: 3,
            memo: vec![9; 16],
            created_at_time_ns: 102,
            expires_at_ns: 202,
        };

        let result = state.prepare_sp_three_usd_absorb(
            77,
            pool,
            three_usd_ledger(),
            icp_ledger(),
            123_000_000,
            102,
            600,
            600,
            1_000_000_000_000_000_000,
            approval,
        );
        assert!(result.is_err());
        assert!(state
            .pending_sp_three_usd_absorbs
            .as_ref()
            .unwrap()
            .is_empty());
        assert_eq!(state.next_sp_three_usd_absorb_id, Some(2));
    }

    #[test]
    fn test_lp_to_usd_e8s_conversion() {
        // 1 3USD at vp=1.0492 → 1.0492 USD
        let vp = 1_049_200_000_000_000_000u128;
        assert_eq!(lp_to_usd_e8s(100_000_000, vp), 104_920_000);

        // 10 3USD
        assert_eq!(lp_to_usd_e8s(1_000_000_000, vp), 1_049_200_000);

        // 0 3USD
        assert_eq!(lp_to_usd_e8s(0, vp), 0);

        // vp=1.0 (exactly 1e18)
        assert_eq!(
            lp_to_usd_e8s(100_000_000, 1_000_000_000_000_000_000),
            100_000_000
        );
    }

    #[test]
    fn test_usd_e8s_to_lp_conversion() {
        let vp = 1_049_200_000_000_000_000u128;
        // 1 USD → ~0.9531 3USD LP
        let lp = usd_e8s_to_lp(100_000_000, vp);
        assert!(
            lp > 95_000_000 && lp < 96_000_000,
            "Expected ~95.3M, got {}",
            lp
        );

        // Round-trip: lp_to_usd then back (may lose 1 unit to rounding)
        let usd = lp_to_usd_e8s(lp, vp);
        assert!(
            (usd as i64 - 100_000_000i64).abs() <= 1,
            "Round-trip drift too large"
        );

        // Zero virtual price → 0
        assert_eq!(usd_e8s_to_lp(100_000_000, 0), 0);
    }

    #[test]
    fn test_total_usd_value_with_lp_token() {
        let state = test_state_with_3usd();
        let vp_map = state.virtual_prices();

        let mut pos = DepositPosition::new(0);
        // 1 icUSD (e8s)
        pos.stablecoin_balances.insert(icusd_ledger(), 100_000_000);
        // 1 3USD LP (worth ~1.0492 USD)
        pos.stablecoin_balances
            .insert(three_usd_ledger(), 100_000_000);

        let total = pos.total_usd_value(&state.stablecoin_registry, vp_map);
        // 100_000_000 + 104_920_000 = 204_920_000
        assert_eq!(total, 204_920_000);
    }

    #[test]
    fn test_total_usd_value_lp_without_virtual_price() {
        let mut state = test_state_with_3usd();
        // Remove cached virtual price
        state.cached_virtual_prices = Some(BTreeMap::new());

        let mut pos = DepositPosition::new(0);
        pos.stablecoin_balances
            .insert(three_usd_ledger(), 100_000_000);

        // Without virtual price, LP tokens valued at 0
        let total = pos.total_usd_value(&state.stablecoin_registry, state.virtual_prices());
        assert_eq!(total, 0);
    }

    #[test]
    fn test_compute_token_draw_with_3usd() {
        let mut state = test_state_with_3usd();

        // Add deposits: 1 icUSD (priority 1), 2 ckUSDT (priority 2), 5 3USD (priority 0)
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_000_000); // 1 icUSD
        add_deposit_direct(&mut state, user_a(), ckusdt_ledger(), 2_000_000); // 2 ckUSDT
        add_deposit_direct(&mut state, user_a(), three_usd_ledger(), 500_000_000); // 5 3USD

        // Draw 3 USD — should consume ckUSDT first (priority 2), then icUSD (priority 1)
        let draw = state.compute_token_draw(300_000_000, &icp_ledger()); // 3 USD e8s
        assert!(
            draw.contains_key(&ckusdt_ledger()),
            "Should draw from ckUSDT (priority 2)"
        );
        assert!(
            draw.contains_key(&icusd_ledger()),
            "Should draw from icUSD (priority 1)"
        );
        assert!(
            !draw.contains_key(&three_usd_ledger()),
            "Should NOT draw from 3USD yet (priority 0)"
        );

        // Draw 5 USD — should consume all ckUSDT + icUSD, then dip into 3USD
        let draw = state.compute_token_draw(500_000_000, &icp_ledger()); // 5 USD e8s
        assert!(
            draw.contains_key(&ckusdt_ledger()),
            "Should draw from ckUSDT"
        );
        assert!(draw.contains_key(&icusd_ledger()), "Should draw from icUSD");
        assert!(
            draw.contains_key(&three_usd_ledger()),
            "Should draw from 3USD for remainder"
        );
    }

    #[test]
    fn test_effective_pool_includes_3usd_at_virtual_price() {
        let mut state = test_state_with_3usd();

        // Only 3USD deposit: 10 LP tokens at vp=1.0492
        add_deposit_direct(&mut state, user_a(), three_usd_ledger(), 1_000_000_000); // 10 3USD

        let effective = state.effective_pool_for_collateral(&icp_ledger());
        // 10 * 1.0492 = 10.492 USD = 1_049_200_000 e8s
        assert_eq!(effective, 1_049_200_000);
    }

    // ─── Test: Pending Refunds (audit IC-S-001) ───

    #[test]
    fn ic_s_001_pending_refund_bookkeeping() {
        let mut state = test_state();

        // Record two refunds for user_a and one for user_b.
        let id0 = state.record_pending_refund(
            user_a(),
            icusd_ledger(),
            5_00000000,
            "approve failed".to_string(),
            100,
        );
        let id1 = state.record_pending_refund(
            user_a(),
            ckusdt_ledger(),
            7_000_000,
            "add_liquidity failed".to_string(),
            200,
        );
        let id2 = state.record_pending_refund(
            user_b(),
            icusd_ledger(),
            1_00000000,
            "approve failed".to_string(),
            300,
        );
        assert_eq!((id0, id1, id2), (0, 1, 2), "refund ids must be monotonic");

        // Per-user listing only returns the owner's records.
        let a_refunds = state.pending_refunds_for(&user_a());
        assert_eq!(a_refunds.len(), 2);
        assert!(a_refunds.iter().all(|r| r.user == user_a()));
        assert_eq!(state.pending_refunds_for(&user_b()).len(), 1);
        assert!(state.pending_refunds_for(&user_c()).is_empty());

        // The obligation remains present while a stable exact-tuple transfer
        // is in flight; only a verified receipt may remove it.
        assert_eq!(state.pending_refunds_for(&user_a()).len(), 2);

        // ids keep growing across retained obligations.
        let id3 = state.record_pending_refund(user_c(), icusd_ledger(), 1, "x".to_string(), 400);
        assert_eq!(id3, 3);
    }

    #[test]
    fn cl10_forward_receipt_index_rebuilds_for_old_batches_and_caps_new_history() {
        let mut state = test_state();
        let payload = InterestMintReceiptPayload {
            token_ledger: icusd_ledger(),
            amount: 50,
            collateral_type: None,
        };
        let batch = state
            .queue_interest_forward_with_receipt_at(44, payload.clone(), 1)
            .expect("queue receipt");
        state.unallocated_interest_mint_index = None;
        assert_eq!(
            state.interest_mint_receipt_status(44, &payload),
            InterestMintReceiptStatus::OutsideReplayWindow,
            "missing index fails closed before migration",
        );
        state.initialize_unallocated_interest_mint_index();
        assert_eq!(
            state.interest_mint_receipt_status(44, &payload),
            InterestMintReceiptStatus::PendingForward(batch),
            "upgrade migration restores O(log n) lookup",
        );
        let mut altered = payload.clone();
        altered.amount += 1;
        assert_eq!(
            state.interest_mint_receipt_status(44, &altered),
            InterestMintReceiptStatus::PayloadMismatch,
            "pending treasury receipts remain bound to the original payload",
        );

        let mut full = test_state();
        let index = full.unallocated_interest_mint_index.as_mut().unwrap();
        index.extend((0..MAX_UNALLOCATED_INTEREST_MINT_RECEIPTS as u64).map(|block| (block, 0)));
        assert!(
            matches!(
                full.queue_unallocated_interest_forward_at(20_000, icusd_ledger(), 1, 2),
                Err(StabilityPoolError::SystemBusy),
            ),
            "bounded receipt history holds new backend notifications at capacity"
        );
    }

    #[test]
    fn pending_refund_too_old_scan_is_persistent_and_only_complete_scan_rotates_identity() {
        let mut state = test_state();
        state.normalize_pending_refund_fee_state();
        state
            .credit_pending_refund_fee_reserve(icusd_ledger(), 90, 20)
            .unwrap();
        let id = state.record_pending_refund(user_a(), icusd_ledger(), 1_000, "failed".into(), 5);
        let first = state
            .prepare_pending_refund_transfer(id, 10, 100, b"refund-0".to_vec())
            .unwrap();
        state.mark_pending_refund_too_old(id).unwrap();
        assert_eq!(
            state.rotate_pending_refund_after_no_effect(id),
            Err("fresh refund identity requires a complete no-effect history proof"),
            "TooOld alone must never rotate an identity",
        );

        state.start_pending_refund_history_scan(id, 130).unwrap();
        state
            .advance_pending_refund_history_scan(id, 0, 130, 64)
            .unwrap();
        let bytes = Encode!(&state).expect("encode partially scanned stable state");
        let mut restored = Decode!(&bytes, StabilityPoolState).expect("restore scan after upgrade");
        assert_eq!(
            restored.pending_refunds.as_ref().unwrap()[&id].transfer_history_scan_cursor,
            Some(64),
        );
        assert_eq!(
            restored.rotate_pending_refund_after_no_effect(id),
            Err("fresh refund identity requires a complete no-effect history proof"),
            "partial history must remain held across upgrade",
        );
        restored
            .advance_pending_refund_history_scan(id, 64, 130, 130)
            .unwrap();
        let rotated = restored
            .rotate_pending_refund_after_no_effect(id)
            .expect("complete no-effect proof may release the old reserved fee");
        assert_eq!(rotated.transfer_attempted, Some(false));
        assert_eq!(rotated.transfer_attempt_no, Some(1));
        assert_eq!(rotated.protocol_fee_reserved, None);
        assert_eq!(rotated.transfer_memo, None);
        assert_eq!(
            restored.pending_refund_fee_reserves.as_ref().unwrap()[&icusd_ledger()],
            20
        );

        let second = restored
            .prepare_pending_refund_transfer(id, 10, 200, b"refund-1".to_vec())
            .unwrap();
        assert_ne!(first.transfer_memo, second.transfer_memo);
        assert_eq!(second.transfer_attempt_no, Some(1));
        assert_eq!(second.transfer_created_at_time_ns, Some(200));
    }

    #[test]
    fn pending_refund_fee_capacity_is_receipt_bound_and_tuple_is_stable() {
        let mut state = test_state();
        let ledger = icusd_ledger();
        let refund_id =
            state.record_pending_refund(user_a(), ledger, 10_000_000, "refund failed".into(), 100);
        assert_eq!(
            state.credit_pending_refund_fee_reserve(ledger, 55, 20),
            Ok(())
        );
        assert_eq!(
            state.credit_pending_refund_fee_reserve(ledger, 55, 20),
            Err("refund fee funding block was already credited"),
        );
        let memo = b"rumi-sp-refund-v1:test".to_vec();
        let first = state
            .prepare_pending_refund_transfer(refund_id, 20, 123, memo.clone())
            .expect("receipt-backed reserve funds the fee");
        let retry = state
            .prepare_pending_refund_transfer(refund_id, 20, 999, b"different".to_vec())
            .expect("retry reuses the existing transfer journal");
        assert_eq!(retry, first);
        assert_eq!(first.amount, 10_000_000, "refund principal is not netted");
        assert_eq!(first.transfer_fee, Some(20));
        assert_eq!(first.transfer_created_at_time_ns, Some(123));
        assert_eq!(first.transfer_memo, Some(memo.clone()));
        assert_eq!(
            state
                .pending_refund_fee_reserves
                .as_ref()
                .unwrap()
                .get(&ledger),
            Some(&0),
            "fee capacity is reserved once before dispatch",
        );

        state
            .credit_pending_refund_fee_reserve(ledger, 56, 15)
            .expect("top-up for the corrected fee is receipt-bound");
        let repriced = state
            .refresh_pending_refund_fee_after_bad_fee(refund_id, 25)
            .expect("BadFee proves the old fee tuple had no effect");
        assert_eq!(repriced.transfer_fee, Some(25));
        assert_eq!(repriced.transfer_created_at_time_ns, Some(123));
        assert_eq!(repriced.transfer_memo, Some(memo));
        assert_eq!(repriced.amount, 10_000_000, "principal remains full");
        assert_eq!(
            state
                .pending_refund_fee_reserves
                .as_ref()
                .unwrap()
                .get(&ledger),
            Some(&10),
            "only corrected fee capacity is reserved",
        );

        let legacy_id =
            state.record_pending_refund(user_b(), ledger, 10_000_000, "legacy".into(), 200);
        let legacy = state
            .pending_refunds
            .as_mut()
            .unwrap()
            .get_mut(&legacy_id)
            .unwrap();
        legacy.transfer_attempted = None;
        assert_eq!(
            state.prepare_pending_refund_transfer(legacy_id, 1, 1, vec![]),
            Err("legacy refund has unknown transfer history and remains held"),
        );
    }

    #[test]
    fn pending_refund_queue_retains_every_unpaid_obligation_past_old_cap() {
        const OLD_CAP: usize = 10_000;
        let mut state = test_state();
        for i in 0..OLD_CAP {
            state.record_pending_refund(
                user_a(),
                icusd_ledger(),
                i as u64 + 1,
                "fail".to_string(),
                i as u64,
            );
        }
        assert_eq!(state.pending_refunds_for(&user_a()).len(), OLD_CAP);

        let id =
            state.record_pending_refund(user_a(), icusd_ledger(), 999, "fail".to_string(), 999);
        assert_eq!(id as usize, OLD_CAP);
        assert_eq!(
            state.pending_refunds_for(&user_a()).len(),
            OLD_CAP + 1,
            "no unpaid obligation may be evicted",
        );
        assert!(state.pending_refunds.as_ref().unwrap().get(&0).is_some());
    }

    #[test]
    fn ic_s_001_state_v1_snapshot_decodes_with_empty_pending_refunds() {
        // Pre-IC-S-001 snapshot bytes (no pending_refunds / next_pending_refund_id)
        // must decode without losing positions; pending refunds start empty.
        let mut current = test_state();
        add_deposit_direct(&mut current, user_a(), icusd_ledger(), 42_00000000);
        let v1 = StabilityPoolStateV1 {
            deposits: current.deposits.clone(),
            total_stablecoin_balances: current.total_stablecoin_balances.clone(),
            stablecoin_registry: current.stablecoin_registry.clone(),
            collateral_registry: current.collateral_registry.clone(),
            protocol_canister_id: current.protocol_canister_id,
            configuration: current.configuration.clone(),
            liquidation_history: current.liquidation_history.clone(),
            in_flight_liquidations: current.in_flight_liquidations.clone(),
            total_liquidations_executed: current.total_liquidations_executed,
            pool_creation_timestamp: current.pool_creation_timestamp,
            total_interest_received_e8s: current.total_interest_received_e8s,
            token_consecutive_failures: current.token_consecutive_failures.clone(),
            cached_virtual_prices: current.cached_virtual_prices.clone(),
            protocol_reserve_address: current.protocol_reserve_address,
            is_initialized: current.is_initialized,
            pool_events: current.pool_events.clone(),
            next_event_id: current.next_event_id,
        };
        let bytes = Encode!(&v1).expect("encode v1 snapshot");

        let mut decoded = try_decode_state(&bytes).expect("v1 snapshot must decode");
        decoded.normalize_pending_refund_fee_state();
        assert_eq!(
            decoded
                .deposits
                .get(&user_a())
                .and_then(|p| p.stablecoin_balances.get(&icusd_ledger()).copied()),
            Some(42_00000000),
            "depositor positions must survive the v1 fallback",
        );
        assert!(
            decoded
                .pending_refunds
                .clone()
                .unwrap_or_default()
                .is_empty(),
            "pending refunds must start empty after a v1 upgrade",
        );
        assert_eq!(decoded.next_pending_refund_id.unwrap_or(0), 0);
        assert!(decoded
            .unallocated_interest_mint_index
            .unwrap_or_default()
            .is_empty());
        assert!(decoded.last_deposit_transfer_created_at.is_none());
        assert!(decoded
            .pending_deposit_intents
            .clone()
            .unwrap_or_default()
            .is_empty());
    }

    /// Exact StabilityPoolState Candid record at the candidate's parent
    /// e1e15800, before cd8d8428 added the deposit intent fields.
    #[derive(CandidType)]
    struct StabilityPoolStatePreCandidate {
        deposits: BTreeMap<Principal, DepositPosition>,
        total_stablecoin_balances: BTreeMap<Principal, u64>,
        stablecoin_registry: BTreeMap<Principal, StablecoinConfig>,
        collateral_registry: BTreeMap<Principal, CollateralInfo>,
        chain_collateral_sentinels: Option<BTreeSet<Principal>>,
        chain_claim_sources: Option<BTreeMap<Principal, Vec<ChainClaimSource>>>,
        pending_chain_absorbs: Option<BTreeMap<u64, ChainSpAbsorbIntent>>,
        completed_chain_absorbs: Option<BTreeMap<u64, ChainSpAbsorbCompletion>>,
        pending_native_xrp_absorbs: Option<BTreeMap<u64, NativeXrpAbsorbIntent>>,
        chain_absorb_auto_config: Option<ChainAbsorbAutoConfig>,
        chain_absorb_auto_last_tick: Option<ChainAbsorbAutoTickRecord>,
        completed_cfx_claim_payout_recoveries:
            Option<BTreeMap<CfxClaimPayoutRecoveryKey, CfxClaimPayoutRecoveryRecord>>,
        completed_cfx_claim_payout_recovery_floor: Option<BTreeMap<Principal, u64>>,
        protocol_canister_id: Principal,
        configuration: PoolConfiguration,
        liquidation_history: Vec<PoolLiquidationRecord>,
        in_flight_liquidations: BTreeSet<u64>,
        total_liquidations_executed: u64,
        pool_creation_timestamp: u64,
        total_interest_received_e8s: Option<u64>,
        token_consecutive_failures: Option<BTreeMap<Principal, u32>>,
        cached_virtual_prices: Option<BTreeMap<Principal, u128>>,
        protocol_reserve_address: Option<Principal>,
        interest_treasury: Option<Principal>,
        unallocated_interest_forward_batches:
            Option<BTreeMap<u64, UnallocatedInterestForwardBatch>>,
        next_unallocated_interest_forward_batch_id: Option<u64>,
        is_initialized: bool,
        pool_events: Option<Vec<PoolEvent>>,
        next_event_id: Option<u64>,
        pending_refunds: Option<BTreeMap<u64, PendingRefund>>,
        next_pending_refund_id: Option<u64>,
    }

    #[test]
    fn pre_candidate_current_schema_snapshot_decodes_without_losing_state() {
        let mut current = test_state();
        add_deposit_direct(&mut current, user_a(), icusd_ledger(), 42_00000000);
        let pre_candidate = StabilityPoolStatePreCandidate {
            deposits: current.deposits.clone(),
            total_stablecoin_balances: current.total_stablecoin_balances.clone(),
            stablecoin_registry: current.stablecoin_registry.clone(),
            collateral_registry: current.collateral_registry.clone(),
            chain_collateral_sentinels: current.chain_collateral_sentinels.clone(),
            chain_claim_sources: current.chain_claim_sources.clone(),
            pending_chain_absorbs: current.pending_chain_absorbs.clone(),
            completed_chain_absorbs: current.completed_chain_absorbs.clone(),
            pending_native_xrp_absorbs: current.pending_native_xrp_absorbs.clone(),
            chain_absorb_auto_config: current.chain_absorb_auto_config.clone(),
            chain_absorb_auto_last_tick: current.chain_absorb_auto_last_tick.clone(),
            completed_cfx_claim_payout_recoveries: current
                .completed_cfx_claim_payout_recoveries
                .clone(),
            completed_cfx_claim_payout_recovery_floor: current
                .completed_cfx_claim_payout_recovery_floor
                .clone(),
            protocol_canister_id: current.protocol_canister_id,
            configuration: current.configuration.clone(),
            liquidation_history: current.liquidation_history.clone(),
            in_flight_liquidations: current.in_flight_liquidations.clone(),
            total_liquidations_executed: current.total_liquidations_executed,
            pool_creation_timestamp: current.pool_creation_timestamp,
            total_interest_received_e8s: current.total_interest_received_e8s,
            token_consecutive_failures: current.token_consecutive_failures.clone(),
            cached_virtual_prices: current.cached_virtual_prices.clone(),
            protocol_reserve_address: current.protocol_reserve_address,
            interest_treasury: current.interest_treasury,
            unallocated_interest_forward_batches: current
                .unallocated_interest_forward_batches
                .clone(),
            next_unallocated_interest_forward_batch_id: current
                .next_unallocated_interest_forward_batch_id,
            is_initialized: current.is_initialized,
            pool_events: current.pool_events.clone(),
            next_event_id: current.next_event_id,
            pending_refunds: current.pending_refunds.clone(),
            next_pending_refund_id: current.next_pending_refund_id,
        };
        let bytes = Encode!(&pre_candidate).expect("encode pre-candidate snapshot");

        let decoded_current = Decode!(&bytes, StabilityPoolState)
            .expect("exact origin/main schema must decode as current state");
        let decoded = try_decode_state(&bytes).expect("pre-candidate snapshot must decode");
        for decoded in [decoded_current, decoded] {
            assert_eq!(
                decoded
                    .deposits
                    .get(&user_a())
                    .and_then(|p| p.stablecoin_balances.get(&icusd_ledger()).copied()),
                Some(42_00000000),
                "pre-candidate depositor position must survive",
            );
            assert!(decoded.last_deposit_transfer_created_at.is_none());
            assert!(decoded
                .pending_deposit_intents
                .as_ref()
                .is_none_or(BTreeMap::is_empty));
            let mut migrated = decoded;
            migrated.initialize_sp_liquidation_v2_journal();
            assert_eq!(migrated.next_sp_liquidation_request_id, Some(1));
            assert_eq!(migrated.completed_sp_liquidation_request_floor, Some(1));
            assert!(migrated.pending_sp_liquidations_v2.unwrap().is_empty());
        }
    }

    #[test]
    fn v2_request_snapshot_roundtrip_preserves_pending_identity_and_fences() {
        let mut state = test_state();
        let request_id = prepare_v2_request(&mut state, 77);
        let bytes = Encode!(&state).expect("encode current state");
        let mut restored = Decode!(&bytes, StabilityPoolState).expect("decode current state");
        let row = restored
            .pending_sp_liquidation_v2(request_id)
            .expect("pending row survives");
        assert_eq!(row.request.request_id, request_id);
        assert_eq!(row.request.vault_id, 77);
        assert_eq!(row.approval.memo, vec![7]);
        assert_eq!(row.approval.created_at_time_ns, 11);
        assert!(row.backend_request.is_none());
        assert_eq!(
            restored.next_sp_liquidation_request_id,
            Some(request_id + 1)
        );
        assert_eq!(
            restored.take_sp_liquidation_v2_recovery_batch(1),
            vec![request_id]
        );
    }

    #[test]
    fn legacy_fallback_guard_recognizes_new_journal_on_option_wire_mismatch() {
        #[derive(CandidType)]
        struct NewerSnapshotWithIncompatibleChainAbsorbJournal {
            deposits: BTreeMap<Principal, DepositPosition>,
            total_stablecoin_balances: BTreeMap<Principal, u64>,
            stablecoin_registry: BTreeMap<Principal, StablecoinConfig>,
            collateral_registry: BTreeMap<Principal, CollateralInfo>,
            protocol_canister_id: Principal,
            configuration: PoolConfiguration,
            liquidation_history: Vec<PoolLiquidationRecord>,
            in_flight_liquidations: BTreeSet<u64>,
            total_liquidations_executed: u64,
            pool_creation_timestamp: u64,
            total_interest_received_e8s: Option<u64>,
            token_consecutive_failures: Option<BTreeMap<Principal, u32>>,
            cached_virtual_prices: Option<BTreeMap<Principal, u128>>,
            protocol_reserve_address: Option<Principal>,
            is_initialized: bool,
            pool_events: Option<Vec<PoolEvent>>,
            next_event_id: Option<u64>,
            pending_chain_absorbs: String,
        }

        let current = test_state();
        let legacy = StabilityPoolStateV1 {
            deposits: current.deposits,
            total_stablecoin_balances: current.total_stablecoin_balances,
            stablecoin_registry: current.stablecoin_registry,
            collateral_registry: current.collateral_registry,
            protocol_canister_id: current.protocol_canister_id,
            configuration: current.configuration,
            liquidation_history: current.liquidation_history,
            in_flight_liquidations: current.in_flight_liquidations,
            total_liquidations_executed: current.total_liquidations_executed,
            pool_creation_timestamp: current.pool_creation_timestamp,
            total_interest_received_e8s: current.total_interest_received_e8s,
            token_consecutive_failures: current.token_consecutive_failures,
            cached_virtual_prices: current.cached_virtual_prices,
            protocol_reserve_address: current.protocol_reserve_address,
            is_initialized: current.is_initialized,
            pool_events: current.pool_events,
            next_event_id: current.next_event_id,
        };
        let newer = NewerSnapshotWithIncompatibleChainAbsorbJournal {
            deposits: legacy.deposits,
            total_stablecoin_balances: legacy.total_stablecoin_balances,
            stablecoin_registry: legacy.stablecoin_registry,
            collateral_registry: legacy.collateral_registry,
            protocol_canister_id: legacy.protocol_canister_id,
            configuration: legacy.configuration,
            liquidation_history: legacy.liquidation_history,
            in_flight_liquidations: legacy.in_flight_liquidations,
            total_liquidations_executed: legacy.total_liquidations_executed,
            pool_creation_timestamp: legacy.pool_creation_timestamp,
            total_interest_received_e8s: legacy.total_interest_received_e8s,
            token_consecutive_failures: legacy.token_consecutive_failures,
            cached_virtual_prices: legacy.cached_virtual_prices,
            protocol_reserve_address: legacy.protocol_reserve_address,
            is_initialized: legacy.is_initialized,
            pool_events: legacy.pool_events,
            next_event_id: legacy.next_event_id,
            pending_chain_absorbs: "incompatible journal shape".into(),
        };
        let incompatible = Encode!(&newer).expect("encode newer incompatible snapshot");

        // Candid's `opt` accepts a value of an unrelated wire type as None,
        // so this exact malformed nested journal shape is consumed by the
        // current decoder as None. The wire/state consistency check must
        // reject that lossy current decode before it can be accepted.
        assert!(Decode!(&incompatible, StabilityPoolState).is_ok());
        assert!(Decode!(&incompatible, StabilityPoolStateV1).is_ok());
        assert!(!current_financial_journals_match_wire(
            &incompatible,
            &Decode!(&incompatible, StabilityPoolState).unwrap()
        ));
        assert!(try_decode_state(&incompatible).is_none());
    }

    #[test]
    fn v2_acknowledgement_advances_contiguous_floor_and_is_idempotent() {
        let mut state = test_state();
        let first = prepare_v2_request(&mut state, 77);
        assert_eq!(first, 1);
        state.complete_sp_liquidation_v2(first).unwrap();
        state.acknowledge_sp_liquidation_v2(first).unwrap();
        state.acknowledge_sp_liquidation_v2(first).unwrap();
        assert_eq!(state.completed_sp_liquidation_request_floor, Some(2));
        assert!(state.sp_liquidation_v2_row(first).is_none());
        assert!(state.sp_liquidation_v2_id_is_stale(first));
        assert_eq!(
            state.take_sp_liquidation_v2_recovery_batch(4),
            Vec::<u64>::new()
        );
        let second = prepare_v2_request(&mut state, 78);
        assert_eq!(second, first + 1);
        state.complete_sp_liquidation_v2(second).unwrap();
        state.acknowledge_sp_liquidation_v2(second).unwrap();
        assert_eq!(state.completed_sp_liquidation_request_floor, Some(3));
    }

    #[test]
    fn v2_pending_and_completed_unacked_rows_block_new_ids_but_preserve_exact_retry() {
        let mut state = test_state();
        let first = prepare_v2_request(&mut state, 77);
        let saved = state.pending_sp_liquidation_v2_for_vault(77).unwrap();
        assert_eq!(saved.request.request_id, first);
        assert!(state.pending_sp_liquidation_v2_for_vault(78).is_none());
        assert!(state
            .prepare_sp_liquidation_v2(
                78,
                icp_ledger(),
                100_000_000,
                icusd_ledger(),
                1_000_000,
                SpLiquidationToken::IcUsd,
                saved.approval.clone(),
            )
            .is_err());
        assert_eq!(state.next_sp_liquidation_request_id, Some(first + 1));
        state.complete_sp_liquidation_v2(first).unwrap();
        assert!(state
            .prepare_sp_liquidation_v2(
                78,
                icp_ledger(),
                100_000_000,
                icusd_ledger(),
                1_000_000,
                SpLiquidationToken::IcUsd,
                saved.approval,
            )
            .is_err());
        assert_eq!(state.next_sp_liquidation_request_id, Some(first + 1));
        state.acknowledge_sp_liquidation_v2(first).unwrap();
        let next = prepare_v2_request(&mut state, 78);
        assert_eq!(next, first + 1);
    }

    #[test]
    fn v2_admission_is_blocked_by_legacy_in_flight_vault_marker() {
        let mut state = test_state();
        state.in_flight_liquidations.insert(77);
        let next_id_before = state.next_sp_liquidation_request_id;
        let approval = SpLiquidationV2ApprovalTuple {
            ledger: icusd_ledger(),
            owner: icrc_ledger_types::icrc1::account::Account {
                owner: Principal::from_slice(&[40]),
                subaccount: None,
            },
            spender: icrc_ledger_types::icrc1::account::Account {
                owner: state.protocol_canister_id,
                subaccount: None,
            },
            allowance_raw: 2_000_000,
            fee_raw: 10_000,
            memo: vec![7],
            created_at_time_ns: 11,
            expires_at_ns: 22,
            fee_accounted: false,
        };
        assert!(state
            .prepare_sp_liquidation_v2(
                77,
                icp_ledger(),
                100_000_000,
                icusd_ledger(),
                1_000_000,
                SpLiquidationToken::IcUsd,
                approval,
            )
            .is_err());
        assert_eq!(state.next_sp_liquidation_request_id, next_id_before);
        assert!(state.pending_sp_liquidation_v2_for_vault(77).is_none());
    }

    #[test]
    fn v2_first_approval_no_effect_reuses_id_and_block_zero_is_valid() {
        let mut state = test_state();
        state.add_deposit_at(user_a(), icusd_ledger(), 2_000_000, 0);
        let request_id = prepare_v2_request(&mut state, 77);
        state
            .mark_sp_liquidation_v2_approval_dispatch(request_id, true, false, None)
            .unwrap();
        state
            .mark_sp_liquidation_v2_approval_no_effect(request_id, "BadFee".into())
            .unwrap();
        let mut repriced = state
            .pending_sp_liquidation_v2(request_id)
            .unwrap()
            .approval;
        repriced.fee_raw = 11;
        repriced.allowance_raw = 1_000_011;
        repriced.created_at_time_ns += 1;
        repriced.expires_at_ns += 1;
        state
            .reprice_sp_liquidation_v2_approval_after_no_effect(request_id, repriced.clone())
            .unwrap();
        assert_eq!(state.next_sp_liquidation_request_id, Some(request_id + 1));
        assert_eq!(
            state
                .pending_sp_liquidation_v2_for_vault(77)
                .unwrap()
                .request
                .request_id,
            request_id
        );
        state
            .account_sp_liquidation_v2_approval_fee(
                request_id,
                SpLiquidationApprovalReceipt {
                    block_index: 0,
                    tuple: SpLiquidationApprovalTuple {
                        ledger: repriced.ledger,
                        owner: repriced.owner.clone(),
                        spender: repriced.spender.clone(),
                        allowance_raw: repriced.allowance_raw,
                        fee_raw: repriced.fee_raw,
                        memo: repriced.memo.clone(),
                        created_at_time_ns: repriced.created_at_time_ns,
                        expires_at_ns: repriced.expires_at_ns,
                    },
                },
            )
            .unwrap();
        let admitted = state
            .pending_sp_liquidation_v2(request_id)
            .unwrap()
            .backend_request
            .unwrap();
        assert_eq!(admitted.request_id, 1);
        assert_eq!(admitted.approval.block_index, 0);

        let mut ambiguous = test_state();
        let first = prepare_v2_request(&mut ambiguous, 88);
        ambiguous
            .mark_sp_liquidation_v2_approval_dispatch(first, true, false, None)
            .unwrap();
        ambiguous
            .mark_sp_liquidation_v2_approval_dispatch(first, false, true, None)
            .unwrap();
        let before_candidate = ambiguous.pending_sp_liquidation_v2(first).unwrap();
        assert!(ambiguous
            .mark_sp_liquidation_v2_approval_dispatch(first, false, true, Some(999))
            .is_err());
        assert_eq!(
            ambiguous.pending_sp_liquidation_v2(first),
            Some(before_candidate)
        );
        let saved = ambiguous.pending_sp_liquidation_v2(first).unwrap().approval;
        assert!(ambiguous
            .reprice_sp_liquidation_v2_approval_after_no_effect(first, saved)
            .is_err());
        assert!(ambiguous
            .prepare_sp_liquidation_v2(
                89,
                icp_ledger(),
                100_000_000,
                icusd_ledger(),
                1_000_000,
                SpLiquidationToken::IcUsd,
                ambiguous
                    .pending_sp_liquidation_v2(first)
                    .unwrap()
                    .approval
                    .clone(),
            )
            .is_err());
    }

    #[test]
    fn v2_stable_pull_reconciliation_candidate_is_persistent_and_exactly_bound() {
        let mut state = test_state();
        let request_id = prepare_v2_request(&mut state, 77);
        let row = state.pending_sp_liquidation_v2(request_id).unwrap();
        let tuple = SpLiquidationStablePullTuple {
            op_nonce: 1,
            ledger: row.stablecoin_ledger,
            from: row.approval.owner.clone(),
            spender: row.approval.spender.clone(),
            to: row.approval.spender.clone(),
            amount_raw: 900_000,
            fee_raw: 0,
            memo: vec![1, 2, 3],
            created_at_time_ns: 99,
        };

        state
            .record_sp_liquidation_v2_stable_pull_candidate(request_id, tuple.clone(), Some(42))
            .unwrap();
        state
            .record_sp_liquidation_v2_stable_pull_candidate(request_id, tuple.clone(), Some(42))
            .expect("lost backend replies permit retrying the same receipt");
        let saved = state.pending_sp_liquidation_v2(request_id).unwrap();
        assert_eq!(saved.stable_pull_candidate_block_index, Some(42));
        assert_eq!(saved.stable_pull_tuple, Some(tuple.clone()));
        assert!(
            state
                .record_sp_liquidation_v2_stable_pull_candidate(request_id, tuple, Some(43),)
                .is_err(),
            "a different candidate cannot replace a persisted one"
        );
        assert_eq!(
            state
                .pending_sp_liquidation_v2(request_id)
                .unwrap()
                .stable_pull_candidate_block_index,
            Some(42),
        );
    }

    #[test]
    fn v2_finalize_preflights_completed_capacity_before_crediting_gains() {
        let mut state = test_state();
        let request_id = prepare_v2_request(&mut state, 77);
        let mut row = state.pending_sp_liquidation_v2(request_id).unwrap();
        row.stable_debit_applied = true;
        row.stable_pull_receipt = Some(SpLiquidationStablePullReceipt {
            block_index: 1,
            tuple: SpLiquidationStablePullTuple {
                op_nonce: 1,
                ledger: icusd_ledger(),
                from: icrc_ledger_types::icrc1::account::Account {
                    owner: Principal::from_slice(&[40]),
                    subaccount: None,
                },
                spender: icrc_ledger_types::icrc1::account::Account {
                    owner: state.protocol_canister_id,
                    subaccount: None,
                },
                to: icrc_ledger_types::icrc1::account::Account {
                    owner: state.protocol_canister_id,
                    subaccount: None,
                },
                amount_raw: 1,
                fee_raw: 1,
                memo: vec![],
                created_at_time_ns: 1,
            },
        });
        row.payout_tuple = Some(SpLiquidationPayoutTuple {
            op_nonce: 2,
            ledger: icp_ledger(),
            source: icrc_ledger_types::icrc1::account::Account {
                owner: state.protocol_canister_id,
                subaccount: None,
            },
            destination: icrc_ledger_types::icrc1::account::Account {
                owner: Principal::from_slice(&[40]),
                subaccount: None,
            },
            gross_amount_raw: 2,
            net_amount_raw: 1,
            fee_raw: 1,
            memo: vec![],
            created_at_time_ns: 2,
            collateral_type: icp_ledger(),
        });
        row.result = Some(SpLiquidationV2SuccessWithFee {
            block_index: 2,
            fee_amount_paid: 1,
            collateral_amount_received: Some(2),
            debt_liquidated_e8s: Some(1),
            stable_pulled_e6s: Some(1),
            xrp_claim_id: None,
        });
        row.pending_collateral_allocations.insert(user_a(), 1);
        row.phase = SpLiquidationV2LocalPhase::StableDebited;
        state.update_pending_sp_liquidation_v2(row.clone()).unwrap();
        let completed = state.completed_sp_liquidations_v2.as_mut().unwrap();
        for id in 100..(100 + MAX_COMPLETED_SP_LIQUIDATIONS_V2 as u64) {
            let mut tombstone = row.clone();
            tombstone.request.request_id = id;
            tombstone.phase = SpLiquidationV2LocalPhase::Complete;
            tombstone.backend_acknowledged = true;
            completed.insert(id, tombstone);
        }

        assert!(state
            .finalize_sp_liquidation_v2_payout(request_id, 3)
            .is_err());
        assert_eq!(
            state
                .deposits
                .get(&user_a())
                .and_then(|p| p.collateral_gains.get(&icp_ledger())),
            None
        );
        assert!(state.liquidation_history.is_empty());
        assert!(state.pending_sp_liquidation_v2(request_id).is_some());
    }

    #[test]
    fn v2_protocol_refund_restores_exact_three_owner_debits_once() {
        let ledger = icusd_ledger();
        let mut state = test_state();
        let balances = [
            (user_a(), 1_000_000),
            (user_b(), 1_000_003),
            (user_c(), 1_000_007),
        ];
        for (owner, amount) in balances {
            state.add_deposit_at(owner, ledger, amount, 0);
        }
        let initial_total: u64 = balances.iter().map(|(_, amount)| *amount).sum();
        let request_id = prepare_v2_request(&mut state, 99);
        let row = state.pending_sp_liquidation_v2(request_id).unwrap();
        let approval = SpLiquidationApprovalReceipt {
            block_index: 44,
            tuple: SpLiquidationApprovalTuple {
                ledger,
                owner: row.approval.owner.clone(),
                spender: row.approval.spender.clone(),
                allowance_raw: row.approval.allowance_raw,
                fee_raw: row.approval.fee_raw,
                memo: row.approval.memo.clone(),
                created_at_time_ns: row.approval.created_at_time_ns,
                expires_at_ns: row.approval.expires_at_ns,
            },
        };
        state
            .account_sp_liquidation_v2_approval_fee(request_id, approval)
            .unwrap();
        let account = |owner| icrc_ledger_types::icrc1::account::Account {
            owner,
            subaccount: None,
        };
        let pool = Principal::from_slice(&[40]);
        let protocol = state.protocol_canister_id;
        let stable_pull_receipt = SpLiquidationStablePullReceipt {
            block_index: 45,
            tuple: SpLiquidationStablePullTuple {
                op_nonce: 5,
                ledger,
                from: account(pool),
                spender: account(protocol),
                to: account(protocol),
                amount_raw: 5,
                fee_raw: 0,
                memo: vec![5],
                created_at_time_ns: 55,
            },
        };
        state
            .apply_sp_liquidation_v2_refundable_stable_pull(request_id, stable_pull_receipt.clone())
            .unwrap();
        assert_eq!(
            state.total_stablecoin_balances.get(&ledger),
            Some(&(initial_total - 10_005))
        );
        let refund_tuple = SpLiquidationStableRefundTuple {
            op_nonce: 6,
            ledger,
            source: account(protocol),
            destination: account(pool),
            principal_refund_raw: 5,
            approval_fee_refund_raw: 10_000,
            pull_fee_refund_raw: 0,
            amount_raw: 10_005,
            fee_raw: 0,
            memo: vec![6],
            created_at_time_ns: 66,
        };
        state
            .record_sp_liquidation_v2_refund_tuple(request_id, refund_tuple.clone(), Some(46))
            .unwrap();
        let refund_receipt = SpLiquidationStableRefundReceipt {
            block_index: 46,
            tuple: refund_tuple,
        };
        state
            .apply_sp_liquidation_v2_refund_receipt(request_id, refund_receipt.clone())
            .unwrap();
        state
            .apply_sp_liquidation_v2_refund_receipt(request_id, refund_receipt)
            .unwrap();
        let remaining: u64 = state
            .deposits
            .values()
            .map(|position| {
                position
                    .stablecoin_balances
                    .get(&ledger)
                    .copied()
                    .unwrap_or(0)
            })
            .sum();
        assert_eq!(remaining, initial_total);
        for (owner, original_balance) in balances {
            assert_eq!(
                state.deposits.get(&owner)
                    .and_then(|position| position.stablecoin_balances.get(&ledger).copied()),
                Some(original_balance),
                "refund must restore the exact approval-fee and principal shares for each depositor",
            );
        }
        assert_eq!(
            state.total_stablecoin_balances.get(&ledger),
            Some(&initial_total)
        );
        let completed = state
            .completed_sp_liquidations_v2
            .as_ref()
            .unwrap()
            .get(&request_id)
            .unwrap();
        assert_eq!(completed.phase, SpLiquidationV2LocalPhase::Rejected);
        assert!(completed.stable_refund_applied);
        assert_eq!(completed.stable_pull_receipt, Some(stable_pull_receipt));
    }

    #[test]
    fn v2_proportional_fee_and_principal_rounding_debits_exact_raw_totals() {
        let ledger = icusd_ledger();
        let balances = vec![(user_a(), 1), (user_b(), 2), (user_c(), 3)];
        let debits = exact_proportional_debit_allocations(&balances, 5).unwrap();
        assert_eq!(debits.values().sum::<u64>(), 5);
        assert_eq!(debits.get(&user_a()), Some(&1));
        assert_eq!(debits.get(&user_b()), Some(&2));
        assert_eq!(debits.get(&user_c()), Some(&2));

        let collateral = exact_weight_allocations(&balances, 5).unwrap();
        assert_eq!(collateral.values().sum::<u64>(), 5);
        assert_eq!(collateral.get(&user_a()), Some(&1));
        assert_eq!(collateral.get(&user_b()), Some(&2));
        assert_eq!(collateral.get(&user_c()), Some(&2));

        let mut state = test_state();
        for (owner, amount) in balances {
            state.add_deposit_at(owner, ledger, amount, 0);
        }
        state.deduct_exact_pool_fee(ledger, 5).unwrap();
        assert_eq!(state.total_stablecoin_balances.get(&ledger), Some(&1));
        assert_eq!(
            state
                .deposits
                .get(&user_a())
                .and_then(|p| p.stablecoin_balances.get(&ledger)),
            None
        );
        assert_eq!(
            state
                .deposits
                .get(&user_b())
                .and_then(|p| p.stablecoin_balances.get(&ledger)),
            None
        );
        assert_eq!(
            state
                .deposits
                .get(&user_c())
                .and_then(|p| p.stablecoin_balances.get(&ledger)),
            Some(&1)
        );
    }

    #[test]
    fn v2_receipt_application_debits_exact_fee_principal_and_collateral_when_fee_drifts() {
        let ledger = icusd_ledger();
        let collateral = icp_ledger();
        let protocol = Principal::from_slice(&[41]);
        let pool = Principal::from_slice(&[40]);
        let mut state = test_state();
        state.protocol_canister_id = protocol;
        let amounts = [
            (user_a(), 1_000_001),
            (user_b(), 1_000_003),
            (user_c(), 1_000_007),
        ];
        for (owner, amount) in amounts {
            state.add_deposit_at(owner, ledger, amount, 0);
        }
        let initial_total: u64 = amounts.iter().map(|(_, amount)| *amount).sum();
        let approval = SpLiquidationV2ApprovalTuple {
            ledger,
            owner: icrc_ledger_types::icrc1::account::Account {
                owner: pool,
                subaccount: None,
            },
            spender: icrc_ledger_types::icrc1::account::Account {
                owner: protocol,
                subaccount: None,
            },
            allowance_raw: 20,
            fee_raw: 2,
            memo: vec![1],
            created_at_time_ns: 10,
            expires_at_ns: 20,
            fee_accounted: false,
        };
        let row = state
            .prepare_sp_liquidation_v2(
                77,
                collateral,
                100_000_000,
                ledger,
                5,
                SpLiquidationToken::IcUsd,
                approval,
            )
            .unwrap();
        state
            .account_sp_liquidation_v2_approval_fee(
                row.request.request_id,
                SpLiquidationApprovalReceipt {
                    block_index: 54,
                    tuple: SpLiquidationApprovalTuple {
                        ledger,
                        owner: row.approval.owner.clone(),
                        spender: row.approval.spender.clone(),
                        allowance_raw: row.approval.allowance_raw,
                        fee_raw: row.approval.fee_raw,
                        memo: row.approval.memo.clone(),
                        created_at_time_ns: row.approval.created_at_time_ns,
                        expires_at_ns: row.approval.expires_at_ns,
                    },
                },
            )
            .unwrap();
        let account = |owner| icrc_ledger_types::icrc1::account::Account {
            owner,
            subaccount: None,
        };
        let stable_receipt = SpLiquidationStablePullReceipt {
            block_index: 55,
            tuple: SpLiquidationStablePullTuple {
                op_nonce: 9,
                ledger,
                from: account(pool),
                spender: account(protocol),
                to: account(protocol),
                amount_raw: 5,
                fee_raw: 3,
                memo: vec![2],
                created_at_time_ns: 30,
            },
        };
        let payout = SpLiquidationPayoutTuple {
            op_nonce: 10,
            ledger: collateral,
            source: account(protocol),
            destination: account(pool),
            gross_amount_raw: 11,
            net_amount_raw: 9,
            fee_raw: 2,
            memo: vec![3],
            created_at_time_ns: 40,
            collateral_type: collateral,
        };
        let result = SpLiquidationV2SuccessWithFee {
            block_index: 56,
            fee_amount_paid: 2,
            collateral_amount_received: Some(11),
            debt_liquidated_e8s: Some(5),
            stable_pulled_e6s: Some(5),
            xrp_claim_id: None,
        };
        state
            .apply_sp_liquidation_v2_stable_receipt(
                row.request.request_id,
                stable_receipt.clone(),
                payout.clone(),
                result.clone(),
            )
            .unwrap();
        state
            .apply_sp_liquidation_v2_stable_receipt(
                row.request.request_id,
                stable_receipt,
                payout,
                result,
            )
            .unwrap();

        let row = state
            .pending_sp_liquidation_v2(row.request.request_id)
            .unwrap();
        assert_eq!(row.pending_collateral_allocations.values().sum::<u64>(), 9);
        assert_eq!(
            state.total_stablecoin_balances.get(&ledger),
            Some(&(initial_total - 10))
        );
        let remaining: u64 = state
            .deposits
            .values()
            .map(|position| {
                position
                    .stablecoin_balances
                    .get(&ledger)
                    .copied()
                    .unwrap_or(0)
            })
            .sum();
        assert_eq!(remaining, initial_total - 10);
        let payout_receipt = SpLiquidationPayoutReceipt {
            block_index: 56,
            tuple: row.payout_tuple.clone().unwrap(),
        };
        state
            .record_sp_liquidation_v2_payout_receipt(row.request.request_id, payout_receipt)
            .unwrap();
        let completed = state
            .finalize_sp_liquidation_v2_payout(row.request.request_id, 50)
            .unwrap();
        assert_eq!(completed.phase, SpLiquidationV2LocalPhase::Complete);
        let gains: u64 = state
            .deposits
            .values()
            .map(|position| {
                position
                    .collateral_gains
                    .get(&collateral)
                    .copied()
                    .unwrap_or(0)
            })
            .sum();
        assert_eq!(gains, 9);
        assert_eq!(state.liquidation_history.len(), 1);
        state
            .finalize_sp_liquidation_v2_payout(row.request.request_id, 51)
            .unwrap();
        let retried_gains: u64 = state
            .deposits
            .values()
            .map(|position| {
                position
                    .collateral_gains
                    .get(&collateral)
                    .copied()
                    .unwrap_or(0)
            })
            .sum();
        assert_eq!(retried_gains, 9);
        assert_eq!(state.liquidation_history.len(), 1);
    }

    #[test]
    fn legacy_ambiguous_pending_refund_survives_fee_state_migration_held() {
        let mut state = test_state();
        let id = state.record_pending_refund(
            user_a(),
            icusd_ledger(),
            12_00000000,
            "legacy failure".into(),
            123,
        );
        let refund = state
            .pending_refunds
            .as_mut()
            .unwrap()
            .get_mut(&id)
            .unwrap();
        refund.transfer_attempted = None;
        refund.transfer_created_at_time_ns = None;
        refund.transfer_fee = None;
        refund.transfer_memo = None;
        refund.protocol_fee_reserved = None;
        state.pending_refund_fee_reserves = None;
        state.pending_refund_fee_funding_blocks = None;

        state.normalize_pending_refund_fee_state();

        let retained = state.pending_refunds.as_ref().unwrap().get(&id).unwrap();
        assert_eq!(retained.transfer_attempted, None);
        assert!(retained.transfer_created_at_time_ns.is_none());
        assert_eq!(state.pending_refund_fee_reserves, Some(BTreeMap::new()));
        assert_eq!(
            state.pending_refund_fee_funding_blocks,
            Some(BTreeSet::new())
        );
    }

    #[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
    struct DepositPositionPreCfx {
        pub stablecoin_balances: BTreeMap<Principal, u64>,
        pub collateral_gains: BTreeMap<Principal, u64>,
        pub opted_out_collateral: BTreeSet<Principal>,
        pub deposit_timestamp: u64,
        pub total_claimed_gains: BTreeMap<Principal, u64>,
        pub total_interest_earned_e8s: Option<u64>,
    }

    #[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
    struct StabilityPoolStatePreCfx {
        pub deposits: BTreeMap<Principal, DepositPositionPreCfx>,
        pub total_stablecoin_balances: BTreeMap<Principal, u64>,
        pub stablecoin_registry: BTreeMap<Principal, StablecoinConfig>,
        pub collateral_registry: BTreeMap<Principal, CollateralInfo>,
        pub protocol_canister_id: Principal,
        pub configuration: PoolConfiguration,
        pub liquidation_history: Vec<PoolLiquidationRecord>,
        pub in_flight_liquidations: BTreeSet<u64>,
        pub total_liquidations_executed: u64,
        pub pool_creation_timestamp: u64,
        pub total_interest_received_e8s: Option<u64>,
        pub token_consecutive_failures: Option<BTreeMap<Principal, u32>>,
        pub cached_virtual_prices: Option<BTreeMap<Principal, u128>>,
        pub protocol_reserve_address: Option<Principal>,
        pub is_initialized: bool,
        pub pool_events: Option<Vec<PoolEvent>>,
        pub next_event_id: Option<u64>,
        pub pending_refunds: Option<BTreeMap<u64, PendingRefund>>,
        pub next_pending_refund_id: Option<u64>,
    }

    #[test]
    fn pre_cfx_snapshot_decodes_with_empty_cfx_claims() {
        let mut current = test_state();
        add_deposit_direct(&mut current, user_a(), icusd_ledger(), 42_00000000);
        let pos = current.deposits.get(&user_a()).unwrap();
        let mut deposits = BTreeMap::new();
        deposits.insert(
            user_a(),
            DepositPositionPreCfx {
                stablecoin_balances: pos.stablecoin_balances.clone(),
                collateral_gains: pos.collateral_gains.clone(),
                opted_out_collateral: pos.opted_out_collateral.clone(),
                deposit_timestamp: pos.deposit_timestamp,
                total_claimed_gains: pos.total_claimed_gains.clone(),
                total_interest_earned_e8s: pos.total_interest_earned_e8s,
            },
        );
        let pre_cfx = StabilityPoolStatePreCfx {
            deposits,
            total_stablecoin_balances: current.total_stablecoin_balances.clone(),
            stablecoin_registry: current.stablecoin_registry.clone(),
            collateral_registry: current.collateral_registry.clone(),
            protocol_canister_id: current.protocol_canister_id,
            configuration: current.configuration.clone(),
            liquidation_history: current.liquidation_history.clone(),
            in_flight_liquidations: current.in_flight_liquidations.clone(),
            total_liquidations_executed: current.total_liquidations_executed,
            pool_creation_timestamp: current.pool_creation_timestamp,
            total_interest_received_e8s: current.total_interest_received_e8s,
            token_consecutive_failures: current.token_consecutive_failures.clone(),
            cached_virtual_prices: current.cached_virtual_prices.clone(),
            protocol_reserve_address: current.protocol_reserve_address,
            is_initialized: current.is_initialized,
            pool_events: current.pool_events.clone(),
            next_event_id: current.next_event_id,
            pending_refunds: current.pending_refunds.clone(),
            next_pending_refund_id: current.next_pending_refund_id,
        };
        let bytes = Encode!(&pre_cfx).expect("encode pre-CFX snapshot");

        let decoded = try_decode_state(&bytes).expect("pre-CFX snapshot must decode");
        let decoded_pos = decoded.deposits.get(&user_a()).expect("deposit survives");
        assert!(
            decoded_pos
                .cfx_claims
                .clone()
                .unwrap_or_default()
                .is_empty(),
            "missing CFX claims field must decode as empty",
        );
        assert!(
            decoded_pos
                .native_payout_addresses
                .clone()
                .unwrap_or_default()
                .is_empty(),
            "missing native payout-address field must decode as empty (no UPG-002 wipe)",
        );
        assert!(
            decoded_pos
                .native_payout_destination_tags
                .clone()
                .unwrap_or_default()
                .is_empty(),
            "missing native payout destination-tag field must decode as empty",
        );
        assert!(
            decoded_pos
                .pending_native_xrp_payouts
                .clone()
                .unwrap_or_default()
                .is_empty(),
            "missing pending native-XRP payout field must decode as empty",
        );
        assert!(
            decoded_pos
                .opted_in_chain_collateral
                .clone()
                .unwrap_or_default()
                .is_empty(),
            "missing chain opt-in field must decode as empty",
        );
        assert!(
            decoded
                .chain_collateral_sentinels
                .clone()
                .unwrap_or_default()
                .is_empty(),
            "missing chain sentinel registry must decode as empty",
        );
        assert!(
            decoded
                .chain_claim_sources
                .clone()
                .unwrap_or_default()
                .is_empty(),
            "missing chain claim source inventory must decode as empty",
        );
        assert!(
            decoded
                .pending_chain_absorbs
                .clone()
                .unwrap_or_default()
                .is_empty(),
            "missing pending chain absorb journal must decode as empty",
        );
        assert!(
            decoded
                .completed_chain_absorbs
                .clone()
                .unwrap_or_default()
                .is_empty(),
            "missing completed chain absorb journal must decode as empty",
        );
        assert!(
            decoded
                .pending_native_xrp_absorbs
                .clone()
                .unwrap_or_default()
                .is_empty(),
            "missing pending native-XRP absorb journal must decode as empty",
        );
        assert!(
            decoded
                .completed_cfx_claim_payout_recoveries
                .clone()
                .unwrap_or_default()
                .is_empty(),
            "missing CFX claim payout recovery journal must decode as empty",
        );
        assert!(
            decoded
                .completed_cfx_claim_payout_recovery_floor
                .clone()
                .unwrap_or_default()
                .is_empty(),
            "missing CFX claim payout recovery floor must decode as empty",
        );
    }

    #[test]
    fn completed_chain_absorbs_are_bounded() {
        let mut state = StabilityPoolState::default();
        for vault_id in 0..(MAX_COMPLETED_CHAIN_ABSORBS as u64 + 5) {
            state.record_completed_chain_absorb(ChainSpAbsorbCompletion {
                vault_id,
                result: ChainSpAbsorbResult {
                    success: true,
                    vault_id,
                    chain_id: rumi_protocol_backend::chains::config::ChainId(1030),
                    icusd_burned_e8s: 100_00000000,
                    liquidated_debt_e8s: 100_00000000,
                    collateral_received_native: 10_000_000_000_000_000_000u128,
                    claim_id: vault_id,
                    custody_address: "0xcustody".to_string(),
                    block_index: vault_id,
                    collateral_price_e8s: 5_000_000,
                },
                completed_at_ns: vault_id,
            });
        }

        let completed = state.completed_chain_absorbs.as_ref().unwrap();
        assert_eq!(completed.len(), MAX_COMPLETED_CHAIN_ABSORBS);
        assert!(!completed.contains_key(&0));
        assert!(completed.contains_key(&(MAX_COMPLETED_CHAIN_ABSORBS as u64 + 4)));
    }

    // ─── Test: Opt-out mid-liquidation burn escape (audit AR-S-002) ───

    #[test]
    fn ar_s_002_opt_out_mid_liquidation_escapes_burn() {
        // Demonstrates WHY opt_in/opt_out must reject while a liquidation holds
        // the SP guard: an opt-out landing between the draw snapshot and the
        // burn apportionment escapes its share of the burn entirely while the
        // opted-in remainder over-absorbs. The endpoint-level fix gates
        // opt_in_collateral / opt_out_collateral on
        // pool_guard::liquidation_in_progress() (SystemBusy), so this state
        // sequence is no longer reachable through the canister interface.
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 50_00000000);
        add_deposit_direct(&mut state, user_b(), icusd_ledger(), 50_00000000);

        // Liquidation snapshot: both users opted in, draw 40 icUSD of debt.
        let draw = state.compute_token_draw(40_00000000, &icp_ledger());
        assert_eq!(draw.get(&icusd_ledger()).copied(), Some(40_00000000));

        // Mid-flight mutation (what AR-S-002 blocks): user_b opts out while
        // the liquidation awaits the backend.
        state.opt_out_collateral(&user_b(), icp_ledger()).unwrap();

        state
            .process_liquidation_gains_at(
                1,
                icp_ledger(),
                &draw,
                10_00000000,
                7_50000000,
                1_000_000_000,
            )
            .unwrap();

        // user_b escaped the burn entirely (balance untouched, no gains)...
        let pos_b = state.deposits.get(&user_b()).unwrap();
        assert_eq!(
            pos_b.stablecoin_balances.get(&icusd_ledger()).copied(),
            Some(50_00000000),
            "opt-out mid-liquidation escapes the burn",
        );
        assert_eq!(
            pos_b
                .collateral_gains
                .get(&icp_ledger())
                .copied()
                .unwrap_or(0),
            0
        );

        // ...while user_a absorbed the FULL 40 instead of their fair 20.
        let pos_a = state.deposits.get(&user_a()).unwrap();
        assert_eq!(
            pos_a.stablecoin_balances.get(&icusd_ledger()).copied(),
            Some(10_00000000),
            "remaining depositor over-absorbs the escaped share",
        );
    }

    #[test]
    fn v2_payout_supersession_adopts_one_exact_successor_and_fails_closed_on_legacy_state() {
        #[derive(CandidType)]
        struct PendingSpLiquidationV2BeforeSupersession {
            request: SpLiquidationV2Intent,
            backend_request: Option<SpLiquidationV2Request>,
            stablecoin_ledger: Principal,
            collateral_type: Principal,
            collateral_price_e8s: u64,
            approval: SpLiquidationV2ApprovalTuple,
            approval_dispatch_in_flight: bool,
            approval_ambiguous_seen: bool,
            approval_proven_no_effect: bool,
            approval_candidate_block_index: Option<u64>,
            approval_receipt_block_index: Option<u64>,
            phase: SpLiquidationV2LocalPhase,
            ambiguous_seen: bool,
            stable_pull_receipt: Option<SpLiquidationStablePullReceipt>,
            stable_pull_tuple: Option<SpLiquidationStablePullTuple>,
            stable_pull_candidate_block_index: Option<u64>,
            payout_tuple: Option<SpLiquidationPayoutTuple>,
            payout_candidate_block_index: Option<u64>,
            payout_receipt: Option<SpLiquidationPayoutReceipt>,
            result: Option<SpLiquidationV2SuccessWithFee>,
            stable_debit_applied: bool,
            pending_collateral_allocations: BTreeMap<Principal, u64>,
            approval_fee_debits: BTreeMap<Principal, u64>,
            stable_pull_fee_debits: BTreeMap<Principal, u64>,
            stable_principal_debits: BTreeMap<Principal, u64>,
            stable_refund_tuple: Option<SpLiquidationStableRefundTuple>,
            stable_refund_candidate_block_index: Option<u64>,
            stable_refund_receipt: Option<SpLiquidationStableRefundReceipt>,
            stable_refund_applied: bool,
            backend_acknowledged: bool,
            last_error: Option<String>,
        }

        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_000_000);
        let request_id = prepare_v2_request(&mut state, 77);
        let old_shape = state.pending_sp_liquidation_v2(request_id).unwrap();
        let old_bytes = Encode!(&PendingSpLiquidationV2BeforeSupersession {
            request: old_shape.request.clone(),
            backend_request: old_shape.backend_request.clone(),
            stablecoin_ledger: old_shape.stablecoin_ledger,
            collateral_type: old_shape.collateral_type,
            collateral_price_e8s: old_shape.collateral_price_e8s,
            approval: old_shape.approval.clone(),
            approval_dispatch_in_flight: old_shape.approval_dispatch_in_flight,
            approval_ambiguous_seen: old_shape.approval_ambiguous_seen,
            approval_proven_no_effect: old_shape.approval_proven_no_effect,
            approval_candidate_block_index: old_shape.approval_candidate_block_index,
            approval_receipt_block_index: old_shape.approval_receipt_block_index,
            phase: old_shape.phase,
            ambiguous_seen: old_shape.ambiguous_seen,
            stable_pull_receipt: old_shape.stable_pull_receipt.clone(),
            stable_pull_tuple: old_shape.stable_pull_tuple.clone(),
            stable_pull_candidate_block_index: old_shape.stable_pull_candidate_block_index,
            payout_tuple: old_shape.payout_tuple.clone(),
            payout_candidate_block_index: old_shape.payout_candidate_block_index,
            payout_receipt: old_shape.payout_receipt.clone(),
            result: old_shape.result.clone(),
            stable_debit_applied: old_shape.stable_debit_applied,
            pending_collateral_allocations: old_shape.pending_collateral_allocations.clone(),
            approval_fee_debits: old_shape.approval_fee_debits.clone(),
            stable_pull_fee_debits: old_shape.stable_pull_fee_debits.clone(),
            stable_principal_debits: old_shape.stable_principal_debits.clone(),
            stable_refund_tuple: old_shape.stable_refund_tuple.clone(),
            stable_refund_candidate_block_index: old_shape.stable_refund_candidate_block_index,
            stable_refund_receipt: old_shape.stable_refund_receipt.clone(),
            stable_refund_applied: old_shape.stable_refund_applied,
            backend_acknowledged: old_shape.backend_acknowledged,
            last_error: old_shape.last_error.clone(),
        })
        .expect("encode pre-supersession stable row");
        let migrated: PendingSpLiquidationV2 = Decode!(&old_bytes, PendingSpLiquidationV2)
            .expect("old stable row decodes with new optional fields");
        assert_eq!(migrated.payout_supersession_generation, None);
        assert_eq!(migrated.payout_supersession_predecessor, None);
        assert_eq!(migrated.payout_supersession_replacement, None);

        state
            .pending_sp_liquidations_v2
            .as_mut()
            .unwrap()
            .get_mut(&request_id)
            .unwrap()
            .approval
            .fee_accounted = true;
        let row = state.pending_sp_liquidation_v2(request_id).unwrap();
        assert_eq!(row.collateral_type, icp_ledger());
        let pool = row.approval.owner.owner;
        let protocol = state.protocol_canister_id;
        let account = |owner| icrc_ledger_types::icrc1::account::Account {
            owner,
            subaccount: None,
        };
        let predecessor = SpLiquidationPayoutTuple {
            op_nonce: 10,
            ledger: icp_ledger(),
            source: account(protocol),
            destination: account(pool),
            gross_amount_raw: 10,
            net_amount_raw: 8,
            fee_raw: 2,
            memo: vec![1],
            created_at_time_ns: 20,
            collateral_type: icp_ledger(),
        };
        let replacement = SpLiquidationPayoutTuple {
            op_nonce: 11,
            ledger: icp_ledger(),
            source: account(protocol),
            destination: account(pool),
            gross_amount_raw: 10,
            net_amount_raw: 6,
            fee_raw: 4,
            memo: rumi_protocol_backend::management::nonce_to_memo(11)
                .0
                .to_vec(),
            created_at_time_ns: rumi_protocol_backend::management::nonce_to_created_at_time(11),
            collateral_type: icp_ledger(),
        };
        let stable_pull_receipt = SpLiquidationStablePullReceipt {
            block_index: 3,
            tuple: SpLiquidationStablePullTuple {
                op_nonce: 4,
                ledger: icusd_ledger(),
                from: row.approval.owner.clone(),
                spender: row.approval.spender.clone(),
                to: row.approval.spender.clone(),
                amount_raw: 5,
                fee_raw: 0,
                memo: vec![3],
                created_at_time_ns: 19,
            },
        };
        let result = SpLiquidationV2SuccessWithFee {
            block_index: 3,
            fee_amount_paid: 0,
            collateral_amount_received: Some(10),
            debt_liquidated_e8s: Some(5),
            stable_pulled_e6s: Some(5),
            xrp_claim_id: None,
        };

        state
            .adopt_sp_liquidation_v2_payout_supersession(
                request_id,
                &stable_pull_receipt,
                predecessor.clone(),
                replacement.clone(),
                &result,
                SpLiquidationPayoutNoEffectEvidence::BadFee {
                    expected_fee_raw: 4,
                },
                1,
            )
            .expect("authenticated first supersession is adopted");
        let saved = state.pending_sp_liquidation_v2(request_id).unwrap();
        assert_eq!(saved.payout_tuple.as_ref(), Some(&replacement));
        assert_eq!(saved.payout_supersession_generation, Some(1));
        assert_eq!(
            saved.payout_supersession_predecessor.as_ref(),
            Some(&predecessor)
        );
        assert_eq!(
            saved.payout_supersession_replacement.as_ref(),
            Some(&replacement)
        );
        assert!(
            saved.stable_debit_applied,
            "first observation applies the proven debit"
        );
        assert_eq!(
            saved.pending_collateral_allocations.values().sum::<u64>(),
            6,
            "first observation allocates the successor net"
        );

        state
            .adopt_sp_liquidation_v2_payout_supersession(
                request_id,
                &stable_pull_receipt,
                predecessor.clone(),
                replacement.clone(),
                &result,
                SpLiquidationPayoutNoEffectEvidence::BadFee {
                    expected_fee_raw: 4,
                },
                1,
            )
            .expect("lost backend accept reply replays the exact transition");
        let mut changed = replacement.clone();
        changed.memo.push(9);
        assert!(
            state
                .adopt_sp_liquidation_v2_payout_supersession(
                    request_id,
                    &stable_pull_receipt,
                    predecessor.clone(),
                    changed,
                    &result,
                    SpLiquidationPayoutNoEffectEvidence::BadFee {
                        expected_fee_raw: 4
                    },
                    1,
                )
                .is_err(),
            "same generation cannot authorize a different tuple"
        );

        let mut legacy = test_state();
        let legacy_id = prepare_v2_request(&mut legacy, 88);
        let mut legacy_row = legacy.pending_sp_liquidation_v2(legacy_id).unwrap();
        // `None` is the serde default for rows created before supersession
        // support; it must remain ineligible after an upgrade.
        legacy_row.payout_supersession_generation = None;
        legacy.update_pending_sp_liquidation_v2(legacy_row).unwrap();
        assert!(
            legacy
                .adopt_sp_liquidation_v2_payout_supersession(
                    legacy_id,
                    &stable_pull_receipt,
                    predecessor.clone(),
                    replacement.clone(),
                    &result,
                    SpLiquidationPayoutNoEffectEvidence::BadFee {
                        expected_fee_raw: 4
                    },
                    1,
                )
                .is_err(),
            "legacy rows fail closed"
        );

        let mut candidate = test_state();
        let candidate_id = prepare_v2_request(&mut candidate, 99);
        let candidate_row = candidate
            .pending_sp_liquidations_v2
            .as_mut()
            .unwrap()
            .get_mut(&candidate_id)
            .unwrap();
        candidate_row.approval.fee_accounted = true;
        candidate_row.payout_tuple = Some(predecessor.clone());
        candidate_row.payout_candidate_block_index = Some(0);
        assert!(
            candidate
                .adopt_sp_liquidation_v2_payout_supersession(
                    candidate_id,
                    &stable_pull_receipt,
                    predecessor.clone(),
                    replacement.clone(),
                    &result,
                    SpLiquidationPayoutNoEffectEvidence::BadFee {
                        expected_fee_raw: 4
                    },
                    1,
                )
                .is_err(),
            "a payout candidate blocks tuple replacement"
        );

        let mut receipted = test_state();
        let receipted_id = prepare_v2_request(&mut receipted, 100);
        let receipted_row = receipted
            .pending_sp_liquidations_v2
            .as_mut()
            .unwrap()
            .get_mut(&receipted_id)
            .unwrap();
        receipted_row.approval.fee_accounted = true;
        receipted_row.payout_tuple = Some(predecessor.clone());
        receipted_row.payout_receipt = Some(SpLiquidationPayoutReceipt {
            block_index: 0,
            tuple: predecessor.clone(),
        });
        assert!(
            receipted
                .adopt_sp_liquidation_v2_payout_supersession(
                    receipted_id,
                    &stable_pull_receipt,
                    predecessor.clone(),
                    replacement.clone(),
                    &result,
                    SpLiquidationPayoutNoEffectEvidence::BadFee {
                        expected_fee_raw: 4
                    },
                    1,
                )
                .is_err(),
            "an old payout receipt blocks tuple replacement"
        );

        // A previously applied predecessor debit keeps the stablecoin share
        // maps unchanged while the collateral liability is rebased to net=6.
        let mut already_debited = test_state();
        add_deposit_direct(&mut already_debited, user_a(), icusd_ledger(), 100_000_000);
        let rebased_id = prepare_v2_request(&mut already_debited, 101);
        already_debited
            .pending_sp_liquidations_v2
            .as_mut()
            .unwrap()
            .get_mut(&rebased_id)
            .unwrap()
            .approval
            .fee_accounted = true;
        already_debited
            .apply_sp_liquidation_v2_stable_receipt(
                rebased_id,
                stable_pull_receipt.clone(),
                predecessor.clone(),
                result.clone(),
            )
            .expect("predecessor debit is applied before the fee correction");
        let debits_before = already_debited
            .pending_sp_liquidation_v2(rebased_id)
            .unwrap()
            .stable_principal_debits
            .clone();
        let pool_balance_before = already_debited.total_stablecoin_balances[&icusd_ledger()];
        already_debited
            .adopt_sp_liquidation_v2_payout_supersession(
                rebased_id,
                &stable_pull_receipt,
                predecessor.clone(),
                replacement.clone(),
                &result,
                SpLiquidationPayoutNoEffectEvidence::BadFee {
                    expected_fee_raw: 4,
                },
                1,
            )
            .expect("BadFee successor rebases collateral allocations");
        let rebased = already_debited
            .pending_sp_liquidation_v2(rebased_id)
            .unwrap();
        assert_eq!(rebased.stable_principal_debits, debits_before);
        assert_eq!(
            already_debited.total_stablecoin_balances[&icusd_ledger()],
            pool_balance_before
        );
        assert_eq!(
            rebased.pending_collateral_allocations.values().sum::<u64>(),
            6
        );

        // An injected allocation precondition failure occurs before any local
        // adoption or stable debit and therefore cannot leave a partial edge.
        let mut allocation_error = test_state();
        let error_id = prepare_v2_request(&mut allocation_error, 102);
        allocation_error
            .pending_sp_liquidations_v2
            .as_mut()
            .unwrap()
            .get_mut(&error_id)
            .unwrap()
            .approval
            .fee_accounted = true;
        assert!(allocation_error
            .adopt_sp_liquidation_v2_payout_supersession(
                error_id,
                &stable_pull_receipt,
                predecessor.clone(),
                replacement.clone(),
                &result,
                SpLiquidationPayoutNoEffectEvidence::BadFee {
                    expected_fee_raw: 4
                },
                1,
            )
            .is_err());
        let unchanged = allocation_error
            .pending_sp_liquidation_v2(error_id)
            .unwrap();
        assert_eq!(unchanged.payout_supersession_generation, Some(0));
        assert!(unchanged.payout_supersession_predecessor.is_none());
        assert!(unchanged.payout_tuple.is_none());
        assert!(!unchanged.stable_debit_applied);

        // InsufficientFunds preserves the exact transfer economics while
        // adopting a fresh dedup identity.
        let mut insufficient = test_state();
        add_deposit_direct(&mut insufficient, user_a(), icusd_ledger(), 50_000_000);
        let insufficient_id = prepare_v2_request(&mut insufficient, 103);
        insufficient
            .pending_sp_liquidations_v2
            .as_mut()
            .unwrap()
            .get_mut(&insufficient_id)
            .unwrap()
            .approval
            .fee_accounted = true;
        let fresh = SpLiquidationPayoutTuple {
            op_nonce: 12,
            ledger: predecessor.ledger,
            source: predecessor.source.clone(),
            destination: predecessor.destination.clone(),
            gross_amount_raw: predecessor.gross_amount_raw,
            net_amount_raw: predecessor.net_amount_raw,
            fee_raw: predecessor.fee_raw,
            memo: rumi_protocol_backend::management::nonce_to_memo(12)
                .0
                .to_vec(),
            created_at_time_ns: rumi_protocol_backend::management::nonce_to_created_at_time(12),
            collateral_type: predecessor.collateral_type,
        };
        insufficient
            .adopt_sp_liquidation_v2_payout_supersession(
                insufficient_id,
                &stable_pull_receipt,
                predecessor.clone(),
                fresh.clone(),
                &result,
                SpLiquidationPayoutNoEffectEvidence::InsufficientFunds {
                    reported_balance_raw: 0,
                },
                1,
            )
            .expect("InsufficientFunds successor preserves gross, net, and fee");
        let saved = insufficient
            .pending_sp_liquidation_v2(insufficient_id)
            .unwrap();
        assert_eq!(saved.payout_tuple.as_ref(), Some(&fresh));
    }
}
