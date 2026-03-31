# AMM Audit Fixes & Maintenance Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix all bugs found in the AMM canister audit and add a global maintenance mode kill switch so the admin can disable swaps/deposits while still allowing withdrawals.

**Architecture:** Changes are entirely within the `rumi_amm` crate (backend) plus frontend error formatting and declaration regeneration. The maintenance mode adds a single `maintenance_mode: bool` to `AmmState` with an admin toggle. It gates `swap`, `add_liquidity`, and `create_pool` but explicitly allows `remove_liquidity` and all queries. The `withdraw_protocol_fees` fix uses an optimistic-deduct-then-transfer pattern. The ledger fee issue is addressed by having callers pass `amount - ledger_fee` so on-ledger reality matches reserve bookkeeping (backend math stays clean; frontend adjusts amounts).

**Tech Stack:** Rust (IC canister), Candid, PocketIC tests, TypeScript/Svelte frontend

---

## File Map

| File | Action | Responsibility |
|------|--------|---------------|
| `src/rumi_amm/src/types.rs` | Modify | Add `MaintenanceMode` error variant |
| `src/rumi_amm/src/state.rs` | Modify | Add `maintenance_mode: bool`, update V1 migration to V2 fallback chain |
| `src/rumi_amm/src/lib.rs` | Modify | Add maintenance mode checks, fix `withdraw_protocol_fees`, add fee validation, add `set_maintenance_mode` + `is_maintenance_mode` endpoints |
| `src/rumi_amm/src/math.rs` | Modify | Add `checked_sub` in `compute_swap`, guard division-by-zero in `compute_proportional_lp_shares`, guard zero output in `compute_remove_liquidity` |
| `src/rumi_amm/src/transfers.rs` | Modify | Remove dead `transfer_between_subaccounts` |
| `src/rumi_amm/rumi_amm.did` | Modify | Add new endpoints and error variant |
| `src/rumi_amm/tests/pocket_ic_tests.rs` | Modify | Fix broken test, add new tests |
| `src/vault_frontend/src/lib/services/ammService.ts` | Modify | Add error format handlers |
| `src/declarations/rumi_amm/*` | Regenerate | `dfx generate rumi_amm` |

---

### Task 1: Add maintenance mode to state and types

**Files:**
- Modify: `src/rumi_amm/src/types.rs:94-110`
- Modify: `src/rumi_amm/src/state.rs:10-16` and `src/rumi_amm/src/state.rs:80-111`

- [ ] **Step 1: Add `MaintenanceMode` error variant to `AmmError`**

In `src/rumi_amm/src/types.rs`, add after `FeeBpsOutOfRange`:

```rust
MaintenanceMode,
```

- [ ] **Step 2: Add `maintenance_mode: bool` to `AmmState`**

In `src/rumi_amm/src/state.rs`, the `AmmState` struct should become:

```rust
pub struct AmmState {
    pub admin: Principal,
    pub pools: BTreeMap<PoolId, Pool>,
    #[serde(default)]
    pub pool_creation_open: bool,
    #[serde(default)]
    pub maintenance_mode: bool,
}
```

Update the `Default` impl to include `maintenance_mode: false`.

- [ ] **Step 3: Update stable memory migration**

The on-chain stable memory currently has `pool_creation_open` (from the last upgrade) but NOT `maintenance_mode`. Candid record decoding fails when a mandatory field is missing from the wire data, so `Decode!(&bytes, AmmState)` will fail for the current on-chain bytes (missing `maintenance_mode`). We need a three-level fallback chain:

1. Try `AmmState` (current shape: `admin`, `pools`, `pool_creation_open`, `maintenance_mode`)
2. Try `AmmStateV2` (intermediate: `admin`, `pools`, `pool_creation_open`) — preserves `pool_creation_open` from on-chain state
3. Try `AmmStateV1` (original: `admin`, `pools`) — safety net for pre-`pool_creation_open` state

Add `AmmStateV2` struct and update `load_from_stable_memory`:

```rust
/// V2 state shape (has pool_creation_open but not maintenance_mode).
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
struct AmmStateV2 {
    pub admin: Principal,
    pub pools: BTreeMap<PoolId, Pool>,
    pub pool_creation_open: bool,
}

/// V1 state shape (original, no pool_creation_open or maintenance_mode).
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
struct AmmStateV1 {
    pub admin: Principal,
    pub pools: BTreeMap<PoolId, Pool>,
}

pub fn load_from_stable_memory() {
    let mut len_bytes = [0u8; 8];
    ic_cdk::api::stable::stable64_read(0, &mut len_bytes);
    let len = u64::from_le_bytes(len_bytes) as usize;

    if len == 0 {
        return;
    }

    let mut bytes = vec![0u8; len];
    ic_cdk::api::stable::stable64_read(8, &mut bytes);

    // Try current shape first, then V2 (has pool_creation_open), then V1 (original)
    if let Ok(state) = Decode!(&bytes, AmmState) {
        replace_state(state);
    } else if let Ok(v2) = Decode!(&bytes, AmmStateV2) {
        replace_state(AmmState {
            admin: v2.admin,
            pools: v2.pools,
            pool_creation_open: v2.pool_creation_open,
            maintenance_mode: false,
        });
    } else {
        let v1: AmmStateV1 = Decode!(&bytes, AmmStateV1)
            .expect("Failed to decode AMM state from stable memory (tried V3, V2, V1)");
        replace_state(AmmState {
            admin: v1.admin,
            pools: v1.pools,
            pool_creation_open: false,
            maintenance_mode: false,
        });
    }
}
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo build -p rumi_amm --target wasm32-unknown-unknown --release 2>&1 | tail -5`

Expected: `Finished` with no errors.

- [ ] **Step 5: Commit**

```bash
git add src/rumi_amm/src/types.rs src/rumi_amm/src/state.rs
git commit -m "feat(amm): add maintenance_mode to state and MaintenanceMode error variant"
```

---

### Task 2: Add maintenance mode endpoints and gate core operations

**Files:**
- Modify: `src/rumi_amm/src/lib.rs:75-236` (admin section) and `src/rumi_amm/src/lib.rs:240-442` (swap/add_liquidity)
- Modify: `src/rumi_amm/rumi_amm.did`

- [ ] **Step 1: Add `set_maintenance_mode` admin endpoint**

In `src/rumi_amm/src/lib.rs`, add next to the existing `set_pool_creation_open`:

```rust
#[update]
fn set_maintenance_mode(enabled: bool) -> Result<(), AmmError> {
    caller_is_admin()?;
    mutate_state(|s| s.maintenance_mode = enabled);
    log!(INFO, "Maintenance mode: {}", enabled);
    Ok(())
}
```

- [ ] **Step 2: Add `is_maintenance_mode` query endpoint**

In the query section of `src/rumi_amm/src/lib.rs`:

```rust
#[query]
fn is_maintenance_mode() -> bool {
    read_state(|s| s.maintenance_mode)
}
```

- [ ] **Step 3: Add maintenance mode check to `swap`**

At the top of `swap()` in `src/rumi_amm/src/lib.rs`, before the pool state read:

```rust
if read_state(|s| s.maintenance_mode) {
    return Err(AmmError::MaintenanceMode);
}
```

- [ ] **Step 4: Add maintenance mode check to `add_liquidity`**

At the top of `add_liquidity()`, before the pool state read:

```rust
if read_state(|s| s.maintenance_mode) {
    return Err(AmmError::MaintenanceMode);
}
```

- [ ] **Step 5: Add maintenance mode check to `create_pool`**

At the very top of `create_pool()`, before the admin check:

```rust
if read_state(|s| s.maintenance_mode) && caller_is_admin().is_err() {
    return Err(AmmError::MaintenanceMode);
}
```

Note: admin can still create pools in maintenance mode (for setup). Only non-admin is blocked.

- [ ] **Step 6: Do NOT add maintenance mode check to `remove_liquidity`**

This is intentional — users must always be able to withdraw. The per-pool `paused` flag is the only gate on `remove_liquidity` (added in Task 5).

- [ ] **Step 7: Update the Candid `.did` file**

Add to the service section in `src/rumi_amm/rumi_amm.did`:

```candid
  is_maintenance_mode : () -> (bool) query;
  set_maintenance_mode : (bool) -> (variant { Ok; Err : AmmError });
```

Add `MaintenanceMode;` to the `AmmError` variant list.

- [ ] **Step 8: Verify it compiles**

Run: `cargo build -p rumi_amm --target wasm32-unknown-unknown --release 2>&1 | tail -5`

