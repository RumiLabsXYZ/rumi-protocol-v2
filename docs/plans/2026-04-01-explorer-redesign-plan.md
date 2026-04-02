# Explorer Redesign Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Reorganize the Explorer from 4 tabs (Dashboard, Events, Liquidations, Stats) into 2 tabs (Overview, Activity) + address detail pages, fixing all data display bugs along the way.

**Architecture:** The Explorer is a SvelteKit route group under `/explorer` with a shared layout for sub-nav. Data comes from 3 canisters (backend, 3Pool, AMM) via `explorerService.ts`. The redesign merges Dashboard+Stats into Overview, merges Events+Liquidations into Activity, fixes event filtering, adds principal display, adds missing token logos, and adds AMM trade history.

**Tech Stack:** Svelte 5 (runes), TypeScript, Tailwind CSS, Rust (IC canisters), Candid

---

## Phase 1: Backend — AMM Trade History

### Step 1.1: Add SwapEvent type to AMM canister

**File:** `/Users/robertripley/coding/rumi-protocol-v2/src/rumi_amm/src/types.rs`

Add after the `PendingClaim` struct:

```rust
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct AmmSwapEvent {
    pub id: u64,
    pub caller: Principal,
    pub pool_id: PoolId,
    pub token_in: Principal,
    pub amount_in: u128,
    pub token_out: Principal,
    pub amount_out: u128,
    pub fee: u128,
    pub timestamp: u64,
}
```

### Step 1.2: Add swap event storage to AMM state

**File:** `/Users/robertripley/coding/rumi-protocol-v2/src/rumi_amm/src/state.rs`

Add two fields to `AmmState`:

```rust
#[serde(default)]
pub swap_events: Vec<AmmSwapEvent>,
#[serde(default)]
pub next_swap_event_id: u64,
```

Add a `record_swap_event` method to `AmmState`:

```rust
pub fn record_swap_event(&mut self, caller: Principal, pool_id: PoolId, token_in: Principal, amount_in: u128, token_out: Principal, amount_out: u128, fee: u128) {
    let event = AmmSwapEvent {
        id: self.next_swap_event_id,
        caller,
        pool_id,
        token_in,
        amount_in,
        token_out,
        amount_out,
        fee,
        timestamp: ic_cdk::api::time(),
    };
    self.swap_events.push(event);
    self.next_swap_event_id += 1;
}
```

Add `AmmState` (with swap_events) as a new V-shape for migration in `load_from_stable_memory`. The current on-chain state becomes `AmmStateV4` (without swap_events). The new `AmmState` has them.

### Step 1.3: Record swap events in the swap function

**File:** `/Users/robertripley/coding/rumi-protocol-v2/src/rumi_amm/src/lib.rs`

In the `swap` function, after a successful swap (after `amount_out` is computed and before returning), add:

```rust
state.record_swap_event(caller, pool_id.clone(), token_in, amount_in, token_out, result.amount_out, result.fee);
```

### Step 1.4: Add query methods for swap events

**File:** `/Users/robertripley/coding/rumi-protocol-v2/src/rumi_amm/src/lib.rs`

Add two query methods:

```rust
#[ic_cdk::query]
fn get_amm_swap_events(start: u64, length: u64) -> Vec<AmmSwapEvent> {
    read_state(|s| {
        let start = start as usize;
        let length = length as usize;
        let end = std::cmp::min(start + length, s.swap_events.len());
        if start >= s.swap_events.len() {
            return vec![];
        }
        s.swap_events[start..end].to_vec()
    })
}

#[ic_cdk::query]
fn get_amm_swap_event_count() -> u64 {
    read_state(|s| s.swap_events.len() as u64)
}
```

### Step 1.5: Update AMM .did file

**File:** `/Users/robertripley/coding/rumi-protocol-v2/src/rumi_amm/rumi_amm.did`

Add the `AmmSwapEvent` type and two query methods to the Candid interface.

### Step 1.6: Build and test AMM canister locally

```bash
cd /Users/robertripley/coding/rumi-protocol-v2
cargo build --target wasm32-unknown-unknown --release -p rumi_amm
```

Verify it compiles. Commit.

---

## Phase 2: Bug Fixes (Independent of Redesign)

### Step 2.1: Add missing token logos

**Files to create** in `/Users/robertripley/coding/rumi-protocol-v2/src/vault_frontend/static/`:

We need logos for EXE, ckETH, nICP, BOB. BOB already has `bob-logo.png`. For the others, create simple SVG circle logos with the token symbol text as placeholder if official logos aren't available, or source official ones.

