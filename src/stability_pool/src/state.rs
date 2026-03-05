use std::collections::{BTreeMap, BTreeSet};
use std::cell::RefCell;
use candid::{CandidType, Principal, Decode, Encode};
use serde::{Serialize, Deserialize};

use crate::types::*;

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
    pub is_initialized: bool,
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
            is_initialized: false,
        }
    }
}

impl StabilityPoolState {
    pub fn initialize(&mut self, args: StabilityPoolInitArgs) {
        self.protocol_canister_id = args.protocol_canister_id;
        self.configuration.authorized_admins = args.authorized_admins;
        self.pool_creation_timestamp = ic_cdk::api::time();
        self.is_initialized = true;
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

        *position.stablecoin_balances.get_mut(&token_ledger).unwrap() -= amount;
        *self.total_stablecoin_balances.get_mut(&token_ledger).unwrap() -= amount;

        // Clean up zero balances
        if position.stablecoin_balances.get(&token_ledger) == Some(&0) {
            position.stablecoin_balances.remove(&token_ledger);
        }
        if position.is_empty() {
            self.deposits.remove(&user);
        }
        Ok(())
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
        self.deposits.values()
            .filter(|pos| pos.is_opted_in(collateral_type))
            .map(|pos| pos.total_usd_value(&self.stablecoin_registry))
            .sum()
    }

    // ─── Liquidation Processing ───

    /// Compute the stablecoin draw for a liquidation of a given debt amount (e8s).
    /// Returns a map of token_ledger -> amount to consume (in native decimals).
    /// Follows priority ordering: higher priority consumed first, same priority proportional.
    pub fn compute_token_draw(&self, debt_e8s: u64, collateral_type: &Principal) -> BTreeMap<Principal, u64> {
        let mut result = BTreeMap::new();
        let mut remaining_e8s = debt_e8s;

        // Gather available balances per priority, only from opted-in depositors
        let mut priority_buckets: BTreeMap<u8, Vec<(Principal, u64, u8)>> = BTreeMap::new(); // priority -> [(ledger, available_native, decimals)]

        for (ledger, config) in &self.stablecoin_registry {
            // Sum balances of opted-in depositors for this token
            let available_native: u64 = self.deposits.values()
                .filter(|pos| pos.is_opted_in(collateral_type))
                .map(|pos| pos.stablecoin_balances.get(ledger).copied().unwrap_or(0))
                .sum();
            if available_native > 0 {
                priority_buckets.entry(config.priority).or_default()
                    .push((*ledger, available_native, config.decimals));
            }
        }

        // Process from highest priority to lowest
        let mut priorities: Vec<u8> = priority_buckets.keys().copied().collect();
        priorities.sort_by(|a, b| b.cmp(a)); // descending

        for priority in priorities {
            if remaining_e8s == 0 {
                break;
            }
            let tokens = priority_buckets.get(&priority).unwrap();

            // Total available at this priority level (in e8s)
            let total_available_e8s: u64 = tokens.iter()
                .map(|(_, amount, decimals)| normalize_to_e8s(*amount, *decimals))
                .sum();

            if total_available_e8s == 0 {
                continue;
            }

            // How much to draw from this priority tier
            let draw_e8s = remaining_e8s.min(total_available_e8s);

            // Proportional draw within this tier
            for (ledger, available_native, decimals) in tokens {
                let token_available_e8s = normalize_to_e8s(*available_native, *decimals);
                if token_available_e8s == 0 {
                    continue;
                }
                // Proportional share: (token_available / total_available) * draw
                let token_draw_e8s = (draw_e8s as u128 * token_available_e8s as u128 / total_available_e8s as u128) as u64;
                let token_draw_native = normalize_from_e8s(token_draw_e8s, *decimals);
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

        // Phase 2: Compute total e8s consumed to determine collateral distribution shares
        let total_consumed_e8s: u64 = stables_consumed.iter()
            .map(|(ledger, &amount)| {
                let decimals = self.stablecoin_registry.get(ledger).map(|c| c.decimals).unwrap_or(8);
                normalize_to_e8s(amount, decimals)
            })
            .sum();

        if total_consumed_e8s == 0 {
            return;
        }

        // Phase 3: For each opted-in depositor, reduce their token balances and add collateral gains
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

                    // Track consumed value in e8s for collateral distribution
                    let decimals = self.stablecoin_registry.get(token_ledger).map(|c| c.decimals).unwrap_or(8);
                    user_consumed_e8s += normalize_to_e8s(user_share_native, decimals);
                }

                // Distribute collateral proportional to e8s consumed
                if user_consumed_e8s > 0 {
                    let user_collateral = (collateral_gained as u128 * user_consumed_e8s as u128 / total_consumed_e8s as u128) as u64;
                    *position.collateral_gains.entry(collateral_type).or_insert(0) += user_collateral;
                }
            }
        }

