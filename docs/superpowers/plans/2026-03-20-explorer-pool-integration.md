# Explorer: Stability Pool & 3Pool Integration

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Surface stability pool and 3pool activity in the Protocol Explorer — swap/liquidity events in the main feed, and user positions on address pages.

**Architecture:** The 3pool canister currently has no swap event log, so Task 1 adds a `SwapEvent` vec to the canister state with a query endpoint. The frontend explorer then queries all three canisters (backend, stability pool, 3pool) and merges events by timestamp into a unified feed. Address pages gain new sections showing the user's stability pool deposit and 3pool LP position.

**Tech Stack:** Rust (IC canisters), SvelteKit (frontend), Candid (IDL)

---

## File Structure

### 3Pool Canister Changes
- **Modify:** `src/rumi_3pool/src/types.rs` — add `SwapEvent` struct
- **Modify:** `src/rumi_3pool/src/state.rs` — add `swap_events: Option<Vec<SwapEvent>>` to `ThreePoolState` with accessor methods
- **Modify:** `src/rumi_3pool/src/lib.rs` — record swap events in `swap()`, add `get_swap_events` query
- **Modify:** `src/rumi_3pool/rumi_3pool.did` — add `SwapEvent` type and `get_swap_events` query

### Frontend Changes
- **Modify:** `src/vault_frontend/src/lib/services/threePoolService.ts` — add `getSwapEvents()` and `getIcrc3Blocks()` query methods
- **Modify:** `src/vault_frontend/src/lib/services/stabilityPoolService.ts` — add `getLiquidationHistory()` already exists, verify it works for explorer
- **Modify:** `src/vault_frontend/src/lib/stores/explorerStore.ts` — add pool event fetching and merge logic
- **Modify:** `src/vault_frontend/src/lib/utils/eventFormatters.ts` — add formatters for pool events (new source type prefix)
- **Modify:** `src/vault_frontend/src/lib/components/explorer/EventRow.svelte` — handle pool event objects (different shape from backend events)
- **Modify:** `src/vault_frontend/src/routes/explorer/+page.svelte` — add "3Pool" and "Stability Pool" filter categories, fetch pool events on mount
- **Modify:** `src/vault_frontend/src/routes/explorer/address/[principal]/+page.svelte` — add SP position and 3pool LP position sections

### Declarations (generated after canister changes)
- **Modify:** `src/declarations/rumi_3pool/rumi_3pool.did` — copy from primary
- **Modify:** `src/declarations/rumi_3pool/rumi_3pool.did.js` — regenerate
- **Modify:** `src/declarations/rumi_3pool/rumi_3pool.did.d.ts` — regenerate

---

## Task 1: Add Swap Event Logging to 3Pool Canister

**Files:**
- Modify: `src/rumi_3pool/src/types.rs`
- Modify: `src/rumi_3pool/src/state.rs`
- Modify: `src/rumi_3pool/src/lib.rs`
- Modify: `src/rumi_3pool/rumi_3pool.did`

- [ ] **Step 1: Add `SwapEvent` struct to types.rs**

Add after the `VirtualPriceSnapshot` struct (~line 143):

```rust
/// A recorded swap event for explorer/analytics.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct SwapEvent {
    /// Sequential event index.
    pub id: u64,
    /// Timestamp in nanoseconds since UNIX epoch.
    pub timestamp: u64,
    /// The principal who initiated the swap.
    pub caller: Principal,
    /// Index of the input token (0, 1, or 2).
    pub token_in: u8,
    /// Index of the output token (0, 1, or 2).
    pub token_out: u8,
    /// Amount of input token (native decimals).
    pub amount_in: u128,
    /// Amount of output token received (native decimals).
    pub amount_out: u128,
    /// Fee charged (in output token units, native decimals).
    pub fee: u128,
}
```

- [ ] **Step 2: Add swap event storage to state.rs**

Add new field to `ThreePoolState` struct (after `authorized_burn_callers`):

