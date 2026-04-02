# Explorer Activity Page Fixes — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Log EVERY operation across ALL 5 non-backend canisters (AMM, 3Pool, Stability Pool, Treasury, Liquidation Bot) and fix all broken frontend Activity page functionality.

**Architecture:** Five-layer fix — (1) AMM canister: liquidity + admin events, (2) 3Pool canister: liquidity + admin events, (3) Stability Pool: 11 new event types, (4) Treasury: full event infrastructure from scratch, (5) Liquidation Bot: production event logging. Then frontend Activity page rewrite to fix filtering, pagination, principal display, and data merging.

**Total operations being logged:** 35+ across 5 canisters (was 0 for liquidity/admin events).

**Tech Stack:** Rust (IC canisters), Svelte 5 (frontend), Candid IDL

---

## Phase 1: AMM Canister — Add Liquidity Event Tracking

### Task 1.1: Add AmmLiquidityEvent type

**File:** `src/rumi_amm/src/types.rs`

After the `AmmSwapEvent` struct (~line 119), add:

```rust
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub enum AmmLiquidityAction {
    AddLiquidity,
    RemoveLiquidity,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct AmmLiquidityEvent {
    pub id: u64,
    pub caller: Principal,
    pub pool_id: PoolId,
    pub action: AmmLiquidityAction,
    pub token_a: Principal,
    pub amount_a: u128,
    pub token_b: Principal,
    pub amount_b: u128,
    pub lp_shares: u128,
    pub timestamp: u64,
}
```

### Task 1.2: Add state storage for liquidity events

**File:** `src/rumi_amm/src/state.rs`

Add fields to `AmmState` (with `#[serde(default)]` for upgrade safety):

```rust
#[serde(default)]
pub liquidity_events: Vec<AmmLiquidityEvent>,
#[serde(default)]
pub next_liquidity_event_id: u64,
```

Add recording method after `record_swap_event`:

```rust
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
```

### Task 1.3: Record events in add_liquidity and remove_liquidity

**File:** `src/rumi_amm/src/lib.rs`

In `add_liquidity()` — insert `mutate_state` call just before the final `Ok(shares)` return. Need to capture `token_a` and `token_b` from the pool read earlier in the function and pass them through.

In `remove_liquidity()` — insert `mutate_state` call just before the final `Ok((amount_a, amount_b))` return. Same pattern — capture token principals from pool.

### Task 1.4: Add query endpoints

**File:** `src/rumi_amm/src/lib.rs`

```rust
#[query]
fn get_amm_liquidity_events(start: u64, length: u64) -> Vec<AmmLiquidityEvent> {
    read_state(|s| {
        let start = start as usize;
        let length = length as usize;
        if start >= s.liquidity_events.len() {
            return vec![];
        }
        let end = std::cmp::min(start + length, s.liquidity_events.len());
        s.liquidity_events[start..end].to_vec()
    })
}

#[query]
fn get_amm_liquidity_event_count() -> u64 {
    read_state(|s| s.liquidity_events.len() as u64)
}
```

### Task 1.5: Update Candid interface

**File:** `src/rumi_amm/rumi_amm.did`

Add types and query methods to match the Rust code.

### Task 1.6: Build and verify AMM compiles

```bash
cargo build --target wasm32-unknown-unknown --release -p rumi_amm
```

### Task 1.7: Commit

Message: `Add liquidity event tracking to AMM canister`

---

## Phase 2: 3Pool Canister — Add Liquidity Event Tracking

### Task 2.1: Add LiquidityEvent type

**File:** `src/rumi_3pool/src/types.rs`

After the `SwapEvent` struct, add:

```rust
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub enum LiquidityAction {
    AddLiquidity,
    RemoveLiquidity,
    RemoveOneCoin,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct LiquidityEvent {
    pub id: u64,
    pub timestamp: u64,
    pub caller: Principal,
    pub action: LiquidityAction,
    /// Per-token amounts (3 elements: icUSD, ckUSDT, ckUSDC)
    pub amounts: [u128; 3],
    /// LP tokens minted or burned
    pub lp_amount: u128,
    /// For RemoveOneCoin: which coin index was withdrawn
    pub coin_index: Option<u8>,
    /// Fee charged (for RemoveOneCoin)
    pub fee: Option<u128>,
}
```

