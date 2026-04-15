# Position Summary Strip — Design

**Date:** 2026-04-14
**Status:** Approved, awaiting implementation plan
**Scope:** `src/vault_frontend` only (no backend changes)

## Problem

Users can't see their overall position in the protocol without navigating to `/vaults` and mentally aggregating across multiple vault cards. There's no single glanceable answer to "how much collateral do I have, how much am I borrowing, and am I safe?" On pages like `/borrow`, `/swap`, or `/3usd`, the user has no indication of their existing position at all.

## Goal

Add a persistent, always-visible position summary on every authenticated page of the vault_frontend that shows:

1. Total collateral value (USD, aggregated across all vaults and collateral types)
2. Total icUSD borrowed (aggregated across all vaults)
3. Overall collateral ratio, color-coded for health
4. Per-collateral breakdown on demand (ICP, ckBTC, ckXAUT, future assets)

## Non-Goals

- Stability Pool deposits, 3USD LP positions, wallet balances — not in v1, can extend later.
- Per-vault health or liquidation price — those stay on `VaultCard`.
- Backend / canister changes — this is a pure frontend aggregation over existing `userVaults` data.
- A dedicated "Portfolio" page (evaluated as option D during brainstorming, deferred).

## Placement

A thin strip directly under the top bar (`header.top-bar`) and above `main.main-content`. Persistent on every route when the user is connected and has at least one vault. Hidden when disconnected. Replaced with a CTA variant when connected with zero vaults.

On mobile (≤768px) the strip sits between the top bar and the main content, above the existing bottom mobile nav.

## States

### 1. Connected + ≥1 vault — collapsed (default)

```
┌─────────────────────────────────────────────────────────────┐
│ COLLATERAL $4,218.42 │ BORROWED 1,500 icUSD │ CR 281% ▾     │
└─────────────────────────────────────────────────────────────┘
```

- Three stat cells separated by thin vertical dividers.
- `Show breakdown ▾` affordance on the right.
- `CR` colored by tier: green ≥200%, amber 150–199%, red <150%.

### 2. Connected + ≥1 vault — expanded

Same top row, plus a breakdown row of per-collateral pills:

```
┌─────────────────────────────────────────────────────────────┐
│ COLLATERAL $4,218.42 │ BORROWED 1,500 icUSD │ CR 281% ▴     │
│ ┌─ICP 1,250.00 ($3,037)─┐ ┌─ckBTC 0.0180 ($1,080)─┐ ┌─...┐  │
└─────────────────────────────────────────────────────────────┘
```

- One pill per collateral type the user currently holds (zero-balance types omitted).
- Each pill: asset dot/logo, symbol, native amount, USD value.
- Expand state persisted in `localStorage` under `rumi:positionStrip:expanded` (boolean).

### 3. Connected, zero vaults — CTA variant

```
┌─────────────────────────────────────────────────────────────┐
│ No active position. Lock ICP, ckBTC, or ckXAUT as collateral│
│ to mint icUSD.                         Open your first vault│→
└─────────────────────────────────────────────────────────────┘
```

- Thin single-line pitch + link to `/` (the Borrow page).
- Same height as the collapsed strip (~36px).

### 4. Disconnected

Strip is not rendered at all — no vertical space taken.

### 5. Loading

When `$isLoadingVaults && $userVaults.length === 0` on initial connect, show a skeleton version of state 1 (three grey placeholder cells, no caret). Avoids layout jump when data resolves.

## Mobile Adaptation (≤768px)

- Stat labels shortened (`COLL` / `BORR` / `CR`), values kept but USD-abbreviated (`$4.2k`) if > $1,000.
- Dividers removed, tighter gap.
- Expand caret becomes `▾` glyph only.
- Expanded breakdown pills wrap to multiple rows.

## Component Architecture

New file: `src/vault_frontend/src/lib/components/layout/PositionStrip.svelte`

```
PositionStrip.svelte
├─ reads $userVaults and $isLoadingVaults from appDataStore
├─ reads $collateralStore for price + symbol lookup per collateral principal
├─ reads $isConnected from wallet store
├─ reads $permissionStore for view gating (same as vaults page)
├─ derives { totalCollateralUsd, totalBorrowed, overallCr, perCollateral[] } reactively
└─ renders one of: null (disconnected), skeleton (loading), cta (empty), collapsed, expanded
```

Mounted once in `src/vault_frontend/src/routes/+layout.svelte` between the existing `<header>` and `<main>` elements:

```svelte
<header class="top-bar">…</header>
<PositionStrip />
<main class="main-content"><slot /></main>
```

`main.main-content`'s top padding (`4.75rem` desktop, `4.25rem` mobile) must be reviewed — the strip adds vertical content in document flow, not overlaid, so main padding likely stays the same but the top-bar offset no longer needs to account for strip height since both are in flow.

## Data Flow

All data derives from existing stores — no new API endpoints.

