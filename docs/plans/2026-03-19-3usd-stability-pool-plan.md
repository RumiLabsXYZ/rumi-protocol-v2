# 3USD Stability Pool Integration — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Allow 3USD LP token deposits in the stability pool for liquidations, with an atomic burn mechanism on the 3pool and a fallback to protocol reserves.

**Architecture:** The stability pool registers 3USD as a new stablecoin with priority 0 (consumed last). During liquidation, it calls a new `authorized_redeem_and_burn` endpoint on the 3pool to atomically burn LP tokens and destroy icUSD from the pool's reserves. If the 3pool call fails, 3USD is transferred to the backend's protocol reserves as a fallback.

**Tech Stack:** Rust, IC CDK 0.12, ICRC-1/ICRC-2 ledger standards, Candid

---

## Phase 1: 3Pool — `authorized_redeem_and_burn` Endpoint

### Step 1.1: Add authorization state to 3pool

**File:** `src/rumi_3pool/src/state.rs`

Add `authorized_burn_callers` field to `ThreePoolState`:

```rust
// In ThreePoolState struct, after `is_initialized: bool`:
/// Canisters authorized to call `authorized_redeem_and_burn`.
/// Option for upgrade compatibility — old state won't have this field.
#[serde(default)]
pub authorized_burn_callers: Option<BTreeSet<Principal>>,
```

Add accessor methods in the `impl ThreePoolState` block:

```rust
/// Get authorized burn callers (empty if None for upgrade compat).
pub fn burn_callers(&self) -> &BTreeSet<Principal> {
    static EMPTY: std::sync::LazyLock<BTreeSet<Principal>> =
        std::sync::LazyLock::new(BTreeSet::new);
    self.authorized_burn_callers.as_ref().unwrap_or(&EMPTY)
}

/// Get mutable burn callers set (initializes if None for upgrade compat).
pub fn burn_callers_mut(&mut self) -> &mut BTreeSet<Principal> {
    self.authorized_burn_callers.get_or_insert_with(BTreeSet::new)
}
```

Add `use std::collections::BTreeSet;` to the imports at the top of `state.rs`.

**Test:** Unit test that `burn_callers()` returns empty set on fresh state, and `burn_callers_mut().insert(...)` works.

**Commit:** `feat(3pool): add authorized_burn_callers state field`

---

### Step 1.2: Add admin endpoints for managing authorized burn callers

**File:** `src/rumi_3pool/src/admin.rs`

Add three new functions:

```rust
/// Add a canister to the authorized burn callers set.
pub fn add_authorized_burn_caller(caller: Principal, canister: Principal) -> Result<(), ThreePoolError> {
    let admin = read_state(|s| s.config.admin);
    if caller != admin {
        return Err(ThreePoolError::Unauthorized);
    }
    mutate_state(|s| {
        s.burn_callers_mut().insert(canister);
    });
    Ok(())
}

/// Remove a canister from the authorized burn callers set.
pub fn remove_authorized_burn_caller(caller: Principal, canister: Principal) -> Result<(), ThreePoolError> {
    let admin = read_state(|s| s.config.admin);
    if caller != admin {
        return Err(ThreePoolError::Unauthorized);
    }
    mutate_state(|s| {
        s.burn_callers_mut().remove(&canister);
    });
    Ok(())
}

/// Get all authorized burn callers.
pub fn get_authorized_burn_callers() -> Vec<Principal> {
    read_state(|s| s.burn_callers().iter().copied().collect())
}
```

**File:** `src/rumi_3pool/src/lib.rs`

Expose as canister endpoints:

```rust
#[update]
pub fn add_authorized_burn_caller(canister: Principal) -> Result<(), ThreePoolError> {
    admin::add_authorized_burn_caller(ic_cdk::caller(), canister)
}

#[update]
pub fn remove_authorized_burn_caller(canister: Principal) -> Result<(), ThreePoolError> {
    admin::remove_authorized_burn_caller(ic_cdk::caller(), canister)
}

#[query]
pub fn get_authorized_burn_callers() -> Vec<Principal> {
    admin::get_authorized_burn_callers()
}
```

**File:** `src/rumi_3pool/rumi_3pool.did`

Add Candid declarations for the new endpoints.

**Commit:** `feat(3pool): add admin endpoints for authorized burn callers`

---

### Step 1.3: Add new error variants and types for authorized_redeem_and_burn

**File:** `src/rumi_3pool/src/types.rs`

Add error variants to `ThreePoolError`:

```rust
/// Caller is not in the authorized burn callers set.
NotAuthorizedBurnCaller,
/// LP/token ratio exceeds max slippage tolerance.
BurnSlippageExceeded { max_bps: u16, actual_bps: u16 },
/// Insufficient pool balance of the target token.
InsufficientPoolBalance { token: String, required: u128, available: u128 },
/// Insufficient LP balance for the caller.
InsufficientLpBalance { required: u128, available: u128 },
/// The token burn on the ledger failed.
BurnFailed { token: String, reason: String },
```

Add the request/response types:

```rust
/// Arguments for the authorized redeem-and-burn operation.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct AuthorizedRedeemAndBurnArgs {
    /// Which token to remove from the pool and burn (by ledger principal).
    pub token_ledger: Principal,
    /// Amount of the token to remove and burn (native decimals).
    pub token_amount: u128,
    /// Amount of LP tokens to burn in exchange.
    pub lp_amount: u128,
    /// Maximum acceptable slippage from virtual price (basis points).
    pub max_slippage_bps: u16,
}

/// Result of a successful redeem-and-burn operation.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct RedeemAndBurnResult {
    /// Actual token amount burned on the ledger.
    pub token_amount_burned: u128,
    /// LP tokens burned.
    pub lp_amount_burned: u128,
    /// Block index from the token ledger burn call.
    pub burn_block_index: u64,
}
```

**Commit:** `feat(3pool): add types for authorized_redeem_and_burn`

---

### Step 1.4: Implement `authorized_redeem_and_burn` core logic

**File:** `src/rumi_3pool/src/lib.rs`

```rust
/// Authorized redeem-and-burn: an authorized canister burns its LP tokens
/// and a corresponding amount of one token is removed from pool reserves
/// and burned on that token's ledger.
///
/// This is a general-purpose function for protocol operations like
/// stability pool liquidations and peg management.
#[update]
pub async fn authorized_redeem_and_burn(
    args: AuthorizedRedeemAndBurnArgs,
) -> Result<RedeemAndBurnResult, ThreePoolError> {
    let caller = ic_cdk::caller();

    // 1. Authorization check
    if !read_state(|s| s.burn_callers().contains(&caller)) {
        return Err(ThreePoolError::NotAuthorizedBurnCaller);
    }

    // 2. Resolve token index
    let (token_idx, token_symbol) = read_state(|s| {
        for (i, tc) in s.config.tokens.iter().enumerate() {
            if tc.ledger_id == args.token_ledger {
                return Ok((i, tc.symbol.clone()));
            }
        }
        Err(ThreePoolError::InvalidCoinIndex)
    })?;

    // 3. Validate LP balance
    let caller_lp = read_state(|s| s.lp_balances.get(&caller).copied().unwrap_or(0));
    if caller_lp < args.lp_amount {
        return Err(ThreePoolError::InsufficientLpBalance {
            required: args.lp_amount,
            available: caller_lp,
        });
    }

    // 4. Validate pool has enough of the target token
    let pool_balance = read_state(|s| s.balances[token_idx]);
    if pool_balance < args.token_amount {
        return Err(ThreePoolError::InsufficientPoolBalance {
            token: token_symbol.clone(),
            required: args.token_amount,
            available: pool_balance,
        });
    }

    // 5. Validate slippage: compare LP-to-token ratio against virtual price
    // Virtual price = value of 1 LP token in USD (scaled 1e18, LP is 8-dec)
    // Expected token per LP = lp_amount * virtual_price / 1e18 (result in 8-dec)
    // Then convert to token's native decimals
    let (vp, precision_muls, amp, lp_supply) = read_state(|s| {
        let pms = [
            s.config.tokens[0].precision_mul,
            s.config.tokens[1].precision_mul,
            s.config.tokens[2].precision_mul,
        ];
        let a = crate::math::get_a(
            s.config.initial_a, s.config.future_a,
            s.config.initial_a_time, s.config.future_a_time,
            ic_cdk::api::time(),
        );
        let vp = crate::math::virtual_price(&s.balances, &pms, a, s.lp_total_supply);
        (vp, pms, a, s.lp_total_supply)
    });

    if let Some(vp) = vp {
        // Expected token value of the LP being burned (in 18-dec)
        let expected_value_18 = args.lp_amount as u128 * vp / 100_000_000; // LP is 8-dec
        // token_amount in 18-dec for comparison
        let token_decimals = read_state(|s| s.config.tokens[token_idx].decimals);
        let token_amount_18 = args.token_amount * 10u128.pow((18 - token_decimals) as u32);

        // Check slippage: token_amount should not exceed expected_value * (1 + slippage)
        let max_token_18 = expected_value_18 * (10_000 + args.max_slippage_bps as u128) / 10_000;
        if token_amount_18 > max_token_18 {
            let actual_bps = if expected_value_18 > 0 {
                ((token_amount_18 - expected_value_18) * 10_000 / expected_value_18) as u16
            } else {
                u16::MAX
            };
            return Err(ThreePoolError::BurnSlippageExceeded {
                max_bps: args.max_slippage_bps,
                actual_bps,
            });
        }
    }

    // 6. Deduct LP and pool balance BEFORE the async burn call (deduct-before-transfer)
    mutate_state(|s| {
        if let Some(lp) = s.lp_balances.get_mut(&caller) {
            *lp -= args.lp_amount;
        }
        s.lp_total_supply -= args.lp_amount;
        s.balances[token_idx] -= args.token_amount;
    });

    // 7. Burn the token on its ledger
    // For icUSD: transfer to the minting account (standard ICRC-1 burn)
    // For ckUSDT/ckUSDC: transfer to the burn address (minter)
    // In both cases, we use icrc1_transfer to the zero subaccount of the minting account
    let burn_result = burn_token_on_ledger(args.token_ledger, args.token_amount).await;

    match burn_result {
        Ok(block_index) => {
            // Log ICRC-3 block for the LP burn
            mutate_state(|s| {
                s.log_block(crate::types::Icrc3Transaction::Burn {
                    from: caller,
                    amount: args.lp_amount,
                });
            });

            Ok(RedeemAndBurnResult {
                token_amount_burned: args.token_amount,
                lp_amount_burned: args.lp_amount,
                burn_block_index: block_index,
            })
        }
        Err(reason) => {
            // Rollback: restore LP and pool balance
            mutate_state(|s| {
                *s.lp_balances.entry(caller).or_insert(0) += args.lp_amount;
                s.lp_total_supply += args.lp_amount;
                s.balances[token_idx] += args.token_amount;
            });

            Err(ThreePoolError::BurnFailed {
                token: token_symbol,
                reason,
            })
        }
    }
}

/// Burn tokens by transferring to the minting account (ICRC-1 burn standard).
/// Returns the block index on success, or error reason string on failure.
async fn burn_token_on_ledger(ledger: Principal, amount: u128) -> Result<u64, String> {
    use icrc_ledger_types::icrc1::transfer::{TransferArg, TransferError};
    use icrc_ledger_types::icrc1::account::Account;

    // ICRC-1 burn = transfer to the minting account.
    // For IC ledgers, sending to Account { owner: ledger, subaccount: None }
    // with from_subaccount: None burns the tokens.
    // However, the standard burn mechanism is `icrc1_transfer` to the minting_account.
    // We need to query the minting account first, or use a known pattern.
    //
    // Actually, the simplest approach: call `icrc1_transfer` with `to` set to
    // the minting account. For ICP-based ICRC-1 ledgers, the minting account
    // is typically the ledger canister itself or a governance canister.
    // We query `icrc1_minting_account` to be safe.

    let minting_result: Result<(Option<Account>,), _> = ic_cdk::call(
        ledger, "icrc1_minting_account", ()
    ).await;

    let minting_account = match minting_result {
        Ok((Some(account),)) => account,
        Ok((None,)) => {
            return Err("Ledger has no minting account — cannot burn".to_string());
        }
        Err(e) => {
            return Err(format!("Failed to query minting account: {:?}", e));
        }
    };

    let transfer_args = TransferArg {
        to: minting_account,
        amount: amount.into(),
        fee: None,
        memo: None,
        created_at_time: Some(ic_cdk::api::time()),
        from_subaccount: None,
    };

    let result: Result<(Result<candid::Nat, TransferError>,), _> = ic_cdk::call(
        ledger, "icrc1_transfer", (transfer_args,)
    ).await;

    match result {
        Ok((Ok(block_index),)) => {
            let idx: u64 = block_index.0.try_into().unwrap_or(0);
            Ok(idx)
        }
        Ok((Err(e),)) => Err(format!("Transfer error: {:?}", e)),
        Err(e) => Err(format!("Call error: {:?}", e)),
    }
}
```