### Task 2.2: Add state storage

**File:** `src/rumi_3pool/src/state.rs`

Add to `ThreePoolState`:
```rust
#[serde(default)]
pub liquidity_events: Option<Vec<LiquidityEvent>>,
```

Add accessor methods like `liquidity_events()` / `liquidity_events_mut()`, following the swap_events pattern.

### Task 2.3: Record events in add_liquidity, remove_liquidity, remove_one_coin

**File:** `src/rumi_3pool/src/lib.rs`

- `add_liquidity()`: Record inside the `mutate_state` block around line 345, after `s.lp_total_supply += lp_minted`. Data available: `caller`, `amounts_arr`, `lp_minted`.
- `remove_liquidity()`: Record inside the `mutate_state` block around line 408, after the burn log. Data available: `caller`, `amounts`, `lp_burn`.
- `remove_one_coin()`: Record inside the `mutate_state` block around line 488, after the burn log. Data available: `caller`, `coin_index`, `amount`, `fee`, `lp_burn`.

### Task 2.4: Add query endpoints

**File:** `src/rumi_3pool/src/lib.rs`

Same pattern as AMM: `get_liquidity_events(start, length)` and `get_liquidity_event_count()`.

### Task 2.5: Update Candid interface

**File:** `src/rumi_3pool/rumi_3pool.did`

### Task 2.6: Build and verify 3Pool compiles

```bash
cargo build --target wasm32-unknown-unknown --release -p rumi_3pool
```

### Task 2.7: Commit

Message: `Add liquidity event tracking to 3Pool canister`

---

## Phase 3: NO BACKEND WORK NEEDED — Stability Pool Already Tracks Events

**Investigation result:** The REAL stability pool canister lives at `src/stability_pool/` (NOT `src/rumi_stability_pool/` which is dead code). It already has:

- ✅ `PoolEvent` type with `Deposit`, `Withdraw`, `ClaimCollateral`, `DepositAs3USD`, `InterestReceived` variants
- ✅ `push_event()` method called on every deposit, withdraw, claim, and interest receipt
- ✅ `pre_upgrade` / `post_upgrade` hooks with stable memory persistence
- ✅ `#[serde(default)]` on event fields for upgrade compatibility
- ✅ `get_pool_events(start, length)` and `get_pool_event_count()` query endpoints
- ✅ Frontend service layer (`stabilityPoolService.getPoolEvents()`) and caching (`fetchStabilityPoolEvents()`)

**Why Rob only sees InterestReceived:** Interest accrual events fire frequently and vastly outnumber deposit/withdraw events. The frontend loads the most recent page of events, which is dominated by InterestReceived spam. Per Rob's original instruction, InterestReceived should be filtered OUT of the explorer display since it's noise.

**Frontend fix needed in Phase 6:** Filter out `InterestReceived` events from stability pool display. Show only Deposit, Withdraw, ClaimCollateral, and DepositAs3USD.

---

## Phase 3b: Remove Dead Code — `src/rumi_stability_pool/`

The `src/rumi_stability_pool/` directory is an old, unused copy of the stability pool canister. The real canister lives at `src/stability_pool/` (referenced by dfx.json as package `stability_pool`). The dead code has no pre/post_upgrade hooks, no event tracking, and is not compiled or deployed.

### Task 3b.1: Remove the directory

```bash
rm -rf src/rumi_stability_pool/
```

### Task 3b.2: Remove from Cargo.toml workspace members (if listed)

Check `Cargo.toml` at the repo root — if `rumi_stability_pool` is listed in `[workspace] members`, remove it.

### Task 3b.3: Build to verify nothing breaks

```bash
cargo build --target wasm32-unknown-unknown --release -p stability_pool
```

### Task 3b.4: Commit

Message: `Remove dead code: src/rumi_stability_pool/ (real canister is src/stability_pool/)`

---

## Phase 4: Deploy AMM + 3Pool Canisters

(No stability pool deploy needed — it already tracks events.)

### Task 4.1: Deploy AMM canister

```bash
dfx deploy rumi_amm --network ic --argument '(record { admin = principal "fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae" })'
```

### Task 4.2: Deploy 3Pool canister

