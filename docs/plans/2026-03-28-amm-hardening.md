# AMM Hardening: Pending Claims, Refunds, & Frontend Precision

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add self-service recovery for failed transfers (pending claims system), refund input tokens on swap failure, fix frontend precision bugs, and harden backend against silent failures and anonymous callers.

**Architecture:** A new `PendingClaim` struct tracks failed outbound transfers. When `remove_liquidity` or `swap` can't deliver tokens, the record is written to a `Vec<PendingClaim>` in `AmmState` (with `#[serde(default)]` for backward compat). Users retry via `claim_pending(claim_id)`. Admin can query all claims via `get_pending_claims()` and force-resolve via `resolve_pending_claim(claim_id)`. The `withdraw_protocol_fees` partial-failure path also writes claims for the admin. Frontend gets BigInt-safe parsing/formatting to eliminate precision loss.

**Tech Stack:** Rust (IC canister), Candid, PocketIC tests, TypeScript/Svelte frontend

---

## File Map

| File | Action | Responsibility |
|------|--------|---------------|
| `src/rumi_amm/src/types.rs` | Modify | Add `PendingClaim` struct, `ClaimNotFound` error variant |
| `src/rumi_amm/src/state.rs` | Modify | Add `pending_claims: Vec<PendingClaim>`, `next_claim_id: u64`, update migration chain (V3→V2→V1) |
| `src/rumi_amm/src/lib.rs` | Modify | Add `claim_pending`, `get_pending_claims`, `resolve_pending_claim` endpoints; refactor `swap` rollback to attempt refund + record claim; refactor `remove_liquidity` to record claims; replace `if let Some(pool)` with `.expect()`; add anonymous caller check |
| `src/rumi_amm/rumi_amm.did` | Modify | Add new types and endpoints |
| `src/vault_frontend/src/lib/services/ammService.ts` | Modify | Fix `parseTokenAmount`, `formatTokenAmount`, add `ClaimNotFound` error handler |
| `src/vault_frontend/src/lib/components/swap/AmmLiquidityPanel.svelte` | Modify | Set error variable in `loadPool` catch block |
| `src/rumi_amm/tests/pocket_ic_tests.rs` | Modify | Add tests for pending claims, anonymous caller rejection |
| `src/declarations/rumi_amm/*` | Regenerate | `dfx generate rumi_amm` |

---

### Task 1: Add PendingClaim type and ClaimNotFound error

**Files:**
- Modify: `src/rumi_amm/src/types.rs`

- [ ] **Step 1: Add `PendingClaim` struct**

In `src/rumi_amm/src/types.rs`, add after the `CreatePoolArgs` struct (after line 90):

```rust
// ─── Pending Claims ───

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct PendingClaim {
    pub id: u64,
    pub pool_id: PoolId,
    pub claimant: Principal,
    pub token: Principal,
    pub subaccount: [u8; 32],
    pub amount: u128,
    pub reason: String,
    pub created_at: u64,
}
```

- [ ] **Step 2: Add `ClaimNotFound` error variant**

In the `AmmError` enum, add after `MaintenanceMode`:

```rust
ClaimNotFound,
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p rumi_amm --target wasm32-unknown-unknown --release 2>&1 | tail -5`

- [ ] **Step 4: Commit**

```bash
git add src/rumi_amm/src/types.rs
git commit -m "feat(amm): add PendingClaim type and ClaimNotFound error variant"
```

---

### Task 2: Add pending_claims to state with migration

**Files:**
- Modify: `src/rumi_amm/src/state.rs`

The on-chain stable memory currently has the shape: `{ admin, pools, pool_creation_open, maintenance_mode }`. Adding `pending_claims` and `next_claim_id` means a new migration level. The current `AmmState` becomes V3 in the fallback chain:

1. Try `AmmState` (new: has `pending_claims` + `next_claim_id`)
2. Try `AmmStateV3` (current on-chain: has `pool_creation_open` + `maintenance_mode`)
3. Try `AmmStateV2` (has `pool_creation_open` only)
4. Try `AmmStateV1` (original: `admin` + `pools` only)

- [ ] **Step 1: Add fields to `AmmState`**

In `src/rumi_amm/src/state.rs`, update `AmmState`:

```rust
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
}
```

Update the `Default` impl:

```rust
impl Default for AmmState {
    fn default() -> Self {
        Self {
            admin: Principal::anonymous(),
            pools: BTreeMap::new(),
            pool_creation_open: false,
            maintenance_mode: false,
            pending_claims: Vec::new(),
            next_claim_id: 0,
        }
    }
}
```

- [ ] **Step 2: Add `AmmStateV3` fallback struct and update migration**

Rename the existing `AmmStateV2` to `AmmStateV3` (since it now represents the second-to-latest shape), then add the new V3 (which is the current on-chain shape). Actually — simpler approach: add a new fallback for the current on-chain shape.

Add **before** the existing `AmmStateV2`:

```rust
/// V3 state shape (has pool_creation_open + maintenance_mode, but no pending_claims).
/// This is what's currently on-chain.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
struct AmmStateV3 {
    pub admin: Principal,
    pub pools: BTreeMap<PoolId, Pool>,
    pub pool_creation_open: bool,
    pub maintenance_mode: bool,
}
```

Update `load_from_stable_memory` to try 4 levels:

```rust
pub fn load_from_stable_memory() {
    let mut len_bytes = [0u8; 8];
    ic_cdk::api::stable::stable64_read(0, &mut len_bytes);
    let len = u64::from_le_bytes(len_bytes) as usize;

    if len == 0 {
        return;
    }

    let mut bytes = vec![0u8; len];
    ic_cdk::api::stable::stable64_read(8, &mut bytes);

    // Try current shape first, then V3 (has maintenance_mode), then V2 (pool_creation_open only), then V1 (original)
    if let Ok(state) = Decode!(&bytes, AmmState) {
        replace_state(state);
    } else if let Ok(v3) = Decode!(&bytes, AmmStateV3) {
        replace_state(AmmState {
            admin: v3.admin,
            pools: v3.pools,
            pool_creation_open: v3.pool_creation_open,
            maintenance_mode: v3.maintenance_mode,
            pending_claims: Vec::new(),
            next_claim_id: 0,
        });
    } else if let Ok(v2) = Decode!(&bytes, AmmStateV2) {
        replace_state(AmmState {
            admin: v2.admin,
            pools: v2.pools,
            pool_creation_open: v2.pool_creation_open,
            maintenance_mode: false,
            pending_claims: Vec::new(),
            next_claim_id: 0,
        });
    } else {
        let v1: AmmStateV1 = Decode!(&bytes, AmmStateV1)
            .expect("Failed to decode AMM state from stable memory (tried V4, V3, V2, V1)");
        replace_state(AmmState {
            admin: v1.admin,
            pools: v1.pools,
            pool_creation_open: false,
            maintenance_mode: false,
            pending_claims: Vec::new(),
            next_claim_id: 0,
        });
    }
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p rumi_amm --target wasm32-unknown-unknown --release 2>&1 | tail -5`

- [ ] **Step 4: Commit**

```bash
git add src/rumi_amm/src/state.rs
git commit -m "feat(amm): add pending_claims to state with V3 migration fallback"
```

---

### Task 3: Add pending claims helper and endpoints

**Files:**
- Modify: `src/rumi_amm/src/lib.rs`
- Modify: `src/rumi_amm/rumi_amm.did`

- [ ] **Step 1: Add helper function to record a pending claim**

In `src/rumi_amm/src/lib.rs`, after the `make_pool_id` helper (after line 73), add:

```rust
/// Record a failed outbound transfer as a pending claim so the user can retry.
fn record_pending_claim(
    pool_id: &PoolId,
    claimant: Principal,
    token: Principal,
    subaccount: [u8; 32],
    amount: u128,
    reason: &str,
) -> u64 {
    mutate_state(|s| {
        let id = s.next_claim_id;
        s.next_claim_id += 1;
        s.pending_claims.push(PendingClaim {
            id,
            pool_id: pool_id.clone(),
            claimant,
            token,
            subaccount,
            amount,
            reason: reason.to_string(),
            created_at: ic_cdk::api::time() / 1_000_000_000, // seconds
        });
        log!(INFO, "Pending claim #{} recorded: {} owes {} of token {} (pool {})",
            id, claimant, amount, token, pool_id);
        id
    })
}
```

