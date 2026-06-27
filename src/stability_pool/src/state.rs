use candid::{CandidType, Decode, Encode, Principal};
use ic_canister_log::log;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use crate::logs::INFO;
use crate::types::*;

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
            is_initialized: false,
            pool_events: Some(Vec::new()),
            next_event_id: Some(0),
            pending_refunds: Some(BTreeMap::new()),
            next_pending_refund_id: Some(0),
        }
    }
}

/// Maximum pool events retained in memory.
const MAX_POOL_EVENTS: usize = 10_000;

/// Maximum outstanding pending refunds (audit IC-S-001). A record is only
/// created when a refund transfer fails after the user's tokens were pulled,
/// which is not caller-controllable, so this is a memory-safety bound rather
/// than an anti-DoS one (mirrors rumi_3pool's MAX_PENDING_CLAIMS).
pub const MAX_PENDING_REFUNDS: usize = 10_000;
pub const MAX_PENDING_CHAIN_ABSORBS: usize = 1_000;
pub const MAX_COMPLETED_CHAIN_ABSORBS: usize = 10_000;
pub const MAX_COMPLETED_CFX_CLAIM_PAYOUT_RECOVERIES: usize = 10_000;

impl StabilityPoolState {
    pub fn initialize(&mut self, args: StabilityPoolInitArgs) {
        self.protocol_canister_id = args.protocol_canister_id;
        self.configuration.authorized_admins = args.authorized_admins;
        self.pool_creation_timestamp = ic_cdk::api::time();
        self.is_initialized = true;
    }