        // Phase 4: Update aggregate totals
        for (token_ledger, &consumed) in stables_consumed {
            if let Some(total) = self.total_stablecoin_balances.get_mut(token_ledger) {
                *total = total.saturating_sub(consumed);
            }
        }

        // Phase 5: Record in history
        let record = PoolLiquidationRecord {
            vault_id,
            timestamp: ic_cdk::api::time(),
            stables_consumed: stables_consumed.clone(),
            collateral_gained,
            collateral_type,
            depositors_count: opted_in_principals.len() as u64,
        };
        self.liquidation_history.push(record);
        self.total_liquidations_executed += 1;

        // Phase 6: Clean up empty positions
        self.deposits.retain(|_, pos| !pos.is_empty());
    }

    // ─── Query Helpers ───

    pub fn get_pool_status(&self) -> StabilityPoolStatus {
        let total_e8s: u64 = self.total_stablecoin_balances.iter()
            .map(|(ledger, &amount)| {
                let decimals = self.stablecoin_registry.get(ledger).map(|c| c.decimals).unwrap_or(8);
                normalize_to_e8s(amount, decimals)
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
        }
    }

    pub fn get_user_position(&self, user: &Principal) -> Option<UserStabilityPosition> {
        self.deposits.get(user).map(|pos| UserStabilityPosition {
            stablecoin_balances: pos.stablecoin_balances.clone(),
            collateral_gains: pos.collateral_gains.clone(),
            opted_out_collateral: pos.opted_out_collateral.iter().cloned().collect(),
            deposit_timestamp: pos.deposit_timestamp,
            total_claimed_gains: pos.total_claimed_gains.clone(),
            total_usd_value_e8s: pos.total_usd_value(&self.stablecoin_registry),
        })
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
pub fn save_to_stable_memory() {
    STATE.with(|s| {
        let state = s.borrow();
        let bytes = Encode!(&*state).expect("Failed to encode stability pool state");
        let len = bytes.len() as u64;

        // Write length prefix (8 bytes) then data
        ic_cdk::api::stable::stable64_grow((len + 8 + 65535) / 65536)
            .expect("Failed to grow stable memory");
        ic_cdk::api::stable::stable64_write(0, &len.to_le_bytes());
        ic_cdk::api::stable::stable64_write(8, &bytes);
    });
}

/// Restore state from stable memory (called from post_upgrade).
pub fn load_from_stable_memory() {
    let mut len_bytes = [0u8; 8];
    ic_cdk::api::stable::stable64_read(0, &mut len_bytes);
    let len = u64::from_le_bytes(len_bytes) as usize;

    if len == 0 {
        return; // No saved state — fresh start
    }

    let mut bytes = vec![0u8; len];
    ic_cdk::api::stable::stable64_read(8, &mut bytes);

    let state: StabilityPoolState = Decode!(&bytes, StabilityPoolState)
        .expect("Failed to decode stability pool state from stable memory");
    replace_state(state);
}
