# Stability Pool Refactor Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Rebuild the stability pool canister as a push-model, multi-token, multi-collateral pool with dynamic registries, stable memory, and correct liquidation math.

**Architecture:** The backend pushes liquidatable vault notifications to the pool after price updates. The pool holds icUSD/ckUSDT/ckUSDC from depositors, consumes ckstables first (proportionally) then icUSD, and distributes seized collateral gains to opted-in depositors. All state persists via stable memory.

**Tech Stack:** Rust, IC CDK 0.12, ICRC-1/ICRC-2 ledger types, candid serialization for stable memory, rust_decimal for precise math.

**Design Doc:** `docs/plans/2026-03-04-stability-pool-refactor-design.md`

---

## Task 1: New Type System

Gut `src/stability_pool/src/types.rs` and replace with the new multi-token, multi-collateral types.

**Files:**
- Rewrite: `src/stability_pool/src/types.rs`

**Step 1: Write the new types file**

Replace entire contents of `types.rs` with:

```rust
use candid::{CandidType, Deserialize, Principal};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

// ──────────────────────────────────────────────────────────────
// Registry types (dynamic, admin-configurable)
// ──────────────────────────────────────────────────────────────

/// Configuration for an accepted stablecoin deposit token.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StablecoinConfig {
    pub ledger_id: Principal,
    pub symbol: String,
    pub decimals: u8,
    /// Higher priority = consumed first during liquidations.
    /// e.g. ckstables = 2, icUSD = 1.
    pub priority: u8,
    /// false = no new deposits accepted, existing balances still withdrawable/consumable.
    pub is_active: bool,
}

/// Subset of backend CollateralConfig needed by the pool.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollateralInfo {
    pub ledger_id: Principal,
    pub symbol: String,
    pub decimals: u8,
    pub status: CollateralStatus,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CollateralStatus {
    Active,
    Paused,
    Frozen,
    Sunset,
    Deprecated,
}

// ──────────────────────────────────────────────────────────────
// Depositor types
// ──────────────────────────────────────────────────────────────

/// Per-user position in the stability pool.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DepositPosition {
    /// Stablecoin balances keyed by ledger principal, in native decimals.
    pub stablecoin_balances: BTreeMap<Principal, u64>,
    /// Claimable collateral gains keyed by collateral ledger principal.
    pub collateral_gains: BTreeMap<Principal, u64>,
    /// Collateral types this user has opted out of.
    pub opted_out_collateral: BTreeSet<Principal>,
    /// First deposit timestamp (nanos).
    pub deposit_timestamp: u64,
    /// Lifetime claimed gains per collateral type.
    pub total_claimed_gains: BTreeMap<Principal, u64>,
}

impl DepositPosition {
    pub fn new(timestamp: u64) -> Self {
        Self {
            stablecoin_balances: BTreeMap::new(),
            collateral_gains: BTreeMap::new(),
            opted_out_collateral: BTreeSet::new(),
            deposit_timestamp: timestamp,
            total_claimed_gains: BTreeMap::new(),
        }
    }

    /// Total stablecoin value in e8s (USD-equivalent).
    /// Converts each token to e8s using its decimal config.
    pub fn total_usd_value(&self, stablecoin_registry: &BTreeMap<Principal, StablecoinConfig>) -> u64 {
        self.stablecoin_balances.iter().map(|(ledger, &amount)| {
            match stablecoin_registry.get(ledger) {
                Some(config) => normalize_to_e8s(amount, config.decimals),
                None => 0,
            }
        }).sum()
    }

    /// Whether this user is opted in for a given collateral type.
    pub fn is_opted_in(&self, collateral_type: &Principal) -> bool {
        !self.opted_out_collateral.contains(collateral_type)
    }

    /// Whether the position is entirely empty (no balances, no gains).
    pub fn is_empty(&self) -> bool {
        self.stablecoin_balances.values().all(|&v| v == 0)
            && self.collateral_gains.values().all(|&v| v == 0)
    }
}

/// Convert a token amount from its native decimals to e8s (8 decimal places).
pub fn normalize_to_e8s(amount: u64, decimals: u8) -> u64 {
    match decimals.cmp(&8) {
        std::cmp::Ordering::Equal => amount,
        std::cmp::Ordering::Less => amount * 10u64.pow((8 - decimals) as u32),
        std::cmp::Ordering::Greater => amount / 10u64.pow((decimals - 8) as u32),
    }
}

/// Convert an e8s amount to a token's native decimals.
pub fn normalize_from_e8s(amount_e8s: u64, decimals: u8) -> u64 {
    match decimals.cmp(&8) {
        std::cmp::Ordering::Equal => amount_e8s,
        std::cmp::Ordering::Less => amount_e8s / 10u64.pow((8 - decimals) as u32),
        std::cmp::Ordering::Greater => amount_e8s * 10u64.pow((decimals - 8) as u32),
    }
}

// ──────────────────────────────────────────────────────────────
// Liquidation types
// ──────────────────────────────────────────────────────────────

/// Info pushed from backend to pool when vaults become liquidatable.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiquidatableVaultInfo {
    pub vault_id: u64,
    pub collateral_type: Principal,
    pub debt_amount: u64,         // icUSD e8s
    pub collateral_amount: u64,   // native decimals
}

/// Result of a single liquidation attempt.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiquidationResult {
    pub vault_id: u64,
    pub stables_consumed: BTreeMap<Principal, u64>, // ledger -> amount consumed (native decimals)
    pub collateral_gained: u64,                      // native decimals of collateral received
    pub collateral_type: Principal,
    pub success: bool,
    pub error_message: Option<String>,
}

/// Audit trail record for a completed liquidation.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolLiquidationRecord {
    pub vault_id: u64,
    pub timestamp: u64,
    pub stables_consumed: BTreeMap<Principal, u64>,
    pub collateral_gained: u64,
    pub collateral_type: Principal,
    pub depositors_count: u64,
}

// ──────────────────────────────────────────────────────────────
// Init / Config / API types
// ──────────────────────────────────────────────────────────────

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StabilityPoolInitArgs {
    pub protocol_canister_id: Principal,
    pub authorized_admins: Vec<Principal>,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolConfiguration {
    pub min_deposit_e8s: u64,
    pub max_liquidations_per_batch: u64,
    pub emergency_pause: bool,
    pub authorized_admins: Vec<Principal>,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StabilityPoolStatus {
    pub total_deposits_e8s: u64,              // all stables normalized to e8s
    pub total_depositors: u64,
    pub total_liquidations_executed: u64,
    pub stablecoin_balances: BTreeMap<Principal, u64>,  // per-token totals
    pub collateral_gains: BTreeMap<Principal, u64>,      // per-collateral totals distributed
    pub stablecoin_registry: Vec<StablecoinConfig>,
    pub collateral_registry: Vec<CollateralInfo>,
    pub emergency_paused: bool,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserStabilityPosition {
    pub stablecoin_balances: BTreeMap<Principal, u64>,
    pub collateral_gains: BTreeMap<Principal, u64>,
    pub opted_out_collateral: Vec<Principal>,
    pub deposit_timestamp: u64,
    pub total_claimed_gains: BTreeMap<Principal, u64>,
    pub total_usd_value_e8s: u64,
}

// ──────────────────────────────────────────────────────────────
// Error types
// ──────────────────────────────────────────────────────────────

#[derive(CandidType, Debug, Clone, Deserialize)]
pub enum StabilityPoolError {
    InsufficientBalance { token: Principal, required: u64, available: u64 },
    AmountTooLow { minimum_e8s: u64 },
    NoPositionFound,
    InsufficientPoolBalance,
    Unauthorized,
    TokenNotAccepted { ledger: Principal },
    TokenNotActive { ledger: Principal },
    CollateralNotFound { ledger: Principal },
    LedgerTransferFailed { reason: String },
    InterCanisterCallFailed { target: String, method: String },
    LiquidationFailed { vault_id: u64, reason: String },
    EmergencyPaused,
    SystemBusy,
    AlreadyOptedOut { collateral: Principal },
    AlreadyOptedIn { collateral: Principal },
}
```