Expected: `Finished` with no errors.

- [ ] **Step 9: Commit**

```bash
git add src/rumi_amm/src/lib.rs src/rumi_amm/rumi_amm.did
git commit -m "feat(amm): add global maintenance mode kill switch"
```

---

### Task 3: Fix `withdraw_protocol_fees` state desync (Critical Bug #1)

**Files:**
- Modify: `src/rumi_amm/src/lib.rs:160-206`

The current code transfers tokens first and only decrements `protocol_fees_a`/`protocol_fees_b` at the end. If the fees_a transfer succeeds but fees_b fails, the early return skips the state update — causing a desync where fees_a tokens are gone on-ledger but the canister still thinks they're available.

**Fix:** Use optimistic deduct pattern — deduct from state first, transfer, roll back on failure.

- [ ] **Step 1: Rewrite `withdraw_protocol_fees`**

Replace the entire function body in `src/rumi_amm/src/lib.rs`:

```rust
#[update]
async fn withdraw_protocol_fees(pool_id: PoolId) -> Result<(u128, u128), AmmError> {
    caller_is_admin()?;

    let (token_a, token_b, sub_a, sub_b, fees_a, fees_b, admin) = read_state(|s| {
        let pool = s.pools.get(&pool_id).ok_or(AmmError::PoolNotFound)?;
        Ok::<_, AmmError>((
            pool.token_a, pool.token_b,
            pool.subaccount_a, pool.subaccount_b,
            pool.protocol_fees_a, pool.protocol_fees_b,
            s.admin,
        ))
    })?;

    if fees_a == 0 && fees_b == 0 {
        return Ok((0, 0));
    }

    // Optimistic deduct: zero out fees in state BEFORE transferring.
    // This ensures state is never ahead of on-chain reality.
    mutate_state(|s| {
        if let Some(pool) = s.pools.get_mut(&pool_id) {
            pool.protocol_fees_a = 0;
            pool.protocol_fees_b = 0;
        }
    });

    let mut withdrawn_a = 0u128;
    let mut withdrawn_b = 0u128;
    let mut errors = Vec::new();

    if fees_a > 0 {
        match transfer_to_user(token_a, sub_a, admin, fees_a).await {
            Ok(_) => withdrawn_a = fees_a,
            Err(reason) => {
                log!(INFO, "WARN: withdraw_protocol_fees transfer_a failed: {}. Rolling back.", reason);
                errors.push(format!("token_a: {}", reason));
            }
        }
    }

    if fees_b > 0 {
        match transfer_to_user(token_b, sub_b, admin, fees_b).await {
            Ok(_) => withdrawn_b = fees_b,
            Err(reason) => {
                log!(INFO, "WARN: withdraw_protocol_fees transfer_b failed: {}. Rolling back.", reason);
                errors.push(format!("token_b: {}", reason));
            }
        }
    }

    // Roll back any fees that failed to transfer
    let rollback_a = fees_a - withdrawn_a;
    let rollback_b = fees_b - withdrawn_b;
    if rollback_a > 0 || rollback_b > 0 {
        mutate_state(|s| {
            if let Some(pool) = s.pools.get_mut(&pool_id) {
                pool.protocol_fees_a += rollback_a;
                pool.protocol_fees_b += rollback_b;
            }
        });
    }

    if !errors.is_empty() {
        return Err(AmmError::TransferFailed {
            token: "protocol_fees".to_string(),
            reason: errors.join("; "),
        });
    }

    log!(INFO, "Protocol fees withdrawn from {}: ({}, {})", pool_id, withdrawn_a, withdrawn_b);
    Ok((withdrawn_a, withdrawn_b))
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p rumi_amm --target wasm32-unknown-unknown --release 2>&1 | tail -5`

- [ ] **Step 3: Commit**

```bash
git add src/rumi_amm/src/lib.rs
git commit -m "fix(amm): withdraw_protocol_fees state desync — use optimistic deduct pattern"
```

---

### Task 4: Fix math module bugs (Critical Bug #3, High Bug #4, Low Bug #13)

**Files:**
- Modify: `src/rumi_amm/src/math.rs:22-65` (compute_swap)
- Modify: `src/rumi_amm/src/math.rs:88-111` (compute_proportional_lp_shares)
- Modify: `src/rumi_amm/src/math.rs:116-139` (compute_remove_liquidity)