```rust
    /// Swap event log for explorer/analytics queries.
    /// Option for upgrade compatibility — old state won't have this field.
    #[serde(default)]
    pub swap_events: Option<Vec<SwapEvent>>,
```

Add accessor methods to the `impl ThreePoolState` block:

```rust
    /// Get swap events vec (empty if None for upgrade compat).
    pub fn swap_events(&self) -> &Vec<SwapEvent> {
        static EMPTY: std::sync::LazyLock<Vec<SwapEvent>> =
            std::sync::LazyLock::new(Vec::new);
        self.swap_events.as_ref().unwrap_or(&EMPTY)
    }

    /// Get mutable swap events vec (initializes if None for upgrade compat).
    pub fn swap_events_mut(&mut self) -> &mut Vec<SwapEvent> {
        self.swap_events.get_or_insert_with(Vec::new)
    }
```

Also update the `Default` impl to include `swap_events: Some(Vec::new()),`.

- [ ] **Step 3: Record swap events in `swap()` function in lib.rs**

In the `swap()` function (around line 238, after updating state), add swap event recording:

```rust
    // 9. Record swap event for explorer
    mutate_state(|s| {
        let id = s.swap_events().len() as u64;
        s.swap_events_mut().push(SwapEvent {
            id,
            timestamp: ic_cdk::api::time(),
            caller,
            token_in: i,
            token_out: j,
            amount_in: dx,
            amount_out: output,
            fee,
        });
    });
```

Note: This is a second `mutate_state` call — the first one (step 8) updates balances. Combine them into one call to avoid the overhead:

```rust
    // 8. Update state + record swap event
    mutate_state(|s| {
        s.balances[i_idx] += dx;
        s.balances[j_idx] -= output + fee;
        s.admin_fees[j_idx] += admin_fee_share;

        // Record swap event for explorer
        let id = s.swap_events().len() as u64;
        s.swap_events_mut().push(SwapEvent {
            id,
            timestamp: ic_cdk::api::time(),
            caller,
            token_in: i,
            token_out: j,
            amount_in: dx,
            amount_out: output,
            fee,
        });
    });
```

Add `use crate::types::SwapEvent;` to imports if not already covered by the wildcard.

- [ ] **Step 4: Add `get_swap_events` query endpoint to lib.rs**

Add a new query function:

```rust
/// Query swap events for explorer. Returns events in the requested range.
/// If `start` + `length` exceeds the number of events, returns up to the end.
#[query]
pub fn get_swap_events(start: u64, length: u64) -> Vec<SwapEvent> {
    read_state(|s| {
        let events = s.swap_events();
        let total = events.len() as u64;
        if start >= total {
            return vec![];
        }
        let end = (start + length).min(total) as usize;
        events[start as usize..end].to_vec()
    })
}

/// Query total number of swap events.
#[query]
pub fn get_swap_event_count() -> u64 {
    read_state(|s| s.swap_events().len() as u64)
}
```

- [ ] **Step 5: Update Candid interface**

Add to `rumi_3pool.did`:

```candid
type SwapEvent = record {
    id : nat64;
    timestamp : nat64;
    caller : principal;
    token_in : nat8;
    token_out : nat8;
    amount_in : nat;
    amount_out : nat;
    fee : nat;
};
```

Add to the service block:

```candid
    get_swap_events : (nat64, nat64) -> (vec SwapEvent) query;
    get_swap_event_count : () -> (nat64) query;
```

- [ ] **Step 6: Build and verify**

Run: `cargo build --target wasm32-unknown-unknown --release -p rumi_3pool`
Expected: Compiles without errors.

- [ ] **Step 7: Commit**

```bash
git add src/rumi_3pool/
git commit -m "feat(3pool): add swap event logging and query endpoint

Record SwapEvent on each swap with caller, tokens, amounts, and fee.
Expose get_swap_events and get_swap_event_count query endpoints for
the explorer frontend."
```

---

## Task 2: Deploy 3Pool Canister & Regenerate Declarations