**File:** `src/rumi_3pool/rumi_3pool.did`

Add Candid type declarations for `AuthorizedRedeemAndBurnArgs`, `RedeemAndBurnResult`, and the `authorized_redeem_and_burn` update method.

**Test:** Write unit tests for slippage validation math. Integration test (PocketIC) for the full flow: deploy 3 ledgers + 3pool, add liquidity, add authorized caller, call `authorized_redeem_and_burn`, verify balances and LP supply changed correctly.

**Commit:** `feat(3pool): implement authorized_redeem_and_burn endpoint`

---

## Phase 2: Stability Pool — Accept 3USD Deposits

### Step 2.1: Extend StablecoinConfig for LP tokens

**File:** `src/stability_pool/src/types.rs`

Add two fields to `StablecoinConfig`:

```rust
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StablecoinConfig {
    pub ledger_id: Principal,
    pub symbol: String,
    pub decimals: u8,
    pub priority: u8,
    pub is_active: bool,
    #[serde(default)]
    pub transfer_fee: Option<u64>,
    /// True if this token is an LP token requiring special liquidation handling.
    #[serde(default)]
    pub is_lp_token: Option<bool>,
    /// Pool canister to call for LP token burn operations (e.g., 3pool canister).
    #[serde(default)]
    pub underlying_pool: Option<Principal>,
}
```

Using `Option<bool>` with `#[serde(default)]` for backward-compatible deserialization from stable memory.

**Commit:** `feat(stability-pool): add is_lp_token and underlying_pool to StablecoinConfig`

---

### Step 2.2: Add 3pool canister ID and protocol reserve address to pool state

**File:** `src/stability_pool/src/state.rs`

Add to `StabilityPoolState`:

```rust
/// Backend canister to receive 3USD as fallback protocol reserves.
/// Same as protocol_canister_id in practice, but explicit for clarity.
#[serde(default)]
pub protocol_reserve_address: Option<Principal>,
```