- Check if `bob-logo.png` already exists (it does per the file list)
- Source/create: `exe-logo.svg`, `cketh-logo.svg`, `nicp-logo.svg`
- If official SVGs aren't available, create minimal branded SVGs

### Step 2.2: Register all token logos in TokenBadge

**File:** `/Users/robertripley/coding/rumi-protocol-v2/src/vault_frontend/src/lib/components/explorer/TokenBadge.svelte`

Update the `logos` record to include all tokens:

```typescript
const logos: Record<string, string> = {
    ICP: '/icp-token-dark.svg',
    ckBTC: '/ckBTC_logo.svg',
    ckXAUT: '/ckXAUT_logo.svg',
    icUSD: '/icusd-logo_v3.svg',
    BOB: '/bob-logo.png',
    EXE: '/exe-logo.svg',
    ckETH: '/cketh-logo.svg',
    nICP: '/nicp-logo.svg',
};
```

Commit.

### Step 2.3: Fix sticky sub-nav underline overlap

**File:** `/Users/robertripley/coding/rumi-protocol-v2/src/vault_frontend/src/routes/explorer/+layout.svelte`

The sub-nav uses `sticky top-0 z-40`. The main app nav bar also sticks. When both are sticky, the purple underline from the explorer sub-nav peeks out under the green underline of the main nav.

Fix: Change `top-0` to `top-[3.5rem]` (or whatever the main nav height is — the main nav is `h-14` = 3.5rem) so the explorer sub-nav sticks below the main nav instead of overlapping. Or alternatively, remove `sticky` from the explorer header entirely and let it scroll normally — simpler and avoids the visual conflict.

Recommended: Remove `sticky` from the explorer header. Users can scroll back up. The main app nav already has Explorer as a link.

Change line 46 from:
```html
<header class="sticky top-0 z-40 border-b border-white/10 bg-gray-950/80 backdrop-blur-md">
```
to:
```html
<header class="border-b border-white/10 bg-gray-950/80">
```

Commit.

### Step 2.4: Add principal/caller column to EventRow

**File:** `/Users/robertripley/coding/rumi-protocol-v2/src/vault_frontend/src/lib/components/explorer/EventRow.svelte`

The event data contains the caller/owner but it's not displayed. Add extraction logic and a column.

In the script section, add a helper to extract the principal from event data:

```typescript
import { shortenPrincipal } from '$utils/explorerHelpers';

function extractPrincipal(event: any): string | null {
    // Try common event shapes
    const variant = Object.keys(event)[0];
    const data = event[variant];
    if (!data) return null;
    // Most events have 'owner' or 'caller' or 'from'
    for (const key of ['owner', 'caller', 'from', 'liquidator', 'redeemer']) {
        const val = data[key];
        if (val && typeof val === 'object' && typeof val.toText === 'function') {
            return val.toText();
        }
        if (typeof val === 'string' && val.length > 20) {
            return val;
        }
    }
    return null;
}

const principal = $derived(extractPrincipal(event));
```

Add a new `<td>` column between the Time and Type columns:

```html
<!-- Principal -->
<td class="px-4 py-3 text-xs text-gray-400 whitespace-nowrap">
    {#if principal}
        <a href="/explorer/address/{principal}" class="hover:text-blue-400 transition-colors font-mono">
            {shortenPrincipal(principal)}
        </a>
    {:else}
        <span class="text-gray-600">&mdash;</span>
    {/if}
</td>
```

Also update the Events page table header to include a "Who" column.

Commit.

### Step 2.5: Fix NaN% in collateral config display

**File:** `/Users/robertripley/coding/rumi-protocol-v2/src/vault_frontend/src/routes/explorer/stats/+page.svelte` (will be moved to Overview, but fix the logic now)

The NaN% comes from trying to format undefined/null interest rate and borrow fee values. Add null checks:

```typescript
// Before displaying, check for NaN
const interestRate = cfg?.interest_rate != null ? formatPercent(cfg.interest_rate) : '—';
const borrowFee = cfg?.borrowing_fee != null ? formatBps(cfg.borrowing_fee) : '—';
```

This fix will carry forward into the Overview page.

Commit.

---

## Phase 3: Navigation Restructure

### Step 3.1: Update layout nav links

**File:** `/Users/robertripley/coding/rumi-protocol-v2/src/vault_frontend/src/routes/explorer/+layout.svelte`

Change `navLinks` from:

```typescript
const navLinks = [
    { href: '/explorer', label: 'Dashboard', exact: true },
    { href: '/explorer/events', label: 'Events', exact: false },
    { href: '/explorer/liquidations', label: 'Liquidations', exact: false },
    { href: '/explorer/stats', label: 'Stats', exact: false },
];
```