    /// Append a pool event. Trims oldest events if over capacity.
    pub fn push_event(&mut self, caller: Principal, event_type: PoolEventType) {
        let id = self.next_event_id.unwrap_or(0);
        self.next_event_id = Some(id + 1);

        let events = self.pool_events.get_or_insert_with(Vec::new);
        events.push(PoolEvent {
            id,
            timestamp: ic_cdk::api::time(),
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

    pub fn is_admin(&self, caller: &Principal) -> bool {
        self.configuration.authorized_admins.contains(caller)
    }

    // ─── Stablecoin Registry ───

    pub fn register_stablecoin(&mut self, config: StablecoinConfig) {
        self.total_stablecoin_balances
            .entry(config.ledger_id)
            .or_insert(0);
        self.stablecoin_registry.insert(config.ledger_id, config);
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

    pub fn native_payout_address(&self, user: &Principal, collateral_type: &Principal) -> Option<String> {
        self.deposits
            .get(user)
            .and_then(|pos| pos.native_payout_addresses.as_ref())
            .and_then(|addresses| addresses.get(collateral_type).cloned())
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

    pub fn add_deposit(&mut self, user: Principal, token_ledger: Principal, amount: u64) {
        let position = self
            .deposits
            .entry(user)
            .or_insert_with(|| DepositPosition::new(ic_cdk::api::time()));
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
    /// If no eligible depositors hold icUSD when this is called, the function
    /// logs a warning and returns without crediting anyone. The minted icUSD
    /// remains in the SP canister's ICRC-1 balance, untracked by
    /// `total_stablecoin_balances`. This is a known operator-visible edge case:
    /// the icUSD will be picked up on the next distribution that has eligible
    /// depositors, or will need manual reconciliation if the no-icUSD-depositors
    /// state persists.
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
            // No icUSD depositors. The minted icUSD remains in the SP canister's
            // ICRC-1 balance, untracked in total_stablecoin_balances. Surface for
            // operator visibility: a persistent occurrence indicates the SP has
            // drifted away from icUSD-only and may need manual reconciliation.
            log!(
                INFO,
                "WARN distribute_interest_revenue: {} of token {} received but no eligible icUSD depositors; amount remains in SP canister",
                amount,
                token_ledger
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
        if !self.collateral_requires_payout_address(&collateral_type) {
            return self.opt_in_collateral(user, collateral_type);
        }

        let address = payout_address.trim().to_string();
        rumi_protocol_backend::chains::xrp::address::account_id_from_classic_address(&address)
            .map_err(|reason| StabilityPoolError::InvalidPayoutAddress { reason })?;

        let position = self.deposits.get_mut(user)
            .ok_or(StabilityPoolError::NoPositionFound)?;
        position.opted_out_collateral.remove(&collateral_type);
        position
            .native_payout_addresses
            .get_or_insert_with(BTreeMap::new)
            .insert(collateral_type, address);
        Ok(())
    }

    // ─── Pending Refunds (audit IC-S-001) ───

    /// Record tokens the pool owes `user` after a failed `deposit_as_3usd`
    /// refund so they can be recovered via `claim_pending_refund`. `amount` is
    /// the GROSS amount still held by the pool; the payout nets the ledger fee.
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
        // Bound memory. Ids are monotonic, so the smallest key is the oldest
        // record; dropping it is a (logged at the call site) value loss, but
        // reaching the cap requires thousands of genuine ledger failures.
        if refunds.len() >= MAX_PENDING_REFUNDS {
            if let Some(oldest) = refunds.keys().next().copied() {
                refunds.remove(&oldest);
            }
        }
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
            },
        );
        id
    }

    /// Remove and return a pending refund. Removal happens BEFORE the payout
    /// transfer so two concurrent claims cannot both pay out; the caller
    /// re-inserts via `put_pending_refund` if the transfer fails.
    pub fn take_pending_refund(&mut self, id: u64) -> Option<PendingRefund> {
        self.pending_refunds.as_mut().and_then(|m| m.remove(&id))
    }

    pub fn put_pending_refund(&mut self, refund: PendingRefund) {
        self.pending_refunds
            .get_or_insert_with(BTreeMap::new)
            .insert(refund.id, refund);
    }

    pub fn pending_refunds_for(&self, user: &Principal) -> Vec<PendingRefund> {
        self.pending_refunds
            .as_ref()
            .map(|m| m.values().filter(|r| r.user == *user).cloned().collect())
            .unwrap_or_default()
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
    ) {
        self.process_liquidation_gains_at(
            vault_id,
            collateral_type,
            stables_consumed,
            collateral_gained,
            collateral_price_e8s,
            ic_cdk::api::time(),
        );
    }

    pub fn process_chain_liquidation_gains(
        &mut self,
        vault_id: u64,
        chain_sentinel: Principal,
        stables_consumed: &BTreeMap<Principal, u64>,
        cfx_gained_native: u128,
        collateral_price_e8s: u64,
    ) {
        self.process_chain_liquidation_gains_at(
            vault_id,
            chain_sentinel,
            stables_consumed,
            cfx_gained_native,
            collateral_price_e8s,
            ic_cdk::api::time(),
        );
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
    ) {
        // Phase 1: Compute each opted-in depositor's share of the consumed stables (in e8s)
        let opted_in_principals: Vec<Principal> = self
            .deposits
            .iter()
            .filter(|(_, pos)| self.position_opted_in_for(pos, &collateral_type))
            .map(|(p, _)| *p)
            .collect();

        // For each consumed token, compute total opted-in balance for that token
        let mut per_token_opted_in_totals: BTreeMap<Principal, u64> = BTreeMap::new();
        for token_ledger in stables_consumed.keys() {
            let total: u64 = opted_in_principals
                .iter()
                .filter_map(|p| self.deposits.get(p))
                .map(|pos| {
                    pos.stablecoin_balances
                        .get(token_ledger)
                        .copied()
                        .unwrap_or(0)
                })
                .sum();
            per_token_opted_in_totals.insert(*token_ledger, total);
        }

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
            return;
        }

        // Phase 3: For each opted-in depositor, reduce their token balances and add collateral gains.
        // Track actual deductions per token to avoid rounding drift between aggregate and individual totals.
        let mut actual_deductions_per_token: BTreeMap<Principal, u64> = BTreeMap::new();
        let mut total_collateral_distributed: u64 = 0;

        for principal in &opted_in_principals {
            let mut user_consumed_e8s: u64 = 0;

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

                    // User's share of this token's consumption
                    let user_share_native = (total_consumed as u128 * user_balance as u128
                        / total_opted_in as u128)
                        as u64;
                    let user_share_native = user_share_native.min(user_balance);

                    // Reduce balance
                    if let Some(bal) = position.stablecoin_balances.get_mut(token_ledger) {
                        *bal = bal.saturating_sub(user_share_native);
                    }

                    // Track actual deduction for aggregate update
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
        let record = PoolLiquidationRecord {
            vault_id,
            timestamp,
            stables_consumed: stables_consumed.clone(),
            collateral_gained,
            collateral_type,
            depositors_count: opted_in_principals.len() as u64,
            collateral_price_e8s: Some(collateral_price_e8s),
        };
        self.liquidation_history.push(record);
        self.total_liquidations_executed += 1;

        // Cap history to prevent unbounded memory growth
        if self.liquidation_history.len() > MAX_LIQUIDATION_HISTORY {
            let excess = self.liquidation_history.len() - MAX_LIQUIDATION_HISTORY;
            self.liquidation_history.drain(..excess);
        }

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
    ) {
        if !self.is_chain_collateral_sentinel(&chain_sentinel) || cfx_gained_native == 0 {
            return;
        }

        let opted_in_principals: Vec<Principal> = self
            .deposits
            .iter()
            .filter(|(_, pos)| pos.is_opted_in_for_chain(&chain_sentinel))
            .map(|(p, _)| *p)
            .collect();
        if opted_in_principals.is_empty() {
            return;
        }

        let mut per_token_opted_in_totals: BTreeMap<Principal, u64> = BTreeMap::new();
        for token_ledger in stables_consumed.keys() {
            let total: u64 = opted_in_principals
                .iter()
                .filter_map(|p| self.deposits.get(p))
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
            return;
        }

        let mut actual_deductions_per_token: BTreeMap<Principal, u64> = BTreeMap::new();
        let mut total_cfx_distributed: u128 = 0;

        for principal in &opted_in_principals {
            let mut user_consumed_e8s: u64 = 0;

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
                    if let Some(bal) = position.stablecoin_balances.get_mut(token_ledger) {
                        *bal = bal.saturating_sub(user_share_native);
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
        }
    }

    /// For each collateral type, compute the total USD value (e8s) of deposits
    /// held by depositors who are opted in to that collateral type.
    /// Counts all stablecoins (icUSD, 3USD, ckUSDT, etc.) since all depositors
    /// now earn interest proportional to their total deposit value.
    fn eligible_icusd_per_collateral(&self) -> Vec<(Principal, u64)> {
        let vps = self.virtual_prices();
        self.collateral_registry
            .keys()
            .map(|ct| {
                let eligible: u64 = self
                    .deposits
                    .values()
                    .filter(|pos| self.position_opted_in_for(pos, ct))
                    .map(|pos| pos.total_usd_value(&self.stablecoin_registry, vps))
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
            native_payout_addresses: pos.native_payout_addresses.clone().unwrap_or_default(),
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
    pub fn deduct_fee_from_pool(&mut self, token_ledger: Principal, fee: u64) {
        let total = match self.total_stablecoin_balances.get(&token_ledger).copied() {
            Some(t) if t > 0 => t,
            _ => return,
        };

        let mut deducted: u64 = 0;
        let depositor_keys: Vec<Principal> = self.deposits.keys().copied().collect();

        for key in &depositor_keys {
            if let Some(pos) = self.deposits.get_mut(key) {
                if let Some(bal) = pos.stablecoin_balances.get_mut(&token_ledger) {
                    if *bal > 0 {
                        // Proportional share: fee * bal / total (rounded down)
                        let share = (fee as u128 * *bal as u128 / total as u128) as u64;
                        let actual = share.min(*bal);
                        *bal = bal.saturating_sub(actual);
                        deducted += actual;
                        if *bal == 0 {
                            pos.stablecoin_balances.remove(&token_ledger);
                        }
                    }
                }
            }
        }

        // Apply any rounding remainder (at most depositor_count - 1 units) to the aggregate
        if let Some(agg) = self.total_stablecoin_balances.get_mut(&token_ledger) {
            *agg = agg.saturating_sub(deducted);
        }
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
            is_initialized: v1.is_initialized,
            pool_events: v1.pool_events,
            next_event_id: v1.next_event_id,
            pending_refunds: Some(BTreeMap::new()),
            next_pending_refund_id: Some(0),
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
        return Some(state);
    }
    // v1: pre-IC-S-001 (no pending_refunds / next_pending_refund_id).
    if let Ok(prev) = Decode!(bytes, StabilityPoolStateV1) {
        return Some(prev.into());
    }
    None
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
            transfer_fee: Some(10),
            is_lp_token: None,
            underlying_pool: None,
        });
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

        state.process_liquidation_gains_at(
            1, // vault_id
            icp_ledger(),
            &stables_consumed,
            5_00000000,    // 5 ICP
            7_50000000,    // collateral price $7.50
            1_000_000_000, // timestamp
        );

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

        state.process_liquidation_gains_at(
            2,
            icp_ledger(),
            &stables_consumed,
            10_00000000,
            7_50000000,
            2_000_000_000,
        );

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
            state.compute_token_draw(10_00000000, &xrp_ledger()).is_empty(),
            "XRP liquidations must not draw from users without a payout address"
        );

        let err = state.opt_in_collateral(&user_a(), xrp_ledger()).unwrap_err();
        assert!(matches!(err, StabilityPoolError::PayoutAddressRequired { .. }));

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

        let mut consumed = BTreeMap::new();
        consumed.insert(icusd_ledger(), 20_00000000);
        state.process_liquidation_gains_at(
            144,
            xrp_ledger(),
            &consumed,
            5_000_000,
            50_00000000,
            3_000_000_000,
        );

        let pos_a = state.deposits.get(&user_a()).unwrap();
        let pos_b = state.deposits.get(&user_b()).unwrap();
        assert_eq!(pos_a.stablecoin_balances.get(&icusd_ledger()).copied().unwrap_or(0), 80_00000000);
        assert_eq!(pos_a.collateral_gains.get(&xrp_ledger()).copied().unwrap_or(0), 5_000_000);
        assert_eq!(pos_b.stablecoin_balances.get(&icusd_ledger()).copied().unwrap_or(0), 50_00000000);
        assert_eq!(pos_b.collateral_gains.get(&xrp_ledger()).copied().unwrap_or(0), 0);
    }

    #[test]
    fn xrp_interest_only_goes_to_address_opted_depositors() {
        let mut state = test_state();

        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        add_deposit_direct(&mut state, user_b(), icusd_ledger(), 50_00000000);

        state.distribute_interest_revenue(icusd_ledger(), 9_00000000, Some(xrp_ledger()));
        assert_eq!(
            state.deposits.get(&user_a()).unwrap().stablecoin_balances.get(&icusd_ledger()).copied().unwrap_or(0),
            100_00000000,
            "XRP interest must not be distributed until a depositor provides a payout address"
        );
        assert_eq!(
            state.deposits.get(&user_b()).unwrap().stablecoin_balances.get(&icusd_ledger()).copied().unwrap_or(0),
            50_00000000,
        );

        state
            .opt_in_native_collateral(&user_a(), xrp_ledger(), valid_xrp_address())
            .unwrap();
        state.distribute_interest_revenue(icusd_ledger(), 9_00000000, Some(xrp_ledger()));

        let pos_a = state.deposits.get(&user_a()).unwrap();
        let pos_b = state.deposits.get(&user_b()).unwrap();
        assert_eq!(pos_a.stablecoin_balances.get(&icusd_ledger()).copied().unwrap_or(0), 109_00000000);
        assert_eq!(pos_b.stablecoin_balances.get(&icusd_ledger()).copied().unwrap_or(0), 50_00000000);
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
        assert!(matches!(err, StabilityPoolError::InvalidPayoutAddress { .. }));
        assert_eq!(state.native_payout_address(&user_a(), &xrp_ledger()), None);
        assert_eq!(state.effective_pool_for_collateral(&xrp_ledger()), 0);
    }

    #[test]
    fn xrp_opt_out_clears_payout_address() {
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 10_00000000);

        state
            .opt_in_native_collateral(&user_a(), xrp_ledger(), valid_xrp_address())
            .unwrap();
        state.opt_out_collateral(&user_a(), xrp_ledger()).unwrap();

        assert_eq!(state.native_payout_address(&user_a(), &xrp_ledger()), None);
        assert_eq!(state.effective_pool_for_collateral(&xrp_ledger()), 0);
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

        state.process_chain_liquidation_gains_at(
            99,
            cfx_sentinel(),
            &stables_consumed,
            20_000 * E18 + 1,
            5_000_000,
            123,
        );

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

        state.process_liquidation_gains_at(
            10,
            icp_ledger(),
            &stables_consumed,
            20_00000000,
            7_50000000,
            3_000_000_000,
        );

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

        state.process_liquidation_gains_at(
            5,
            icp_ledger(),
            &stables_consumed,
            50_00000000,
            7_50000000,
            4_000_000_000,
        );

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
    fn test_distribute_interest_no_icusd_depositors() {
        // When the pool has depositors but none hold icUSD, the function should
        // return without crediting anyone or advancing aggregates. The icUSD
        // amount remains in the SP canister's ICRC-1 balance (orphaned for
        // operator visibility; see function docstring).
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

        state.distribute_interest_revenue(icusd_ledger(), 10_00000000, None);

        // Aggregates unchanged
        assert_eq!(
            state.total_interest_received_e8s.unwrap_or(0),
            total_received_before,
            "total_interest_received_e8s should not advance when no icUSD depositors"
        );
        assert_eq!(
            state
                .total_stablecoin_balances
                .get(&icusd_ledger())
                .copied()
                .unwrap_or(0),
            stable_balance_before,
            "total_stablecoin_balances[icusd] should not advance"
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

        state.process_liquidation_gains_at(
            1,
            icp_ledger(),
            &consumed,
            500_000,
            7_50000000,
            1_000_000_000,
        );

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

        // take removes the record (remove-before-transfer): a second take
        // returns None, so two concurrent claims cannot both pay out.
        let taken = state.take_pending_refund(id0).expect("first take succeeds");
        assert_eq!(taken.amount, 5_00000000);
        assert!(
            state.take_pending_refund(id0).is_none(),
            "double-claim must not pay twice"
        );

        // put_pending_refund restores the record after a failed payout transfer.
        state.put_pending_refund(taken);
        assert_eq!(state.pending_refunds_for(&user_a()).len(), 2);

        // ids keep growing across take/put cycles.
        let id3 = state.record_pending_refund(user_c(), icusd_ledger(), 1, "x".to_string(), 400);
        assert_eq!(id3, 3);
    }

    #[test]
    fn ic_s_001_pending_refund_cap_drops_oldest() {
        let mut state = test_state();
        for i in 0..MAX_PENDING_REFUNDS {
            state.record_pending_refund(
                user_a(),
                icusd_ledger(),
                i as u64 + 1,
                "fail".to_string(),
                i as u64,
            );
        }
        assert_eq!(
            state.pending_refunds_for(&user_a()).len(),
            MAX_PENDING_REFUNDS
        );

        let id =
            state.record_pending_refund(user_a(), icusd_ledger(), 999, "fail".to_string(), 999);
        assert_eq!(id as usize, MAX_PENDING_REFUNDS);
        assert_eq!(
            state.pending_refunds_for(&user_a()).len(),
            MAX_PENDING_REFUNDS,
            "cap must hold",
        );
        assert!(
            state.take_pending_refund(0).is_none(),
            "oldest record dropped at cap"
        );
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

        let decoded = try_decode_state(&bytes).expect("v1 snapshot must decode");
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

        state.process_liquidation_gains_at(
            1,
            icp_ledger(),
            &draw,
            10_00000000,
            7_50000000,
            1_000_000_000,
        );

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
}