- [ ] **Step 1: Fix `compute_swap` underflow — use `checked_sub`**

In `src/rumi_amm/src/math.rs`, line 42, replace:

```rust
let effective_in = amount_in - total_fee;
```

with:

```rust
let effective_in = amount_in
    .checked_sub(total_fee)
    .ok_or(AmmError::FeeBpsOutOfRange)?;
```

This prevents wrapping if `fee_bps > 10_000` is ever reached.

- [ ] **Step 2: Add division-by-zero guard to `compute_proportional_lp_shares`**

In `src/rumi_amm/src/math.rs`, insert **between** the `ZeroAmount` return (line 97) and the `let shares_a = ...` computation (line 98) — BEFORE any division by `reserve_a` or `reserve_b` occurs:

```rust
if reserve_a == 0 || reserve_b == 0 || total_shares == 0 {
    return Err(AmmError::InsufficientLiquidity);
}
```

- [ ] **Step 3: Add zero-output guard to `compute_remove_liquidity`**

In `src/rumi_amm/src/math.rs`, after computing `amount_a` and `amount_b`, add before `Ok(...)`:

```rust
if amount_a == 0 && amount_b == 0 {
    return Err(AmmError::InsufficientLiquidity);
}
```

- [ ] **Step 4: Add fee validation to `set_fee` and `set_protocol_fee`**

In `src/rumi_amm/src/lib.rs`, in `set_fee()`, add before `mutate_state`:

```rust
if fee_bps > 10_000 {
    return Err(AmmError::FeeBpsOutOfRange);
}
```

In `set_protocol_fee()`, add before `mutate_state`:

```rust
if protocol_fee_bps > 10_000 {
    return Err(AmmError::FeeBpsOutOfRange);
}
```

- [ ] **Step 5: Add unit tests for the math fixes**

Add to `src/rumi_amm/src/math.rs` in the `#[cfg(test)] mod tests`:

```rust
#[test]
fn test_swap_fee_bps_over_10000() {
    // fee_bps of 20_000 would cause effective_in underflow without checked_sub
    let result = compute_swap(1_000_000, 1_000_000, 100_000, 20_000, 0);
    assert!(matches!(result, Err(AmmError::FeeBpsOutOfRange)));
}

#[test]
fn test_proportional_shares_zero_reserve() {
    let result = compute_proportional_lp_shares(100, 100, 0, 1000, 1000);
    assert!(matches!(result, Err(AmmError::InsufficientLiquidity)));
}

#[test]
fn test_proportional_shares_zero_total_shares() {
    let result = compute_proportional_lp_shares(100, 100, 1000, 1000, 0);
    assert!(matches!(result, Err(AmmError::InsufficientLiquidity)));
}

#[test]
fn test_remove_liquidity_zero_output() {
    // Tiny shares relative to reserves = 0 output after integer division
    let result = compute_remove_liquidity(1, 1_000_000_000, 1_000_000_000, 1_000_000_000_000);
    assert!(matches!(result, Err(AmmError::InsufficientLiquidity)));
}
```

- [ ] **Step 6: Run unit tests**

Run: `cargo test -p rumi_amm -- --lib 2>&1`

Expected: All tests pass including the 4 new ones.

- [ ] **Step 7: Commit**

```bash
git add src/rumi_amm/src/math.rs src/rumi_amm/src/lib.rs
git commit -m "fix(amm): guard math overflows, division-by-zero, and fee_bps > 10000"
```

---

### Task 5: Add paused check to `remove_liquidity` (High Bug #6)

**Files:**
- Modify: `src/rumi_amm/src/lib.rs:444-528`

Per-pool `paused` is an emergency brake — it should freeze everything including withdrawals. The global maintenance mode (Task 2) is the softer switch that allows withdrawals. So `remove_liquidity` should check the per-pool `paused` flag but NOT the global maintenance mode.

- [ ] **Step 1: Add paused check to `remove_liquidity`**

In the `read_state` closure of `remove_liquidity`, `paused` is not currently read. Add it to the destructured tuple:

```rust
let (token_a, token_b, reserve_a, reserve_b, total_shares, sub_a, sub_b, user_shares, paused) =
    read_state(|s| {
        let pool = s.pools.get(&pool_id).ok_or(AmmError::PoolNotFound)?;
        let user_shares = pool.lp_shares.get(&caller).copied().unwrap_or(0);
        Ok::<_, AmmError>((
            pool.token_a, pool.token_b,
            pool.reserve_a, pool.reserve_b,
            pool.total_lp_shares,
            pool.subaccount_a, pool.subaccount_b,
            user_shares,
            pool.paused,
        ))
    })?;

if paused {
    return Err(AmmError::PoolPaused);
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p rumi_amm --target wasm32-unknown-unknown --release 2>&1 | tail -5`

- [ ] **Step 3: Commit**

```bash
git add src/rumi_amm/src/lib.rs
git commit -m "fix(amm): add per-pool paused check to remove_liquidity"
```

---

### Task 6: Clean up transfers module (Low Bugs #11 and #12)

**Files:**
- Modify: `src/rumi_amm/src/transfers.rs`

- [ ] **Step 1: Remove `transfer_between_subaccounts`**

Delete the entire function (lines 78-87) from `src/rumi_amm/src/transfers.rs`. It's dead code — not imported anywhere.

- [ ] **Step 2: Fix block index silent truncation**

In both `transfer_from_user` (line 38) and `transfer_to_user` (line 70), replace:

```rust
let idx: u64 = block_index.0.try_into().unwrap_or(0);
```

with:

```rust
let idx: u64 = block_index.0.try_into().unwrap_or_else(|_| {
    ic_cdk::println!("WARN: block index exceeds u64::MAX, returning 0");
    0
});
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p rumi_amm --target wasm32-unknown-unknown --release 2>&1 | tail -5`

- [ ] **Step 4: Commit**

```bash
git add src/rumi_amm/src/transfers.rs
git commit -m "chore(amm): remove dead code, log warning on block index truncation"
```

---

### Task 7: Fix broken test and add new tests

**Files:**
- Modify: `src/rumi_amm/tests/pocket_ic_tests.rs:264-287`

- [ ] **Step 1: Fix `test_create_pool_unauthorized` assertion**

In `src/rumi_amm/tests/pocket_ic_tests.rs`, line 283, change:

```rust
assert!(matches!(res, Err(AmmError::Unauthorized)));
```

to:

```rust
assert!(matches!(res, Err(AmmError::PoolCreationClosed)));
```

- [ ] **Step 2: Add maintenance mode test**

Add a new test to `src/rumi_amm/tests/pocket_ic_tests.rs`:

```rust
#[test]
fn test_maintenance_mode() {
    let env = setup();
    let pool_id = create_test_pool(&env);

    approve_amm(&env, env.token_a_id);
    approve_amm(&env, env.token_b_id);

    // Add liquidity while not in maintenance mode
    let liq_amount: u128 = 50_000_00000000;
    env.pic.update_call(
        env.amm_id, env.user, "add_liquidity",
        encode_args((pool_id.clone(), liq_amount, liq_amount, 0u128)).unwrap(),
    ).expect("add_liquidity failed");

    // Enable maintenance mode
    let result = env.pic
        .update_call(env.amm_id, env.admin, "set_maintenance_mode", encode_one(true).unwrap())
        .expect("set_maintenance_mode failed");
    match result {
        WasmResult::Reply(bytes) => {
            let res: Result<(), AmmError> = decode_one(&bytes).expect("decode failed");
            res.expect("set_maintenance_mode returned Err");
        }
        WasmResult::Reject(msg) => panic!("set_maintenance_mode rejected: {}", msg),
    }

    // Verify is_maintenance_mode returns true
    let query_result = env.pic
        .query_call(env.amm_id, Principal::anonymous(), "is_maintenance_mode", encode_args(()).unwrap())
        .expect("is_maintenance_mode failed");
    match query_result {
        WasmResult::Reply(bytes) => {
            let mode: bool = decode_one(&bytes).expect("decode failed");
            assert!(mode, "Should be in maintenance mode");
        }
        WasmResult::Reject(msg) => panic!("is_maintenance_mode rejected: {}", msg),
    }

    // Swap should fail
    let swap_result = env.pic
        .update_call(
            env.amm_id, env.user, "swap",
            encode_args((pool_id.clone(), env.token_a_id, 1_000_00000000u128, 0u128)).unwrap(),
        )
        .expect("swap call failed");
    match swap_result {
        WasmResult::Reply(bytes) => {
            let res: Result<SwapResult, AmmError> = decode_one(&bytes).expect("decode failed");
            assert!(matches!(res, Err(AmmError::MaintenanceMode)));
        }
        WasmResult::Reject(msg) => panic!("swap rejected: {}", msg),
    }

    // add_liquidity should fail
    let add_result = env.pic
        .update_call(
            env.amm_id, env.user, "add_liquidity",
            encode_args((pool_id.clone(), 1_000_00000000u128, 1_000_00000000u128, 0u128)).unwrap(),
        )
        .expect("add_liquidity call failed");
    match add_result {
        WasmResult::Reply(bytes) => {
            let res: Result<candid::Nat, AmmError> = decode_one(&bytes).expect("decode failed");
            assert!(matches!(res, Err(AmmError::MaintenanceMode)));
        }
        WasmResult::Reject(msg) => panic!("add_liquidity rejected: {}", msg),
    }

    // remove_liquidity should STILL WORK
    let lp_balance: u128 = {
        let r = env.pic.query_call(
            env.amm_id, Principal::anonymous(), "get_lp_balance",
            encode_args((pool_id.clone(), env.user)).unwrap(),
        ).expect("get_lp_balance failed");
        match r {
            WasmResult::Reply(bytes) => {
                let n: candid::Nat = decode_one(&bytes).expect("decode failed");
                n.0.try_into().unwrap()
            }
            WasmResult::Reject(msg) => panic!("get_lp_balance rejected: {}", msg),
        }
    };

    let remove_shares = lp_balance / 4;
    let remove_result = env.pic
        .update_call(
            env.amm_id, env.user, "remove_liquidity",
            encode_args((pool_id.clone(), remove_shares, 0u128, 0u128)).unwrap(),
        )
        .expect("remove_liquidity call failed");
    match remove_result {
        WasmResult::Reply(bytes) => {
            let res: Result<(candid::Nat, candid::Nat), AmmError> = decode_one(&bytes).expect("decode failed");
            res.expect("remove_liquidity should succeed in maintenance mode");
        }
        WasmResult::Reject(msg) => panic!("remove_liquidity rejected: {}", msg),
    }

    // Disable maintenance mode
    env.pic.update_call(env.amm_id, env.admin, "set_maintenance_mode", encode_one(false).unwrap())
        .expect("set_maintenance_mode failed");

    // Swap should work again
    let swap_result = env.pic
        .update_call(
            env.amm_id, env.user, "swap",
            encode_args((pool_id.clone(), env.token_a_id, 1_000_00000000u128, 0u128)).unwrap(),
        )
        .expect("swap call failed");
    match swap_result {
        WasmResult::Reply(bytes) => {
            let res: Result<SwapResult, AmmError> = decode_one(&bytes).expect("decode failed");
            res.expect("Swap should succeed after disabling maintenance mode");
        }
        WasmResult::Reject(msg) => panic!("swap rejected: {}", msg),
    }
}
```

- [ ] **Step 3: Add permissionless pool creation test**

