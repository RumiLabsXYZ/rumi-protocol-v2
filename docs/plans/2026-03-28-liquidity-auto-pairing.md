# Liquidity Auto-Pairing Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** When a user types an amount in one liquidity input field, auto-populate the other field with the correctly paired amount based on live prices (existing pool ratio, or ICP price + 3USD virtual price for initial deposit).

**Architecture:** Purely frontend. On each input change, compute the paired amount. For an existing pool with reserves, use the reserve ratio. For an empty pool (initial deposit), fetch ICP price from `priceService` and 3USD virtual price from `threePoolService`, then compute the cross-rate. Display a small info line showing the effective price used.

**Tech Stack:** SvelteKit, TypeScript, existing `priceService` + `threePoolService` singletons.

---

## File Map

| File | Role |
|------|------|
| `src/vault_frontend/src/lib/components/swap/AmmLiquidityPanel.svelte` | The only file. All changes here. |
| `src/vault_frontend/src/lib/services/priceService.ts` | Read-only. Import `priceService` for ICP price. |
| `src/vault_frontend/src/lib/services/threePoolService.ts` | Read-only. Import `threePoolService` for 3USD virtual price. |

---

## Task 1: Fetch prices on mount and expose reactive price ratio

**File:** `src/vault_frontend/src/lib/components/swap/AmmLiquidityPanel.svelte`

Add imports and price-fetching logic. The key insight: there are two modes.

- **Pool has reserves** → ratio = `icpReserve / threeUsdReserve` (derived from pool state, already loaded)
- **Pool is empty** → ratio = `icpPriceUsd / threeUsdPriceUsd` (fetched from price feeds)

Add after the existing imports (line 6):
```typescript
import { priceService } from '../../services/priceService';
import { threePoolService } from '../../services/threePoolService';
```

Add new state variables after `let slippageBps = 50;` (line 24):
```typescript
// Price data for auto-pairing (only needed for empty pool / initial deposit)
let icpPriceUsd: number | null = null;
let threeUsdPriceUsd: number | null = null;
let priceLoading = false;
```

Add a reactive block that identifies if the pool is empty:
```typescript
$: isEmptyPool = !pool || (pool.reserve_a === 0n && pool.reserve_b === 0n);
```

Modify `loadPool()` to also fetch prices when the pool is empty. After the existing `loadPool` try block (after line 78, the `userLpShares` fetch), add:
```typescript
// Fetch external prices if pool is empty (needed for initial deposit pairing)
if (!pool || (pool.reserve_a === 0n && pool.reserve_b === 0n)) {
  priceLoading = true;
  try {
    const [icpP, tpStatus] = await Promise.all([
      priceService.getCurrentIcpPrice(),
      threePoolService.getPoolStatus(),
    ]);
    icpPriceUsd = icpP;
    // virtual_price is scaled by 1e18: divide to get USD value of 1 3USD LP token
    threeUsdPriceUsd = Number(tpStatus.virtual_price) / 1e18;
  } catch (e) {
    console.error('Failed to load prices for auto-pairing:', e);
  } finally {
    priceLoading = false;
  }
}
```

**Commit:** `feat(frontend): fetch ICP + 3USD prices for liquidity auto-pairing`

---

## Task 2: Add reactive auto-pairing logic

**File:** `src/vault_frontend/src/lib/components/swap/AmmLiquidityPanel.svelte`

Add a tracking variable to know which field the user last edited:
```typescript
let lastEdited: 'A' | 'B' | null = null;
```

Add two handler functions that fire on input. These replace the direct `bind:value` with `on:input` handlers:

```typescript
function onAmountAInput(e: Event) {
  const val = (e.target as HTMLInputElement).value;
  addAmountA = val;
  lastEdited = 'A';
  autoPairFromA(val);
}

function onAmountBInput(e: Event) {
  const val = (e.target as HTMLInputElement).value;
  addAmountB = val;
  lastEdited = 'B';
  autoPairFromB(val);
}

function autoPairFromA(val: string) {
  if (!val || val === '.' || parseFloat(val) === 0) {
    addAmountB = '';
    return;
  }
  try {
    const amtA = parseFloat(val);
    if (isNaN(amtA)) return;

    if (!isEmptyPool && pool) {
      // Use pool reserve ratio
      // threeUsdReserve and icpReserve are already in raw units with correct decimals
      // ratio: how many ICP per 3USD = icpReserve / threeUsdReserve (adjusted for decimals)
      const reserveA = Number(threeUsdReserve) / 1e8; // 3USD has 8 decimals
      const reserveB = Number(icpReserve) / 1e8;       // ICP has 8 decimals
      if (reserveA > 0) {
        addAmountB = (amtA * reserveB / reserveA).toFixed(8).replace(/\.?0+$/, '');
      }
    } else if (icpPriceUsd && threeUsdPriceUsd) {
      // Empty pool: use external prices
      // amtA is in 3USD. Value = amtA * threeUsdPriceUsd. Equivalent ICP = value / icpPriceUsd
      const icpAmount = amtA * threeUsdPriceUsd / icpPriceUsd;
      addAmountB = icpAmount.toFixed(8).replace(/\.?0+$/, '');
    }
  } catch {
    // Parse error — don't update the other field
  }
}

function autoPairFromB(val: string) {
  if (!val || val === '.' || parseFloat(val) === 0) {
    addAmountA = '';
    return;
  }
  try {
    const amtB = parseFloat(val);
    if (isNaN(amtB)) return;

    if (!isEmptyPool && pool) {
      const reserveA = Number(threeUsdReserve) / 1e8;
      const reserveB = Number(icpReserve) / 1e8;
      if (reserveB > 0) {
        addAmountA = (amtB * reserveA / reserveB).toFixed(8).replace(/\.?0+$/, '');
      }
    } else if (icpPriceUsd && threeUsdPriceUsd) {
      // amtB is in ICP. Value = amtB * icpPriceUsd. Equivalent 3USD = value / threeUsdPriceUsd
      const threeUsdAmount = amtB * icpPriceUsd / threeUsdPriceUsd;
      addAmountA = threeUsdAmount.toFixed(8).replace(/\.?0+$/, '');
    }
  } catch {
    // Parse error — don't update the other field
  }
}
```

