# Protocol Explorer — Design

## Overview

A protocol-native block explorer for Rumi Protocol. Lives at `/explorer` with sub-routes for vault detail, address lookup, event detail, liquidation history, and protocol stats with historical charts. Promoted to top nav.

## Pages

### `/explorer` — Activity Feed
- Paginated list of all protocol events (uses `get_events()`)
- Filter by event type: vault ops, liquidations, redemptions, stability pool, admin
- Each event row: timestamp, type badge, summary (e.g. "Vault #50 opened with 1.0 ICP"), link to detail
- Search bar at top: "Search by vault ID, principal, or event index"
  - Vault ID (numeric) → routes to `/explorer/vault/[id]`
  - Principal (text with dashes) → routes to `/explorer/address/[principal]`
  - Event index (prefixed with `#` or just numeric with disambiguation) → routes to `/explorer/event/[index]`

### `/explorer/vault/[id]` — Vault Detail
- Current state: owner, collateral amount, debt, CR with health gauge, collateral type
- Link to owner's address page
- Full event history timeline (uses `get_vault_history()`)
- Each event expandable with details (block index, amounts, fees)

### `/explorer/address/[principal]` — Address Page
- List of all vaults owned by this principal
- Aggregate stats: total collateral value, total debt, vault count
- Combined activity feed across all their vaults

### `/explorer/event/[index]` — Event Detail
- Full event data rendered as structured fields
- Links to related vault(s) and address(es)
- Block index reference

### `/explorer/liquidations` — Liquidation History
- Filtered view of liquidation events only: `LiquidateVault`, `PartialLiquidateVault`, `RedistributeVault`
- Columns: timestamp, vault ID, type (full/partial/redistribution), debt paid, collateral seized, liquidator
- Sortable by recency

### `/explorer/stats` — Protocol Stats Dashboard
- **Current metrics** (from `get_protocol_status()` + `get_collateral_totals()`):
  - Total TVL (USD), total debt, total vault count
  - Per-collateral breakdown: TVL, debt, vault count, price, weighted interest rate
  - Protocol mode, global CR
- **Historical charts** (from `get_protocol_snapshots()`):
  - TVL over time
  - Total debt over time
  - Vault count over time
  - Per-collateral TVL breakdown (stacked area)
  - Uses Layerchart (already in project)

## Backend Changes

### Hourly Protocol Snapshot System

**New struct: `ProtocolSnapshot`**
```
timestamp: u64 (nanoseconds)
total_collateral_value_usd: u64 (e8s)
total_debt: u64 (e8s)
total_vault_count: u64
collateral_snapshots: Vec<CollateralSnapshot>
```

**`CollateralSnapshot`:**
```
collateral_type: Principal
total_collateral: u64 (native units)
total_debt: u64 (e8s)
vault_count: u64
price: f64
```

**Storage:**
- New `StableLog` in stable memory using MemoryId 2 (index) and MemoryId 3 (data)
- CBOR-encoded, same pattern as existing event log in `storage.rs`
- ~400 bytes per snapshot, ~3.5 MB/year

**Timer:**
- Add hourly interval timer in `setup_timers()`: `set_timer_interval(Duration::from_secs(3600), ...)`
- Also fire once immediately at startup to capture current state

**New query endpoint:**
```
get_protocol_snapshots(start: nat64, limit: nat64) -> Vec<ProtocolSnapshot>
```
- Returns snapshots paginated by index (0 = oldest)
- Max 2000 per query (same as events)

**No other backend changes needed** — existing endpoints cover all other data needs.

### Existing endpoints used
- `get_events(start, length)` — paginated protocol events
- `get_vault_history(vault_id)` — all events for a vault
- `get_all_vaults()` — current vault states
- `get_vaults(principal)` — vaults by owner
- `get_protocol_status()` — current protocol metrics
- `get_collateral_totals()` — per-collateral aggregates
- `get_collateral_config(principal)` — collateral configuration

## Frontend Architecture

### New files
- `src/routes/explorer/+page.svelte` — Activity feed
- `src/routes/explorer/vault/[id]/+page.svelte` — Vault detail
- `src/routes/explorer/address/[principal]/+page.svelte` — Address page
- `src/routes/explorer/event/[index]/+page.svelte` — Event detail
- `src/routes/explorer/liquidations/+page.svelte` — Liquidation history
- `src/routes/explorer/stats/+page.svelte` — Stats dashboard
- `src/lib/stores/explorerStore.ts` — Data fetching/caching for explorer
- `src/lib/components/explorer/` — Shared components (SearchBar, EventRow, VaultSummaryCard, etc.)

### Data layer
- New `explorerStore` for explorer-specific data (events, snapshots, searched vault/address)
- Uses `publicActor` (anonymous, no auth popup) for all queries
- Request deduplication following `appDataStore` pattern
- Pagination state managed per-page

### Navigation
- Add "Explorer" to top nav bar (desktop header + mobile bottom nav)
- Search bar is the hero element on the main `/explorer` page

### Styling
- Follow existing design system: glass-card, CSS variables, Tailwind
- Event type badges with color coding (teal for vault ops, purple for liquidations, emerald for stability pool)
- Reuse health gauge pattern from liquidations page for vault CR display