to:

```typescript
const navLinks = [
    { href: '/explorer', label: 'Overview', exact: true },
    { href: '/explorer/activity', label: 'Activity', exact: false },
];
```

Commit.

### Step 3.2: Create the Activity page route

**File:** `/Users/robertripley/coding/rumi-protocol-v2/src/vault_frontend/src/routes/explorer/activity/+page.svelte`

This is a reworked version of the Events page with these changes:

1. Replace 8 horizontal sub-tabs with a dropdown `<select>` or pill group with 6 options: All, Vault Operations, Liquidations, DEX, Stability Pool, Admin/System (combine admin+system into one "Governance" option since they're both protocol-admin actions)
2. Add "Who" column with caller principal on every row
3. When "Liquidations" filter is active, show 4 summary cards at top (fetched from `fetchLiquidationStats()`)
4. For DEX filter, fetch from both 3Pool (`fetchSwapEvents`) and AMM (`fetchAmmSwapEvents`) and merge results
5. Fix filtering to work across all events (not per-page). For categories that are client-side filtered (vault_ops, liquidation, redemption, admin, system), the `get_events_filtered` endpoint already returns only non-AccrueInterest events. The fix: when a category is selected, still load all events for the page but filter them. However the "No X events on this page" bug is because page 1 might have no liquidation events even though page 5 does. Real fix: either paginate server-side by category (requires backend change), or scan across pages until we find matching events (expensive). Pragmatic fix: load all events (not paginated) for small categories like liquidations/admin/system since there are <100 of each. For vault_ops which is the majority, pagination works fine.

The page reuses existing components: `EventRow`, `Pagination`, `StatCard`.

Key structure:
```svelte
<script lang="ts">
    // Filter dropdown state
    let selectedFilter = $state('all');

    // Liquidation summary cards (shown only when filter === 'liquidations')
    let liquidationStats = $state(null);

    // Load appropriate data based on filter
    async function loadData(filter, page) {
        if (filter === 'dex') {
            // Merge 3Pool + AMM swap events
        } else if (filter === 'stability_pool') {
            // Load from stability pool canister
        } else {
            // Load from backend get_events_filtered
        }
    }
</script>
```

### Step 3.3: Merge Stats charts into Overview page

**File:** `/Users/robertripley/coding/rumi-protocol-v2/src/vault_frontend/src/routes/explorer/+page.svelte`

Add the Historical Trends section (TVL, Debt, CR charts with time toggles) from Stats to the bottom of the Dashboard page. This involves:

1. Import `fetchProtocolSnapshots` and `fetchSnapshotCount` from explorerService
2. Add the chart rendering logic (copy from stats page)
3. Add time range selector (24h, 7d, 30d, 90d, All)
4. Place it between Treasury & Revenue and Liquidation Health sections

Also merge the extra columns (Borrow Fee, Debt Ceiling) into the existing Collateral Breakdown table, with the NaN fix from Step 2.5.

Rename the page heading from "Protocol Explorer" to just keep it clean — the "Overview" tab label handles context.

Commit.

### Step 3.4: Add redirects for old routes

**File:** Create `/Users/robertripley/coding/rumi-protocol-v2/src/vault_frontend/src/routes/explorer/events/+page.svelte`

Replace the contents with a redirect to the new Activity page:

```svelte
<script lang="ts">
    import { goto } from '$app/navigation';
    import { onMount } from 'svelte';
    onMount(() => goto('/explorer/activity', { replaceState: true }));
</script>
```

Do the same for:
- `/explorer/liquidations/+page.svelte` → redirect to `/explorer/activity?filter=liquidations`
- `/explorer/stats/+page.svelte` → redirect to `/explorer`

Commit.

### Step 3.5: Add AMM card to Overview dashboard

**File:** `/Users/robertripley/coding/rumi-protocol-v2/src/vault_frontend/src/routes/explorer/+page.svelte`

Add a third pool card next to Stability Pool and 3Pool cards. Shows:
- Pool count
- Total liquidity (sum of all pool reserves in USD)
- Average swap fee

Requires adding `fetchAmmPools` to the explorer service to call the AMM canister's `get_pools()`.

Commit.

---

## Phase 4: Address Page Cleanup

### Step 4.1: Remove irrelevant tabs from address Activity section

**File:** `/Users/robertripley/coding/rumi-protocol-v2/src/vault_frontend/src/routes/explorer/address/[principal]/+page.svelte`

Change the activity filter tabs from the full `EVENT_CATEGORIES` list to only user-relevant ones:

```typescript
const addressTabs = [
    { key: 'all', label: 'All' },
    { key: 'vault_ops', label: 'Vault Operations' },
    { key: 'liquidation', label: 'Liquidations' },
    { key: 'stability_pool', label: 'Stability Pool' },
    { key: 'dex', label: 'DEX' },
];
```

Remove Admin and System tabs entirely from the address page.

### Step 4.2: Add 3Pool + AMM swap history to address pages

**File:** `/Users/robertripley/coding/rumi-protocol-v2/src/vault_frontend/src/routes/explorer/address/[principal]/+page.svelte`

When the "DEX" filter is selected, query both:
1. 3Pool `get_swap_events()` → filter by `caller === principal`
2. AMM `get_amm_swap_events()` → filter by `caller === principal`

Merge and sort by timestamp descending.

This requires fetching all swap events and filtering client-side (neither canister has a "by principal" query for swaps). For the 3Pool this is fine since there are few events. For AMM, same.

Commit.

---

## Phase 5: Frontend Service Layer

### Step 5.1: Add AMM service functions to explorerService

**File:** `/Users/robertripley/coding/rumi-protocol-v2/src/vault_frontend/src/lib/services/explorer/explorerService.ts`

Add:

```typescript
export async function fetchAmmPools(): Promise<any[]> {
    // Call AMM canister get_pools()
}

export async function fetchAmmSwapEvents(start: bigint, length: bigint): Promise<any[]> {
    // Call AMM canister get_amm_swap_events(start, length)
}

export async function fetchAmmSwapEventCount(): Promise<bigint> {
    // Call AMM canister get_amm_swap_event_count()
}
```

This requires creating an AMM actor in the data layer, similar to how 3Pool and stability pool actors are created.

### Step 5.2: Add AMM swap event formatting

**File:** `/Users/robertripley/coding/rumi-protocol-v2/src/vault_frontend/src/lib/utils/explorerFormatters.ts`

Add `formatAmmSwapEvent()` function following the same pattern as `formatSwapEvent()` for 3Pool events. Include pool ID in the summary.

Commit.

---

## Phase 6: Polish and Deploy

### Step 6.1: Update event count in Overview

The "Total Events" card should include 3Pool swap count + AMM swap count + protocol event count for a true total.

### Step 6.2: Verify on local dev server

```bash
cd /Users/robertripley/coding/rumi-protocol-v2
npm run dev
```

Walk through every page:
- `/explorer` — Overview with all sections, charts, all token logos
- `/explorer/activity` — All filters work, principal shown, DEX events merge 3Pool+AMM
- `/explorer/activity?filter=liquidations` — Summary cards appear
- `/explorer/address/nqh27-eplay-...` — Only relevant tabs, DEX shows swap history
- `/explorer/events` — Redirects to `/explorer/activity`
- `/explorer/liquidations` — Redirects to `/explorer/activity?filter=liquidations`
- `/explorer/stats` — Redirects to `/explorer`
- Scroll behavior — no sticky nav overlap

### Step 6.3: Deploy

Deploy AMM canister with upgrade argument, then deploy frontend asset canister.

```bash
dfx deploy rumi_amm --network ic --argument '(variant { Upgrade })'
dfx deploy vault_frontend --network ic
```

---

## File Change Summary

| File | Action |
|------|--------|
| `src/rumi_amm/src/types.rs` | Add `AmmSwapEvent` struct |
| `src/rumi_amm/src/state.rs` | Add swap event storage, migration shape |
| `src/rumi_amm/src/lib.rs` | Record swap events, add query methods |
| `src/rumi_amm/rumi_amm.did` | Add new types and queries |
| `src/vault_frontend/static/` | Add missing token logo SVGs |
| `src/vault_frontend/.../TokenBadge.svelte` | Register all token logos |
| `src/vault_frontend/.../+layout.svelte` (explorer) | Update nav links, fix sticky |
| `src/vault_frontend/.../explorer/+page.svelte` | Merge Stats charts + AMM card |
| `src/vault_frontend/.../explorer/activity/+page.svelte` | New Activity page |
| `src/vault_frontend/.../explorer/events/+page.svelte` | Redirect to Activity |
| `src/vault_frontend/.../explorer/liquidations/+page.svelte` | Redirect to Activity |
| `src/vault_frontend/.../explorer/stats/+page.svelte` | Redirect to Overview |
| `src/vault_frontend/.../EventRow.svelte` | Add principal column |
| `src/vault_frontend/.../explorerService.ts` | Add AMM service functions |
| `src/vault_frontend/.../explorerFormatters.ts` | Add AMM event formatter |
| `src/vault_frontend/.../address/[principal]/+page.svelte` | Remove Admin/System tabs, add DEX filter |