```
appDataStore.userVaults ──┐
collateralStore ──────────┼──► derived stats in PositionStrip
wallet.isConnected ───────┘
```

### Derivations

For each vault `v`:
- `collateralPrincipal = v.collateralType || ICP_LEDGER`
- `info = collateralStore.find(c => c.principal === collateralPrincipal)`
- `priceUsd = info?.price ?? 0` (ICP falls back to `protocolStatus.lastIcpRate`)
- `amount = v.collateralAmount ?? v.icpMargin`
- `usdValue = amount * priceUsd`

Aggregate:
- `totalCollateralUsd = Σ usdValue`
- `totalBorrowed = Σ v.borrowedIcusd`
- `overallCr = totalBorrowed > 0 ? totalCollateralUsd / totalBorrowed : Infinity`

Per-collateral breakdown (group by `collateralPrincipal`):
- `{ symbol, logo, nativeAmount, usdValue }` per group
- Omit groups where `nativeAmount === 0`
- Order: by `usdValue` descending so largest position first

### Health tiers

- Green (`--rumi-safe`): `overallCr >= 2.0`
- Amber (`--rumi-warning`): `1.5 <= overallCr < 2.0`
- Red (`--rumi-danger`): `overallCr < 1.5`
- Infinity (no debt): neutral/grey

Note: these thresholds are aggregate across all vaults, not per-vault liquidation ratios. They're a heuristic for the overall strip color, not a liquidation signal — individual vault liquidation prices stay on VaultCard.

## Styling

- Background: `linear-gradient(180deg, rgba(167,139,250,0.04), rgba(52,211,153,0.02))` — subtle brand tint so it reads as an app-level chrome element, not a page content card.
- Border: `border-bottom: 1px solid var(--rumi-border)` to separate from main content.
- Height collapsed: ~36px desktop, ~32px mobile.
- Height expanded: collapsed row + breakdown row (~72px desktop, wraps on mobile).
- Fonts and colors reuse existing CSS custom properties (`--rumi-text-*`, `--rumi-bg-*`, `--rumi-safe`, etc.).

## Interaction

- **Click anywhere on the collapsed strip** (not just the caret) to expand. Same to collapse.
- **Cursor: pointer** on the interactive row, cursor: default on pills.
- **Transition**: height transition 150ms ease on expand/collapse for a gentle reveal.
- **No modal, no overlay** — everything pushes page content down.
- **Keyboard**: strip row is a `<button>` with `aria-expanded`, `aria-controls` pointing at the breakdown row.

## Persistence

Expand/collapse state in `localStorage` at key `rumi:positionStrip:expanded`. Default `false` (collapsed). Read on mount, write on toggle. No other local state.

## Edge Cases

- **Price not yet loaded**: show `—` for USD values and skip overall CR color (neutral). Component should not throw; missing price is explicit.
- **One vault with zero debt** (pure collateral, nothing borrowed): `overallCr = ∞`. Show CR as `—` or "No debt" with neutral color; don't show 0% or Infinity%.
- **Multiple vaults on same collateral type**: aggregate amounts per type in the breakdown (don't show one pill per vault).
- **New collateral type with no price feed yet**: include in breakdown but show `—` for USD, exclude from total collateral USD so totals stay honest.
- **Disconnected mid-session**: strip disappears immediately on `isConnected` flip.
- **Empty state gate**: connected + `userVaults.length === 0` AND not loading → CTA. Loading → skeleton (never CTA) to avoid flashing the CTA before vaults resolve.

## Accessibility

- Button role with `aria-expanded`.
- Health color is supplemented by a textual suffix in the expanded view (e.g., "281% · Healthy") so color isn't the sole carrier.
- Contrast checked against existing surface colors.

## Testing

Unit-level (Vitest or Svelte testing library):

- Derivation helpers (aggregation, CR, health tier) as pure functions in `PositionStrip.helpers.ts` or similar. Tested in isolation for:
  - Single vault, multiple vaults, multiple collateral types
  - Zero debt → Infinity CR
  - Missing price → excluded from USD total
  - Zero-balance collateral omitted from breakdown
- Rendering smoke test for each state (disconnected, loading, empty, collapsed, expanded).

Manual verification on mainnet deploy:

- Connect wallet with known position, confirm totals match sum of VaultCards on /vaults.
- Toggle expand/collapse, reload page, confirm state persists.
- Observe mobile viewport at 375px, 414px.
- Disconnect wallet, confirm strip hides.
- Create first vault (or simulate with zero-vault account), confirm CTA variant.

## Out of Scope (for later iterations)

- Net position (collateral − borrowed) as a fourth stat.
- Stability Pool deposits and 3USD LP positions rolled into totals.
- Wallet balances (idle icUSD, idle collateral) shown alongside locked.
- Sparkline or trend indicator (e.g., "+0.3% 24h").
- Per-vault drill-down from the breakdown pills.
- Different color themes or density modes.