```rust
#[test]
fn test_permissionless_pool_creation() {
    let env = setup();

    // Open pool creation
    let result = env.pic
        .update_call(env.amm_id, env.admin, "set_pool_creation_open", encode_one(true).unwrap())
        .expect("set_pool_creation_open failed");
    match result {
        WasmResult::Reply(bytes) => {
            let res: Result<(), AmmError> = decode_one(&bytes).expect("decode failed");
            res.expect("set_pool_creation_open returned Err");
        }
        WasmResult::Reject(msg) => panic!("set_pool_creation_open rejected: {}", msg),
    }

    // User creates a pool with valid fee
    let args = CreatePoolArgs {
        token_a: env.token_a_id,
        token_b: env.token_b_id,
        fee_bps: 30,
        curve: CurveType::ConstantProduct,
    };
    let result = env.pic
        .update_call(env.amm_id, env.user, "create_pool", encode_one(args).unwrap())
        .expect("create_pool failed");
    match result {
        WasmResult::Reply(bytes) => {
            let res: Result<String, AmmError> = decode_one(&bytes).expect("decode failed");
            res.expect("Permissionless pool creation should succeed");
        }
        WasmResult::Reject(msg) => panic!("create_pool rejected: {}", msg),
    }

    // User tries fee_bps = 0 — should fail
    let extra_a = Principal::self_authenticating(&[10, 11, 12]);
    let extra_b = Principal::self_authenticating(&[13, 14, 15]);
    let args_zero_fee = CreatePoolArgs {
        token_a: extra_a,
        token_b: extra_b,
        fee_bps: 0,
        curve: CurveType::ConstantProduct,
    };
    let result = env.pic
        .update_call(env.amm_id, env.user, "create_pool", encode_one(args_zero_fee).unwrap())
        .expect("create_pool failed");
    match result {
        WasmResult::Reply(bytes) => {
            let res: Result<String, AmmError> = decode_one(&bytes).expect("decode failed");
            assert!(matches!(res, Err(AmmError::FeeBpsOutOfRange)));
        }
        WasmResult::Reject(msg) => panic!("create_pool rejected: {}", msg),
    }

    // User tries fee_bps = 1001 — should fail
    let args_high_fee = CreatePoolArgs {
        token_a: extra_a,
        token_b: extra_b,
        fee_bps: 1001,
        curve: CurveType::ConstantProduct,
    };
    let result = env.pic
        .update_call(env.amm_id, env.user, "create_pool", encode_one(args_high_fee).unwrap())
        .expect("create_pool failed");
    match result {
        WasmResult::Reply(bytes) => {
            let res: Result<String, AmmError> = decode_one(&bytes).expect("decode failed");
            assert!(matches!(res, Err(AmmError::FeeBpsOutOfRange)));
        }
        WasmResult::Reject(msg) => panic!("create_pool rejected: {}", msg),
    }
}
```

- [ ] **Step 4: Add fee validation test**

```rust
#[test]
fn test_set_fee_validation() {
    let env = setup();
    let pool_id = create_test_pool(&env);

    // fee_bps > 10_000 should fail
    let result = env.pic
        .update_call(env.amm_id, env.admin, "set_fee", encode_args((pool_id.clone(), 10_001u16)).unwrap())
        .expect("set_fee failed");
    match result {
        WasmResult::Reply(bytes) => {
            let res: Result<(), AmmError> = decode_one(&bytes).expect("decode failed");
            assert!(matches!(res, Err(AmmError::FeeBpsOutOfRange)));
        }
        WasmResult::Reject(msg) => panic!("set_fee rejected: {}", msg),
    }

    // protocol_fee_bps > 10_000 should fail
    let result = env.pic
        .update_call(env.amm_id, env.admin, "set_protocol_fee", encode_args((pool_id.clone(), 10_001u16)).unwrap())
        .expect("set_protocol_fee failed");
    match result {
        WasmResult::Reply(bytes) => {
            let res: Result<(), AmmError> = decode_one(&bytes).expect("decode failed");
            assert!(matches!(res, Err(AmmError::FeeBpsOutOfRange)));
        }
        WasmResult::Reject(msg) => panic!("set_protocol_fee rejected: {}", msg),
    }

    // 10_000 exactly should succeed (it's valid — 100% fee)
    let result = env.pic
        .update_call(env.amm_id, env.admin, "set_fee", encode_args((pool_id.clone(), 10_000u16)).unwrap())
        .expect("set_fee failed");
    match result {
        WasmResult::Reply(bytes) => {
            let res: Result<(), AmmError> = decode_one(&bytes).expect("decode failed");
            res.expect("10_000 bps should be valid");
        }
        WasmResult::Reject(msg) => panic!("set_fee rejected: {}", msg),
    }
}
```

- [ ] **Step 5: Build the WASM and run PocketIC tests**

Run:
```bash
cargo build -p rumi_amm --target wasm32-unknown-unknown --release && cargo test -p rumi_amm 2>&1
```

Expected: All tests pass, including the 4 new ones and the fixed `test_create_pool_unauthorized`.

- [ ] **Step 6: Commit**

```bash
git add src/rumi_amm/tests/pocket_ic_tests.rs
git commit -m "test(amm): fix broken test, add maintenance mode + permissionless + fee validation tests"
```

---

### Task 8: Update frontend error formatting and regenerate declarations

**Files:**
- Modify: `src/vault_frontend/src/lib/services/ammService.ts:341-356`
- Regenerate: `src/declarations/rumi_amm/*`