**Commit:** `feat(frontend): auto-pair liquidity amounts from reserve ratio or price feeds`

---

## Task 3: Wire up input handlers in the template

**File:** `src/vault_frontend/src/lib/components/swap/AmmLiquidityPanel.svelte`

Replace the two `<input>` elements in the Add Liquidity section. Change from `bind:value` to `value` + `on:input` so we control updates without infinite reactive loops.

Replace the 3USD input (currently line ~192):
```svelte
<input type="number" step="any" min="0" placeholder="0.00"
       value={addAmountA} on:input={onAmountAInput}
       disabled={addLoading} class="token-input" />
```

Replace the ICP input (currently line ~197):
```svelte
<input type="number" step="any" min="0" placeholder="0.00"
       value={addAmountB} on:input={onAmountBInput}
       disabled={addLoading} class="token-input" />
```

**Commit:** `feat(frontend): wire auto-pair handlers to liquidity inputs`

---

## Task 4: Add price info display

**File:** `src/vault_frontend/src/lib/components/swap/AmmLiquidityPanel.svelte`

Show the user what rate is being used. Add between the ICP input group and the submit button (after the closing `</div>` of the ICP input-group, before the `<button class="submit-btn">`):

```svelte
{#if pool && pool.reserve_a > 0n && pool.reserve_b > 0n}
  {@const reserveA_f = Number(threeUsdReserve) / 1e8}
  {@const reserveB_f = Number(icpReserve) / 1e8}
  {@const rate = reserveA_f > 0 ? (reserveB_f / reserveA_f) : 0}
  <div class="price-info">
    1 3USD = {rate.toFixed(4)} ICP <span class="price-source">(pool ratio)</span>
  </div>
{:else if icpPriceUsd && threeUsdPriceUsd}
  {@const rate = threeUsdPriceUsd / icpPriceUsd}
  <div class="price-info">
    1 3USD = {rate.toFixed(4)} ICP <span class="price-source">(price feeds)</span>
  </div>
{:else if priceLoading}
  <div class="price-info">Loading prices...</div>
{/if}
```

Add the CSS for `.price-info` in the `<style>` block:
```css
.price-info {
  text-align: center;
  font-size: 0.75rem;
  color: var(--rumi-text-muted);
  padding: 0.375rem 0;
  margin-bottom: 0.25rem;
}

.price-source {
  opacity: 0.6;
  font-style: italic;
}
```

**Commit:** `feat(frontend): display effective price rate on liquidity panel`

---

## Task 5: Build, deploy frontend, and verify

1. Build and deploy:
```bash
dfx deploy vault_frontend --network ic
```

2. Verify on https://app.rumiprotocol.com/swap:
   - Navigate to the 3USD/ICP pool's "Add" tab
   - With the pool empty: type `100` in the 3USD field → ICP field auto-populates based on price feeds
   - Type `1` in the ICP field → 3USD field auto-populates inversely
   - The price info line shows `1 3USD = X.XXXX ICP (price feeds)`
   - Clear both fields, enter `0` → other field stays empty
   - After seeding the pool: the ratio switches to `(pool ratio)` source

**Commit:** n/a (deploy only)

---

## Summary

| Task | What | Files |
|------|------|-------|
| 1 | Fetch ICP + 3USD prices on mount | `AmmLiquidityPanel.svelte` |
| 2 | Auto-pair computation logic (reserve ratio or price feeds) | `AmmLiquidityPanel.svelte` |
| 3 | Wire `on:input` handlers to template | `AmmLiquidityPanel.svelte` |
| 4 | Price info display + CSS | `AmmLiquidityPanel.svelte` |
| 5 | Build, deploy, verify | deploy only |

All changes are in a single file. No backend changes. Tasks 1-3 can be combined into one commit if preferred since they're tightly coupled.
