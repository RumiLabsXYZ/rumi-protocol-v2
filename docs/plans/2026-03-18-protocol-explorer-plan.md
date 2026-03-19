# Protocol Explorer Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a protocol-native block explorer at `/explorer` with activity feed, vault/address/event detail pages, liquidation history, and stats dashboard with historical charts powered by hourly snapshots.

**Architecture:** Backend adds an hourly `ProtocolSnapshot` stored in a new StableLog (MemoryIds 2 & 3), queried via `get_protocol_snapshots()`. Frontend adds 6 new routes under `/explorer`, a shared `explorerStore`, and a universal search bar. All queries use `publicActor` (no auth).

**Tech Stack:** Rust (ic_stable_structures, ciborium), SvelteKit 5, Tailwind CSS, Layerchart, Candid

---

## Phase 1: Backend — Snapshot System

### Step 1.1: Define ProtocolSnapshot struct

**File:** `src/rumi_protocol_backend/src/lib.rs`

Add after the `CollateralTotals` struct (~line 201):

```rust
#[derive(CandidType, Deserialize, Serialize, Debug, Clone)]
pub struct CollateralSnapshot {
    pub collateral_type: Principal,
    pub total_collateral: u64,
    pub total_debt: u64,
    pub vault_count: u64,
    pub price: f64,
}

#[derive(CandidType, Deserialize, Serialize, Debug, Clone)]
pub struct ProtocolSnapshot {
    pub timestamp: u64,
    pub total_collateral_value_usd: u64,
    pub total_debt: u64,
    pub total_vault_count: u64,
    pub collateral_snapshots: Vec<CollateralSnapshot>,
}
```

Add `use serde::Serialize;` to the imports if not already present.

Also add `GetSnapshotsArg`:

```rust
#[derive(CandidType, Deserialize)]
pub struct GetSnapshotsArg {
    pub start: u64,
    pub length: u64,
}
```

**Commit:** `feat(backend): add ProtocolSnapshot and CollateralSnapshot structs`

### Step 1.2: Add snapshot storage module

**File:** `src/rumi_protocol_backend/src/storage.rs`

Add new MemoryId constants alongside the existing ones:

```rust
const SNAPSHOT_INDEX_MEMORY_ID: MemoryId = MemoryId::new(2);
const SNAPSHOT_DATA_MEMORY_ID: MemoryId = MemoryId::new(3);
```

Add a new `SnapshotLog` type and thread-local, mirroring the existing event log pattern:

```rust
type SnapshotLog = StableLog<Vec<u8>, VMem, VMem>;

thread_local! {
    // ... existing EVENT_LOG ...

    static SNAPSHOT_LOG: RefCell<SnapshotLog> = RefCell::new(
        StableLog::init(
            MEMORY_MANAGER.with(|m| m.borrow().get(SNAPSHOT_INDEX_MEMORY_ID)),
            MEMORY_MANAGER.with(|m| m.borrow().get(SNAPSHOT_DATA_MEMORY_ID)),
        ).expect("failed to initialize snapshot log")
    );
}
```

Add encode/decode/record/iterate functions for snapshots:

```rust
fn encode_snapshot(snapshot: &crate::ProtocolSnapshot) -> Vec<u8> {
    let mut buf = vec![];
    ciborium::ser::into_writer(snapshot, &mut buf).expect("failed to encode snapshot");
    buf
}

fn decode_snapshot(bytes: &[u8]) -> crate::ProtocolSnapshot {
    ciborium::de::from_reader(bytes).expect("failed to decode snapshot")
}

pub fn record_snapshot(snapshot: &crate::ProtocolSnapshot) {
    let bytes = encode_snapshot(snapshot);
    SNAPSHOT_LOG.with(|log| {
        log.borrow().append(&bytes).expect("failed to append snapshot");
    });
}

pub fn snapshots() -> impl Iterator<Item = crate::ProtocolSnapshot> {
    let len = SNAPSHOT_LOG.with(|log| log.borrow().len());
    (0..len).map(|i| {
        let bytes = SNAPSHOT_LOG.with(|log| log.borrow().get(i).unwrap());
        decode_snapshot(&bytes)
    })
}

pub fn count_snapshots() -> u64 {
    SNAPSHOT_LOG.with(|log| log.borrow().len())
}
```

**Commit:** `feat(backend): add snapshot stable storage (MemoryIds 2 & 3)`

### Step 1.3: Add snapshot capture function and timer