No new field needed for 3pool canister ID — it's stored in the `StablecoinConfig.underlying_pool` field for the 3USD entry.

**Commit:** `feat(stability-pool): add protocol_reserve_address to state`

---

### Step 2.3: Modify `compute_token_draw` to handle 3USD valuation via virtual price

**File:** `src/stability_pool/src/state.rs`

The current `compute_token_draw` normalizes token amounts to e8s using `normalize_to_e8s(amount, decimals)`. For 3USD, we need to account for virtual price: 1 3USD ≠ $1.00.

Add a helper method and modify `compute_token_draw`:

```rust
/// Convert a 3USD (LP token) amount to its USD value in e8s using virtual price.
/// `virtual_price` is scaled by 1e18, LP token has 8 decimals.
/// Result: amount_e8s = lp_amount * virtual_price / 1e18
fn lp_to_usd_e8s(lp_amount: u64, virtual_price: u128) -> u64 {
    // lp_amount is 8-dec. virtual_price is ~1e18.
    // lp_amount * vp / 1e18 gives value in 8-dec USD (e8s).
    (lp_amount as u128 * virtual_price / 1_000_000_000_000_000_000u128) as u64
}

/// Convert a USD e8s amount to the equivalent 3USD LP token amount.
fn usd_e8s_to_lp(usd_e8s: u64, virtual_price: u128) -> u64 {
    // Inverse: lp_amount = usd_e8s * 1e18 / virtual_price
    (usd_e8s as u128 * 1_000_000_000_000_000_000u128 / virtual_price) as u64
}
```

In `compute_token_draw`, modify the normalization to use virtual price for LP tokens. The virtual price must be cached in state (fetched periodically) since `compute_token_draw` is a synchronous function called from `read_state`.

Add to `StabilityPoolState`:

```rust
/// Cached virtual price for LP tokens (fetched from 3pool periodically).
/// Keyed by LP token ledger principal. Scaled by 1e18.
#[serde(default)]
pub cached_virtual_prices: Option<BTreeMap<Principal, u128>>,
```

With accessor:

```rust
pub fn virtual_prices(&self) -> &BTreeMap<Principal, u128> {
    static EMPTY: std::sync::LazyLock<BTreeMap<Principal, u128>> =
        std::sync::LazyLock::new(BTreeMap::new);
    self.cached_virtual_prices.as_ref().unwrap_or(&EMPTY)
}
```

Modify `effective_pool_for_collateral` and `compute_token_draw` to use `lp_to_usd_e8s` for LP tokens instead of plain `normalize_to_e8s`.

In `DepositPosition::total_usd_value`, add LP token handling:

```rust
pub fn total_usd_value(
    &self,
    stablecoin_registry: &BTreeMap<Principal, StablecoinConfig>,
    virtual_prices: &BTreeMap<Principal, u128>,
) -> u64 {
    self.stablecoin_balances.iter().map(|(ledger, &amount)| {
        match stablecoin_registry.get(ledger) {
            Some(config) if config.is_lp_token.unwrap_or(false) => {
                // Use virtual price for LP tokens
                virtual_prices.get(ledger)
                    .map(|&vp| lp_to_usd_e8s(amount, vp))
                    .unwrap_or(0)
            }
            Some(config) => normalize_to_e8s(amount, config.decimals),
            None => 0,
        }
    }).sum()
}
```

Update all call sites of `total_usd_value` to pass virtual_prices.

**Test:** Unit tests for `lp_to_usd_e8s` and `usd_e8s_to_lp` with virtual price = 1.0492e18. Test `compute_token_draw` with a mix of icUSD (priority 2), and 3USD (priority 0) to verify correct draw ordering and valuation.

**Commit:** `feat(stability-pool): handle 3USD valuation via virtual price in token draw`

---

### Step 2.4: Add virtual price fetch timer

**File:** `src/stability_pool/src/lib.rs` (new function, called from `post_upgrade` and `init`)

Add a timer that fetches virtual price from the 3pool every 5 minutes:

```rust
fn setup_virtual_price_timer() {
    ic_cdk_timers::set_timer_interval(
        std::time::Duration::from_secs(300),
        || ic_cdk::spawn(fetch_virtual_prices()),
    );
}

async fn fetch_virtual_prices() {
    let lp_configs: Vec<(Principal, Principal)> = read_state(|s| {
        s.stablecoin_registry.iter()
            .filter(|(_, c)| c.is_lp_token.unwrap_or(false))
            .filter_map(|(ledger, c)| c.underlying_pool.map(|pool| (*ledger, pool)))
            .collect()
    });

    for (lp_ledger, pool_canister) in lp_configs {
        // Call the 3pool's get_pool_status() which includes virtual_price
        let result: Result<(crate::types::ThreePoolStatus,), _> = ic_cdk::call(
            pool_canister, "get_pool_status", ()
        ).await;

        if let Ok((status,)) = result {
            mutate_state(|s| {
                s.cached_virtual_prices
                    .get_or_insert_with(BTreeMap::new)
                    .insert(lp_ledger, status.virtual_price);
            });
        }
    }
}
```

