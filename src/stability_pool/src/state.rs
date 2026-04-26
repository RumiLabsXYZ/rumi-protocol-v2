use std::collections::{BTreeMap, BTreeSet};
use std::cell::RefCell;
use candid::{CandidType, Principal, Decode, Encode};
use ic_canister_log::log;
use serde::{Serialize, Deserialize};

use crate::types::*;
use crate::logs::INFO;

/// Maximum number of liquidation records retained in memory.
/// Older entries are dropped when this limit is exceeded.
const MAX_LIQUIDATION_HISTORY: usize = 1_000;

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct StabilityPoolState {
    // Depositor positions
    pub deposits: BTreeMap<Principal, DepositPosition>,

    // Aggregate stablecoin balances per token
    pub total_stablecoin_balances: BTreeMap<Principal, u64>,

    // Registries
    pub stablecoin_registry: BTreeMap<Principal, StablecoinConfig>,
    pub collateral_registry: BTreeMap<Principal, CollateralInfo>,

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
}

impl Default for StabilityPoolState {
    fn default() -> Self {
        Self {
            deposits: BTreeMap::new(),
            total_stablecoin_balances: BTreeMap::new(),
            stablecoin_registry: BTreeMap::new(),
            collateral_registry: BTreeMap::new(),
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
        }
    }
}