**File:** `src/rumi_protocol_backend/src/main.rs`

Add a `capture_protocol_snapshot()` function near `setup_timers()`:

```rust
fn capture_protocol_snapshot() {
    use rumi_protocol_backend::{ProtocolSnapshot, CollateralSnapshot};

    let snapshot = read_state(|s| {
        let mut total_collateral_value_usd: u64 = 0;
        let mut total_debt: u64 = 0;
        let mut total_vault_count: u64 = 0;
        let mut collateral_snapshots = Vec::new();

        for (ct, config) in s.collateral_configs.iter() {
            let col_total = s.total_collateral_for(ct);
            let debt = s.total_debt_for_collateral(ct).to_u64();
            let vault_count = s.collateral_to_vault_ids
                .get(ct)
                .map(|ids| ids.len() as u64)
                .unwrap_or(0);
            let price = config.last_price.unwrap_or(0.0);

            // Convert collateral to USD value (e8s)
            let col_decimal = rust_decimal::Decimal::from(col_total)
                / rust_decimal::Decimal::from(10u64.pow(config.decimals as u32));
            let usd_value = (col_decimal * rust_decimal::Decimal::try_from(price).unwrap_or_default())
                * rust_decimal::Decimal::from(100_000_000u64);
            let usd_e8s = usd_value.to_u64().unwrap_or(0);

            total_collateral_value_usd += usd_e8s;
            total_debt += debt;
            total_vault_count += vault_count;

            collateral_snapshots.push(CollateralSnapshot {
                collateral_type: *ct,
                total_collateral: col_total,
                total_debt: debt,
                vault_count,
                price,
            });
        }

        ProtocolSnapshot {
            timestamp: ic_cdk::api::time(),
            total_collateral_value_usd,
            total_debt,
            total_vault_count,
            collateral_snapshots,
        }
    });

    rumi_protocol_backend::storage::record_snapshot(&snapshot);
}
```

Add to `setup_timers()` at the end:

```rust
    // Hourly protocol snapshot
    ic_cdk_timers::set_timer(std::time::Duration::from_secs(5), || {
        capture_protocol_snapshot();
    });
    ic_cdk_timers::set_timer_interval(std::time::Duration::from_secs(3600), || {
        capture_protocol_snapshot();
    });
```

Note: the initial snapshot fires after 5 seconds (not immediately) to let prices load first.

Add `use rust_decimal::prelude::ToPrimitive;` to the imports at the top of main.rs if not already present.

**Commit:** `feat(backend): add hourly snapshot capture timer`

### Step 1.4: Add query endpoint and update .did file

**File:** `src/rumi_protocol_backend/src/main.rs`

Add the query function near `get_events()`:

```rust
#[candid_method(query)]
#[query]
fn get_protocol_snapshots(args: GetSnapshotsArg) -> Vec<ProtocolSnapshot> {
    if ic_cdk::api::data_certificate().is_none() {
        ic_cdk::trap("update call rejected");
    }
    const MAX_SNAPSHOTS_PER_QUERY: usize = 2000;

    rumi_protocol_backend::storage::snapshots()
        .skip(args.start as usize)
        .take(MAX_SNAPSHOTS_PER_QUERY.min(args.length as usize))
        .collect()
}

#[candid_method(query)]
#[query]
fn get_snapshot_count() -> u64 {
    rumi_protocol_backend::storage::count_snapshots()
}
```

Add imports at top: `use rumi_protocol_backend::{GetSnapshotsArg, ProtocolSnapshot};`

Build wasm, then regenerate .did file by running the candid interface test and capturing output:

```bash
cargo build --target wasm32-unknown-unknown -p rumi_protocol_backend
cargo test -p rumi_protocol_backend --bin rumi_protocol_backend -- --nocapture 2>&1 | \
  awk '/^actual.*interface:$/{f=1;next}/^declared.*interface/{exit}f' \
  > src/rumi_protocol_backend/rumi_protocol_backend.did
cargo test -p rumi_protocol_backend --bin rumi_protocol_backend
```

Ensure the candid interface test passes.

**Commit:** `feat(backend): add get_protocol_snapshots query endpoint`

### Step 1.5: Generate frontend declarations

Run dfx to regenerate the TypeScript declarations so the frontend can see the new types:

```bash
dfx generate rumi_protocol_backend
```