- [ ] **Step 2: Add `claim_pending` endpoint**

In the admin endpoints section of `src/rumi_amm/src/lib.rs`, add after `set_maintenance_mode`:

```rust
/// Retry a failed outbound transfer. The original claimant or admin can call this.
#[update]
async fn claim_pending(claim_id: u64) -> Result<(), AmmError> {
    let caller = ic_cdk::caller();

    let claim = read_state(|s| {
        s.pending_claims
            .iter()
            .find(|c| c.id == claim_id)
            .cloned()
            .ok_or(AmmError::ClaimNotFound)
    })?;

    // Only the original claimant or admin can claim
    let is_admin = caller_is_admin().is_ok();
    if caller != claim.claimant && !is_admin {
        return Err(AmmError::Unauthorized);
    }

    // Attempt the transfer
    transfer_to_user(claim.token, claim.subaccount, claim.claimant, claim.amount)
        .await
        .map_err(|reason| AmmError::TransferFailed {
            token: claim.token.to_string(),
            reason,
        })?;

    // Transfer succeeded — remove the claim
    mutate_state(|s| {
        s.pending_claims.retain(|c| c.id != claim_id);
    });

    log!(INFO, "Pending claim #{} resolved: {} received {} of token {}",
        claim_id, claim.claimant, claim.amount, claim.token);

    Ok(())
}

/// Admin: view all pending claims.
#[query]
fn get_pending_claims() -> Vec<PendingClaim> {
    // Return all claims — admin uses this for monitoring.
    // Non-admin callers can also see (it's just claim metadata, no secrets).
    read_state(|s| s.pending_claims.clone())
}

/// Admin: force-remove a pending claim without transferring (e.g., after manual resolution).
#[update]
fn resolve_pending_claim(claim_id: u64) -> Result<(), AmmError> {
    caller_is_admin()?;
    mutate_state(|s| {
        let before = s.pending_claims.len();
        s.pending_claims.retain(|c| c.id != claim_id);
        if s.pending_claims.len() == before {
            return Err(AmmError::ClaimNotFound);
        }
        log!(INFO, "Pending claim #{} force-resolved by admin", claim_id);
        Ok(())
    })
}
```

- [ ] **Step 3: Update the Candid `.did` file**

In `src/rumi_amm/rumi_amm.did`, add the `PendingClaim` type after `SwapResult`:

```candid
type PendingClaim = record {
  id : nat64;
  pool_id : text;
  claimant : principal;
  token : principal;
  subaccount : blob;
  amount : nat;
  reason : text;
  created_at : nat64;
};
```

Add `ClaimNotFound` to the `AmmError` variant list:

```candid
ClaimNotFound;
```

Add the endpoints to the service block:

```candid
  // ── Claims ──
  claim_pending : (nat64) -> (variant { Ok; Err : AmmError });
  get_pending_claims : () -> (vec PendingClaim) query;
  resolve_pending_claim : (nat64) -> (variant { Ok; Err : AmmError });
```

- [ ] **Step 4: Add `use crate::types::PendingClaim` import if needed**

The wildcard `use crate::types::*` already covers it. No change needed — just verify.

- [ ] **Step 5: Verify it compiles**

Run: `cargo build -p rumi_amm --target wasm32-unknown-unknown --release 2>&1 | tail -5`

- [ ] **Step 6: Commit**

```bash
git add src/rumi_amm/src/lib.rs src/rumi_amm/rumi_amm.did
git commit -m "feat(amm): add claim_pending, get_pending_claims, resolve_pending_claim endpoints"
```

---

### Task 4: Refund input tokens on swap output failure + record claims

**Files:**
- Modify: `src/rumi_amm/src/lib.rs` (the `swap` function, lines 371-391)

Currently when the output transfer fails, the swap rolls back reserve accounting but abandons the user's input tokens. Fix: attempt to refund, and if the refund also fails, record a pending claim.

- [ ] **Step 1: Replace the swap output failure handler**

In `src/rumi_amm/src/lib.rs`, replace the `Err(reason)` arm of the output transfer match (lines 371-391):

