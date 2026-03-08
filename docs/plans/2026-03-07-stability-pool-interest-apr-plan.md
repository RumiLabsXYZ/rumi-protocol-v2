# Stability Pool Interest Distribution & APR Display — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Distribute interest revenue pro-rata to stability pool depositors and display a live pool APR on the frontend.

**Architecture:** Backend calls a new `receive_interest_revenue` endpoint on the pool canister after each interest mint. Pool loops depositors and credits proportionally. Backend computes live APR from existing `weighted_average_interest_rate()` and exposes it on `ProtocolStatus`. Frontend reads and displays it.

**Tech Stack:** Rust (IC canisters), Svelte/TypeScript (frontend), Candid (interface definitions)

---

### Task 1: Add interest tracking fields to pool state & types

**Files:**
- Modify: `src/stability_pool/src/types.rs` (lines 46-57, DepositPosition; lines 167-176, StabilityPoolStatus; lines 178-186, UserStabilityPosition)
- Modify: `src/stability_pool/src/state.rs` (lines 12-34, StabilityPoolState; lines 36-56, Default impl)

**Step 1: Add `total_interest_earned_e8s` to `DepositPosition`**

In `src/stability_pool/src/types.rs`, add after `total_claimed_gains` (line 56):

```rust
    /// Lifetime interest earned by this depositor (e8s, for display).
    #[serde(default)]
    pub total_interest_earned_e8s: u64,
```

Update `DepositPosition::new()` to initialize it:

```rust
    total_interest_earned_e8s: 0,
```

**Step 2: Add `total_interest_received_e8s` to `StabilityPoolState`**

In `src/stability_pool/src/state.rs`, add after `pool_creation_timestamp` (line 32):

```rust
    /// Lifetime interest revenue received from backend (e8s).
    #[serde(default)]
    pub total_interest_received_e8s: u64,
```

Set default to `0` in the `Default` impl.

**Step 3: Add `total_interest_received_e8s` to `StabilityPoolStatus`**

In `src/stability_pool/src/types.rs`, add to `StabilityPoolStatus` (after line 175):

```rust
    pub total_interest_received_e8s: u64,
```

**Step 4: Add `total_interest_earned_e8s` to `UserStabilityPosition`**

In `src/stability_pool/src/types.rs`, add to `UserStabilityPosition` (after line 185):

```rust
    pub total_interest_earned_e8s: u64,
```

**Step 5: Update query helpers to populate new fields**

In `src/stability_pool/src/state.rs`, update `get_pool_status()` (line 390-399) to include:

```rust
    total_interest_received_e8s: self.total_interest_received_e8s,
```

Update `get_user_position()` (line 402-410) to include:

```rust
    total_interest_earned_e8s: pos.total_interest_earned_e8s,
```

**Step 6: Run tests**

```bash
cd src/stability_pool && cargo test
```

Expected: All 24 existing tests pass (new fields are `serde(default)` so no breakage).

**Step 7: Commit**

```bash
git add src/stability_pool/src/types.rs src/stability_pool/src/state.rs
git commit -m "feat(pool): add interest tracking fields to state and types"
```

---

### Task 2: Implement `distribute_interest_revenue` on pool state

**Files:**
- Modify: `src/stability_pool/src/state.rs` (add method to `impl StabilityPoolState`)
- Modify: `src/stability_pool/src/state.rs` (add tests in `mod tests`)

**Step 1: Write failing tests**

Add to the `mod tests` block in `src/stability_pool/src/state.rs`:

```rust
    #[test]
    fn test_distribute_interest_single_depositor() {
        let mut state = test_state();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);

        state.distribute_interest_revenue(icusd_ledger(), 5_00000000);

        let pos = state.deposits.get(&user_a()).unwrap();
        assert_eq!(pos.stablecoin_balances[&icusd_ledger()], 105_00000000);
        assert_eq!(pos.total_interest_earned_e8s, 5_00000000); // icUSD is 8 decimals = e8s
        assert_eq!(state.total_interest_received_e8s, 5_00000000);
        assert_eq!(state.total_stablecoin_balances[&icusd_ledger()], 105_00000000);
    }

    #[test]
    fn test_distribute_interest_proportional() {
        let mut state = test_state();
        // A has 75%, B has 25%
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 75_00000000);
        add_deposit_direct(&mut state, user_b(), icusd_ledger(), 25_00000000);

        state.distribute_interest_revenue(icusd_ledger(), 10_00000000);

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
        state.distribute_interest_revenue(icusd_ledger(), 5_00000000);
        assert_eq!(state.total_interest_received_e8s, 0);
    }

    #[test]
    fn test_distribute_interest_dust_handling() {
        let mut state = test_state();
        // 3 depositors with equal balances, interest = 10 (not divisible by 3)
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100);
        add_deposit_direct(&mut state, user_b(), icusd_ledger(), 100);
        add_deposit_direct(&mut state, Principal::from_slice(&[99]), icusd_ledger(), 100);

        state.distribute_interest_revenue(icusd_ledger(), 10);

        // Each gets floor(10 * 100/300) = 3. Dust = 10 - 9 = 1 goes to first depositor.
        let total: u64 = state.deposits.values()
            .map(|p| p.stablecoin_balances.get(&icusd_ledger()).copied().unwrap_or(0))
            .sum();
        assert_eq!(total, 310, "All interest must be accounted for");
        assert_eq!(state.total_stablecoin_balances[&icusd_ledger()], 310);
    }
```

**Step 2: Run tests to verify they fail**

```bash
cd src/stability_pool && cargo test
```

Expected: Compile error — `distribute_interest_revenue` not found.

**Step 3: Implement `distribute_interest_revenue`**

Add to `impl StabilityPoolState` in `src/stability_pool/src/state.rs`, after the `add_deposit` method:

```rust
    /// Distribute interest revenue pro-rata to all depositors of a given stablecoin.
    /// Called by the backend after minting interest to the pool canister.
    pub fn distribute_interest_revenue(&mut self, token_ledger: Principal, amount: u64) {
        if amount == 0 {
            return;
        }

        let total = self.total_stablecoin_balances.get(&token_ledger).copied().unwrap_or(0);
        if total == 0 {
            return; // No depositors hold this token — nothing to distribute
        }

        let decimals = self.stablecoin_registry.get(&token_ledger)
            .map(|c| c.decimals)
            .unwrap_or(8);

        let mut distributed: u64 = 0;
        let mut first_eligible: Option<Principal> = None;

        // Collect (principal, balance) pairs to avoid borrow conflict
        let holders: Vec<(Principal, u64)> = self.deposits.iter()
            .filter_map(|(p, pos)| {
                let bal = pos.stablecoin_balances.get(&token_ledger).copied().unwrap_or(0);
                if bal > 0 { Some((*p, bal)) } else { None }
            })
            .collect();

        for (principal, balance) in &holders {
            if first_eligible.is_none() {
                first_eligible = Some(*principal);
            }
            let credit = (amount as u128 * *balance as u128 / total as u128) as u64;
            if credit > 0 {
                if let Some(pos) = self.deposits.get_mut(principal) {
                    *pos.stablecoin_balances.entry(token_ledger).or_insert(0) += credit;
                    pos.total_interest_earned_e8s += normalize_to_e8s(credit, decimals);
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
                    pos.total_interest_earned_e8s += normalize_to_e8s(dust, decimals);
                }
            }
        }

        // Update aggregate totals
        *self.total_stablecoin_balances.entry(token_ledger).or_insert(0) += amount;
        self.total_interest_received_e8s += normalize_to_e8s(amount, decimals);
    }
```

**Step 4: Run tests to verify they pass**

```bash
cd src/stability_pool && cargo test
```

Expected: All tests pass including the 4 new ones.

**Step 5: Commit**

```bash
git add src/stability_pool/src/state.rs
git commit -m "feat(pool): implement pro-rata interest distribution to depositors"
```

