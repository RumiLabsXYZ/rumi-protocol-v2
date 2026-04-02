# Explorer Redesign

**Date**: 2026-04-01
**Status**: Approved

## Problem

The Explorer has grown organically and has several UX and data issues:

1. **Navigation confusion**: 4 top-level tabs (Dashboard, Events, Liquidations, Stats) where Events contains 8 sub-tabs, one of which (Liquidations) duplicates the top-level Liquidations tab
2. **Dashboard vs Stats overlap**: Both show nearly identical summary cards and collateral breakdown tables
3. **Broken event filtering**: Sub-tabs (Liquidations, Admin, System, 3Pool) show empty because filtering is per-page client-side, not across all events
4. **Missing data on event rows**: No principal/caller shown on any events
5. **Missing token logos**: EXE, ckETH, nICP, BOB have no logos in Collateral Breakdown
6. **Sticky nav visual bug**: Purple sub-nav underline stacks under green main nav underline when scrolling
7. **No AMM explorer data**: The constant-product AMM canister has no integration and stores no trade history
8. **Address page has irrelevant tabs**: Admin and System tabs on user profile pages make no sense
9. **3Pool events not clickable**: No detail view, no caller principal shown
10. **3Pool activity missing from address pages**: `get_events_by_principal` only queries the backend event log, not the 3Pool canister's swap events
11. **NaN% in Stats**: Interest Rate and Borrow Fee columns show NaN%

## Design: 3 Top-Level Tabs

Replace the current 4-tab structure with 3 tabs: **Overview**, **Activity**, **Addresses** (detail page, not a tab).

### Overview (merges Dashboard + Stats)

Layout top to bottom:

1. **Summary cards** (6 cards): Protocol Mode, TVL, Debt, System CR, Active Vaults, Total Events
2. **Collateral Breakdown table**: All current columns (Token, Price, Total Locked, Locked USD, Total Debt, Vaults, Utilization, Interest Rate, Status) plus Borrow Fee and Debt Ceiling from old Stats page. All tokens get logos.
3. **Pool cards** side by side: Stability Pool, 3Pool (StableSwap), AMM (new card showing pool count, total liquidity, swap fee)
4. **Treasury & Revenue** card with interest split bar
5. **Historical Trends** (moved from Stats): TVL, Total Debt, System CR charts with 24h/7d/30d/90d/All time toggles
6. **Liquidation Health** card + **At-Risk Vaults** table
7. **Recent Activity** feed showing caller principal on each row, clickable

Stats page is removed entirely.

### Activity (replaces Events + Liquidations)

**Filter**: Single dropdown/pill selector replaces 8 horizontal sub-tabs:
- All (default)
- Vault Operations (open, close, borrow, repay, add collateral, withdraw)
- Liquidations (liquidate, partial liquidate, redistribute, redemptions)
- DEX (3Pool swaps + AMM swaps combined)
- Stability Pool (provide/withdraw liquidity, claim returns)
- Admin (parameter changes, collateral config)
- System (init, upgrade, dust forgiven)

**Every event row shows**:
- Event ID
- Timestamp
- Type badge (color-coded)
- Caller/principal (truncated, clickable to address page)
- Summary text
- Click for detail view

**Liquidation summary banner**: When Liquidations filter is active, show 4 summary cards as header (Liquidation Events count, Liquidatable Vaults, Bot Budget Remaining, Bot Debt Covered). Hidden on other filters.

**DEX event detail view**:
- Caller principal
- Token in -> Token out with amounts
- Fee
- Pool source (3Pool or AMM pool name)
- Timestamp

**Filtering is server-side or properly paginated** across all events, not client-side per-page.

### Address Pages

Reached by clicking a principal or searching. Shows:
- Address header with principal and account type badge
- Summary cards (Total Vaults, Collateral Value, Total Debt, Weighted Avg CR)
- Vaults table
- Activity section with dropdown filter (only relevant categories):
  - All
  - Vault Operations
  - Liquidations
  - DEX (3Pool + AMM)
  - Stability Pool

No Admin or System tabs on user profiles.

3Pool and AMM swap events are queried from their respective canisters and filtered by principal.

## Backend Changes Required

### AMM Canister: Add Trade History
- Add `SwapEvent` struct: `{ id, caller, pool_id, token_in, amount_in, token_out, amount_out, fee, timestamp }`
- Store in state (same pattern as 3Pool)
- Add `get_swap_events(start: u64, length: u64) -> Vec<SwapEvent>` query
- Add `get_swap_event_count() -> u64` query
- Record a SwapEvent on every successful swap

### Backend Canister: Category Filtering (optional optimization)
- Add optional category parameter to `get_events_filtered` so the backend can filter by event type before pagination, rather than the frontend scanning pages client-side

## Bug Fixes (independent of redesign)

1. **Token logos**: Source/create logos for EXE, ckETH, nICP, BOB
2. **Sticky nav**: Either don't stick the sub-nav, or hide its underline indicator when stuck under the main nav (adjust z-index/top offset so they don't visually overlap)
3. **NaN% fix**: Interest Rate and Borrow Fee columns showing NaN% in collateral config display
4. **Principal on events**: Extract caller/owner from event data and display on every event row
5. **Per-page filtering**: Fix to filter across all events, not just current page
6. **DEX events clickable**: Add detail view for swap events with full transaction info
7. **3Pool events on address pages**: Query 3Pool canister's `get_swap_events` and filter by caller principal
8. **AMM events on address pages**: Same pattern once AMM trade history is added