```rust
        Err(reason) => {
            // Output transfer failed — rollback input reserve change
            mutate_state(|s| {
                let pool = s.pools.get_mut(&pool_id).expect("pool must exist: verified at start of swap");
                if is_a_to_b {
                    pool.reserve_a -= amount_in - protocol_fee;
                    pool.protocol_fees_a -= protocol_fee;
                } else {
                    pool.reserve_b -= amount_in - protocol_fee;
                    pool.protocol_fees_b -= protocol_fee;
                }
            });

            // Attempt to refund input tokens to user
            if let Err(refund_err) = transfer_to_user(ledger_in, sub_in, caller, amount_in).await {
                log!(INFO, "CRITICAL: swap output failed AND input refund failed for {}: {}. \
                     Recording pending claim for {} of {} tokens.", pool_id, refund_err, amount_in, ledger_in);
                record_pending_claim(&pool_id, caller, ledger_in, sub_in, amount_in, &format!(
                    "Swap output transfer failed, then refund failed: {}", refund_err
                ));
            }

            return Err(AmmError::TransferFailed {
                token: "output".to_string(),
                reason,
            });
        }
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p rumi_amm --target wasm32-unknown-unknown --release 2>&1 | tail -5`

- [ ] **Step 3: Commit**

```bash
git add src/rumi_amm/src/lib.rs
git commit -m "fix(amm): refund input tokens on swap output failure, record claim if refund fails"
```

---

### Task 5: Record pending claims on remove_liquidity transfer failure

**Files:**
- Modify: `src/rumi_amm/src/lib.rs` (the `remove_liquidity` function, lines 554-579)

Currently, failed transfers in `remove_liquidity` just log a warning. The user's shares are burned but they get no tokens and no way to retry. Fix: record pending claims.

- [ ] **Step 1: Replace the transfer failure handling in `remove_liquidity`**

In `src/rumi_amm/src/lib.rs`, replace lines 554-579 (from `// Send tokens to user` to the `if !transfer_errors.is_empty()` block):

```rust
    // Send tokens to user. If either fails, shares are already burned
    // but tokens remain in the pool subaccount. Record pending claims.
    let mut transfer_errors = Vec::new();

    if amount_a > 0 {
        if let Err(reason) = transfer_to_user(token_a, sub_a, caller, amount_a).await {
            log!(INFO, "WARN: remove_liquidity transfer_a failed for {}: {}. Recording pending claim.", pool_id, reason);
            record_pending_claim(&pool_id, caller, token_a, sub_a, amount_a, &format!(
                "remove_liquidity transfer_a failed: {}", reason
            ));
            transfer_errors.push(format!("token_a: {}", reason));
        }
    }

    if amount_b > 0 {
        if let Err(reason) = transfer_to_user(token_b, sub_b, caller, amount_b).await {
            log!(INFO, "WARN: remove_liquidity transfer_b failed for {}: {}. Recording pending claim.", pool_id, reason);
            record_pending_claim(&pool_id, caller, token_b, sub_b, amount_b, &format!(
                "remove_liquidity transfer_b failed: {}", reason
            ));
            transfer_errors.push(format!("token_b: {}", reason));
        }
    }

    if !transfer_errors.is_empty() {
        return Err(AmmError::TransferFailed {
            token: "output".to_string(),
            reason: format!("{}. Pending claims recorded — retry via claim_pending().", transfer_errors.join("; ")),
        });
    }
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p rumi_amm --target wasm32-unknown-unknown --release 2>&1 | tail -5`

- [ ] **Step 3: Commit**

```bash
git add src/rumi_amm/src/lib.rs
git commit -m "fix(amm): record pending claims on remove_liquidity transfer failure"
```

---

### Task 6: Replace `if let Some(pool)` with `.expect()` and add anonymous caller check

**Files:**
- Modify: `src/rumi_amm/src/lib.rs`

- [ ] **Step 1: Replace silent `if let Some(pool)` in `swap`**

In `src/rumi_amm/src/lib.rs`, the `swap` function has 3 `if let Some(pool)` patterns (at lines 346, 362, 377). Replace each with:

Line 346 (input recording):
```rust
            let pool = s.pools.get_mut(&pool_id).expect("pool must exist: verified at start of swap");
```