Add a minimal `ThreePoolStatus` struct to stability pool types (just the fields we need):

```rust
/// Minimal subset of the 3pool's PoolStatus for virtual price queries.
#[derive(CandidType, Clone, Debug, Deserialize)]
pub struct ThreePoolStatus {
    pub virtual_price: u128,
}
```

Call `setup_virtual_price_timer()` from `init` and `post_upgrade`.

**Commit:** `feat(stability-pool): add periodic virtual price fetch from 3pool`

---

### Step 2.5: Add `deposit_as_3usd` convenience endpoint

**File:** `src/stability_pool/src/deposits.rs`

```rust
/// Convenience deposit: user sends icUSD (or ckUSDT/ckUSDC) and the pool
/// deposits it into the 3pool on their behalf, crediting the resulting 3USD.
pub async fn deposit_as_3usd(
    token_ledger: Principal,
    amount: u64,
) -> Result<u64, StabilityPoolError> {
    let caller = ic_cdk::api::caller();

    // Validate source token is accepted and active
    let config = read_state(|s| s.get_stablecoin_config(&token_ledger).cloned())
        .ok_or(StabilityPoolError::TokenNotAccepted { ledger: token_ledger })?;
    if !config.is_active {
        return Err(StabilityPoolError::TokenNotActive { ledger: token_ledger });
    }
    // Source token must NOT be an LP token
    if config.is_lp_token.unwrap_or(false) {
        return Err(StabilityPoolError::TokenNotAccepted { ledger: token_ledger });
    }

    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }

    // Find the 3USD config (LP token with underlying_pool set)
    let (three_usd_ledger, three_pool_canister) = read_state(|s| {
        s.stablecoin_registry.iter()
            .find(|(_, c)| c.is_lp_token.unwrap_or(false) && c.underlying_pool.is_some())
            .map(|(ledger, c)| (*ledger, c.underlying_pool.unwrap()))
            .ok_or(StabilityPoolError::TokenNotAccepted { ledger: token_ledger })
    })?;

    // Step 1: Pull tokens from user via ICRC-2 transfer_from
    let transfer_args = TransferFromArgs {
        from: Account { owner: caller, subaccount: None },
        to: Account { owner: ic_cdk::api::id(), subaccount: None },
        amount: amount.into(),
        fee: None,
        memo: None,
        created_at_time: Some(ic_cdk::api::time()),
        spender_subaccount: None,
    };

    let result: Result<(Result<candid::Nat, TransferFromError>,), _> = ic_cdk::call(
        token_ledger, "icrc2_transfer_from", (transfer_args,)
    ).await;

    match result {
        Ok((Ok(_),)) => {},
        Ok((Err(e),)) => return Err(StabilityPoolError::LedgerTransferFailed {
            reason: format!("{:?}", e),
        }),
        Err(e) => return Err(StabilityPoolError::InterCanisterCallFailed {
            target: format!("{}", token_ledger),
            method: "icrc2_transfer_from".to_string(),
        }),
    }

    // Step 2: Approve 3pool to spend the token
    let approve_args = ApproveArgs {
        from_subaccount: None,
        spender: Account { owner: three_pool_canister, subaccount: None },
        amount: candid::Nat::from(amount as u128 * 2),
        expected_allowance: None,
        expires_at: Some(ic_cdk::api::time() + 300_000_000_000),
        fee: None,
        memo: None,
        created_at_time: Some(ic_cdk::api::time()),
    };

    let approve_result: Result<(Result<candid::Nat, ApproveError>,), _> = ic_cdk::call(
        token_ledger, "icrc2_approve", (approve_args,)
    ).await;

    if let Err(_) | Ok((Err(_),)) = approve_result {
        // Refund user
        refund_user(caller, token_ledger, amount).await;
        return Err(StabilityPoolError::InterCanisterCallFailed {
            target: format!("{}", token_ledger),
            method: "icrc2_approve".to_string(),
        });
    }

    // Step 3: Determine which coin index this token is in the 3pool
    // Build the amounts array: [0, 0, 0] with `amount` at the right index
    // We need to know token ordering in the 3pool. Query the 3pool's status.
    let pool_status_result: Result<(crate::types::ThreePoolFullStatus,), _> = ic_cdk::call(
        three_pool_canister, "get_pool_status", ()
    ).await;

    let pool_status = match pool_status_result {
        Ok((status,)) => status,
        Err(_) => {
            refund_user(caller, token_ledger, amount).await;
            return Err(StabilityPoolError::InterCanisterCallFailed {
                target: "3pool".to_string(),
                method: "get_pool_status".to_string(),
            });
        }
    };

    // Find the index of our token in the pool's token array
    let coin_index = pool_status.tokens.iter().position(|t| t.ledger_id == token_ledger);
    let coin_index = match coin_index {
        Some(idx) => idx,
        None => {
            refund_user(caller, token_ledger, amount).await;
            return Err(StabilityPoolError::TokenNotAccepted { ledger: token_ledger });
        }
    };

    let mut amounts = vec![0u128; 3];
    amounts[coin_index] = amount as u128;

    // Step 4: Call add_liquidity on the 3pool
    let lp_result: Result<(Result<u128, crate::types::ThreePoolErrorRemote>,), _> = ic_cdk::call(
        three_pool_canister, "add_liquidity", (amounts.clone(), 0u128)
    ).await;

    let lp_minted = match lp_result {
        Ok((Ok(lp),)) => lp,
        _ => {
            refund_user(caller, token_ledger, amount).await;
            return Err(StabilityPoolError::InterCanisterCallFailed {
                target: "3pool".to_string(),
                method: "add_liquidity".to_string(),
            });
        }
    };

    // Step 5: Credit user's 3USD balance in the stability pool
    let lp_amount_u64 = lp_minted as u64; // 3USD LP is 8-dec, fits in u64
    mutate_state(|s| s.add_deposit(caller, three_usd_ledger, lp_amount_u64));

    log!(INFO, "deposit_as_3usd: {} deposited {} of {} → {} 3USD LP", caller, amount, token_ledger, lp_amount_u64);

    Ok(lp_amount_u64)
}

/// Best-effort refund of tokens to user after a failed deposit_as_3usd.
async fn refund_user(user: Principal, token_ledger: Principal, amount: u64) {
    let transfer_args = TransferArg {
        to: Account { owner: user, subaccount: None },
        amount: amount.into(),
        fee: None,
        memo: None,
        created_at_time: Some(ic_cdk::api::time()),
        from_subaccount: None,
    };
    let _ = ic_cdk::call::<_, (Result<candid::Nat, TransferError>,)>(
        token_ledger, "icrc1_transfer", (transfer_args,)
    ).await;
}
```