---

### Task 3: Add `receive_interest_revenue` canister endpoint

**Files:**
- Modify: `src/stability_pool/src/lib.rs` (add update endpoint)
- Modify: `src/stability_pool/stability_pool.did` (add to service interface)

**Step 1: Add the canister endpoint**

In `src/stability_pool/src/lib.rs`, add after the `execute_liquidation` endpoint (around line 98):

```rust
/// Receive interest revenue from the protocol backend and distribute pro-rata to depositors.
/// Only callable by the protocol canister.
#[update]
pub fn receive_interest_revenue(token_ledger: Principal, amount: u64) -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    let expected = read_state(|s| s.protocol_canister_id);
    if caller != expected {
        return Err(StabilityPoolError::Unauthorized);
    }

    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }

    if !read_state(|s| s.stablecoin_registry.contains_key(&token_ledger)) {
        return Err(StabilityPoolError::TokenNotAccepted { ledger: token_ledger });
    }

    mutate_state(|s| s.distribute_interest_revenue(token_ledger, amount));

    log!(INFO, "Distributed {} interest for token {} from backend", amount, token_ledger);
    Ok(())
}
```

**Step 2: Update the .did file**

In `src/stability_pool/stability_pool.did`, add to the service block after `execute_liquidation` (line 196):

```candid
  receive_interest_revenue : (principal, nat64) -> (variant { Ok; Err : StabilityPoolError });
```

**Step 3: Verify compilation**

```bash
cd src/stability_pool && cargo build --target wasm32-unknown-unknown --release
```

Expected: Compiles successfully.

**Step 4: Commit**

```bash
git add src/stability_pool/src/lib.rs src/stability_pool/stability_pool.did
git commit -m "feat(pool): add receive_interest_revenue canister endpoint"
```

---

### Task 4: Backend calls pool after interest mint

**Files:**
- Modify: `src/rumi_protocol_backend/src/treasury.rs` (lines 163-190, `mint_interest_to_stability_pool`)

**Step 1: Add inter-canister call after successful mint**

Update `mint_interest_to_stability_pool` in `src/rumi_protocol_backend/src/treasury.rs` to notify the pool after minting:

```rust
/// Mint icUSD interest revenue to the stability pool canister.
/// The stability pool distributes this pro-rata to depositors.
pub async fn mint_interest_to_stability_pool(interest_share: ICUSD) {
    if interest_share.0 == 0 {
        return;
    }
    let (stability_pool, icusd_ledger) = read_state(|s| (s.stability_pool_canister, s.icusd_ledger_principal));
    if let Some(pool_principal) = stability_pool {
        match management::mint_icusd(interest_share, pool_principal).await {
            Ok(block_index) => {
                log!(
                    INFO,
                    "[treasury] Minted {} icUSD interest to stability pool (block {})",
                    interest_share.to_u64(),
                    block_index
                );

                // Notify pool to distribute interest pro-rata to depositors.
                // Fire-and-forget: failure is logged but does not block repayment.
                let amount = interest_share.to_u64();
                let result: Result<(Result<(), String>,), _> = ic_cdk::call(
                    pool_principal,
                    "receive_interest_revenue",
                    (icusd_ledger, amount),
                )
                .await;
                match result {
                    Ok((Ok(()),)) => {
                        log!(INFO, "[treasury] Pool acknowledged interest distribution ({} icUSD)", amount);
                    }
                    Ok((Err(e),)) => {
                        log!(INFO, "[treasury] WARNING: pool rejected interest distribution: {}", e);
                    }
                    Err(e) => {
                        log!(INFO, "[treasury] WARNING: pool interest notification call failed: {:?}", e);
                    }
                }
            }
            Err(e) => {
                log!(
                    INFO,
                    "[treasury] WARNING: stability pool interest mint failed ({} icUSD): {:?}",
                    interest_share.to_u64(),
                    e
                );
            }
        }
    }
}
```