Line 362 (output deduction):
```rust
            let pool = s.pools.get_mut(&pool_id).expect("pool must exist: verified at start of swap");
```

Line 377 (rollback — this one was already fixed in Task 4, but verify it uses `.expect()`).

- [ ] **Step 2: Replace silent `if let Some(pool)` in `withdraw_protocol_fees`**

Lines 191 and 226 use `if let Some(pool)`. Replace:

Line 191 (optimistic deduct):
```rust
        let pool = s.pools.get_mut(&pool_id).expect("pool must exist: verified at start of withdraw_protocol_fees");
        pool.protocol_fees_a = 0;
        pool.protocol_fees_b = 0;
```

Line 226 (rollback):
```rust
        let pool = s.pools.get_mut(&pool_id).expect("pool must exist: verified at start of withdraw_protocol_fees");
        pool.protocol_fees_a += rollback_a;
        pool.protocol_fees_b += rollback_b;
```

- [ ] **Step 3: Add anonymous caller check helper**

After `caller_is_admin()` (line 50), add:

```rust
fn reject_anonymous() -> Result<(), AmmError> {
    if ic_cdk::caller() == Principal::anonymous() {
        return Err(AmmError::Unauthorized);
    }
    Ok(())
}
```

- [ ] **Step 4: Add anonymous check to user-facing endpoints**

At the top of `swap()` (after the maintenance mode check, before `let caller`):
```rust
    reject_anonymous()?;
```

At the top of `add_liquidity()` (after the maintenance mode check):
```rust
    reject_anonymous()?;
```

At the top of `remove_liquidity()` (before `let caller`):
```rust
    reject_anonymous()?;
```

- [ ] **Step 5: Verify it compiles**

Run: `cargo build -p rumi_amm --target wasm32-unknown-unknown --release 2>&1 | tail -5`

- [ ] **Step 6: Run unit tests**

Run: `cargo test -p rumi_amm --lib 2>&1`

- [ ] **Step 7: Commit**

```bash
git add src/rumi_amm/src/lib.rs
git commit -m "fix(amm): replace silent if-let with expect, add anonymous caller rejection"
```

---

### Task 7: Fix frontend precision (parseTokenAmount and formatTokenAmount)

**Files:**
- Modify: `src/vault_frontend/src/lib/services/ammService.ts`

- [ ] **Step 1: Replace `parseTokenAmount` with string-based parsing**

In `src/vault_frontend/src/lib/services/ammService.ts`, replace lines 108-112:

```typescript
export function parseTokenAmount(amount: string, decimals: number): bigint {
  const trimmed = amount.trim();
  if (trimmed === '' || trimmed === '.') throw new Error('Invalid amount');

  const parts = trimmed.split('.');
  if (parts.length > 2) throw new Error('Invalid amount');

  const whole = parts[0] || '0';
  let frac = parts.length === 2 ? parts[1] : '';

  // Pad or truncate fractional part to exact `decimals` digits
  if (frac.length > decimals) {
    frac = frac.slice(0, decimals);
  } else {
    frac = frac.padEnd(decimals, '0');
  }

  const raw = BigInt(whole) * BigInt(10 ** decimals) + BigInt(frac);
  if (raw < 0n) throw new Error('Invalid amount');
  return raw;
}
```

- [ ] **Step 2: Replace `formatTokenAmount` with BigInt-safe formatting**

Replace lines 114-125:

```typescript
export function formatTokenAmount(amount: bigint, decimals: number): string {
  const divisor = 10n ** BigInt(decimals);
  const whole = amount / divisor;
  const frac = amount % divisor;

  // Pad fractional part to full decimals width
  const fracStr = frac.toString().padStart(decimals, '0');

  // Show up to 4 decimal places for normal values, more for tiny values
  const value = amount;
  const threshold = divisor / 100n; // 0.01 in token units

  if (value > 0n && value < threshold) {
    // Tiny value — show up to 6 decimals
    const places = Math.min(decimals, 6);
    const trimmed = fracStr.slice(0, places).replace(/0+$/, '') || '0';
    return `${whole}.${trimmed}`;
  }

  // Normal: 4 decimal places, trim trailing zeros but keep at least 2
  let display = fracStr.slice(0, 4);
  display = display.replace(/0+$/, '');
  if (display.length === 0) display = '00';
  else if (display.length === 1) display += '0';

  return `${whole}.${display}`;
}
```