Verify that `src/declarations/rumi_protocol_backend/rumi_protocol_backend.did.js` and `.did.d.ts` include `get_protocol_snapshots`, `GetSnapshotsArg`, `ProtocolSnapshot`, `CollateralSnapshot`, and `get_snapshot_count`.

**Commit:** `chore: regenerate candid declarations for snapshot endpoints`

---

## Phase 2: Frontend — Foundation

### Step 2.1: Create explorer store

**File:** `src/vault_frontend/src/lib/stores/explorerStore.ts`

```typescript
import { writable, derived } from 'svelte/store';
import type { Event as ProtocolEvent } from '../../../../declarations/rumi_protocol_backend/rumi_protocol_backend.did';

// Events pagination
export const explorerEvents = writable<ProtocolEvent[]>([]);
export const explorerEventsLoading = writable(false);
export const explorerEventsPage = writable(0);
export const explorerEventsTotalCount = writable(0);

// Snapshots
export const protocolSnapshots = writable<any[]>([]);
export const snapshotsLoading = writable(false);

const PAGE_SIZE = 100;

export async function fetchEvents(actor: any, page: number = 0) {
    explorerEventsLoading.set(true);
    try {
        // get_events returns oldest-first; we want newest-first
        // First get total count, then fetch from the end
        const allEvents = await actor.get_events({ start: BigInt(0), length: BigInt(1) });
        // We need total count - use count from storage
        // Fetch the latest page
        const totalCount = Number(await actor.get_event_count());
        explorerEventsTotalCount.set(totalCount);

        const start = Math.max(0, totalCount - ((page + 1) * PAGE_SIZE));
        const length = Math.min(PAGE_SIZE, totalCount - (page * PAGE_SIZE));

        const events = await actor.get_events({
            start: BigInt(start),
            length: BigInt(length)
        });
        explorerEvents.set(events.reverse());
        explorerEventsPage.set(page);
    } catch (e) {
        console.error('Failed to fetch events:', e);
    } finally {
        explorerEventsLoading.set(false);
    }
}

export async function fetchSnapshots(actor: any) {
    snapshotsLoading.set(true);
    try {
        const count = Number(await actor.get_snapshot_count());
        if (count === 0) {
            protocolSnapshots.set([]);
            return;
        }
        const snaps = await actor.get_protocol_snapshots({
            start: BigInt(0),
            length: BigInt(count)
        });
        protocolSnapshots.set(snaps);
    } catch (e) {
        console.error('Failed to fetch snapshots:', e);
    } finally {
        snapshotsLoading.set(false);
    }
}
```

Note: The `get_event_count` endpoint may need to be added. Check if `count_events()` is exposed — if not, add a simple query:

```rust
#[candid_method(query)]
#[query]
fn get_event_count() -> u64 {
    rumi_protocol_backend::storage::count_events()
}
```

And regenerate declarations again.

**Commit:** `feat(frontend): add explorerStore with event and snapshot fetching`

### Step 2.2: Add Explorer to top nav

**File:** `src/vault_frontend/src/routes/+layout.svelte`

In the `<nav class="top-nav">` section, add Explorer link after Docs:

```svelte
<a href="/explorer" class="nav-link" class:active={currentPath.startsWith('/explorer')}><span>Explorer</span></a>
```

Also add to the mobile bottom nav section if one exists.

**Commit:** `feat(frontend): add Explorer to top nav`

### Step 2.3: Create shared explorer components

**Directory:** `src/vault_frontend/src/lib/components/explorer/`