Check how 3Pool is deployed (may need specific upgrade args).

---

## Phase 5: Frontend Service Layer — Add Liquidity Event Fetching

### Task 5.1: Update ammService.ts

**File:** `src/vault_frontend/src/lib/services/ammService.ts`

Add methods:
```typescript
async getLiquidityEvents(start: bigint, length: bigint): Promise<any[]> {
    const actor = await this.getQueryActor();
    return await actor.get_amm_liquidity_events(start, length);
}

async getLiquidityEventCount(): Promise<bigint> {
    const actor = await this.getQueryActor();
    return await actor.get_amm_liquidity_event_count();
}
```

### Task 5.2: Update explorerService.ts

**File:** `src/vault_frontend/src/lib/services/explorer/explorerService.ts`

Add cached fetch functions for:
- `fetchAmmLiquidityEvents(start, length)`
- `fetchAmmLiquidityEventCount()`
- `fetch3PoolLiquidityEvents(start, length)`
- `fetch3PoolLiquidityEventCount()`
- `fetchStabilityPoolEvents(start, length)` (update to use new canister endpoint)
- `fetchStabilityPoolEventCount()`

### Task 5.3: Update threePoolService

Add query methods for the new 3Pool liquidity event canister endpoints. (Stability pool service already has `getPoolEvents` and `getPoolEventCount`.)

### Task 5.4: Update explorerFormatters.ts

**File:** `src/vault_frontend/src/lib/utils/explorerFormatters.ts`

Add formatters:
- `formatAmmLiquidityEvent(event)` — "Added liquidity: X tokenA + Y tokenB → Z LP shares" / "Removed liquidity: Z LP shares → X tokenA + Y tokenB"
- `format3PoolLiquidityEvent(event)` — similar for 3Pool
- `formatStabilityPoolEvent(event)` — update to handle Deposit/Withdraw/ClaimCollateral/LiquidationProcessed

### Task 5.5: Commit

Message: `Add frontend service layer for liquidity and stability pool events`

---

## Phase 6: Frontend Activity Page Rewrite

### Task 6.1: Rename "Governance" → "System" and "Who" → "Principal"

**File:** `src/vault_frontend/src/routes/explorer/activity/+page.svelte`

- Change `FILTERS` array: replace `{ key: 'governance', label: 'Governance' }` with `{ key: 'system', label: 'System' }`
- In the `filteredEvents` derived, update the governance/system mapping
- Change column header from "Who" to "Principal" everywhere it appears

### Task 6.2: Fix extractPrincipal in EventRow

**File:** `src/vault_frontend/src/lib/components/explorer/EventRow.svelte`

Replace the current extractPrincipal with:

```typescript
function extractPrincipal(event: any): string | null {
    // Handle both structures:
    // Protocol events: { OpenVault: { owner: Principal, ... } }
    // Stability pool: { event_type: { Deposit: { ... } }, caller: Principal }

    // Direct caller field (stability pool, swap events)
    if (event.caller) {
        if (typeof event.caller === 'object' && typeof event.caller.toText === 'function') {
            return event.caller.toText();
        }
        if (typeof event.caller === 'string' && event.caller.length > 10) {
            return event.caller;
        }
    }

    // Variant-wrapped events (protocol backend)
    const eventType = event.event_type ?? event;
    const variant = Object.keys(eventType)[0];
    const data = eventType[variant];
    if (!data) return null;

    for (const key of ['owner', 'caller', 'from', 'liquidator', 'redeemer']) {
        const val = data[key];
        if (val && typeof val === 'object' && typeof val.toText === 'function') {
            return val.toText();
        }
        if (typeof val === 'string' && val.length > 10) {
            return val;
        }
    }

    // Check if the variant data itself IS a vault (open_vault contains { vault: { owner } })
    if (data?.vault?.owner) {
        const owner = data.vault.owner;
        if (typeof owner === 'object' && typeof owner.toText === 'function') {
            return owner.toText();
        }
    }

    return null;
}
```

### Task 6.3: Fix "All" tab to merge all event sources

The "All" tab needs to load events from ALL sources (backend + DEX + stability pool) and merge by timestamp. This is the most complex fix.