- [ ] **Step 3: Add `ClaimNotFound` to `formatError`**

In the `formatError` method, add before the `return 'Unknown AMM error'` line:

```typescript
if ('ClaimNotFound' in err) return 'Claim not found — it may have already been resolved';
```

- [ ] **Step 4: Commit**

```bash
git add src/vault_frontend/src/lib/services/ammService.ts
git commit -m "fix(frontend): BigInt-safe token parsing/formatting, add ClaimNotFound error handler"
```

---

### Task 8: Fix loadPool error swallowing in AmmLiquidityPanel

**Files:**
- Modify: `src/vault_frontend/src/lib/components/swap/AmmLiquidityPanel.svelte`

- [ ] **Step 1: Set error variable in catch block**

In `src/vault_frontend/src/lib/components/swap/AmmLiquidityPanel.svelte`, line 79-81, replace:

```typescript
    } catch (e) {
      console.error('Failed to load AMM pool:', e);
    } finally {
```

with:

```typescript
    } catch (e) {
      console.error('Failed to load AMM pool:', e);
      error = 'Failed to load pool data. Please try refreshing.';
    } finally {
```

- [ ] **Step 2: Commit**

```bash
git add src/vault_frontend/src/lib/components/swap/AmmLiquidityPanel.svelte
git commit -m "fix(frontend): show error message when pool loading fails"
```

---

### Task 9: Add PocketIC tests

**Files:**
- Modify: `src/rumi_amm/tests/pocket_ic_tests.rs`

- [ ] **Step 1: Add anonymous caller rejection test**

```rust
#[test]
fn test_anonymous_caller_rejected() {
    let env = setup();
    let pool_id = create_test_pool(&env);

    approve_amm(&env, env.token_a_id);
    approve_amm(&env, env.token_b_id);

    let liq_amount: u128 = 50_000_00000000;
    env.pic.update_call(
        env.amm_id, env.user, "add_liquidity",
        encode_args((pool_id.clone(), liq_amount, liq_amount, 0u128)).unwrap(),
    ).expect("add_liquidity failed");

    // Anonymous swap should fail
    let result = env.pic
        .update_call(
            env.amm_id, Principal::anonymous(), "swap",
            encode_args((pool_id.clone(), env.token_a_id, 1_000_00000000u128, 0u128)).unwrap(),
        )
        .expect("swap call failed");
    match result {
        WasmResult::Reply(bytes) => {
            let res: Result<SwapResult, AmmError> = decode_one(&bytes).expect("decode failed");
            assert!(matches!(res, Err(AmmError::Unauthorized)));
        }
        WasmResult::Reject(msg) => panic!("swap rejected: {}", msg),
    }

    // Anonymous add_liquidity should fail
    let result = env.pic
        .update_call(
            env.amm_id, Principal::anonymous(), "add_liquidity",
            encode_args((pool_id.clone(), 1_000_00000000u128, 1_000_00000000u128, 0u128)).unwrap(),
        )
        .expect("add_liquidity call failed");
    match result {
        WasmResult::Reply(bytes) => {
            let res: Result<candid::Nat, AmmError> = decode_one(&bytes).expect("decode failed");
            assert!(matches!(res, Err(AmmError::Unauthorized)));
        }
        WasmResult::Reject(msg) => panic!("add_liquidity rejected: {}", msg),
    }

    // Anonymous remove_liquidity should fail
    let result = env.pic
        .update_call(
            env.amm_id, Principal::anonymous(), "remove_liquidity",
            encode_args((pool_id.clone(), 1_000u128, 0u128, 0u128)).unwrap(),
        )
        .expect("remove_liquidity call failed");
    match result {
        WasmResult::Reply(bytes) => {
            let res: Result<(candid::Nat, candid::Nat), AmmError> = decode_one(&bytes).expect("decode failed");
            assert!(matches!(res, Err(AmmError::Unauthorized)));
        }
        WasmResult::Reject(msg) => panic!("remove_liquidity rejected: {}", msg),
    }
}
```