**Note:** The inter-canister call return type deserialisation needs to match what the pool canister returns. The pool returns `Result<(), StabilityPoolError>` which Candid encodes as a variant. We'll use a generic `Result<(), String>` for the caller side since we don't import pool types into the backend. If the Candid decoding is tricky, an alternative is to just ignore the response entirely (pure fire-and-forget with `ic_cdk::spawn`). Test and adjust during implementation.

**Step 2: Verify compilation**

```bash
cargo build -p rumi_protocol_backend --target wasm32-unknown-unknown --release
```

Expected: Compiles successfully.

**Step 3: Commit**

```bash
git add src/rumi_protocol_backend/src/treasury.rs
git commit -m "feat(backend): notify stability pool after interest mint for pro-rata distribution"
```

---

### Task 5: Add `stability_pool_apr` to ProtocolStatus

**Files:**
- Modify: `src/rumi_protocol_backend/src/lib.rs` (lines 104-120, ProtocolStatus struct; lines 291-311, get_protocol_status fn)
- Modify: `src/rumi_protocol_backend/rumi_protocol_backend.did` (lines 316-332, ProtocolStatus type)

**Step 1: Add field to ProtocolStatus struct**

In `src/rumi_protocol_backend/src/lib.rs`, add to `ProtocolStatus` (after line 119):

```rust
    pub stability_pool_apr: f64,
```

**Step 2: Compute APR in `get_protocol_status`**

In the `get_protocol_status` function (line 291), add the computation. After line 310 (`interest_pool_share`), add:

```rust
        stability_pool_apr: {
            let total_debt = s.total_borrowed_icusd_amount().to_u64() as f64;
            let weighted_rate = s.weighted_average_interest_rate().to_f64();
            let pool_share = s.interest_pool_share.to_f64();
            // Pool TVL: query cached value or use 0
            // For now, use total_debt * weighted_rate * pool_share as the annual interest to pool.
            // APR = annual_interest_to_pool / pool_tvl
            // We need pool TVL. Since this is a query call, we can't do inter-canister calls.
            // Store last-known pool TVL in backend state, updated on each price check cycle.
            // For MVP: expose the raw annual interest figure and let frontend compute APR.
            // Better approach: cache pool TVL in backend state.
            // Final field: stability_pool_apr = weighted_rate * pool_share * total_debt / pool_tvl
            // If pool_tvl is 0, return 0.
            // See implementation note below.
            0.0 // placeholder — see Step 3
        },
```

**Step 3: Cache pool TVL in backend state**

The backend can't query the pool canister in a query call. Two options:
1. Cache pool TVL during the timer cycle (every 300s when `check_vaults` runs)
2. Compute APR on the frontend from data it already has (`weighted_avg_rate`, `interest_pool_share`, `total_debt` from ProtocolStatus + `pool_tvl` from pool status)

**Recommended: Option 2 (frontend computation).** The frontend already fetches both `ProtocolStatus` and `PoolStatus` on the stability pool page. This avoids adding cross-canister caching. Instead of `stability_pool_apr` on `ProtocolStatus`, add a new field `weighted_average_interest_rate` so the frontend has the raw rate.

**Revised Step 2:** Replace `stability_pool_apr` with `weighted_average_interest_rate`:

In `ProtocolStatus` struct:

```rust
    pub weighted_average_interest_rate: f64,
```

In `get_protocol_status`:

```rust
        weighted_average_interest_rate: s.weighted_average_interest_rate().to_f64(),
```

The frontend then computes: `APR = weighted_avg_rate * interest_pool_share * total_icusd_borrowed / pool_tvl`

**Step 4: Update the .did file**

In `src/rumi_protocol_backend/rumi_protocol_backend.did`, add to `ProtocolStatus` (after line 331):

```candid
  weighted_average_interest_rate : float64;
```

**Step 5: Verify compilation**

```bash
cargo build -p rumi_protocol_backend --target wasm32-unknown-unknown --release
```

**Step 6: Commit**