/// Maximum pool events retained in memory.
const MAX_POOL_EVENTS: usize = 10_000;

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
        self.pool_events.as_ref().map(|v| v.len() as u64).unwrap_or(0)
    }

    pub fn is_admin(&self, caller: &Principal) -> bool {
        self.configuration.authorized_admins.contains(caller)
    }

    // ─── Stablecoin Registry ───

    pub fn register_stablecoin(&mut self, config: StablecoinConfig) {
        self.total_stablecoin_balances.entry(config.ledger_id).or_insert(0);
        self.stablecoin_registry.insert(config.ledger_id, config);
    }

    pub fn get_stablecoin_config(&self, ledger: &Principal) -> Option<&StablecoinConfig> {
        self.stablecoin_registry.get(ledger)
    }

    pub fn is_accepted_stablecoin(&self, ledger: &Principal) -> bool {
        self.stablecoin_registry.get(ledger).map(|c| c.is_active).unwrap_or(false)
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

    // ─── Deposits ───

    pub fn add_deposit(&mut self, user: Principal, token_ledger: Principal, amount: u64) {
        let position = self.deposits.entry(user).or_insert_with(|| {
            DepositPosition::new(ic_cdk::api::time())
        });
        *position.stablecoin_balances.entry(token_ledger).or_insert(0) += amount;
        *self.total_stablecoin_balances.entry(token_ledger).or_insert(0) += amount;
    }

    /// Distribute interest revenue pro-rata to all depositors of a given stablecoin.
    /// Called by the backend after minting interest to the pool canister.
    ///
    /// Interest is distributed pro-rata based on each depositor's **total USD
    /// value** across all stablecoins (icUSD, 3USD, ckUSDT, etc.), not just
    /// their balance of the token being distributed. This ensures all depositors
    /// earn interest proportional to their total deposit, regardless of which
    /// stablecoin(s) they deposited.
    ///
    /// When `collateral_type` is provided, depositors who have opted out of that
    /// collateral are excluded from the distribution — they should not earn
    /// interest from vaults backed by collateral they've opted out of.
    pub fn distribute_interest_revenue(&mut self, token_ledger: Principal, amount: u64, collateral_type: Option<Principal>) {
        if amount == 0 {
            return;
        }

        let decimals = self.stablecoin_registry.get(&token_ledger)
            .map(|c| c.decimals)
            .unwrap_or(8);

        let vps = self.virtual_prices().clone();

        // Collect eligible (principal, total_usd_value_e8s) pairs — use total
        // deposit value across ALL stablecoins for share calculation.
        // Exclude depositors who opted out of the collateral type.
        let holders: Vec<(Principal, u64)> = self.deposits.iter()
            .filter_map(|(p, pos)| {
                let total_value = pos.total_usd_value(&self.stablecoin_registry, &vps);
                if total_value == 0 {
                    return None;
                }
                // If we know the collateral source, skip opted-out depositors
                if let Some(ct) = &collateral_type {
                    if !pos.is_opted_in(ct) {
                        return None;
                    }
                }
                Some((*p, total_value))
            })
            .collect();

        let eligible_total: u64 = holders.iter().map(|(_, b)| *b).sum();
        if eligible_total == 0 {
            return; // No eligible depositors — nothing to distribute
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
                    *pos.total_interest_earned_e8s.get_or_insert(0) += normalize_to_e8s(credit, decimals);
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
                    *pos.total_interest_earned_e8s.get_or_insert(0) += normalize_to_e8s(dust, decimals);
                }
            }
        }

        // Update aggregate totals
        *self.total_stablecoin_balances.entry(token_ledger).or_insert(0) += amount;
        *self.total_interest_received_e8s.get_or_insert(0) += normalize_to_e8s(amount, decimals);
    }

    pub fn process_withdrawal(&mut self, user: Principal, token_ledger: Principal, amount: u64) -> Result<(), StabilityPoolError> {
        let position = self.deposits.get_mut(&user)
            .ok_or(StabilityPoolError::NoPositionFound)?;

        let balance = position.stablecoin_balances.get(&token_ledger).copied().unwrap_or(0);
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
        let total = self.total_stablecoin_balances.get(&token_ledger).copied().unwrap_or(0);
        if total == 0 || burned_amount == 0 {
            return;
        }
        let actual_deduct = burned_amount.min(total);

        // Distribute proportionally across depositors
        let depositors: Vec<(Principal, u64)> = self.deposits.iter()
            .filter_map(|(p, pos)| {
                let bal = pos.stablecoin_balances.get(&token_ledger).copied().unwrap_or(0);
                if bal > 0 { Some((*p, bal)) } else { None }
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
            if let Some(largest_p) = depositors.iter()
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
        *self.total_stablecoin_balances.entry(token_ledger).or_insert(0) += amount;

        // Distribute proportionally across depositors who hold this token
        let holders: Vec<(Principal, u64)> = self.deposits.iter()
            .filter_map(|(p, pos)| {
                let bal = pos.stablecoin_balances.get(&token_ledger).copied().unwrap_or(0);
                if bal > 0 { Some((*p, bal)) } else { None }
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
        self.deposits.get(user)
            .map(|p| p.collateral_gains.clone())
            .unwrap_or_default()
    }

    pub fn mark_gains_claimed(&mut self, user: &Principal, collateral_ledger: &Principal, amount: u64) {
        if let Some(position) = self.deposits.get_mut(user) {
            if let Some(gains) = position.collateral_gains.get_mut(collateral_ledger) {
                *gains = gains.saturating_sub(amount);
                if *gains == 0 {
                    position.collateral_gains.remove(collateral_ledger);
                }
            }
            *position.total_claimed_gains.entry(*collateral_ledger).or_insert(0) += amount;
        }
    }

    // ─── Opt-in / Opt-out ───

    pub fn opt_out_collateral(&mut self, user: &Principal, collateral_type: Principal) -> Result<(), StabilityPoolError> {
        let position = self.deposits.get_mut(user)
            .ok_or(StabilityPoolError::NoPositionFound)?;
        if !position.opted_out_collateral.insert(collateral_type) {
            return Err(StabilityPoolError::AlreadyOptedOut { collateral: collateral_type });
        }
        Ok(())
    }

    pub fn opt_in_collateral(&mut self, user: &Principal, collateral_type: Principal) -> Result<(), StabilityPoolError> {
        let position = self.deposits.get_mut(user)
            .ok_or(StabilityPoolError::NoPositionFound)?;
        if !position.opted_out_collateral.remove(&collateral_type) {
            return Err(StabilityPoolError::AlreadyOptedIn { collateral: collateral_type });
        }
        Ok(())
    }

    // ─── Effective Pool Computation ───

    /// Compute total opted-in stablecoin value (e8s) for a given collateral type.
    pub fn effective_pool_for_collateral(&self, collateral_type: &Principal) -> u64 {
        let vps = self.virtual_prices();
        self.deposits.values()
            .filter(|pos| pos.is_opted_in(collateral_type))
            .map(|pos| pos.total_usd_value(&self.stablecoin_registry, vps))
            .sum()
    }

    // ─── Liquidation Processing ───

    /// Compute the stablecoin draw for a liquidation of a given debt amount (e8s).
    /// Returns a map of token_ledger -> amount to consume (in native decimals).
    ///
    /// For small debts (< 1 icUSD / 100_000_000 e8s), uses a single token — whichever
    /// has the highest balance — to avoid splitting into amounts too small for the backend.
    /// For larger debts, follows priority ordering with proportional splits.
    pub fn compute_token_draw(&self, debt_e8s: u64, collateral_type: &Principal) -> BTreeMap<Principal, u64> {
        let vps = self.virtual_prices();

        // Gather all available tokens with their e8s-equivalent balances
        // Tuple: (ledger, available_native, decimals, is_lp, available_e8s)
        let mut all_tokens: Vec<(Principal, u64, u8, bool, u64)> = Vec::new();

        for (ledger, config) in &self.stablecoin_registry {
            let available_native: u64 = self.deposits.values()
                .filter(|pos| pos.is_opted_in(collateral_type))
                .map(|pos| pos.stablecoin_balances.get(ledger).copied().unwrap_or(0))
                .sum();
            if available_native > 0 {
                let is_lp = config.is_lp_token.unwrap_or(false);
                let available_e8s = if is_lp {
                    vps.get(ledger).map(|&vp| lp_to_usd_e8s(available_native, vp)).unwrap_or(0)
                } else {
                    normalize_to_e8s(available_native, config.decimals)
                };
                if available_e8s > 0 {
                    all_tokens.push((*ledger, available_native, config.decimals, is_lp, available_e8s));
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
            let best = all_tokens.iter()
                .max_by_key(|(_, _, _, _, e8s)| *e8s)
                .unwrap(); // safe: all_tokens is non-empty

            let (ledger, available_native, decimals, is_lp, available_e8s) = *best;
            let draw_e8s = debt_e8s.min(available_e8s);
            let draw_native = if is_lp {
                vps.get(&ledger).map(|&vp| usd_e8s_to_lp(draw_e8s, vp)).unwrap_or(0)
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
            let available_native: u64 = self.deposits.values()
                .filter(|pos| pos.is_opted_in(collateral_type))
                .map(|pos| pos.stablecoin_balances.get(ledger).copied().unwrap_or(0))
                .sum();
            if available_native > 0 {
                let is_lp = config.is_lp_token.unwrap_or(false);
                priority_buckets.entry(config.priority).or_default()
                    .push((*ledger, available_native, config.decimals, is_lp));
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

            let total_available_e8s: u64 = tokens.iter()
                .map(|(ledger, amount, decimals, is_lp)| {
                    if *is_lp {
                        vps.get(ledger).map(|&vp| lp_to_usd_e8s(*amount, vp)).unwrap_or(0)
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
                    vps.get(ledger).map(|&vp| lp_to_usd_e8s(*available_native, vp)).unwrap_or(0)
                } else {
                    normalize_to_e8s(*available_native, *decimals)
                };
                if token_available_e8s == 0 {
                    continue;
                }
                let token_draw_e8s = (draw_e8s as u128 * token_available_e8s as u128 / total_available_e8s as u128) as u64;
                let token_draw_native = if *is_lp {
                    vps.get(ledger).map(|&vp| usd_e8s_to_lp(token_draw_e8s, vp)).unwrap_or(0)
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
        self.process_liquidation_gains_at(vault_id, collateral_type, stables_consumed, collateral_gained, collateral_price_e8s, ic_cdk::api::time());
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
        let opted_in_principals: Vec<Principal> = self.deposits.iter()
            .filter(|(_, pos)| pos.is_opted_in(&collateral_type))
            .map(|(p, _)| *p)
            .collect();

        // For each consumed token, compute total opted-in balance for that token
        let mut per_token_opted_in_totals: BTreeMap<Principal, u64> = BTreeMap::new();
        for token_ledger in stables_consumed.keys() {
            let total: u64 = opted_in_principals.iter()
                .filter_map(|p| self.deposits.get(p))
                .map(|pos| pos.stablecoin_balances.get(token_ledger).copied().unwrap_or(0))
                .sum();
            per_token_opted_in_totals.insert(*token_ledger, total);
        }

        // Phase 2: Compute total e8s consumed to determine collateral distribution shares.
        // LP tokens are valued at virtual price, not face value.
        // Clone virtual prices and registry info upfront to avoid borrow conflicts with Phase 3.
        let vps = self.virtual_prices().clone();
        let registry_snapshot: BTreeMap<Principal, (u8, bool)> = stables_consumed.keys()
            .filter_map(|ledger| {
                self.stablecoin_registry.get(ledger).map(|c| {
                    (*ledger, (c.decimals, c.is_lp_token.unwrap_or(false)))
                })
            })
            .collect();
        let total_consumed_e8s: u64 = stables_consumed.iter()
            .map(|(ledger, &amount)| {
                let (decimals, is_lp) = registry_snapshot.get(ledger).copied().unwrap_or((8, false));
                if is_lp {
                    vps.get(ledger).map(|&vp| lp_to_usd_e8s(amount, vp)).unwrap_or(0)
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
                    let total_opted_in = per_token_opted_in_totals.get(token_ledger).copied().unwrap_or(0);
                    if total_opted_in == 0 {
                        continue;
                    }
                    let user_balance = position.stablecoin_balances.get(token_ledger).copied().unwrap_or(0);
                    if user_balance == 0 {
                        continue;
                    }

                    // User's share of this token's consumption
                    let user_share_native = (total_consumed as u128 * user_balance as u128 / total_opted_in as u128) as u64;
                    let user_share_native = user_share_native.min(user_balance);

                    // Reduce balance
                    if let Some(bal) = position.stablecoin_balances.get_mut(token_ledger) {
                        *bal = bal.saturating_sub(user_share_native);
                    }

                    // Track actual deduction for aggregate update
                    *actual_deductions_per_token.entry(*token_ledger).or_insert(0) += user_share_native;

                    // Track consumed value in e8s for collateral distribution.
                    // LP tokens valued at virtual price, not face value.
                    let (decimals, is_lp) = registry_snapshot.get(token_ledger).copied().unwrap_or((8, false));
                    let share_e8s = if is_lp {
                        vps.get(token_ledger).map(|&vp| lp_to_usd_e8s(user_share_native, vp)).unwrap_or(0)
                    } else {
                        normalize_to_e8s(user_share_native, decimals)
                    };
                    user_consumed_e8s += share_e8s;
                }

                // Distribute collateral proportional to e8s consumed
                if user_consumed_e8s > 0 {
                    let user_collateral = (collateral_gained as u128 * user_consumed_e8s as u128 / total_consumed_e8s as u128) as u64;
                    *position.collateral_gains.entry(collateral_type).or_insert(0) += user_collateral;
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

    // ─── Query Helpers ───

    pub fn get_pool_status(&self) -> StabilityPoolStatus {
        let vps = self.virtual_prices();
        let total_e8s: u64 = self.total_stablecoin_balances.iter()
            .map(|(ledger, &amount)| {
                let config = self.stablecoin_registry.get(ledger);
                if config.map(|c| c.is_lp_token.unwrap_or(false)).unwrap_or(false) {
                    vps.get(ledger).map(|&vp| lp_to_usd_e8s(amount, vp)).unwrap_or(0)
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
        self.collateral_registry.keys().map(|ct| {
            let eligible: u64 = self.deposits.values()
                .filter(|pos| pos.is_opted_in(ct))
                .map(|pos| pos.total_usd_value(&self.stablecoin_registry, vps))
                .sum();
            (*ct, eligible)
        }).collect()
    }

    pub fn get_user_position(&self, user: &Principal) -> Option<UserStabilityPosition> {
        self.deposits.get(user).map(|pos| UserStabilityPosition {
            stablecoin_balances: pos.stablecoin_balances.clone(),
            collateral_gains: pos.collateral_gains.clone(),
            opted_out_collateral: pos.opted_out_collateral.iter().cloned().collect(),
            deposit_timestamp: pos.deposit_timestamp,
            total_claimed_gains: pos.total_claimed_gains.clone(),
            total_usd_value_e8s: pos.total_usd_value(&self.stablecoin_registry, self.virtual_prices()),
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
    pub fn correct_balance(&mut self, user: Principal, token_ledger: Principal, correct_amount: u64) -> String {
        let old_amount = self.deposits.get(&user)
            .and_then(|pos| pos.stablecoin_balances.get(&token_ledger).copied())
            .unwrap_or(0);

        if old_amount == correct_amount {
            return format!("No change needed: user {} balance for {} is already {}", user, token_ledger, correct_amount);
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

        format!("Corrected {} balance for {}: {} -> {}", token_ledger, user, old_amount, correct_amount)
    }

    /// Set a depositor's collateral gain for a specific collateral type to `correct_amount`.
    /// Used to fix drift between tracked gains and actual ledger balance (e.g., transfer fee dust).
    pub fn correct_collateral_gain(&mut self, user: Principal, collateral_ledger: Principal, correct_amount: u64) -> String {
        let old_amount = self.deposits.get(&user)
            .and_then(|pos| pos.collateral_gains.get(&collateral_ledger).copied())
            .unwrap_or(0);

        if old_amount == correct_amount {
            return format!("No change needed: user {} gain for {} is already {}", user, collateral_ledger, correct_amount);
        }

        if let Some(pos) = self.deposits.get_mut(&user) {
            if correct_amount == 0 {
                pos.collateral_gains.remove(&collateral_ledger);
            } else {
                pos.collateral_gains.insert(collateral_ledger, correct_amount);
            }
        }

        format!("Corrected {} collateral gain for {}: {} -> {}", collateral_ledger, user, old_amount, correct_amount)
    }

    // ─── State Validation ───

    pub fn validate_state(&self) -> Result<(), String> {
        for (ledger, &tracked_total) in &self.total_stablecoin_balances {
            let computed_total: u64 = self.deposits.values()
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
where F: FnOnce(&mut StabilityPoolState) -> R {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

pub fn read_state<F, R>(f: F) -> R
where F: FnOnce(&StabilityPoolState) -> R {
    STATE.with(|s| f(&s.borrow()))
}

pub fn replace_state(state: StabilityPoolState) {
    STATE.with(|s| { *s.borrow_mut() = state; });
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
    // Future: insert prior schema versions here, e.g.
    //     if let Ok(prev) = Decode!(bytes, StabilityPoolStateVN) {
    //         return Some(prev.into());
    //     }
    None
}

/// Restore state from stable memory (called from post_upgrade).
///
/// UPG-001 fix: rather than trapping on decode failure (which bricks the
/// canister until a hotfix wasm with a compatible decoder is shipped), walk
/// the known-version fallback chain via `try_decode_state`. If every known
/// version fails, log a CRITICAL diagnostic with the snapshot length and a
/// short hex preview, then fall back to empty state. The empty fallback is a
/// last resort: it zeroes depositor positions. Operators should treat it as a
/// recoverable incident (rebuild from event history if possible) rather than
/// a routine outcome.
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
        "CRITICAL UPG-001: stability pool snapshot decode failed for all known schema versions. \
         snapshot_len={} bytes, first_{}_bytes_hex={}. \
         Falling back to empty state. Depositor positions are reset; operator must \
         restore from event history or admin endpoints.",
        bytes.len(),
        preview_len,
        preview_hex
    );
    replace_state(StabilityPoolState::default());
}

// ──────────────────────────────────────────────────────────────
// Unit tests
// ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    // Deterministic test principals
    fn user_a() -> Principal { Principal::from_slice(&[1]) }
    fn user_b() -> Principal { Principal::from_slice(&[2]) }
    fn user_c() -> Principal { Principal::from_slice(&[3]) }
    fn icusd_ledger() -> Principal { Principal::from_slice(&[10]) }
    fn ckusdt_ledger() -> Principal { Principal::from_slice(&[11]) }
    fn ckusdc_ledger() -> Principal { Principal::from_slice(&[12]) }
    fn icp_ledger() -> Principal { Principal::from_slice(&[20]) }
    fn ckbtc_ledger() -> Principal { Principal::from_slice(&[21]) }

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

        state
    }

    /// Helper: directly add a deposit without ic_cdk::api::time().
    fn add_deposit_direct(
        state: &mut StabilityPoolState,
        user: Principal,
        token: Principal,
        amount: u64,
    ) {
        let position = state.deposits.entry(user).or_insert_with(|| DepositPosition::new(0));
        *position.stablecoin_balances.entry(token).or_insert(0) += amount;
        *state.total_stablecoin_balances.entry(token).or_insert(0) += amount;
    }

    // ─── Test: Deposit and Withdrawal ───

    #[test]
    fn test_deposit_and_withdrawal() {
        let mut state = test_state();

        // Add deposits for user_a
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_000_000); // 1 icUSD
        add_deposit_direct(&mut state, user_a(), ckusdt_ledger(), 2_000_000);  // 2 ckUSDT

        // Verify balances
        let pos = state.deposits.get(&user_a()).unwrap();
        assert_eq!(pos.stablecoin_balances.get(&icusd_ledger()), Some(&100_000_000));
        assert_eq!(pos.stablecoin_balances.get(&ckusdt_ledger()), Some(&2_000_000));

        // Verify aggregate totals
        assert_eq!(state.total_stablecoin_balances.get(&icusd_ledger()), Some(&100_000_000));
        assert_eq!(state.total_stablecoin_balances.get(&ckusdt_ledger()), Some(&2_000_000));

        // Partial withdrawal
        state.process_withdrawal(user_a(), icusd_ledger(), 30_000_000).unwrap();
        let pos = state.deposits.get(&user_a()).unwrap();
        assert_eq!(pos.stablecoin_balances.get(&icusd_ledger()), Some(&70_000_000));
        assert_eq!(state.total_stablecoin_balances.get(&icusd_ledger()), Some(&70_000_000));

        // Full withdrawal of ckUSDT -- zero-balance entry should be cleaned up
        state.process_withdrawal(user_a(), ckusdt_ledger(), 2_000_000).unwrap();
        let pos = state.deposits.get(&user_a()).unwrap();
        assert_eq!(pos.stablecoin_balances.get(&ckusdt_ledger()), None);

        // Full withdrawal of remaining icUSD -- empty position should be removed
        state.process_withdrawal(user_a(), icusd_ledger(), 70_000_000).unwrap();
        assert!(state.deposits.get(&user_a()).is_none(), "Empty position should be removed");

        // Attempt to withdraw from nonexistent position
        let err = state.process_withdrawal(user_a(), icusd_ledger(), 1).unwrap_err();
        assert!(matches!(err, StabilityPoolError::NoPositionFound));
    }

    #[test]
    fn test_withdrawal_insufficient_balance() {
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 50_000_000);

        let err = state.process_withdrawal(user_a(), icusd_ledger(), 100_000_000).unwrap_err();
        match err {
            StabilityPoolError::InsufficientBalance { token, required, available } => {
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

        assert_eq!(usdt_draw, 80_000_000, "All 80 USD should come from ckUSDT (priority 2)");
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
            5_00000000, // 5 ICP
            7_50000000, // collateral price $7.50
            1_000_000_000, // timestamp
        );

        // Check proportional reduction of icUSD balances:
        // user_a consumed: 10 * (50/100) = 5 icUSD -> remaining: 45
        // user_b consumed: 10 * (30/100) = 3 icUSD -> remaining: 27
        // user_c consumed: 10 * (20/100) = 2 icUSD -> remaining: 18
        let pos_a = state.deposits.get(&user_a()).unwrap();
        let pos_b = state.deposits.get(&user_b()).unwrap();
        let pos_c = state.deposits.get(&user_c()).unwrap();

        assert_eq!(pos_a.stablecoin_balances.get(&icusd_ledger()).copied().unwrap_or(0), 45_00000000);
        assert_eq!(pos_b.stablecoin_balances.get(&icusd_ledger()).copied().unwrap_or(0), 27_00000000);
        assert_eq!(pos_c.stablecoin_balances.get(&icusd_ledger()).copied().unwrap_or(0), 18_00000000);

        // Check proportional collateral gains:
        // user_a gain: 5 ICP * (5/10) = 2.5 ICP = 2_50000000
        // user_b gain: 5 ICP * (3/10) = 1.5 ICP = 1_50000000
        // user_c gain: 5 ICP * (2/10) = 1.0 ICP = 1_00000000
        assert_eq!(pos_a.collateral_gains.get(&icp_ledger()).copied().unwrap_or(0), 2_50000000);
        assert_eq!(pos_b.collateral_gains.get(&icp_ledger()).copied().unwrap_or(0), 1_50000000);
        assert_eq!(pos_c.collateral_gains.get(&icp_ledger()).copied().unwrap_or(0), 1_00000000);

        // Verify aggregate total was reduced
        assert_eq!(
            state.total_stablecoin_balances.get(&icusd_ledger()).copied().unwrap_or(0),
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
        assert_eq!(effective, 60_00000000, "Only user_a's 60 icUSD should be in effective pool");

        // Effective pool for ckBTC should include both (user_b only opted out of ICP)
        let effective_btc = state.effective_pool_for_collateral(&ckbtc_ledger());
        assert_eq!(effective_btc, 100_00000000, "Both users should be in ckBTC pool");

        // Token draw for ICP should only draw from user_a's balance
        let draw = state.compute_token_draw(30_00000000, &icp_ledger());
        assert_eq!(draw.get(&icusd_ledger()).copied().unwrap_or(0), 30_00000000);

        // Liquidation gains: only user_a participates for ICP
        let mut stables_consumed = BTreeMap::new();
        stables_consumed.insert(icusd_ledger(), 20_00000000);

        state.process_liquidation_gains_at(
            2, icp_ledger(), &stables_consumed, 10_00000000, 7_50000000, 2_000_000_000,
        );

        // user_a should lose all 20 icUSD (only opted-in depositor)
        let pos_a = state.deposits.get(&user_a()).unwrap();
        assert_eq!(pos_a.stablecoin_balances.get(&icusd_ledger()).copied().unwrap_or(0), 40_00000000);
        assert_eq!(pos_a.collateral_gains.get(&icp_ledger()).copied().unwrap_or(0), 10_00000000);

        // user_b should be completely untouched
        let pos_b = state.deposits.get(&user_b()).unwrap();
        assert_eq!(pos_b.stablecoin_balances.get(&icusd_ledger()).copied().unwrap_or(0), 40_00000000);
        assert_eq!(pos_b.collateral_gains.get(&icp_ledger()).copied().unwrap_or(0), 0);
    }

    #[test]
    fn test_opt_out_duplicate_errors() {
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 10_00000000);

        // First opt-out succeeds
        state.opt_out_collateral(&user_a(), icp_ledger()).unwrap();

        // Second opt-out of same collateral should fail
        let err = state.opt_out_collateral(&user_a(), icp_ledger()).unwrap_err();
        assert!(matches!(err, StabilityPoolError::AlreadyOptedOut { .. }));

        // Opt back in
        state.opt_in_collateral(&user_a(), icp_ledger()).unwrap();

        // Double opt-in should fail
        let err = state.opt_in_collateral(&user_a(), icp_ledger()).unwrap_err();
        assert!(matches!(err, StabilityPoolError::AlreadyOptedIn { .. }));
    }

    #[test]
    fn test_opt_no_position_errors() {
        let mut state = test_state();

        // Opt-out on nonexistent position
        let err = state.opt_out_collateral(&user_a(), icp_ledger()).unwrap_err();
        assert!(matches!(err, StabilityPoolError::NoPositionFound));

        // Opt-in on nonexistent position
        let err = state.opt_in_collateral(&user_a(), icp_ledger()).unwrap_err();
        assert!(matches!(err, StabilityPoolError::NoPositionFound));
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
        assert_eq!(normalize_from_e8s(normalize_to_e8s(original, 6), 6), original);

        // Round-trip for 8-decimal
        let original = 98_765_432u64;
        assert_eq!(normalize_from_e8s(normalize_to_e8s(original, 8), 8), original);

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
        *state.total_stablecoin_balances.get_mut(&icusd_ledger()).unwrap() = 999;
        let err = state.validate_state();
        assert!(err.is_err());
        let msg = err.unwrap_err();
        assert!(msg.contains("mismatch"), "Error should mention mismatch: {}", msg);
        assert!(msg.contains("tracked=999"), "Error should show tracked value: {}", msg);

        // Fix the corruption
        *state.total_stablecoin_balances.get_mut(&icusd_ledger()).unwrap() = 150_00000000;
        assert!(state.validate_state().is_ok());
    }

    #[test]
    fn test_state_validation_empty_state() {
        let state = test_state();
        // Empty state with zero totals should pass
        assert!(state.validate_state().is_ok());
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
    }

    // ─── Test: Mark gains claimed ───

    #[test]
    fn test_mark_gains_claimed() {
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);

        // Manually add collateral gains
        state.deposits.get_mut(&user_a()).unwrap()
            .collateral_gains.insert(icp_ledger(), 5_00000000);

        // Partially claim
        state.mark_gains_claimed(&user_a(), &icp_ledger(), 2_00000000);
        let pos = state.deposits.get(&user_a()).unwrap();
        assert_eq!(pos.collateral_gains.get(&icp_ledger()).copied().unwrap_or(0), 3_00000000);
        assert_eq!(pos.total_claimed_gains.get(&icp_ledger()).copied().unwrap_or(0), 2_00000000);

        // Claim the rest
        state.mark_gains_claimed(&user_a(), &icp_ledger(), 3_00000000);
        let pos = state.deposits.get(&user_a()).unwrap();
        assert_eq!(pos.collateral_gains.get(&icp_ledger()), None, "Zero gains should be cleaned up");
        assert_eq!(pos.total_claimed_gains.get(&icp_ledger()).copied().unwrap_or(0), 5_00000000);
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
        assert_eq!(state.effective_pool_for_collateral(&icp_ledger()), 100_00000000);

        // user_a opts out of ICP -> only user_b remains
        state.opt_out_collateral(&user_a(), icp_ledger()).unwrap();
        assert_eq!(state.effective_pool_for_collateral(&icp_ledger()), 30_00000000);

        // ckBTC effective pool still has everyone
        assert_eq!(state.effective_pool_for_collateral(&ckbtc_ledger()), 100_00000000);
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
            10, icp_ledger(), &stables_consumed, 20_00000000, 7_50000000, 3_000_000_000,
        );

        // user_a has all the ckUSDT, so consumes all 20 ckUSDT
        let pos_a = state.deposits.get(&user_a()).unwrap();
        assert_eq!(pos_a.stablecoin_balances.get(&ckusdt_ledger()).copied().unwrap_or(0), 30_000_000); // 50 - 20
        // user_a's icUSD should be untouched (not consumed)
        assert_eq!(pos_a.stablecoin_balances.get(&icusd_ledger()).copied().unwrap_or(0), 100_00000000);

        // user_b has all the ckUSDC, so consumes all 20 ckUSDC
        let pos_b = state.deposits.get(&user_b()).unwrap();
        assert_eq!(pos_b.stablecoin_balances.get(&ckusdc_ledger()).copied().unwrap_or(0), 30_000_000); // 50 - 20

        // Collateral distribution: each consumed 20 USD worth out of 40 total = 50% each
        // user_a: 50% of 20 ICP = 10 ICP
        // user_b: 50% of 20 ICP = 10 ICP
        assert_eq!(pos_a.collateral_gains.get(&icp_ledger()).copied().unwrap_or(0), 10_00000000);
        assert_eq!(pos_b.collateral_gains.get(&icp_ledger()).copied().unwrap_or(0), 10_00000000);
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
            5, icp_ledger(), &stables_consumed, 50_00000000, 7_50000000, 4_000_000_000,
        );

        // user_a's stablecoin balance is zero, but they have collateral gains
        // so position should NOT be removed
        let pos = state.deposits.get(&user_a());
        assert!(pos.is_some(), "Position with collateral gains should not be cleaned up");
        let pos = pos.unwrap();
        assert_eq!(pos.stablecoin_balances.get(&icusd_ledger()).copied().unwrap_or(0), 0);
        assert_eq!(pos.collateral_gains.get(&icp_ledger()).copied().unwrap_or(0), 50_00000000);
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
        assert_eq!(state.total_stablecoin_balances.get(&icusd_ledger()), Some(&0));
        assert_eq!(state.total_stablecoin_balances.get(&ckusdt_ledger()), Some(&0));
        assert_eq!(state.total_stablecoin_balances.get(&ckusdc_ledger()), Some(&0));
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

        let pos = state.get_user_position(&user_a()).unwrap();
        assert_eq!(pos.total_usd_value_e8s, 125_00000000); // 100 + 25
        assert_eq!(pos.stablecoin_balances.get(&icusd_ledger()), Some(&100_00000000));
        assert_eq!(pos.stablecoin_balances.get(&ckusdt_ledger()), Some(&25_000_000));

        // Nonexistent user
        assert!(state.get_user_position(&user_b()).is_none());
    }

    // ─── Test: Multiple deposits accumulate ───

    #[test]
    fn test_multiple_deposits_accumulate() {
        let mut state = test_state();

        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 50_00000000);
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 30_00000000);
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 20_00000000);

        let pos = state.deposits.get(&user_a()).unwrap();
        assert_eq!(pos.stablecoin_balances.get(&icusd_ledger()), Some(&100_00000000));
        assert_eq!(state.total_stablecoin_balances.get(&icusd_ledger()), Some(&100_00000000));
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
        assert_eq!(state.total_stablecoin_balances[&icusd_ledger()], 105_00000000);
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
        assert_eq!(state.total_stablecoin_balances[&icusd_ledger()], 110_00000000);
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
        add_deposit_direct(&mut state, Principal::from_slice(&[99]), icusd_ledger(), 100);

        state.distribute_interest_revenue(icusd_ledger(), 10, None);

        // Each gets floor(10 * 100/300) = 3. Dust = 10 - 9 = 1 goes to first depositor.
        let total: u64 = state.deposits.values()
            .map(|p| p.stablecoin_balances.get(&icusd_ledger()).copied().unwrap_or(0))
            .sum();
        assert_eq!(total, 310, "All interest must be accounted for");
        assert_eq!(state.total_stablecoin_balances[&icusd_ledger()], 310);
    }

    #[test]
    fn test_distribute_interest_cross_stablecoin() {
        // A ckUSDT-only depositor should earn interest proportional to their total value
        let mut state = test_state(); // Already has ckUSDT registered (6 decimals, priority 2)

        // A deposits 50 icUSD, B deposits 50 ckUSDT (both worth $50)
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 50_00000000);
        add_deposit_direct(&mut state, user_b(), ckusdt_ledger(), 50_000_000); // 50 * 10^6

        // Distribute 10 icUSD interest
        state.distribute_interest_revenue(icusd_ledger(), 10_00000000, None);

        let a = state.deposits.get(&user_a()).unwrap();
        let b = state.deposits.get(&user_b()).unwrap();
        // Both have equal $50 deposits, so each gets 5 icUSD
        assert_eq!(a.stablecoin_balances[&icusd_ledger()], 55_00000000, "A: 50 + 5 icUSD");
        assert_eq!(b.stablecoin_balances[&icusd_ledger()], 5_00000000, "B: 0 + 5 icUSD (newly created)");
        assert_eq!(b.stablecoin_balances[&ckusdt_ledger()], 50_000_000, "B: ckUSDT unchanged");
        assert_eq!(state.total_stablecoin_balances[&icusd_ledger()], 60_00000000);
    }

    #[test]
    fn test_distribute_interest_3usd_lp_depositor() {
        // 3USD LP depositor should earn interest based on virtual-price-adjusted value
        let mut state = test_state_with_3usd();

        // A deposits 100 icUSD ($100), B deposits 100 3USD (worth ~$104.92 at vp=1.0492)
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        add_deposit_direct(&mut state, user_b(), three_usd_ledger(), 100_00000000);

        // Total value: A=$100 + B=~$104.92 = ~$204.92
        // Distribute 20 icUSD interest
        state.distribute_interest_revenue(icusd_ledger(), 20_00000000, None);

        let a = state.deposits.get(&user_a()).unwrap();
        let b = state.deposits.get(&user_b()).unwrap();
        // A's share: 20 * 100_00000000 / 204_92000000 ≈ 9.76 icUSD
        // B's share: 20 * 104_92000000 / 204_92000000 ≈ 10.24 icUSD
        let a_interest = a.stablecoin_balances[&icusd_ledger()] - 100_00000000;
        let b_interest = b.stablecoin_balances.get(&icusd_ledger()).copied().unwrap_or(0);
        // Total interest must equal 20 icUSD
        assert_eq!(a_interest + b_interest, 20_00000000, "All interest accounted for");
        // B (3USD depositor) should have earned interest
        assert!(b_interest > 0, "3USD depositor must earn interest");
        // B should get slightly more than A since 3USD is worth more at vp > 1.0
        assert!(b_interest > a_interest, "3USD depositor earns more due to higher value");
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
            1, icp_ledger(), &consumed, 500_000, 7_50000000, 1_000_000_000,
        );

        // The critical assertion: aggregate should match sum of individual balances
        // even when rounding dust occurs. validate_state() checks this.
        assert!(state.validate_state().is_ok(), "State must remain consistent after rounding");

        // Verify individual balances sum to aggregate
        let sum: u64 = state.deposits.values()
            .map(|p| p.stablecoin_balances.get(&icusd_ledger()).copied().unwrap_or(0))
            .sum();
        let tracked = state.total_stablecoin_balances.get(&icusd_ledger()).copied().unwrap_or(0);
        assert_eq!(sum, tracked, "Sum of individual balances must equal aggregate");
    }

    // ─── 3USD / LP Token Tests ───

    fn three_usd_ledger() -> Principal { Principal::from_slice(&[30]) }
    fn three_pool_canister() -> Principal { Principal::from_slice(&[31]) }

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
        state.cached_virtual_prices
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
        assert_eq!(lp_to_usd_e8s(100_000_000, 1_000_000_000_000_000_000), 100_000_000);
    }

    #[test]
    fn test_usd_e8s_to_lp_conversion() {
        let vp = 1_049_200_000_000_000_000u128;
        // 1 USD → ~0.9531 3USD LP
        let lp = usd_e8s_to_lp(100_000_000, vp);
        assert!(lp > 95_000_000 && lp < 96_000_000, "Expected ~95.3M, got {}", lp);

        // Round-trip: lp_to_usd then back (may lose 1 unit to rounding)
        let usd = lp_to_usd_e8s(lp, vp);
        assert!((usd as i64 - 100_000_000i64).abs() <= 1, "Round-trip drift too large");

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
        pos.stablecoin_balances.insert(three_usd_ledger(), 100_000_000);

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
        pos.stablecoin_balances.insert(three_usd_ledger(), 100_000_000);

        // Without virtual price, LP tokens valued at 0
        let total = pos.total_usd_value(&state.stablecoin_registry, state.virtual_prices());
        assert_eq!(total, 0);
    }

    #[test]
    fn test_compute_token_draw_with_3usd() {
        let mut state = test_state_with_3usd();

        // Add deposits: 1 icUSD (priority 1), 2 ckUSDT (priority 2), 5 3USD (priority 0)
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_000_000); // 1 icUSD
        add_deposit_direct(&mut state, user_a(), ckusdt_ledger(), 2_000_000);  // 2 ckUSDT
        add_deposit_direct(&mut state, user_a(), three_usd_ledger(), 500_000_000); // 5 3USD

        // Draw 3 USD — should consume ckUSDT first (priority 2), then icUSD (priority 1)
        let draw = state.compute_token_draw(300_000_000, &icp_ledger()); // 3 USD e8s
        assert!(draw.contains_key(&ckusdt_ledger()), "Should draw from ckUSDT (priority 2)");
        assert!(draw.contains_key(&icusd_ledger()), "Should draw from icUSD (priority 1)");
        assert!(!draw.contains_key(&three_usd_ledger()), "Should NOT draw from 3USD yet (priority 0)");

        // Draw 5 USD — should consume all ckUSDT + icUSD, then dip into 3USD
        let draw = state.compute_token_draw(500_000_000, &icp_ledger()); // 5 USD e8s
        assert!(draw.contains_key(&ckusdt_ledger()), "Should draw from ckUSDT");
        assert!(draw.contains_key(&icusd_ledger()), "Should draw from icUSD");
        assert!(draw.contains_key(&three_usd_ledger()), "Should draw from 3USD for remainder");
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
}