Approach:
- When filter is `'all'`, load a page from each source in parallel
- Merge and sort by timestamp descending
- Client-side pagination on the merged result
- Total count = sum of all source counts

### Task 6.4: Fix liquidation pagination

When filter is `'liquidations'`, the current approach loads a page of ALL backend events then filters client-side, which produces empty pages.

Fix: Load ALL backend events (they're in append-only stable memory, typically <10k total), filter for liquidation categories, then paginate the filtered result. For small event counts this is fine. For large counts, the backend's `get_events_filtered` should add a category parameter (future optimization).

### Task 6.5: Update DEX tab to include liquidity events

When filter is `'dex'`, load:
- 3Pool swap events + 3Pool liquidity events
- AMM swap events + AMM liquidity events
- Merge all four, sort by timestamp, paginate

### Task 6.6: Fix Stability Pool tab — filter out InterestReceived spam

The stability pool already returns Deposit, Withdraw, ClaimCollateral, DepositAs3USD, and InterestReceived events. The problem is InterestReceived fires constantly and drowns out meaningful events. Filter it out client-side after fetching:

```typescript
const poolEvents = await fetchStabilityPoolEvents(BigInt(reverseStart), BigInt(length));
const meaningful = poolEvents.filter((evt: any) => {
    const key = Object.keys(evt.event_type ?? {})[0];
    return key !== 'InterestReceived';
});
```

This means pagination needs adjustment — we may need to fetch more than one page and filter, or load all events and paginate the filtered set (stability pool event counts are small enough for this).

### Task 6.7: Update System tab

When filter is `'system'`, load protocol backend events filtered for admin + system categories. Same client-side filter approach but with proper pagination (load enough pages to fill the view).

### Task 6.8: Verify frontend build

```bash
npm run build
```

### Task 6.9: Commit

Message: `Rewrite Activity page: fix filtering, pagination, principal display, merge all event sources`

---

## Phase 7: Address Page Updates

### Task 7.1: Update DEX tab on address page to include liquidity events

**File:** `src/vault_frontend/src/routes/explorer/address/[principal]/+page.svelte`

Update `loadDexEvents()` to also fetch liquidity events from both 3Pool and AMM, filter by principal, merge with swap events.

### Task 7.2: Add Stability Pool tab on address page

Currently filtered out. Add it back — fetch stability pool events and filter by the address's principal.

### Task 7.3: Commit

Message: `Update address page: add liquidity events to DEX tab, add stability pool tab`

---

## Phase 8: Deploy Frontend

### Task 8.1: Deploy frontend

```bash
dfx deploy vault_frontend --network ic
```

### Task 8.2: Verify live site

Walk through all Activity filters on the live site and confirm data loads correctly.

---

## Event Category Reference

For the "System" filter, these backend event variants are admin/system:
- `init`, `upgrade` — lifecycle
- `set_borrowing_fee`, `set_interest_rate`, `set_liquidation_bonus`, `set_redemption_fee_ceiling`, `set_redemption_fee_floor`, `set_recovery_target_cr`, `set_recovery_cr_multiplier`, `set_recovery_parameters`, `set_recovery_rate_curve` — parameter tuning
- `set_borrowing_fee_curve`, `set_interest_pool_share`, `set_interest_split`, `set_rate_curve_markers` — rate configuration
- `set_min_icusd_amount`, `set_global_icusd_mint_cap`, `set_ckstable_repay_fee`, `set_max_partial_liquidation_ratio` — limits
- `set_rmr_ceiling`, `set_rmr_ceiling_cr`, `set_rmr_floor`, `set_rmr_floor_cr` — recovery mode
- `set_stable_ledger_principal`, `set_treasury_principal`, `set_stability_pool_principal`, `set_liquidation_bot_principal`, `set_three_pool_canister` — canister config
- `set_bot_budget`, `set_reserve_redemptions_enabled`, `set_stable_token_enabled`, `set_healthy_cr` — feature flags
- `set_collateral_borrowing_fee`, `set_liquidation_protocol_share` — fee config
- `add_collateral_type`, `update_collateral_config`, `update_collateral_status` — collateral management
- `admin_mint`, `admin_sweep_to_treasury`, `admin_vault_correction` — admin operations
- `dust_forgiven`, `accrue_interest`, `redistribute_vault` — system operations