```bash
git add src/rumi_protocol_backend/src/lib.rs src/rumi_protocol_backend/rumi_protocol_backend.did
git commit -m "feat(backend): expose weighted_average_interest_rate on ProtocolStatus"
```

---

### Task 6: Frontend — compute and display pool APR

**Files:**
- Modify: `src/vault_frontend/src/lib/services/types.ts` (ProtocolStatusDTO)
- Modify: `src/vault_frontend/src/lib/services/protocol/queryOperations.ts` (getProtocolStatus mapping)
- Modify: `src/vault_frontend/src/lib/components/stability-pool/PoolStats.svelte`
- Modify: `src/vault_frontend/src/routes/stability-pool/+page.svelte` (pass new prop)

**Step 1: Update ProtocolStatusDTO**

In `src/vault_frontend/src/lib/services/types.ts`, add to `ProtocolStatusDTO` (after line 100):

```typescript
  weightedAverageInterestRate: number;
```

**Step 2: Map the new field in queryOperations**

In `src/vault_frontend/src/lib/services/protocol/queryOperations.ts`, add to the return object in `getProtocolStatus` (around line 40):

```typescript
        weightedAverageInterestRate: Number(canisterStatus.weighted_average_interest_rate),
```

**Step 3: Regenerate declarations**

```bash
dfx generate
```

This updates the TypeScript declarations to include the new Candid fields.

**Step 4: Pass APR data to PoolStats**

In `src/routes/stability-pool/+page.svelte`, the page already fetches both `protocolStatus` and `poolStatus`. Pass the needed values to `PoolStats`:

```svelte
<PoolStats {poolStatus} {protocolStatus} />
```

(Adjust the existing `<PoolStats>` tag to also pass `protocolStatus`.)

**Step 5: Compute and display APR in PoolStats**

In `src/vault_frontend/src/lib/components/stability-pool/PoolStats.svelte`:

Add the `protocolStatus` prop and APR computation:

```svelte
<script lang="ts">
  import type { PoolStatus } from '../../services/stabilityPoolService';
  import type { ProtocolStatusDTO } from '../../services/types';
  import { formatE8s, formatTokenAmount, symbolForLedger, decimalsForLedger } from '../../services/stabilityPoolService';

  export let poolStatus: PoolStatus | null = null;
  export let protocolStatus: ProtocolStatusDTO | null = null;

  // ... existing reactive declarations ...

  $: poolApr = (() => {
    if (!protocolStatus || !poolStatus || poolStatus.total_deposits_e8s === 0n) return null;
    const weightedRate = protocolStatus.weightedAverageInterestRate;
    const poolShare = protocolStatus.interestPoolShare ?? 0.75; // fallback
    const totalDebt = protocolStatus.totalIcusdBorrowed;
    const poolTvl = Number(poolStatus.total_deposits_e8s) / 1e8;
    if (poolTvl === 0) return null;
    const apr = (weightedRate * poolShare * totalDebt) / poolTvl;
    return (apr * 100).toFixed(2);
  })();
</script>
```

Note: `interestPoolShare` is already on `ProtocolStatus` from the backend (field `interest_pool_share`). Verify it's already mapped in `ProtocolStatusDTO` — if not, add it just like `weightedAverageInterestRate`.

Add the APR display to the stats row (after the Liquidations stat, before the closing `</div>`):

```svelte
    {#if poolApr !== null}
      <div class="stat-divider"></div>
      <div class="stat">
        <span class="stat-label">Interest APR</span>
        <span class="stat-value apr-value">{poolApr}%</span>
      </div>
    {/if}
```

Add CSS for the APR highlight:

```css
  .apr-value { color: var(--rumi-teal); }
```

**Step 6: Commit**

```bash
git add src/vault_frontend/
git commit -m "feat(frontend): compute and display stability pool interest APR"
```

---

### Task 7: Frontend — show per-user interest earned

**Files:**
- Modify: `src/vault_frontend/src/lib/services/stabilityPoolService.ts` (UserPosition type if needed)
- Modify: `src/vault_frontend/src/lib/components/stability-pool/UserAccount.svelte`