**Files:**
- Modify: `src/declarations/rumi_3pool/rumi_3pool.did`
- Modify: `src/declarations/rumi_3pool/rumi_3pool.did.js`
- Modify: `src/declarations/rumi_3pool/rumi_3pool.did.d.ts`

- [ ] **Step 1: Deploy 3pool canister to mainnet**

```bash
dfx deploy rumi_3pool --network ic
```

If the deploy fails because `dfx deploy` tries to pass init args, use:

```bash
dfx canister install rumi_3pool --mode upgrade --network ic --argument-type raw --argument ''
```

- [ ] **Step 2: Regenerate declarations**

```bash
dfx generate rumi_3pool
```

If `dfx generate` doesn't produce clean output, manually copy:

```bash
cp src/rumi_3pool/rumi_3pool.did src/declarations/rumi_3pool/rumi_3pool.did
```

And update `rumi_3pool.did.js` and `rumi_3pool.did.d.ts` to include the new `SwapEvent` type and the two new service methods.

- [ ] **Step 3: Verify the deployed canister**

```bash
dfx canister call rumi_3pool get_swap_event_count --network ic
```

Expected: `(0 : nat64)` (no swaps since upgrade, events will accumulate going forward).

- [ ] **Step 4: Commit**

```bash
git add src/declarations/rumi_3pool/
git commit -m "chore: regenerate 3pool declarations with swap event types"
```

---

## Task 3: Add Pool Event Fetching to Explorer Store

**Files:**
- Modify: `src/vault_frontend/src/lib/stores/explorerStore.ts`
- Modify: `src/vault_frontend/src/lib/services/threePoolService.ts`

- [ ] **Step 1: Add `getSwapEvents` and `getIcrc3Blocks` to threePoolService.ts**

Add to the `ThreePoolService` class queries section:

```typescript
  // ── Explorer queries ──

  async getSwapEvents(start: bigint, length: bigint): Promise<any[]> {
    const actor = await this.getQueryActor();
    return await actor.get_swap_events(start, length) as any[];
  }

  async getSwapEventCount(): Promise<bigint> {
    const actor = await this.getQueryActor();
    return await actor.get_swap_event_count() as bigint;
  }

  async getIcrc3Blocks(start: bigint, length: bigint): Promise<any> {
    const actor = await this.getQueryActor();
    return await actor.icrc3_get_blocks([{ start, length }]);
  }
```

Also add the `SwapEvent` interface:

```typescript
export interface SwapEvent {
  id: bigint;
  timestamp: bigint;
  caller: Principal;
  token_in: number;
  token_out: number;
  amount_in: bigint;
  amount_out: bigint;
  fee: bigint;
}
```

- [ ] **Step 2: Define a unified event type in explorerStore.ts**

Add at the top of `explorerStore.ts`:

```typescript
export type EventSource = 'backend' | 'stability_pool' | '3pool_swap' | '3pool_lp';

export interface UnifiedEvent {
  source: EventSource;
  timestamp: bigint | null;
  event: any;           // backend event object, LiquidationRecord, SwapEvent, or Icrc3Block
  globalIndex: number;  // unique within source
}
```

- [ ] **Step 3: Add pool event fetch functions**

Add to `explorerStore.ts`:

```typescript
import { stabilityPoolService } from '$lib/services/stabilityPoolService';
import { threePoolService } from '$lib/services/threePoolService';

// Pool events (fetched once, not paginated like backend events)
export const poolEvents = writable<UnifiedEvent[]>([]);
export const poolEventsLoading = writable(false);

export async function fetchPoolEvents() {
  poolEventsLoading.set(true);
  try {
    const results: UnifiedEvent[] = [];

    // Stability Pool: liquidation history
    const spLiquidations = await stabilityPoolService.getLiquidationHistory(100);
    for (const liq of spLiquidations) {
      results.push({
        source: 'stability_pool',
        timestamp: liq.timestamp,
        event: liq,
        globalIndex: Number(liq.vault_id), // use vault_id as identifier
      });
    }

    // 3Pool: swap events
    const swapCount = await threePoolService.getSwapEventCount();
    if (swapCount > 0n) {
      // Fetch last 200 swap events (newest)
      const fetchCount = 200n;
      const start = swapCount > fetchCount ? swapCount - fetchCount : 0n;
      const swapEvents = await threePoolService.getSwapEvents(start, fetchCount);
      for (const evt of swapEvents) {
        results.push({
          source: '3pool_swap',
          timestamp: evt.timestamp,
          event: evt,
          globalIndex: Number(evt.id),
        });
      }
    }

    // 3Pool: LP mint/burn events from ICRC-3
    const icrc3Result = await threePoolService.getIcrc3Blocks(0n, 500n);
    if (icrc3Result?.blocks) {
      for (const block of icrc3Result.blocks) {
        // Only include mint (add_liquidity) and burn (remove_liquidity) — skip transfers/approvals
        const tx = block.block; // Icrc3Value map
        const btype = extractIcrc3Field(tx, 'btype');
        if (btype === '1mint' || btype === '1burn') {
          const ts = extractIcrc3Field(tx, 'ts');
          results.push({
            source: '3pool_lp',
            timestamp: ts ? BigInt(ts) : null,
            event: { block: tx, btype, id: Number(block.id) },
            globalIndex: Number(block.id),
          });
        }
      }
    }

    poolEvents.set(results);
  } catch (e) {
    console.error('Failed to fetch pool events:', e);
  } finally {
    poolEventsLoading.set(false);
  }
}

// Helper to extract a field from an ICRC-3 value map
function extractIcrc3Field(value: any, fieldName: string): any {
  // ICRC-3 blocks come as nested Map Icrc3Value
  // The exact shape depends on how the Candid IDL decodes it
  if (Array.isArray(value)) {
    for (const [key, val] of value) {
      if (key === fieldName) {
        // Icrc3Value variant: {Text: "..."}, {Nat: bigint}, {Blob: Uint8Array}
        if (val?.Text !== undefined) return val.Text;
        if (val?.Nat !== undefined) return val.Nat;
        if (val?.Int !== undefined) return val.Int;
        return val;
      }
    }
  }
  // Object map shape
  if (value && typeof value === 'object' && fieldName in value) {
    return value[fieldName];
  }
  return null;
}
```

- [ ] **Step 4: Commit**

```bash
git add src/vault_frontend/src/lib/stores/explorerStore.ts src/vault_frontend/src/lib/services/threePoolService.ts
git commit -m "feat(explorer): add pool event fetching to explorer store

Fetch stability pool liquidations, 3pool swap events, and 3pool LP
mint/burn events. Define UnifiedEvent type for cross-source merging."
```

---

## Task 4: Add Pool Event Formatters

**Files:**
- Modify: `src/vault_frontend/src/lib/utils/eventFormatters.ts`

- [ ] **Step 1: Add pool event formatting functions**

Add at the bottom of `eventFormatters.ts`:

```typescript
// ─── Pool Event Formatting ───

import type { EventSource, UnifiedEvent } from '$lib/stores/explorerStore';

const THREE_POOL_TOKENS: Record<number, string> = { 0: 'icUSD', 1: 'ckUSDT', 2: 'ckUSDC' };

/** Get display type for a pool event. */
export function getPoolEventType(unified: UnifiedEvent): string {
  switch (unified.source) {
    case 'stability_pool':
      return 'SP Liquidation';
    case '3pool_swap':
      return '3Pool Swap';
    case '3pool_lp':
      return unified.event.btype === '1mint' ? '3Pool Add Liquidity' : '3Pool Remove Liquidity';
    default:
      return 'Unknown';
  }
}

/** Get badge color for a pool event. */
export function getPoolEventBadgeColor(unified: UnifiedEvent): string {
  switch (unified.source) {
    case 'stability_pool':
      return 'var(--rumi-danger)';
    case '3pool_swap':
      return 'var(--rumi-info, #60a5fa)';
    case '3pool_lp':
      return 'var(--rumi-purple-accent)';
    default:
      return 'var(--rumi-text-muted)';
  }
}

/** Get one-line summary for a pool event. */
export function getPoolEventSummary(unified: UnifiedEvent): string {
  const evt = unified.event;
  switch (unified.source) {
    case 'stability_pool': {
      const debt = formatAmount(evt.stables_consumed?.[0]?.[1] ?? 0n);
      return `SP absorbed Vault #${evt.vault_id} debt (${debt} icUSD)`;
    }
    case '3pool_swap': {
      const tokenIn = THREE_POOL_TOKENS[evt.token_in] ?? `token${evt.token_in}`;
      const tokenOut = THREE_POOL_TOKENS[evt.token_out] ?? `token${evt.token_out}`;
      const decimalsIn = evt.token_in === 0 ? 8 : 6;
      const decimalsOut = evt.token_out === 0 ? 8 : 6;
      return `Swapped ${formatAmount(evt.amount_in, decimalsIn)} ${tokenIn} → ${formatAmount(evt.amount_out, decimalsOut)} ${tokenOut}`;
    }
    case '3pool_lp': {
      const btype = evt.btype;
      if (btype === '1mint') return '3Pool liquidity added';
      if (btype === '1burn') return '3Pool liquidity removed';
      return '3Pool LP operation';
    }
    default:
      return 'Pool event';
  }
}

/** Get caller principal from a pool event. */
export function getPoolEventCaller(unified: UnifiedEvent): string | null {
  const evt = unified.event;
  switch (unified.source) {
    case '3pool_swap':
      return evt.caller?.toString?.() ?? null;
    case '3pool_lp': {
      // Extract from ICRC-3 block tx fields
      const block = evt.block;
      // Try to find 'from' or 'to' in the tx map
      const tx = extractNestedField(block, 'tx');
      if (tx) {
        const from = extractNestedField(tx, 'from');
        const to = extractNestedField(tx, 'to');
        const principal = from || to;
        if (principal?.Blob) {
          try { return Principal.fromUint8Array(new Uint8Array(principal.Blob)).toText(); } catch {}
        }
      }
      return null;
    }
    case 'stability_pool':
      return null; // SP liquidations don't have a caller field in the record
    default:
      return null;
  }
}

function extractNestedField(value: any, fieldName: string): any {
  if (Array.isArray(value)) {
    for (const entry of value) {
      const [key, val] = Array.isArray(entry) ? entry : [entry?.key, entry?.value];
      if (key === fieldName) return val;
    }
  }
  if (value && typeof value === 'object' && fieldName in value) {
    return value[fieldName];
  }
  return null;
}
```

Note: The `Principal` import may need to be added at the top of `eventFormatters.ts`:

```typescript
import { Principal } from '@dfinity/principal';
```

- [ ] **Step 2: Commit**

```bash
git add src/vault_frontend/src/lib/utils/eventFormatters.ts
git commit -m "feat(explorer): add pool event formatters

Type labels, badge colors, summaries, and caller extraction for
stability pool liquidations, 3pool swaps, and 3pool LP operations."
```

---

## Task 5: Update EventRow to Handle Pool Events

**Files:**
- Modify: `src/vault_frontend/src/lib/components/explorer/EventRow.svelte`

- [ ] **Step 1: Read the current EventRow.svelte**

Read the file to understand current structure before modifying.

- [ ] **Step 2: Add pool event support to EventRow**

The component needs to detect whether it's rendering a backend event or a pool event. Add an optional `unified` prop:

```svelte
export let unified: UnifiedEvent | null = null;
```

When `unified` is provided, use the pool formatters instead of the backend formatters:

```svelte
$: isPoolEvent = unified !== null;
$: displayType = isPoolEvent
  ? getPoolEventType(unified!)
  : getEventType(event);
$: displayColor = isPoolEvent
  ? getPoolEventBadgeColor(unified!)
  : getEventBadgeColor(event);