**File: `SearchBar.svelte`**
- Text input styled with `.icp-input`
- On submit: parse input to determine type (vault ID if numeric, principal if contains dashes, event index if prefixed with #)
- Navigate to appropriate route using `goto()` from `$app/navigation`

**File: `EventRow.svelte`**
- Takes a single `Event` prop
- Renders: timestamp, type badge (colored by category), one-line summary, link to detail
- Event type categories and badge colors:
  - Vault ops (open, close, borrow, repay, add margin, withdraw): teal (`--rumi-safe`)
  - Liquidations (liquidate, partial liquidate, redistribute): pink (`--rumi-danger`)
  - Stability pool (provide, withdraw, claim): purple (`--rumi-purple-accent`)
  - Redemptions: amber/caution (`--rumi-caution`)
  - Admin/config: muted (`--rumi-text-muted`)

**File: `VaultSummaryCard.svelte`**
- Takes a `CandidVault` prop
- Renders: vault ID, owner (truncated principal, linked), collateral amount, debt, CR with color indicator
- Clickable → links to `/explorer/vault/[id]`

**File: `Pagination.svelte`**
- Takes `currentPage`, `totalPages`, `onPageChange` props
- Previous / page numbers / Next buttons

**Commit:** `feat(frontend): add shared explorer components (SearchBar, EventRow, VaultSummaryCard, Pagination)`

---

## Phase 3: Activity Feed Page

### Step 3.1: Create `/explorer` route

**File:** `src/vault_frontend/src/routes/explorer/+page.svelte`

Structure:
- Page title "Protocol Explorer" with `.page-title`
- `<SearchBar />` component prominently at top
- Quick stats row: total events, total vaults, latest event timestamp
- Filter buttons for event type categories (All, Vault Ops, Liquidations, Stability Pool, Redemptions, Admin)
- Event list using `<EventRow />` for each event
- `<Pagination />` at bottom
- Loading spinner while fetching

Data flow:
- `onMount` → get `publicActor`, call `fetchEvents(actor, 0)`
- Filter buttons update a local `selectedFilter` variable
- Filtered events derived from `$explorerEvents` based on filter
- Page changes call `fetchEvents(actor, newPage)`

**Commit:** `feat(frontend): add explorer activity feed page`

---

## Phase 4: Vault Detail Page

### Step 4.1: Create `/explorer/vault/[id]` route

**File:** `src/vault_frontend/src/routes/explorer/vault/[id]/+page.svelte`

Structure:
- Back link to `/explorer`
- `<SearchBar />` at top
- Vault header: "Vault #50" with status badge
- Current state card (`.glass-card`):
  - Owner (linked to `/explorer/address/[principal]`)
  - Collateral type + amount (human-readable with symbol)
  - Debt amount
  - Collateral ratio with color-coded health indicator
  - Accrued interest
- Event history timeline:
  - Fetch via `publicActor.get_vault_history(BigInt(id))`
  - Render each event with `<EventRow />` or a more detailed inline format
  - Show chronologically (oldest first) as a timeline

Data flow:
- Extract `id` from `$page.params.id`
- `onMount` → fetch vault via `publicActor.get_all_vaults()` and filter (or add a `get_vault(id)` if needed)
- Fetch history via `publicActor.get_vault_history(BigInt(id))`
- Resolve collateral config for symbol/decimals via `publicActor.get_collateral_config(collateral_type)`

**Commit:** `feat(frontend): add vault detail page`

---

## Phase 5: Address Page

### Step 5.1: Create `/explorer/address/[principal]` route

**File:** `src/vault_frontend/src/routes/explorer/address/[principal]/+page.svelte`

Structure:
- Back link to `/explorer`
- `<SearchBar />` at top
- Address header: truncated + full principal with copy button
- Aggregate stats row: total collateral value, total debt, vault count
- Vault list: `<VaultSummaryCard />` for each vault owned
- Combined activity: merge all vault histories, sort by timestamp desc

Data flow:
- Extract `principal` from `$page.params.principal`
- `onMount` → fetch vaults via `publicActor.get_vaults([Principal.fromText(principal)])`
- For each vault, fetch history and merge into unified timeline
- Resolve collateral configs for display

**Commit:** `feat(frontend): add address detail page`

---

## Phase 6: Event Detail Page

### Step 6.1: Create `/explorer/event/[index]` route

**File:** `src/vault_frontend/src/routes/explorer/event/[index]/+page.svelte`

Structure:
- Back link to `/explorer`
- `<SearchBar />` at top
- Event type header with colored badge
- Structured detail card showing all event fields:
  - Vault ID (linked to vault page)
  - Principals (linked to address pages)
  - Amounts (human-readable with symbols)
  - Block indices
  - Timestamps
- Raw event data in a collapsible `<details>` section

Data flow:
- Extract `index` from `$page.params.index`
- `onMount` → fetch via `publicActor.get_events({ start: BigInt(index), length: BigInt(1) })`
- Render based on which variant key is present in the event object

**Commit:** `feat(frontend): add event detail page`

---

## Phase 7: Liquidation History Page

### Step 7.1: Create `/explorer/liquidations` route

**File:** `src/vault_frontend/src/routes/explorer/liquidations/+page.svelte`

Structure:
- Page title "Liquidation History" with `.page-title`
- `<SearchBar />` at top
- Summary stats: total liquidations, total debt liquidated, total collateral seized
- Filter tabs: All, Full Liquidations, Partial Liquidations, Redistributions
- Table/list of liquidation events:
  - Timestamp, vault ID (linked), type badge, debt amount, collateral seized, liquidator (linked)
- Pagination

Data flow:
- `onMount` → fetch all events paginated, filter client-side for liquidation types
- Liquidation event types: `liquidate_vault`, `partial_liquidate_vault`, `redistribute_vault`
- Alternative: walk events from newest backwards until enough liquidations found

Note: This could be slow if liquidations are sparse among many events. Consider adding a backend helper `get_liquidation_events(start, length)` if performance is an issue — but start with client-side filtering.

**Commit:** `feat(frontend): add liquidation history page`

---

## Phase 8: Stats Dashboard

### Step 8.1: Create `/explorer/stats` route — current metrics

**File:** `src/vault_frontend/src/routes/explorer/stats/+page.svelte`

Structure (current metrics section):
- Page title "Protocol Stats" with `.page-title`
- Top-level metric cards (`.glass-card`):
  - Total TVL (USD)
  - Total Debt (icUSD)
  - Total Vaults
  - Global Collateral Ratio
  - Protocol Mode
- Per-collateral breakdown table:
  - Collateral type, TVL, debt, vault count, price, weighted interest rate

Data flow:
- `onMount` → fetch `publicActor.get_protocol_status()` and `publicActor.get_collateral_totals()`
- Format values for display

**Commit:** `feat(frontend): add stats page with current protocol metrics`

### Step 8.2: Add historical charts to stats page

Add below the current metrics section:
- Chart section header "Historical"
- Time range selector: 24h, 7d, 30d, 90d, All
- Charts using Layerchart:
  - **TVL over time** — line chart from `total_collateral_value_usd` in snapshots
  - **Total debt over time** — line chart from `total_debt`
  - **Vault count over time** — line chart from `total_vault_count`
  - **Per-collateral TVL** — stacked area chart from `collateral_snapshots`

Data flow:
- `onMount` → call `fetchSnapshots(actor)` from explorerStore
- Filter snapshots by selected time range
- Transform snapshot data into chart-friendly format: `{ date: Date, value: number }[]`

Check Layerchart docs/examples already in the project (rate curve visualization) for the exact import pattern.

**Commit:** `feat(frontend): add historical charts to stats dashboard`

---

## Phase 9: Polish & Integration

### Step 9.1: Wire up search bar routing logic

Ensure `SearchBar.svelte` correctly:
- Detects vault IDs (pure numeric)
- Detects principals (contains `-` and matches principal format)
- Handles edge cases (empty input, invalid input with toast error)
- Provides autocomplete or suggestions (stretch goal, skip for v1)

### Step 9.2: Event formatting helpers

**File:** `src/vault_frontend/src/lib/utils/eventFormatters.ts`

Centralized helpers for:
- `getEventType(event)` → string label
- `getEventCategory(event)` → 'vault' | 'liquidation' | 'stability' | 'redemption' | 'admin'
- `getEventSummary(event)` → one-line human-readable description
- `getEventBadgeColor(event)` → CSS color var
- `formatTimestamp(nanos)` → human-readable date string
- `truncatePrincipal(principal)` → "4rktk...kqe"
- `formatAmount(e8s, decimals)` → human-readable amount

These are used by `EventRow.svelte`, the activity feed, vault history, and liquidation history.

**Commit:** `feat(frontend): add event formatting utilities`

### Step 9.3: Final build verification

```bash
cargo build --target wasm32-unknown-unknown -p rumi_protocol_backend
cargo test -p rumi_protocol_backend --bin rumi_protocol_backend
cd src/vault_frontend && npm run build
```

Verify no build errors in backend or frontend.

**Commit:** `chore: verify full build passes`

---

## Execution Order

The phases are designed to be executed roughly in order, but with some flexibility:

1. **Phase 1** (backend) is a prerequisite for Phase 8 (stats charts) but NOT for Phases 3-7
2. **Phase 2** (foundation) is a prerequisite for all frontend phases
3. **Phases 3-7** (individual pages) are independent of each other
4. **Phase 8** depends on Phase 1 for snapshot data
5. **Phase 9** (polish) comes last

Recommended: Phase 1 → Phase 2 → Phase 9.2 (formatters, needed by all pages) → Phases 3-8 → Phase 9.1, 9.3