**Step 1: Display interest earned in UserAccount**

The `UserPosition` type comes from the generated declarations, which will include `total_interest_earned_e8s` after `dfx generate`. Add to the meta-row in `UserAccount.svelte` (around line 124-128), alongside the "Since" date:

```svelte
    <div class="meta-row">
      <div class="meta-item">
        <span class="meta-label">Since</span>
        <span class="meta-value">{depositDate}</span>
      </div>
      {#if userPosition && userPosition.total_interest_earned_e8s > 0n}
        <div class="meta-item">
          <span class="meta-label">Interest Earned</span>
          <span class="meta-value interest-earned">
            <span class="tv-dollar">$</span>{formatE8s(userPosition.total_interest_earned_e8s)}
          </span>
        </div>
      {/if}
    </div>
```

Add CSS:

```css
  .interest-earned { color: var(--rumi-teal); font-weight: 600; }
```

**Step 2: Commit**

```bash
git add src/vault_frontend/src/lib/components/stability-pool/UserAccount.svelte
git commit -m "feat(frontend): display per-user interest earned in stability pool position"
```

---

### Task 8: Update Candid .did for pool + add `collateral_price_e8s` to liquidation records

**Files:**
- Modify: `src/stability_pool/src/types.rs` (PoolLiquidationRecord, lines 138-146)
- Modify: `src/stability_pool/src/state.rs` (where PoolLiquidationRecord is created, ~line 351)
- Modify: `src/stability_pool/stability_pool.did` (StabilityPoolStatus, UserStabilityPosition, PoolLiquidationRecord)

**Step 1: Add `collateral_price_e8s` to PoolLiquidationRecord**

In `src/stability_pool/src/types.rs`, add to `PoolLiquidationRecord`:

```rust
    /// USD price of the collateral at liquidation time (e8s), for future ROI calculations.
    #[serde(default)]
    pub collateral_price_e8s: u64,
```

**Step 2: Update the liquidation record creation**

In `src/stability_pool/src/state.rs`, where `PoolLiquidationRecord` is built (~line 351), add:

```rust
    collateral_price_e8s: 0, // TODO: pass from backend in future update
```

Note: Passing the actual price requires the backend to include it in `LiquidatableVaultInfo` or the liquidation result. This is a data-structure-only change for now — the actual price plumbing is deferred but the field is in place.

**Step 3: Update all .did types**

In `src/stability_pool/stability_pool.did`:

Update `StabilityPoolStatus` to add:
```candid
  total_interest_received_e8s : nat64;
```

Update `UserStabilityPosition` to add:
```candid
  total_interest_earned_e8s : nat64;
```

Update `PoolLiquidationRecord` to add:
```candid
  collateral_price_e8s : nat64;
```

**Step 4: Verify compilation and tests**

```bash
cd src/stability_pool && cargo test
cargo build --target wasm32-unknown-unknown --release
```

**Step 5: Commit**

```bash
git add src/stability_pool/
git commit -m "feat(pool): add collateral_price_e8s to liquidation records + update .did interface"
```

---

### Task 9: End-to-end verification

**Step 1: Build both canisters**

```bash
cargo build -p rumi_protocol_backend --target wasm32-unknown-unknown --release
cd src/stability_pool && cargo build --target wasm32-unknown-unknown --release
```

**Step 2: Run all backend tests**

```bash
cargo test -p rumi_protocol_backend
```

**Step 3: Run all pool tests**

```bash
cd src/stability_pool && cargo test
```

**Step 4: Build frontend**

```bash
cd src/vault_frontend && npm run build
```

**Step 5: Verify the generated TypeScript declarations include new fields**

```bash
dfx generate
grep -n "weighted_average_interest_rate\|total_interest_earned\|total_interest_received\|collateral_price" src/declarations/**/*.ts
```

**Step 6: Final commit if any fixups needed, then push branch**

```bash
git push -u origin feat/stability-pool-interest-apr
```