$: displaySummary = isPoolEvent
  ? getPoolEventSummary(unified!)
  : getEventSummary(event, vaultCollateralMap);
$: displayTimestamp = isPoolEvent && unified!.timestamp
  ? formatTimestamp(unified!.timestamp!)
  : (isPoolEvent ? '' : getEventTimestamp(event) ? formatTimestamp(getEventTimestamp(event)!) : '');
$: displayCaller = isPoolEvent
  ? getPoolEventCaller(unified!)
  : getEventCaller(event);
```

The link target should differ too — pool events don't have a detail page in the backend explorer, so they shouldn't link to `/explorer/event/{index}`. Either make them non-clickable or link to the relevant pool page.

- [ ] **Step 3: Commit**

```bash
git add src/vault_frontend/src/lib/components/explorer/EventRow.svelte
git commit -m "feat(explorer): EventRow supports pool events via unified prop

When a UnifiedEvent is passed, uses pool-specific formatters for type,
color, summary, timestamp, and caller display."
```

---

## Task 6: Update Explorer Main Page to Show Pool Events

**Files:**
- Modify: `src/vault_frontend/src/routes/explorer/+page.svelte`

- [ ] **Step 1: Import pool stores and add filter tabs**

Add imports:

```typescript
import { poolEvents, poolEventsLoading, fetchPoolEvents } from '$lib/stores/explorerStore';
import type { UnifiedEvent } from '$lib/stores/explorerStore';
```

Update the `EventCategory` type and filter list to include pool categories:

```typescript
// Extend the filter with pool sources
type ExplorerFilter = EventCategory | 'all' | '3pool';