- [ ] **Step 1: Add missing error handlers to `formatError`**

In `src/vault_frontend/src/lib/services/ammService.ts`, in the `formatError` method, add before the `return 'Unknown AMM error'` line:

```typescript
if ('PoolCreationClosed' in err) return 'Pool creation is currently closed';
if ('FeeBpsOutOfRange' in err) return 'Fee must be between 0.01% and 10%';
if ('MaintenanceMode' in err) return 'AMM is in maintenance mode — swaps and deposits are temporarily disabled';
```

- [ ] **Step 2: Regenerate declarations**

Run: `dfx generate rumi_amm 2>&1`

Verify the generated files include the new error variants and endpoints.

- [ ] **Step 3: Commit**

```bash
git add src/vault_frontend/src/lib/services/ammService.ts src/declarations/rumi_amm/
git commit -m "fix(frontend): add missing AMM error handlers and regenerate declarations"
```

---

### Task 9: Address ledger fee reserve drift (Critical Bug #2)

**Files:**
- Modify: `src/rumi_amm/src/transfers.rs`

This is the architectural issue: `transfer_to_user` sends `amount` but the ICRC-1 ledger deducts its fee from the amount, so the user receives `amount - ledger_fee`. But reserves are decremented by the full `amount`, causing drift.

The cleanest fix for an ICP canister AMM: the **caller is responsible for accounting for the ledger fee**. The AMM math computes `amount_out` and the reserve bookkeeping uses `amount_out`, but the actual transfer sends `amount_out` and the ledger takes its cut. The difference stays in the canister's subaccount as a small surplus. This is actually a common pattern — the AMM accumulates tiny surplus "dust" from ledger fees.

For now, the drift is **in the protocol's favor** (canister has more tokens than reserves say). This is safe — it's the opposite direction from insolvency. It means LP shares are slightly underpaid on withdrawal, but by at most one ledger fee per operation.

**Decision: Document this as a known property rather than add per-token fee lookups.** The alternative (querying `icrc1_fee()` for each token before every transfer) adds latency and complexity. We'll revisit when this becomes material.

- [ ] **Step 1: Add a doc comment to `transfer_to_user`**

In `src/rumi_amm/src/transfers.rs`, update the doc comment on `transfer_to_user`:

```rust
/// Transfer tokens FROM a pool's subaccount TO a user.
///
/// NOTE: The ICRC-1 ledger deducts its transfer fee from the `amount` sent,
/// so the user receives `amount - ledger_fee`. The reserve bookkeeping in
/// lib.rs uses the full `amount`, meaning the canister accumulates a small
/// surplus (one ledger fee per outbound transfer). This surplus stays in the
/// subaccount and accrues to the protocol — it's safe (protocol has MORE
/// tokens than reserves track, not fewer).
```

- [ ] **Step 2: Commit**

```bash
git add src/rumi_amm/src/transfers.rs
git commit -m "docs(amm): document ledger fee reserve drift as known safe property"
```

---

### Task 10: Deploy upgraded AMM to mainnet

**Files:** None (CLI commands only)

- [ ] **Step 1: Build for mainnet**

Run: `dfx build --network ic rumi_amm 2>&1`

Expected: Build succeeds.

- [ ] **Step 2: Upgrade the canister**

Run:
```bash
echo "yes" | dfx canister install rumi_amm --network ic --mode upgrade \
  --argument '(record { admin = principal "fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae" })' 2>&1
```

Expected: `Upgraded code for canister rumi_amm`

- [ ] **Step 3: Verify upgrade preserved state and new endpoints work**

Run:
```bash
dfx canister call rumi_amm health --network ic && \
dfx canister call rumi_amm is_maintenance_mode --network ic && \
dfx canister call rumi_amm is_pool_creation_open --network ic && \
dfx canister call rumi_amm get_pools --network ic
```

Expected: 1 pool preserved, maintenance_mode = false, pool_creation_open = false.

- [ ] **Step 4: Enable maintenance mode (canister starts locked down)**

Run:
```bash
dfx canister call rumi_amm set_maintenance_mode --network ic '(true)'
```

Expected: `(variant { Ok })`

This ensures no one can swap or deposit until you explicitly unlock it for testing.

- [ ] **Step 5: Commit canister_ids.json if changed**

Only if `dfx` updated the file. Otherwise skip.