**File:** `src/stability_pool/src/lib.rs`

Add the endpoint:

```rust
#[update]
pub async fn deposit_as_3usd(token_ledger: Principal, amount: u64) -> Result<u64, StabilityPoolError> {
    crate::deposits::deposit_as_3usd(token_ledger, amount).await
}
```

Add required types for the 3pool interop to `types.rs`:

```rust
/// Minimal 3pool status with token info for deposit_as_3usd routing.
#[derive(CandidType, Clone, Debug, Deserialize)]
pub struct ThreePoolFullStatus {
    pub virtual_price: u128,
    pub tokens: Vec<ThreePoolTokenInfo>,
}

#[derive(CandidType, Clone, Debug, Deserialize)]
pub struct ThreePoolTokenInfo {
    pub ledger_id: Principal,
    pub symbol: String,
    pub decimals: u8,
}

/// Remote 3pool error (for deserialization only).
#[derive(CandidType, Clone, Debug, Deserialize)]
pub enum ThreePoolErrorRemote {
    InsufficientOutput { expected_min: u128, actual: u128 },
    InsufficientLiquidity,
    InvalidCoinIndex,
    ZeroAmount,
    PoolEmpty,
    SlippageExceeded,
    TransferFailed { token: String, reason: String },
    Unauthorized,
    MathOverflow,
    InvariantNotConverged,
    PoolPaused,
    NotAuthorizedBurnCaller,
    BurnSlippageExceeded { max_bps: u16, actual_bps: u16 },
    InsufficientPoolBalance { token: String, required: u128, available: u128 },
    InsufficientLpBalance { required: u128, available: u128 },
    BurnFailed { token: String, reason: String },
}
```

Update `.did` file with new endpoint.

**Commit:** `feat(stability-pool): add deposit_as_3usd convenience endpoint`

---

## Phase 3: Stability Pool — Liquidation with 3USD

### Step 3.1: Modify liquidation flow to handle LP token burn

**File:** `src/stability_pool/src/liquidation.rs`

The key change is in `execute_single_liquidation`. After `compute_token_draw` returns a draw that includes 3USD, the liquidation loop must handle LP tokens differently:

1. For regular stablecoins (icUSD, ckUSDT, ckUSDC): existing flow (approve backend, call `liquidate_vault_partial` or `liquidate_vault_partial_with_stable`)
2. For LP tokens (3USD): call `authorized_redeem_and_burn` on the 3pool to burn icUSD from reserves, then call `liquidate_vault_partial` with `amount = 0` to signal the debt has been covered externally...

**Wait — this is the tricky part.** The backend's `liquidate_vault_partial` expects to pull icUSD from the caller. With the atomic burn approach, the icUSD is destroyed inside the 3pool — the backend never receives it. The backend needs to know the debt was covered.

**Revised approach:** The stability pool needs to:
1. Call `authorized_redeem_and_burn` on 3pool to destroy icUSD (this reduces icUSD supply)
2. Call a **new** backend endpoint `liquidate_vault_debt_covered(vault_id, amount_e8s)` that writes down the debt without expecting a token transfer

This requires a small backend change.

**File:** `src/rumi_protocol_backend/src/main.rs`

Add new endpoint:

```rust
/// Called by the stability pool after it has already burned icUSD (via 3pool atomic burn).
/// Writes down the vault's debt and releases proportional collateral to the caller.
/// Only callable by the registered stability pool canister.
#[update]
async fn stability_pool_liquidate_debt_burned(
    vault_id: u64,
    icusd_burned_e8s: u64,
) -> Result<StabilityPoolLiquidationResult, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let is_stability_pool = read_state(|s| {
        s.stability_pool_canister.map_or(false, |sp| sp == caller)
    });
    if !is_stability_pool {
        return Err(ProtocolError::GenericError(
            "Caller is not the registered stability pool canister".to_string(),
        ));
    }

    // Write down the debt and release collateral, but don't try to pull icUSD
    // since it was already burned atomically in the 3pool.
    vault::liquidate_vault_debt_already_burned(vault_id, icusd_burned_e8s).await
}
```

The `liquidate_vault_debt_already_burned` function mirrors `liquidate_vault_partial` but skips the ICRC-2 `transfer_from` step since the icUSD was already destroyed.

**File:** `src/stability_pool/src/liquidation.rs`

In `execute_single_liquidation`, after the existing token loop, add handling for LP tokens:

```rust
// Handle LP token draws (3USD) — uses atomic burn on 3pool
let lp_draws: Vec<(Principal, u64)> = token_draw.iter()
    .filter(|(ledger, _)| {
        stablecoin_configs.get(ledger)
            .map(|c| c.is_lp_token.unwrap_or(false))
            .unwrap_or(false)
    })
    .map(|(l, a)| (*l, *a))
    .collect();

for (lp_ledger, lp_amount) in lp_draws {
    let config = match stablecoin_configs.get(&lp_ledger) {
        Some(c) => c,
        None => continue,
    };
    let pool_canister = match config.underlying_pool {
        Some(p) => p,
        None => continue,
    };

    // Calculate icUSD equivalent using cached virtual price
    let vp = read_state(|s| {
        s.virtual_prices().get(&lp_ledger).copied().unwrap_or(1_000_000_000_000_000_000)
    });
    let icusd_equiv_e8s = lp_to_usd_e8s(lp_amount, vp);

    // Get the icUSD ledger from the 3pool's token config
    let icusd_ledger = icusd_ledger.unwrap_or(Principal::anonymous());

    // Try atomic burn on 3pool
    let burn_args = AuthorizedRedeemAndBurnArgs {
        token_ledger: icusd_ledger,
        token_amount: icusd_equiv_e8s as u128,
        lp_amount: lp_amount as u128,
        max_slippage_bps: 50, // 0.5% tolerance
    };

    let burn_result: Result<(Result<RedeemAndBurnResult, ThreePoolErrorRemote>,), _> = call(
        pool_canister, "authorized_redeem_and_burn", (burn_args,)
    ).await;

    match burn_result {
        Ok((Ok(result),)) => {
            // icUSD burned successfully — now tell backend to write down debt
            let liq_result: Result<(Result<StabilityPoolLiquidationResult, _>,), _> = call(
                protocol_id,
                "stability_pool_liquidate_debt_burned",
                (vault_info.vault_id, icusd_equiv_e8s),
            ).await;

            match liq_result {
                Ok((Ok(success),)) => {
                    let collateral = success.collateral_amount_received.unwrap_or(0);
                    actual_consumed.insert(lp_ledger, lp_amount);
                    total_collateral_gained += collateral;
                }
                _ => {
                    log!(INFO, "Backend rejected debt-burned liquidation for vault {}", vault_info.vault_id);
                    // icUSD already burned — this is a problem. Log for admin attention.
                    // The debt was covered (icUSD destroyed) but collateral wasn't released.
                    // Admin needs to manually release collateral.
                }
            }
        }
        Ok((Err(_),)) | Err(_) => {
            // Fallback: transfer 3USD to protocol reserves
            log!(INFO, "3pool burn failed for vault {}, falling back to protocol reserves", vault_info.vault_id);
            fallback_to_protocol_reserves(lp_ledger, lp_amount, vault_info, protocol_id).await;
        }
    }
}
```

Add the fallback function:

```rust
/// Fallback: transfer 3USD LP tokens to the backend as protocol reserves.
async fn fallback_to_protocol_reserves(
    lp_ledger: Principal,
    lp_amount: u64,
    vault_info: &LiquidatableVaultInfo,
    protocol_id: Principal,
) {
    let transfer_args = TransferArg {
        to: Account { owner: protocol_id, subaccount: None },
        amount: (lp_amount as u128).into(),
        fee: None,
        memo: None,
        created_at_time: Some(ic_cdk::api::time()),
        from_subaccount: None,
    };

    let result: Result<(Result<candid::Nat, TransferError>,), _> = call(
        lp_ledger, "icrc1_transfer", (transfer_args,)
    ).await;

    match result {
        Ok((Ok(_),)) => {
            log!(INFO, "Transferred {} 3USD to protocol reserves for vault {}", lp_amount, vault_info.vault_id);
        }
        _ => {
            log!(INFO, "CRITICAL: Failed to transfer 3USD to protocol reserves for vault {}", vault_info.vault_id);
        }
    }
}
```