const filters: { label: string; value: ExplorerFilter }[] = [
  { label: 'All', value: 'all' },
  { label: 'Vault Ops', value: 'vault' },
  { label: 'Liquidations', value: 'liquidation' },
  { label: 'Stability Pool', value: 'stability' },
  { label: '3Pool', value: '3pool' },
  { label: 'Redemptions', value: 'redemption' },
  { label: 'Admin', value: 'admin' },
];
```

Note: The existing "Stability Pool" tab shows backend-recorded SP events (deposits/withdrawals/claims logged in backend). The new "SP Liquidation" events come from the SP canister itself. Keep them merged under the existing "Stability Pool" tab — or add a sub-filter later. For simplicity, merge SP liquidation events into the "Stability Pool" filter and add a new "3Pool" filter for swaps and LP ops.

- [ ] **Step 2: Fetch pool events on mount**

In the `onMount`:

```typescript
onMount(async () => {
  fetchEvents(0);
  fetchPoolEvents(); // new
  const vaults = await fetchAllVaults();
  // ... existing vault map logic
});
```

- [ ] **Step 3: Merge and display pool events alongside backend events**

After backend events, show pool events (either interleaved by timestamp or in a separate section below). The simplest approach: when "3Pool" or "Stability Pool" filter is active, show those pool events. When "All" is active, show backend events first (paginated) with a "Pool Activity" section below.

For the "All" view, add below the backend events list:

```svelte
{#if $poolEvents.length > 0 && (selectedFilter === 'all' || selectedFilter === '3pool' || selectedFilter === 'stability')}
  <h2 class="section-title" style="margin-top:1.5rem;">Pool Activity</h2>
  <div class="events-list glass-card">
    {#each filteredPoolEvents as unified}
      <EventRow event={unified.event} unified={unified} index={unified.globalIndex} />
    {/each}
  </div>
{/if}
```

With a reactive filter:

```typescript
$: filteredPoolEvents = (() => {
  if (selectedFilter === '3pool') {
    return $poolEvents.filter(e => e.source === '3pool_swap' || e.source === '3pool_lp');
  }
  if (selectedFilter === 'stability') {
    return $poolEvents.filter(e => e.source === 'stability_pool');
  }
  if (selectedFilter === 'all') {
    return $poolEvents;
  }
  return [];
})();
```

- [ ] **Step 4: Commit**

```bash
git add src/vault_frontend/src/routes/explorer/+page.svelte
git commit -m "feat(explorer): show pool events in main explorer page

Fetch and display stability pool liquidations and 3pool swap/LP events.
New '3Pool' filter tab. Pool events shown in dedicated section."
```

---

## Task 7: Add Pool Positions to Address Page

**Files:**
- Modify: `src/vault_frontend/src/routes/explorer/address/[principal]/+page.svelte`

- [ ] **Step 1: Import services and fetch pool positions**

Add imports:

```typescript
import { stabilityPoolService } from '$lib/services/stabilityPoolService';
import { threePoolService } from '$lib/services/threePoolService';
import { formatAmount } from '$lib/utils/eventFormatters';
```

Add state variables:

```typescript
let spPosition: any = null;
let lpBalance: bigint = 0n;
let poolStatus: any = null;
```

In the `onMount`, after fetching vaults and events:

```typescript
// Fetch stability pool position
try {
  spPosition = await stabilityPoolService.getUserPosition(Principal.fromText(principalStr));
} catch (e) {
  console.error('Failed to fetch SP position:', e);
}

// Fetch 3pool LP balance
try {
  lpBalance = await threePoolService.getLpBalance(Principal.fromText(principalStr));
  if (lpBalance > 0n) {
    poolStatus = await threePoolService.getPoolStatus();
  }
} catch (e) {
  console.error('Failed to fetch 3pool position:', e);
}
```

- [ ] **Step 2: Add Stability Pool Position card**

After the vaults section, before the activity history:

```svelte
{#if spPosition}
  <h2 class="section-title">Stability Pool</h2>
  <div class="pool-position glass-card">
    <div class="position-row">
      <span class="position-label">Deposited</span>
      <span class="position-value">
        {formatAmount(spPosition.total_usd_value_e8s ?? 0n)} icUSD
      </span>
    </div>
    {#if spPosition.collateral_gains?.length > 0}
      <div class="position-row">
        <span class="position-label">Collateral Gains</span>
        <span class="position-value">
          {#each spPosition.collateral_gains as [ledger, amount]}
            {formatAmount(amount)} {resolveCollateralSymbol(ledger)}{' '}
          {/each}
        </span>
      </div>
    {/if}
  </div>
{/if}
```

- [ ] **Step 3: Add 3Pool LP Position card**

```svelte
{#if lpBalance > 0n}
  <h2 class="section-title">3Pool</h2>
  <div class="pool-position glass-card">
    <div class="position-row">
      <span class="position-label">LP Balance</span>
      <span class="position-value">{formatAmount(lpBalance)} 3USD</span>
    </div>
    {#if poolStatus}
      {@const share = poolStatus.lp_total_supply > 0n
        ? Number(lpBalance) / Number(poolStatus.lp_total_supply)
        : 0}
      <div class="position-row">
        <span class="position-label">Pool Share</span>
        <span class="position-value">{(share * 100).toFixed(2)}%</span>
      </div>
    {/if}
  </div>
{/if}
```

- [ ] **Step 4: Add CSS for position cards**

```css
.pool-position { padding: 1rem; margin-bottom: 1.5rem; }
.position-row { display: flex; justify-content: space-between; align-items: center; padding: 0.375rem 0; }
.position-label { font-size: 0.8125rem; color: var(--rumi-text-muted); }
.position-value { font-size: 0.9375rem; font-weight: 500; }
```

- [ ] **Step 5: Commit**

```bash
git add src/vault_frontend/src/routes/explorer/address/
git commit -m "feat(explorer): show SP deposit and 3pool LP position on address pages

Address pages now show stability pool deposit + collateral gains and
3pool LP balance + pool share percentage."
```

---

## Task 8: Update Stats Row on Explorer Main Page

**Files:**
- Modify: `src/vault_frontend/src/routes/explorer/+page.svelte`

- [ ] **Step 1: Add pool stats to the stats row**

Fetch pool status data on mount and display alongside event count:

```typescript
import { stabilityPoolService } from '$lib/services/stabilityPoolService';
import { threePoolService } from '$lib/services/threePoolService';

let spStatus: any = null;
let tpStatus: any = null;
```

In `onMount`:

```typescript
// Fetch pool stats for overview
try { spStatus = await stabilityPoolService.getPoolStatus(); } catch {}
try { tpStatus = await threePoolService.getPoolStatus(); } catch {}
```

Add stats cards:

```svelte
<div class="stats-row">
  <div class="stat glass-card">
    <span class="stat-label">Protocol Events</span>
    <span class="stat-value key-number">{$explorerEventsTotalCount.toLocaleString()}</span>
  </div>
  {#if spStatus}
    <div class="stat glass-card">
      <span class="stat-label">Stability Pool</span>
      <span class="stat-value key-number">{formatAmount(spStatus.total_deposits_e8s)} icUSD</span>
    </div>
  {/if}
  {#if tpStatus}
    <div class="stat glass-card">
      <span class="stat-label">3Pool TVL</span>
      <span class="stat-value key-number">
        {formatAmount(
          tpStatus.balances.reduce((sum: bigint, b: bigint, i: number) =>
            sum + (i === 0 ? b : b * 100n), 0n)
        )} USD
      </span>
    </div>
  {/if}
</div>
```

Note: The 3pool TVL calculation normalizes ckUSDT/ckUSDC (6 decimals) to e8s by multiplying by 100. This is an approximation for display — all three stablecoins are ~$1.

- [ ] **Step 2: Import `formatAmount` if not already imported**

```typescript
import { formatAmount } from '$lib/utils/eventFormatters';
```

- [ ] **Step 3: Commit**

```bash
git add src/vault_frontend/src/routes/explorer/+page.svelte
git commit -m "feat(explorer): show SP deposit total and 3pool TVL in stats row"
```

---

## Task 9: Deploy Frontend & Test

- [ ] **Step 1: Build frontend locally to verify no TypeScript errors**

```bash
cd src/vault_frontend && npm run build
```

Expected: Builds without errors.

- [ ] **Step 2: Deploy frontend to IC**

```bash
dfx deploy vault_frontend --network ic
```

- [ ] **Step 3: Manual testing checklist**

1. Open `https://app.rumiprotocol.com/explorer` — verify stats row shows protocol events + SP total + 3pool TVL
2. Click "3Pool" filter — verify swap/LP events appear (may be empty if no swaps since deploy)
3. Click "Stability Pool" filter — verify SP liquidation events appear
4. Visit an address page for a known SP depositor — verify "Stability Pool" section appears
5. Visit an address page for a known 3pool LP holder — verify "3Pool" section with LP balance appears
6. Do a test swap on the 3pool, then refresh explorer — verify the swap event appears

- [ ] **Step 4: Commit any fixes found during testing**

```bash
git add -A
git commit -m "fix(explorer): adjustments from manual testing"
```

---

## Task 10: Final Commit & PR

- [ ] **Step 1: Create feature branch and push**

```bash
git checkout -b feat/explorer-pool-integration
git push -u origin feat/explorer-pool-integration
```

- [ ] **Step 2: Create PR**

```bash
gh pr create --title "feat: Add stability pool & 3pool to explorer" --body "$(cat <<'EOF'
## Summary
- Add swap event logging to 3pool canister (SwapEvent struct, get_swap_events query)
- Show stability pool liquidations and 3pool swap/LP events in Protocol Explorer
- Show SP deposit position and 3pool LP balance on address pages
- Add SP total and 3pool TVL to explorer stats row

## Test plan
- [ ] Explorer main page loads with pool stats
- [ ] "3Pool" filter shows swap events (after at least one swap post-deploy)
- [ ] "Stability Pool" filter shows SP liquidation events
- [ ] Address page shows SP position for known depositor
- [ ] Address page shows 3pool LP balance for known LP holder
- [ ] New swap events appear in explorer after performing a swap

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```