- [ ] **Step 2: Add pending claims endpoint test**

```rust
#[test]
fn test_pending_claims_endpoints() {
    let env = setup();
    let _pool_id = create_test_pool(&env);

    // get_pending_claims should return empty vec initially
    let result = env.pic
        .query_call(env.amm_id, Principal::anonymous(), "get_pending_claims", encode_args(()).unwrap())
        .expect("get_pending_claims failed");
    match result {
        WasmResult::Reply(bytes) => {
            let claims: Vec<PendingClaim> = decode_one(&bytes).expect("decode failed");
            assert!(claims.is_empty(), "Should have no pending claims initially");
        }
        WasmResult::Reject(msg) => panic!("get_pending_claims rejected: {}", msg),
    }

    // claim_pending for non-existent claim should fail
    let result = env.pic
        .update_call(env.amm_id, env.admin, "claim_pending", encode_one(999u64).unwrap())
        .expect("claim_pending failed");
    match result {
        WasmResult::Reply(bytes) => {
            let res: Result<(), AmmError> = decode_one(&bytes).expect("decode failed");
            assert!(matches!(res, Err(AmmError::ClaimNotFound)));
        }
        WasmResult::Reject(msg) => panic!("claim_pending rejected: {}", msg),
    }

    // resolve_pending_claim for non-existent claim should fail
    let result = env.pic
        .update_call(env.amm_id, env.admin, "resolve_pending_claim", encode_one(999u64).unwrap())
        .expect("resolve_pending_claim failed");
    match result {
        WasmResult::Reply(bytes) => {
            let res: Result<(), AmmError> = decode_one(&bytes).expect("decode failed");
            assert!(matches!(res, Err(AmmError::ClaimNotFound)));
        }
        WasmResult::Reject(msg) => panic!("resolve_pending_claim rejected: {}", msg),
    }
}
```

Note: Testing actual pending claim creation (from a failed transfer) would require a flaky ledger that rejects transfers on command. The endpoint logic test above verifies the happy-path wiring. A full integration test with failure injection is a future enhancement.

- [ ] **Step 3: Add `PendingClaim` import**

At the top of `src/rumi_amm/tests/pocket_ic_tests.rs`, the `use rumi_amm::types::*` already imports `PendingClaim`. Verify this compiles.

- [ ] **Step 4: Build WASM and run all tests**

```bash
cargo build -p rumi_amm --target wasm32-unknown-unknown --release && \
POCKET_IC_BIN=/tmp/pocket-ic cargo test -p rumi_amm 2>&1
```

Expected: All tests pass (11 unit + 16 PocketIC integration).

- [ ] **Step 5: Commit**

```bash
git add src/rumi_amm/tests/pocket_ic_tests.rs
git commit -m "test(amm): add anonymous caller rejection and pending claims endpoint tests"
```

---

### Task 10: Regenerate declarations and deploy

**Files:**
- Regenerate: `src/declarations/rumi_amm/*`
- CLI commands for mainnet deployment

- [ ] **Step 1: Regenerate declarations**

```bash
dfx generate rumi_amm 2>&1
```

Verify `PendingClaim` and `ClaimNotFound` appear in the generated files.

- [ ] **Step 2: Commit declarations**

```bash
git add src/declarations/rumi_amm/
git commit -m "chore(amm): regenerate declarations with pending claims types"
```

- [ ] **Step 3: Build for mainnet**

```bash
dfx build --network ic rumi_amm 2>&1
```

- [ ] **Step 4: Upgrade the canister**

```bash
echo "yes" | dfx canister install rumi_amm --network ic --mode upgrade \
  --argument '(record { admin = principal "fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae" })' 2>&1
```

- [ ] **Step 5: Verify upgrade preserved state and new endpoints work**

```bash
dfx canister call rumi_amm health --network ic && \
dfx canister call rumi_amm is_maintenance_mode --network ic && \
dfx canister call rumi_amm get_pending_claims --network ic && \
dfx canister call rumi_amm get_pools --network ic
```

Expected: 1 pool preserved, maintenance_mode = true, pending_claims = empty vec.

- [ ] **Step 6: Commit canister_ids.json if changed**

Only if `dfx` updated the file. Otherwise skip.