**Commit:** `feat(stability-pool): handle LP token liquidation with atomic burn and fallback`

---

### Step 3.2: Add backend endpoint for debt-already-burned liquidation

**File:** `src/rumi_protocol_backend/src/main.rs`

Add the `stability_pool_liquidate_debt_burned` endpoint (code in step 3.1 above).

**File:** `src/rumi_protocol_backend/src/vault.rs`

Add `liquidate_vault_debt_already_burned` function that:
1. Validates the vault exists and is below liquidation ratio
2. Computes collateral to release proportional to `icusd_burned_e8s`
3. Writes down the vault's `borrowed_icusd_amount`
4. Transfers collateral to caller (stability pool)
5. Does NOT attempt any `transfer_from` for icUSD (already burned)

This mirrors the existing `liquidate_vault_partial` logic minus the icUSD pull.

**File:** Update `.did` file with new endpoint.

**Test:** Unit test that `liquidate_vault_debt_already_burned` correctly writes down debt and releases proportional collateral.

**Commit:** `feat(backend): add stability_pool_liquidate_debt_burned endpoint`

---

## Phase 4: ICRC-21 Consent Messages & .did Files

### Step 4.1: Update ICRC-21 consent messages for new endpoints

**File:** `src/stability_pool/src/lib.rs`

Add consent message handling for `deposit_as_3usd` in the `icrc21_canister_call_consent_message` match block.

**File:** `src/stability_pool/stability_pool.did`

Add all new types and endpoints.

**File:** `src/rumi_3pool/rumi_3pool.did`

Add all new types and endpoints.

**File:** `src/rumi_protocol_backend/rumi_protocol_backend.did`

Add `stability_pool_liquidate_debt_burned` endpoint.

**Commit:** `feat: update ICRC-21 consent messages and .did files for 3USD integration`

---

## Phase 5: Testing

### Step 5.1: Unit tests for 3pool authorized_redeem_and_burn

**File:** `src/rumi_3pool/src/lib.rs` (or new test module)

- Test authorization check (unauthorized caller rejected)
- Test slippage validation (reject when ratio exceeds max_slippage_bps)
- Test LP balance validation (reject when insufficient LP)
- Test pool balance validation (reject when pool doesn't have enough of target token)
- Test rollback on burn failure

### Step 5.2: Unit tests for stability pool 3USD token draw

**File:** `src/stability_pool/src/state.rs` (existing test module)

- Test `lp_to_usd_e8s` and `usd_e8s_to_lp` with known virtual prices
- Test `compute_token_draw` with 3USD at priority 0: verify icUSD/ckstables consumed first
- Test `compute_token_draw` when only 3USD is available: verify correct valuation
- Test `process_liquidation_gains` with mixed icUSD + 3USD consumption
- Test `total_usd_value` with LP token using virtual price

### Step 5.3: Integration test (PocketIC)

**File:** `src/stability_pool/tests/` (new integration test)

Full flow:
1. Deploy backend, 3pool, stability pool, and 3 ICRC-1 ledgers
2. Register 3USD as stablecoin in stability pool
3. Add stability pool as authorized burn caller on 3pool
4. User deposits 3USD into stability pool
5. Create an undercollateralized vault
6. Trigger liquidation
7. Verify: vault debt written down, icUSD burned from 3pool, depositor has collateral gains

**Commit:** `test: add unit and integration tests for 3USD stability pool`

---

## Summary: File Change Matrix

| File | Changes |
|------|---------|
| `src/rumi_3pool/src/state.rs` | Add `authorized_burn_callers` field + accessors |
| `src/rumi_3pool/src/types.rs` | Add error variants, `AuthorizedRedeemAndBurnArgs`, `RedeemAndBurnResult` |
| `src/rumi_3pool/src/admin.rs` | Add burn caller management functions |
| `src/rumi_3pool/src/lib.rs` | Add `authorized_redeem_and_burn` endpoint + admin endpoints |
| `src/rumi_3pool/rumi_3pool.did` | New types and endpoints |
| `src/stability_pool/src/types.rs` | Add `is_lp_token`, `underlying_pool` to `StablecoinConfig`; add 3pool interop types |
| `src/stability_pool/src/state.rs` | Add `cached_virtual_prices`, `protocol_reserve_address`; modify `compute_token_draw`, `effective_pool_for_collateral`, `total_usd_value` |
| `src/stability_pool/src/deposits.rs` | Add `deposit_as_3usd`, `refund_user` |
| `src/stability_pool/src/liquidation.rs` | Add LP token handling in liquidation loop, `fallback_to_protocol_reserves` |
| `src/stability_pool/src/lib.rs` | Add `deposit_as_3usd` endpoint, virtual price timer |
| `src/stability_pool/stability_pool.did` | New types and endpoints |
| `src/rumi_protocol_backend/src/main.rs` | Add `stability_pool_liquidate_debt_burned` |
| `src/rumi_protocol_backend/src/vault.rs` | Add `liquidate_vault_debt_already_burned` |
| `src/rumi_protocol_backend/rumi_protocol_backend.did` | New endpoint |