**Step 2: Verify it compiles in isolation**

Run: `cargo check -p stability_pool 2>&1 | head -20`
Expected: Errors from other files referencing old types (this is fine — we'll fix them in subsequent tasks)

**Step 3: Commit**

```bash
git add src/stability_pool/src/types.rs
git commit -m "refactor(stability-pool): replace type system with multi-token multi-collateral types"
```

---

## Task 2: New State Module with Stable Memory

Rewrite `src/stability_pool/src/state.rs` with the new state model, stable memory serialization, and core state mutation logic.

**Files:**
- Rewrite: `src/stability_pool/src/state.rs`

**Step 1: Write the new state module**

Replace entire contents of `state.rs` with:

```rust
use std::collections::{BTreeMap, BTreeSet};
use std::cell::RefCell;
use candid::{Principal, Decode, Encode};
use serde::{Serialize, Deserialize};

use crate::types::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
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
```

**Step 2: Verify it compiles**

Run: `cargo check -p stability_pool 2>&1 | head -20`
Expected: Errors from `lib.rs`, `deposits.rs`, `liquidation.rs` referencing old state (expected — fixed in later tasks)

**Step 3: Commit**

```bash
git add src/stability_pool/src/state.rs
git commit -m "refactor(stability-pool): new state module with stable memory and multi-token support"
```

---

## Task 3: Rewrite Deposits Module

Rewrite `src/stability_pool/src/deposits.rs` for multi-token deposit/withdraw/claim flows.

**Files:**
- Rewrite: `src/stability_pool/src/deposits.rs`

**Step 1: Write the new deposits module**

Replace entire contents of `deposits.rs` with:

```rust
use candid::Principal;
use ic_canister_log::log;
use ic_cdk::call;
use icrc_ledger_types::icrc2::transfer_from::{TransferFromArgs, TransferFromError};
use icrc_ledger_types::icrc1::transfer::{TransferArg, TransferError};
use icrc_ledger_types::icrc1::account::Account;
use num_traits::ToPrimitive;

use crate::logs::INFO;
use crate::types::*;
use crate::state::{read_state, mutate_state};

/// Deposit a stablecoin into the pool. User must have pre-approved the pool canister.
pub async fn deposit(token_ledger: Principal, amount: u64) -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();

    // Validate token is accepted
    let config = read_state(|s| s.get_stablecoin_config(&token_ledger).cloned())
        .ok_or(StabilityPoolError::TokenNotAccepted { ledger: token_ledger })?;

    if !config.is_active {
        return Err(StabilityPoolError::TokenNotActive { ledger: token_ledger });
    }

    // Validate minimum deposit (normalize to e8s for comparison)
    let amount_e8s = normalize_to_e8s(amount, config.decimals);
    let min_deposit = read_state(|s| s.configuration.min_deposit_e8s);
    if amount_e8s < min_deposit {
        return Err(StabilityPoolError::AmountTooLow { minimum_e8s: min_deposit });
    }

    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }

    log!(INFO, "Deposit: {} {} ({}) from {}", amount, config.symbol, token_ledger, caller);

    // ICRC-2 transfer_from: pull tokens from user to pool canister
    let transfer_args = TransferFromArgs {
        from: Account { owner: caller, subaccount: None },
        to: Account { owner: ic_cdk::api::id(), subaccount: None },
        amount: amount.into(),
        fee: None,
        memo: None,
        created_at_time: Some(ic_cdk::api::time()),
        spender_subaccount: None,
    };

    let result: Result<(Result<candid::Nat, TransferFromError>,), _> = call(
        token_ledger, "icrc2_transfer_from", (transfer_args,)
    ).await;

    match result {
        Ok((Ok(block_index),)) => {
            log!(INFO, "Transfer succeeded, block: {}", block_index);
            mutate_state(|s| s.add_deposit(caller, token_ledger, amount));
            log!(INFO, "Deposit recorded for {}", caller);
            Ok(())
        },
        Ok((Err(transfer_error),)) => {
            log!(INFO, "Transfer failed: {:?}", transfer_error);
            Err(StabilityPoolError::LedgerTransferFailed {
                reason: format!("{:?}", transfer_error),
            })
        },
        Err(call_error) => {
            log!(INFO, "Inter-canister call failed: {:?}", call_error);
            Err(StabilityPoolError::InterCanisterCallFailed {
                target: format!("{}", token_ledger),
                method: "icrc2_transfer_from".to_string(),
            })
        }
    }
}

/// Withdraw a stablecoin from the pool (only unconsumed balances).
pub async fn withdraw(token_ledger: Principal, amount: u64) -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();

    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }

    // Validate user has sufficient balance
    let available = read_state(|s| {
        s.deposits.get(&caller)
            .and_then(|pos| pos.stablecoin_balances.get(&token_ledger).copied())
            .unwrap_or(0)
    });
    if available < amount {
        return Err(StabilityPoolError::InsufficientBalance {
            token: token_ledger,
            required: amount,
            available,
        });
    }

    log!(INFO, "Withdraw: {} from {} by {}", amount, token_ledger, caller);

    // ICRC-1 transfer: send tokens from pool to user
    let transfer_args = TransferArg {
        to: Account { owner: caller, subaccount: None },
        amount: amount.into(),
        fee: None,
        memo: None,
        created_at_time: Some(ic_cdk::api::time()),
        from_subaccount: None,
    };

    let result: Result<(Result<candid::Nat, TransferError>,), _> = call(
        token_ledger, "icrc1_transfer", (transfer_args,)
    ).await;

    match result {
        Ok((Ok(block_index),)) => {
            log!(INFO, "Withdrawal transfer succeeded, block: {}", block_index);
            mutate_state(|s| {
                if let Err(e) = s.process_withdrawal(caller, token_ledger, amount) {
                    log!(INFO, "WARNING: State update failed after transfer: {:?}", e);
                }
            });
            Ok(())
        },
        Ok((Err(transfer_error),)) => {
            log!(INFO, "Withdrawal transfer failed: {:?}", transfer_error);
            Err(StabilityPoolError::LedgerTransferFailed {
                reason: format!("{:?}", transfer_error),
            })
        },
        Err(call_error) => {
            log!(INFO, "Inter-canister call failed: {:?}", call_error);
            Err(StabilityPoolError::InterCanisterCallFailed {
                target: format!("{}", token_ledger),
                method: "icrc1_transfer".to_string(),
            })
        }
    }
}

/// Claim collateral gains for a single collateral type.
pub async fn claim_collateral(collateral_ledger: Principal) -> Result<u64, StabilityPoolError> {
    let caller = ic_cdk::api::caller();

    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }

    let gains = read_state(|s| {
        s.deposits.get(&caller)
            .and_then(|pos| pos.collateral_gains.get(&collateral_ledger).copied())
            .unwrap_or(0)
    });

    if gains == 0 {
        return Ok(0);
    }

    log!(INFO, "Claim: {} of collateral {} by {}", gains, collateral_ledger, caller);

    let transfer_args = TransferArg {
        to: Account { owner: caller, subaccount: None },
        amount: gains.into(),
        fee: None,
        memo: None,
        created_at_time: Some(ic_cdk::api::time()),
        from_subaccount: None,
    };

    let result: Result<(Result<candid::Nat, TransferError>,), _> = call(
        collateral_ledger, "icrc1_transfer", (transfer_args,)
    ).await;

    match result {
        Ok((Ok(block_index),)) => {
            log!(INFO, "Collateral claim transfer succeeded, block: {}", block_index);
            mutate_state(|s| s.mark_gains_claimed(&caller, &collateral_ledger, gains));
            Ok(gains)
        },
        Ok((Err(transfer_error),)) => {
            log!(INFO, "Collateral claim failed: {:?}", transfer_error);
            Err(StabilityPoolError::LedgerTransferFailed {
                reason: format!("{:?}", transfer_error),
            })
        },
        Err(call_error) => {
            log!(INFO, "Inter-canister call failed: {:?}", call_error);
            Err(StabilityPoolError::InterCanisterCallFailed {
                target: format!("{}", collateral_ledger),
                method: "icrc1_transfer".to_string(),
            })
        }
    }
}

/// Claim all nonzero collateral gains across all collateral types.
pub async fn claim_all_collateral() -> Result<BTreeMap<Principal, u64>, StabilityPoolError> {
    let caller = ic_cdk::api::caller();

    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }

    let all_gains = read_state(|s| s.get_collateral_gains(&caller));
    let nonzero_gains: BTreeMap<Principal, u64> = all_gains.into_iter()
        .filter(|(_, v)| *v > 0)
        .collect();

    if nonzero_gains.is_empty() {
        return Ok(BTreeMap::new());
    }

    let mut claimed = BTreeMap::new();
    for (collateral_ledger, amount) in &nonzero_gains {
        match claim_collateral(*collateral_ledger).await {
            Ok(claimed_amount) => {
                claimed.insert(*collateral_ledger, claimed_amount);
            },
            Err(e) => {
                log!(INFO, "Failed to claim {} from {}: {:?}", amount, collateral_ledger, e);
                // Continue claiming others — partial success is fine
            }
        }
    }

    Ok(claimed)
}
```

**Step 2: Commit**

```bash
git add src/stability_pool/src/deposits.rs
git commit -m "refactor(stability-pool): multi-token deposit, withdraw, and claim logic"
```

---

## Task 4: Rewrite Liquidation Module (Push Model)

Replace the polling-based liquidation with the push-model receiver and fallback endpoint.

**Files:**
- Rewrite: `src/stability_pool/src/liquidation.rs`

**Step 1: Write the new liquidation module**

Replace entire contents of `liquidation.rs` with:

```rust
use std::collections::BTreeMap;
use candid::Principal;
use ic_canister_log::log;
use ic_cdk::call;
use icrc_ledger_types::icrc2::approve::{ApproveArgs, ApproveError};

use crate::logs::INFO;
use crate::types::*;
use crate::state::{read_state, mutate_state};

/// Called by the backend when it detects liquidatable vaults (push model).
/// Processes each vault sequentially, consuming stablecoins and distributing collateral.
pub async fn notify_liquidatable_vaults(vaults: Vec<LiquidatableVaultInfo>) -> Vec<LiquidationResult> {
    if read_state(|s| s.configuration.emergency_pause) {
        log!(INFO, "Pool is paused — ignoring {} liquidatable vaults", vaults.len());
        return vec![];
    }

    log!(INFO, "Received push notification: {} liquidatable vaults", vaults.len());

    let max_batch = read_state(|s| s.configuration.max_liquidations_per_batch) as usize;

    let mut results = Vec::new();
    for vault_info in vaults.into_iter().take(max_batch) {
        // Skip if already in-flight
        if read_state(|s| s.in_flight_liquidations.contains(&vault_info.vault_id)) {
            log!(INFO, "Vault {} already in-flight, skipping", vault_info.vault_id);
            continue;
        }

        // Check effective pool coverage for this collateral type
        let effective_pool = read_state(|s| s.effective_pool_for_collateral(&vault_info.collateral_type));
        if effective_pool < vault_info.debt_amount {
            log!(INFO, "Insufficient pool coverage for vault {}: need {} e8s, have {} e8s",
                vault_info.vault_id, vault_info.debt_amount, effective_pool);
            continue;
        }

        // Mark as in-flight
        mutate_state(|s| { s.in_flight_liquidations.insert(vault_info.vault_id); });

        let result = execute_single_liquidation(&vault_info).await;

        // Clear in-flight
        mutate_state(|s| { s.in_flight_liquidations.remove(&vault_info.vault_id); });

        if result.success {
            log!(INFO, "Liquidated vault {}: gained {} collateral",
                vault_info.vault_id, result.collateral_gained);
        } else {
            log!(INFO, "Liquidation failed for vault {}: {}",
                vault_info.vault_id, result.error_message.as_deref().unwrap_or("unknown"));
        }

        results.push(result);
    }

    results
}

/// Public fallback: anyone can call this to trigger a liquidation for a specific vault.
/// Per-caller guard is enforced at the lib.rs level.
pub async fn execute_liquidation(vault_id: u64) -> Result<LiquidationResult, StabilityPoolError> {
    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }

    if read_state(|s| s.in_flight_liquidations.contains(&vault_id)) {
        return Err(StabilityPoolError::SystemBusy);
    }

    // Fetch vault info from backend
    let protocol_id = read_state(|s| s.protocol_canister_id);

    let call_result: Result<(Vec<rumi_protocol_backend::vault::CandidVault>,), _> = call(
        protocol_id, "get_liquidatable_vaults", ()
    ).await.map_err(|e| StabilityPoolError::InterCanisterCallFailed {
        target: "Protocol".to_string(),
        method: "get_liquidatable_vaults".to_string(),
    })?;

    let vaults = call_result.0;
    let target_vault = vaults.into_iter().find(|v| v.vault_id == vault_id);

    let vault = match target_vault {
        Some(v) => v,
        None => return Err(StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: "Vault not found in liquidatable list".to_string(),
        }),
    };

    let vault_info = LiquidatableVaultInfo {
        vault_id: vault.vault_id,
        collateral_type: vault.collateral_type,
        debt_amount: vault.borrowed_icusd_amount,
        collateral_amount: vault.icp_margin_amount,
    };

    // Check pool coverage
    let effective_pool = read_state(|s| s.effective_pool_for_collateral(&vault_info.collateral_type));
    if effective_pool < vault_info.debt_amount {
        return Err(StabilityPoolError::InsufficientPoolBalance);
    }

    mutate_state(|s| { s.in_flight_liquidations.insert(vault_id); });
    let result = execute_single_liquidation(&vault_info).await;
    mutate_state(|s| { s.in_flight_liquidations.remove(&vault_id); });

    Ok(result)
}

/// Core liquidation logic for a single vault.
async fn execute_single_liquidation(vault_info: &LiquidatableVaultInfo) -> LiquidationResult {
    let protocol_id = read_state(|s| s.protocol_canister_id);

    // Step 1: Compute token draw
    let token_draw = read_state(|s| s.compute_token_draw(vault_info.debt_amount, &vault_info.collateral_type));

    if token_draw.is_empty() {
        return LiquidationResult {
            vault_id: vault_info.vault_id,
            stables_consumed: BTreeMap::new(),
            collateral_gained: 0,
            collateral_type: vault_info.collateral_type,
            success: false,
            error_message: Some("No stablecoins available for liquidation".to_string()),
        };
    }

    log!(INFO, "Token draw for vault {}: {:?}", vault_info.vault_id, token_draw);

    // Step 2: Approve backend for each token and call appropriate liquidation endpoint
    let mut total_collateral_gained: u64 = 0;
    let mut actual_consumed: BTreeMap<Principal, u64> = BTreeMap::new();

    // Get stablecoin configs for classification
    let stablecoin_configs: BTreeMap<Principal, StablecoinConfig> = read_state(|s| s.stablecoin_registry.clone());
    let icusd_ledger = stablecoin_configs.iter()
        .find(|(_, c)| c.symbol == "icUSD")
        .map(|(id, _)| *id);

    for (token_ledger, amount) in &token_draw {
        let is_icusd = icusd_ledger.map(|id| id == *token_ledger).unwrap_or(false);

        // Approve backend to spend this token
        let approve_args = ApproveArgs {
            from_subaccount: None,
            spender: Account { owner: protocol_id, subaccount: None },
            amount: (*amount as u128 * 2).into(), // 2x buffer for fees
            expected_allowance: None,
            expires_at: Some(ic_cdk::api::time() + 300_000_000_000), // 5 min
            fee: None,
            memo: None,
            created_at_time: Some(ic_cdk::api::time()),
        };

        let approve_result: Result<(Result<candid::Nat, ApproveError>,), _> = call(
            *token_ledger, "icrc2_approve", (approve_args,)
        ).await;

        match approve_result {
            Ok((Ok(_),)) => {},
            Ok((Err(e),)) => {
                log!(INFO, "Approve failed for {}: {:?}", token_ledger, e);
                continue;
            },
            Err(e) => {
                log!(INFO, "Approve call failed for {}: {:?}", token_ledger, e);
                continue;
            }
        }

        // Call the appropriate backend endpoint
        let liq_result = if is_icusd {
            // liquidate_vault_partial(vault_id, amount_e8s)
            let call_result: Result<(Result<rumi_protocol_backend::SuccessWithFee, rumi_protocol_backend::ProtocolError>,), _> = call(
                protocol_id,
                "liquidate_vault_partial",
                (rumi_protocol_backend::vault::VaultArg {
                    vault_id: vault_info.vault_id,
                    amount: *amount,
                },)
            ).await;
            call_result.map(|(r,)| r)
        } else {
            // Determine StableTokenType from ledger principal
            let token_type = determine_stable_token_type(*token_ledger, &stablecoin_configs);
            match token_type {
                Some(tt) => {
                    let call_result: Result<(Result<rumi_protocol_backend::SuccessWithFee, rumi_protocol_backend::ProtocolError>,), _> = call(
                        protocol_id,
                        "liquidate_vault_partial_with_stable",
                        (rumi_protocol_backend::VaultArgWithToken {
                            vault_id: vault_info.vault_id,
                            amount: *amount,
                            token_type: tt,
                        },)
                    ).await;
                    call_result.map(|(r,)| r)
                },
                None => {
                    log!(INFO, "Unknown stable token type for {}, skipping", token_ledger);
                    continue;
                }
            }
        };

        match liq_result {
            Ok(Ok(success)) => {
                log!(INFO, "Liquidation call succeeded for vault {} with token {}: fee={}",
                    vault_info.vault_id, token_ledger, success.fee_amount_paid);
                actual_consumed.insert(*token_ledger, *amount);
                total_collateral_gained += success.fee_amount_paid; // fee_amount_paid is actually the collateral value
            },
            Ok(Err(protocol_error)) => {
                log!(INFO, "Protocol rejected liquidation for vault {} with token {}: {:?}",
                    vault_info.vault_id, token_ledger, protocol_error);
            },
            Err(call_error) => {
                log!(INFO, "Liquidation call failed for vault {} with token {}: {:?}",
                    vault_info.vault_id, token_ledger, call_error);
            }
        }
    }

    // Step 3: If any liquidation calls succeeded, process gains
    if !actual_consumed.is_empty() && total_collateral_gained > 0 {
        mutate_state(|s| {
            s.process_liquidation_gains(
                vault_info.vault_id,
                vault_info.collateral_type,
                &actual_consumed,
                total_collateral_gained,
            );
        });

        LiquidationResult {
            vault_id: vault_info.vault_id,
            stables_consumed: actual_consumed,
            collateral_gained: total_collateral_gained,
            collateral_type: vault_info.collateral_type,
            success: true,
            error_message: None,
        }
    } else {
        LiquidationResult {
            vault_id: vault_info.vault_id,
            stables_consumed: BTreeMap::new(),
            collateral_gained: 0,
            collateral_type: vault_info.collateral_type,
            success: false,
            error_message: Some("All liquidation calls failed".to_string()),
        }
    }
}

/// Thin translation layer: map a ledger principal to the backend's StableTokenType enum.
/// This goes away when the backend gets a dynamic stablecoin registry.
fn determine_stable_token_type(
    ledger: Principal,
    configs: &BTreeMap<Principal, StablecoinConfig>,
) -> Option<rumi_protocol_backend::StableTokenType> {
    let config = configs.get(&ledger)?;
    match config.symbol.as_str() {
        "ckUSDT" => Some(rumi_protocol_backend::StableTokenType::CKUSDT),
        "ckUSDC" => Some(rumi_protocol_backend::StableTokenType::CKUSDC),
        _ => None, // icUSD uses a different endpoint, other tokens not yet mapped
    }
}

use icrc_ledger_types::icrc1::account::Account;
```

**Step 2: Commit**

```bash
git add src/stability_pool/src/liquidation.rs
git commit -m "refactor(stability-pool): push-model liquidation with multi-token draw and fallback endpoint"
```

---

## Task 5: Rewrite Canister Entry Point (lib.rs)

Wire up all endpoints, init/upgrade with stable memory, and the new Candid interface.

**Files:**
- Rewrite: `src/stability_pool/src/lib.rs`

**Step 1: Write the new lib.rs**

Replace entire contents of `lib.rs` with:

```rust
use ic_cdk::{query, update, init, pre_upgrade, post_upgrade};
use candid::{candid_method, Principal};
use ic_canister_log::log;
use std::collections::BTreeMap;

pub mod types;
pub mod state;
pub mod deposits;
pub mod liquidation;
pub mod logs;

use crate::types::*;
use crate::state::{mutate_state, read_state};
use crate::logs::INFO;

// ─── Init / Upgrade ───

#[init]
fn init(args: StabilityPoolInitArgs) {
    mutate_state(|s| s.initialize(args));
    log!(INFO, "Stability Pool initialized. Protocol: {}",
        read_state(|s| s.protocol_canister_id));
}

#[pre_upgrade]
fn pre_upgrade() {
    log!(INFO, "Stability Pool pre-upgrade: saving state to stable memory");
    state::save_to_stable_memory();
}

#[post_upgrade]
fn post_upgrade() {
    state::load_from_stable_memory();
    log!(INFO, "Stability Pool post-upgrade: state restored. {} depositors, {} liquidations",
        read_state(|s| s.deposits.len()),
        read_state(|s| s.total_liquidations_executed));

    if let Err(error) = read_state(|s| s.validate_state()) {
        ic_cdk::trap(&format!("State validation failed after upgrade: {}", error));
    }
}

// ─── Deposit / Withdraw / Claim ───

#[update]
pub async fn deposit(token_ledger: Principal, amount: u64) -> Result<(), StabilityPoolError> {
    crate::deposits::deposit(token_ledger, amount).await
}

#[update]
pub async fn withdraw(token_ledger: Principal, amount: u64) -> Result<(), StabilityPoolError> {
    crate::deposits::withdraw(token_ledger, amount).await
}

#[update]
pub async fn claim_collateral(collateral_ledger: Principal) -> Result<u64, StabilityPoolError> {
    crate::deposits::claim_collateral(collateral_ledger).await
}

#[update]
pub async fn claim_all_collateral() -> Result<BTreeMap<Principal, u64>, StabilityPoolError> {
    crate::deposits::claim_all_collateral().await
}

// ─── Opt-in / Opt-out ───

#[update]
pub fn opt_out_collateral(collateral_type: Principal) -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    mutate_state(|s| s.opt_out_collateral(&caller, collateral_type))
}

#[update]
pub fn opt_in_collateral(collateral_type: Principal) -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    mutate_state(|s| s.opt_in_collateral(&caller, collateral_type))
}

// ─── Liquidation (Push + Fallback) ───

/// Called by the backend to push liquidatable vault notifications.
#[update]
pub async fn notify_liquidatable_vaults(vaults: Vec<LiquidatableVaultInfo>) -> Vec<LiquidationResult> {
    // Optionally: validate caller is the protocol canister
    let caller = ic_cdk::api::caller();
    let expected = read_state(|s| s.protocol_canister_id);
    if caller != expected {
        log!(INFO, "notify_liquidatable_vaults called by {} (expected {}). Allowing for now.",
            caller, expected);
        // TODO: decide whether to enforce caller == protocol_canister_id
    }
    crate::liquidation::notify_liquidatable_vaults(vaults).await
}

/// Public fallback: trigger liquidation for a specific vault.
#[update]
pub async fn execute_liquidation(vault_id: u64) -> Result<LiquidationResult, StabilityPoolError> {
    crate::liquidation::execute_liquidation(vault_id).await
}

// ─── Queries ───

#[query]
pub fn get_pool_status() -> StabilityPoolStatus {
    read_state(|s| s.get_pool_status())
}

#[query]
pub fn get_user_position(user: Option<Principal>) -> Option<UserStabilityPosition> {
    let target = user.unwrap_or_else(ic_cdk::api::caller);
    read_state(|s| s.get_user_position(&target))
}

#[query]
pub fn get_liquidation_history(limit: Option<u64>) -> Vec<PoolLiquidationRecord> {
    let limit = limit.unwrap_or(50).min(100) as usize;
    read_state(|s| {
        s.liquidation_history.iter().rev().take(limit).cloned().collect()
    })
}

#[query]
pub fn check_pool_capacity(collateral_type: Principal, debt_amount_e8s: u64) -> bool {
    read_state(|s| s.effective_pool_for_collateral(&collateral_type) >= debt_amount_e8s)
}

#[query]
pub fn validate_pool_state() -> Result<String, String> {
    read_state(|s| s.validate_state().map(|_| "Pool state is consistent".to_string()))
}

// ─── Admin: Registry Management ───

#[update]
pub fn register_stablecoin(config: StablecoinConfig) -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    if !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    mutate_state(|s| s.register_stablecoin(config));
    Ok(())
}

#[update]
pub fn register_collateral(info: CollateralInfo) -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    if !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    mutate_state(|s| s.register_collateral(info));
    Ok(())
}

// ─── Admin: Configuration ───

#[update]
pub fn update_pool_configuration(new_config: PoolConfiguration) -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    if !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    mutate_state(|s| s.configuration = new_config);
    Ok(())
}

#[update]
pub fn emergency_pause() -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    if !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    mutate_state(|s| s.configuration.emergency_pause = true);
    log!(INFO, "Emergency pause activated by {}", caller);
    Ok(())
}

#[update]
pub fn resume_operations() -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    if !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    mutate_state(|s| s.configuration.emergency_pause = false);
    log!(INFO, "Operations resumed by {}", caller);
    Ok(())
}
```

**Step 2: Verify the full crate compiles**

Run: `cargo check -p stability_pool 2>&1 | head -30`
Expected: Should compile (may have warnings). Fix any remaining import issues.

**Step 3: Commit**

```bash
git add src/stability_pool/src/lib.rs
git commit -m "refactor(stability-pool): new canister entry point with stable memory and push-model endpoints"
```

---

## Task 6: Backend Push Integration

Add the push notification from `check_vaults()` to the stability pool.

**Files:**
- Modify: `src/rumi_protocol_backend/src/lib.rs:233-292` (check_vaults function)

**Step 1: Add the push call at the end of check_vaults()**

In `src/rumi_protocol_backend/src/lib.rs`, after the existing logging of unhealthy vaults (around line 283), add:

```rust
    // Push liquidatable vaults to stability pool (fire-and-forget)
    if !unhealthy_vaults.is_empty() {
        if let Some(pool_canister) = read_state(|s| s.stability_pool_canister) {
            let vault_infos: Vec<_> = unhealthy_vaults.iter().map(|v| {
                // We need a lightweight struct the pool understands.
                // For now, use a tuple or define a shared type.
                (v.vault_id, v.collateral_type, v.borrowed_icusd_amount.to_u64(), v.collateral_amount)
            }).collect();

            // Fire-and-forget: don't await, don't care if it fails
            ic_cdk::spawn(async move {
                let _result: Result<(Vec<()>,), _> = ic_cdk::call(
                    pool_canister,
                    "notify_liquidatable_vaults",
                    (vault_infos,)
                ).await;
                // Intentionally ignoring result — pool failure shouldn't affect backend
            });

            log!(INFO, "[check_vaults] Pushed {} vaults to stability pool {}", unhealthy_vaults.len(), pool_canister);
        }
    }
```

**Note:** The exact Candid types will need alignment between the backend and pool. The `LiquidatableVaultInfo` type from the pool's types.rs needs to be Candid-compatible with what the backend sends. This may require defining a shared type or using Candid tuples/records. Work this out during implementation.

**Step 2: Verify backend compiles**

Run: `cargo check -p rumi_protocol_backend 2>&1 | head -20`
Expected: PASS

**Step 3: Commit**

```bash
git add src/rumi_protocol_backend/src/lib.rs
git commit -m "feat(backend): push liquidatable vault notifications to stability pool"
```

---

## Task 7: Update Candid Interface

Replace the stability pool's `.did` file with the new multi-token interface.

**Files:**
- Rewrite: `src/stability_pool/stability_pool.did`

**Step 1: Generate or write the new Candid interface**

Run `cargo build -p stability_pool` and then use `candid-extractor` or manually write the `.did` to match the new endpoints. Key changes:

- `deposit(principal, nat64)` replaces `deposit_icusd(nat64)`
- `withdraw(principal, nat64)` replaces `withdraw_icusd(nat64)`
- `claim_collateral(principal)` and `claim_all_collateral()` replace `claim_collateral_gains()`
- `notify_liquidatable_vaults(vec LiquidatableVaultInfo)` is new
- `opt_out_collateral(principal)` and `opt_in_collateral(principal)` are new
- `register_stablecoin(StablecoinConfig)` and `register_collateral(CollateralInfo)` are new
- All old single-token types are replaced with multi-token equivalents

**Step 2: Update dfx.json**

Verify `dfx.json` points to the correct `.did` file and canister name. Remove the `rumi_stability_pool` entry if the v1 canister is still listed.

**Step 3: Commit**

```bash
git add src/stability_pool/stability_pool.did dfx.json
git commit -m "refactor(stability-pool): update Candid interface for multi-token pool"
```

---

## Task 8: Unit Tests for State Logic

Write tests for the core state mutation logic: deposits, withdrawals, token draw computation, liquidation gain distribution, opt-out filtering.

**Files:**
- Create: `src/stability_pool/tests/state_tests.rs` (or add to existing test structure)

**Step 1: Write state unit tests**

Key test cases to cover:

1. **Deposit and withdrawal**: add tokens, verify balances, withdraw, verify cleanup of empty positions
2. **Token draw — single priority**: all ckstables, verify proportional draw
3. **Token draw — mixed priorities**: ckstables consumed first, icUSD only for remainder
4. **Token draw — insufficient ckstables**: partial ckstable + icUSD fallback
5. **Liquidation gains distribution**: 3 depositors, verify proportional reduction and collateral gain
6. **Opt-out filtering**: depositor opts out, verify excluded from effective pool and gains
7. **Normalize e8s/e6s conversions**: verify `normalize_to_e8s` and `normalize_from_e8s`
8. **State validation**: verify `validate_state` catches mismatched totals

These tests operate on `StabilityPoolState` directly — no canister calls needed. Use the existing test patterns from `src/rumi_protocol_backend/tests/tests.rs` as a reference.

**Step 2: Run tests**

Run: `cargo test -p stability_pool 2>&1`
Expected: All tests PASS

**Step 3: Commit**

```bash
git add src/stability_pool/tests/
git commit -m "test(stability-pool): unit tests for multi-token state logic"
```

---

## Task 9: Integration Verification

Build the full project, verify both canisters compile, and ensure no regressions.

**Step 1: Build both canisters**

Run: `cargo build --target wasm32-unknown-unknown -p stability_pool -p rumi_protocol_backend 2>&1`
Expected: Both compile successfully

**Step 2: Run all existing backend tests**

Run: `cargo test -p rumi_protocol_backend 2>&1`
Expected: All existing tests still pass

**Step 3: Run stability pool tests**

Run: `cargo test -p stability_pool 2>&1`
Expected: All new tests pass

**Step 4: Validate Candid interface**

Run: `didc check src/stability_pool/stability_pool.did 2>&1` (if didc is available)
Expected: Valid

**Step 5: Commit any fixes**

```bash
git add -A
git commit -m "chore(stability-pool): integration fixes and verification"
```

---

## Summary of Files Changed

| File | Action | Description |
|------|--------|-------------|
| `src/stability_pool/src/types.rs` | Rewrite | Multi-token, multi-collateral type system |
| `src/stability_pool/src/state.rs` | Rewrite | New state with stable memory, token draw, gain distribution |
| `src/stability_pool/src/deposits.rs` | Rewrite | Multi-token deposit/withdraw/claim |
| `src/stability_pool/src/liquidation.rs` | Rewrite | Push-model receiver + fallback endpoint |
| `src/stability_pool/src/lib.rs` | Rewrite | New canister entry point with all endpoints |
| `src/stability_pool/src/logs.rs` | Keep | Unchanged |
| `src/stability_pool/stability_pool.did` | Rewrite | New Candid interface |
| `src/rumi_protocol_backend/src/lib.rs` | Modify | Add push call in check_vaults() |
| `dfx.json` | Modify | Remove v1 stability pool entry |
| `src/stability_pool/tests/state_tests.rs` | Create | Unit tests for state logic |
