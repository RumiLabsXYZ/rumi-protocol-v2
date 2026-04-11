# Explorer Phase 1: Foundation + Dashboard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the explorer's navigation and overview page with a data-forward dashboard powered by the analytics canister, establishing the design foundation for all subsequent phases.

**Architecture:** The explorer lives within the existing vault_frontend SvelteKit app at `/explorer`. We rewrite the layout (navigation) and overview page (`+page.svelte`) while preserving all child routes (activity, holders, vault detail, etc.). New dashboard sections pull from `analyticsService.ts` (analytics canister) and `explorerService.ts` (backend canister). Charts use `lightweight-charts` for financial visualizations and hand-rolled SVG for area charts. The existing Rumi design system (app.css tokens, Circular Std + Inter fonts, indigo base + teal accent) is extended with explorer-specific utilities.

**Tech Stack:** SvelteKit (Svelte 5 runes), TypeScript, Tailwind CSS 3, lightweight-charts, IC agent-js, existing Rumi design tokens

---

## File Structure

### New Files
| Path | Responsibility |
|------|---------------|
| `src/vault_frontend/src/lib/components/explorer/TvlChart.svelte` | TVL area chart with time-range selector (SVG) |
| `src/vault_frontend/src/lib/components/explorer/CollateralTable.svelte` | Collateral overview table with fill bars |
| `src/vault_frontend/src/lib/components/explorer/ProtocolVitals.svelte` | Top vitals strip (mode, CR, TVL, debt, supply, volume) |
| `src/vault_frontend/src/lib/components/explorer/PoolHealthStrip.svelte` | 3pool peg + LP APY + SP APY compact cards |
| `src/vault_frontend/src/lib/components/explorer/CeilingBar.svelte` | Tiny debt-ceiling utilization fill bar |
| `src/vault_frontend/src/lib/components/explorer/ModePill.svelte` | Protocol mode indicator pill (GA/Recovery/ReadOnly/Frozen) |
| `src/vault_frontend/src/lib/utils/explorerChartHelpers.ts` | Shared chart utilities (formatters, scales, colors) |

### Modified Files
| Path | Changes |
|------|---------|
| `src/vault_frontend/package.json` | Add `lightweight-charts` dependency |
| `src/vault_frontend/src/routes/explorer/+layout.svelte` | Full rewrite: new navigation with 7 sections |
| `src/vault_frontend/src/routes/explorer/+page.svelte` | Full rewrite: new dashboard |
| `src/vault_frontend/src/lib/services/explorer/analyticsService.ts` | Add missing wrappers: `fetchOhlc`, `fetchVolatility`, `fetchPriceSeries`, `fetchThreePoolSeries` |
| `src/vault_frontend/src/app.css` | Add explorer-specific utility classes |

### Preserved (no changes)
- All child routes: `/explorer/activity`, `/explorer/holders`, `/explorer/vault/[id]`, `/explorer/address/[principal]`, `/explorer/event/[index]`, `/explorer/canister/[id]`, `/explorer/dex/[source]/[id]`, `/explorer/token/[id]`
- All existing explorer components (StatCard, EventRow, Pagination, etc.)
- `explorerService.ts` (backend data layer)

---

## Task 1: Install lightweight-charts and add missing analytics wrappers

**Files:**
- Modify: `src/vault_frontend/package.json`
- Modify: `src/vault_frontend/src/lib/services/explorer/analyticsService.ts`

- [ ] **Step 1: Install lightweight-charts**

```bash
cd src/vault_frontend && npm install lightweight-charts
```

Verify: `node -e "require('lightweight-charts')"` should not error.

- [ ] **Step 2: Add missing analytics service wrappers**

Add these functions to the end of `analyticsService.ts` (before the closing of the file), following the existing pattern (TTL cache, error handling, lazy actor):

```typescript
/** Fetch OHLC candlestick data for a collateral type. */
export async function fetchOhlc(
  collateral: Principal,
  bucketSecs?: number,
  fromTs?: bigint,
  toTs?: bigint,
  limit?: number
): Promise<any | null> {
  const key = `ohlc:${collateral.toText()}:${bucketSecs ?? 3600}:${fromTs ?? 0}:${toTs ?? 0}:${limit ?? 500}`;
  const cached = getCached(key, TTL.SERIES);
  if (cached !== null) return cached;

  try {
    const actor = getActor();
    const result = await actor.get_ohlc({
      collateral,
      bucket_secs: bucketSecs ? [bucketSecs] : [],
      from_ts: fromTs ? [fromTs] : [],
      to_ts: toTs ? [toTs] : [],
      limit: limit ? [limit] : [],
    });
    return setCache(key, result);
  } catch (err) {
    console.error('[analyticsService] fetchOhlc failed:', err);
    return null;
  }
}

/** Fetch realized volatility for a collateral type. */
export async function fetchVolatility(
  collateral: Principal,
  windowSecs?: number
): Promise<any | null> {
  const key = `volatility:${collateral.toText()}:${windowSecs ?? 86400}`;
  const cached = getCached(key, TTL.AGGREGATE);
  if (cached !== null) return cached;

  try {
    const actor = getActor();
    const result = await actor.get_volatility({
      collateral,
      window_secs: windowSecs ? [windowSecs] : [],
    });
    return setCache(key, result);
  } catch (err) {
    console.error('[analyticsService] fetchVolatility failed:', err);
    return null;
  }
}

/** Fetch 5-minute price snapshots. */
export async function fetchPriceSeries(limit?: number): Promise<any | null> {
  const key = `price_series:${limit ?? 500}`;
  const cached = getCached(key, TTL.SERIES);
  if (cached !== null) return cached;

  try {
    const actor = getActor();
    const result = await actor.get_price_series({
      from_ts: [],
      to_ts: [],
      limit: limit ? [limit] : [],
      offset: [],
    });
    return setCache(key, result);
  } catch (err) {
    console.error('[analyticsService] fetchPriceSeries failed:', err);
    return null;
  }
}

/** Fetch 5-minute 3pool snapshots. */
export async function fetchThreePoolSeries(limit?: number): Promise<any | null> {
  const key = `three_pool_series:${limit ?? 500}`;
  const cached = getCached(key, TTL.SERIES);
  if (cached !== null) return cached;

  try {
    const actor = getActor();
    const result = await actor.get_three_pool_series({
      from_ts: [],
      to_ts: [],
      limit: limit ? [limit] : [],
      offset: [],
    });
    return setCache(key, result);
  } catch (err) {
    console.error('[analyticsService] fetchThreePoolSeries failed:', err);
    return null;
  }
}
```

- [ ] **Step 3: Verify build**

```bash
cd src/vault_frontend && npm run build
```

Expected: Build succeeds with no TypeScript errors.

- [ ] **Step 4: Commit**

```bash
git add src/vault_frontend/package.json src/vault_frontend/package-lock.json src/vault_frontend/src/lib/services/explorer/analyticsService.ts
git commit -m "feat(explorer): install lightweight-charts and add missing analytics wrappers"
```

---

## Task 2: Add explorer-specific CSS utilities

**Files:**
- Modify: `src/vault_frontend/src/app.css`

- [ ] **Step 1: Add explorer utility classes to app.css**

Append to the end of `src/vault_frontend/src/app.css`:

```css
/* ============================================
   Explorer — data-forward utilities
   ============================================ */

/* Tabular numbers for aligned data columns */
.tabular-nums {
  font-variant-numeric: tabular-nums;
}

/* Explorer section card — slightly tighter than glass-panel */
.explorer-card {
  background: var(--rumi-bg-surface1);
  border: 1px solid var(--rumi-border);
  border-radius: 0.625rem;
  padding: 1.25rem;
  box-shadow:
    inset 0 1px 0 0 rgba(200, 210, 240, 0.03),
    0 2px 8px -2px rgba(8, 11, 22, 0.6);
}

/* Explorer nav link — active state */
.explorer-nav-link {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  padding: 0.5rem 0.75rem;
  border-radius: 0.375rem;
  font-family: 'Inter', sans-serif;
  font-size: 0.8125rem;
  font-weight: 500;
  color: var(--rumi-text-secondary);
  transition: all 0.15s ease;
  text-decoration: none;
}

.explorer-nav-link:hover {
  color: var(--rumi-text-primary);
  background: var(--rumi-bg-surface2);
}

.explorer-nav-link.active {
  color: var(--rumi-teal);
  background: var(--rumi-teal-dim);
}

/* Fill bar for utilization metrics */
.fill-bar {
  height: 6px;
  border-radius: 3px;
  background: var(--rumi-bg-surface3);
  overflow: hidden;
}

.fill-bar-inner {
  height: 100%;
  border-radius: 3px;
  background: var(--rumi-teal);
  transition: width 0.4s ease;
}

/* Vitals strip metric */
.vital-metric {
  display: flex;
  flex-direction: column;
  gap: 0.125rem;
}

.vital-label {
  font-size: 0.6875rem;
  font-weight: 500;
  color: var(--rumi-text-muted);
  text-transform: uppercase;
  letter-spacing: 0.06em;
}

.vital-value {
  font-family: 'Inter', sans-serif;
  font-weight: 600;
  font-size: 1.125rem;
  font-variant-numeric: tabular-nums;
  color: var(--rumi-text-primary);
}
```

- [ ] **Step 2: Verify build**

```bash
cd src/vault_frontend && npm run build
```

- [ ] **Step 3: Commit**

```bash
git add src/vault_frontend/src/app.css
git commit -m "feat(explorer): add explorer-specific CSS utilities"
```

---

## Task 3: Create chart helper utilities

**Files:**
- Create: `src/vault_frontend/src/lib/utils/explorerChartHelpers.ts`

- [ ] **Step 1: Create the chart helpers file**

```typescript
/**
 * Shared chart utilities for the explorer.
 * Formatters, scales, and color constants for SVG charts.
 */

/** Format a large number with K/M/B suffixes. */
export function formatCompact(value: number): string {
  if (value >= 1_000_000_000) return `${(value / 1_000_000_000).toFixed(1)}B`;
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(1)}K`;
  return value.toFixed(0);
}

/** Format basis points as percentage string. */
export function bpsToPercent(bps: number): string {
  return `${(bps / 100).toFixed(1)}%`;
}

/** Format e8s (8-decimal) to human-readable number. */
export function e8sToNumber(e8s: number | bigint): number {
  return Number(e8s) / 1e8;
}

/** Format USD value from e8s. */
export function formatUsdE8s(e8s: number | bigint): string {
  const val = e8sToNumber(e8s);
  if (val >= 1_000_000) return `$${(val / 1_000_000).toFixed(2)}M`;
  if (val >= 1_000) return `$${(val / 1_000).toFixed(1)}K`;
  return `$${val.toFixed(2)}`;
}

/** Convert nanosecond timestamp to JS Date. */
export function nsToDate(ns: bigint | number): Date {
  return new Date(Number(ns) / 1_000_000);
}

/** Format date for chart axis labels. */
export function formatDateShort(date: Date): string {
  return date.toLocaleDateString('en-US', { month: 'short', day: 'numeric' });
}

/** Chart color palette (matching Rumi design tokens). */
export const CHART_COLORS = {
  teal: '#2DD4BF',
  tealDim: 'rgba(45, 212, 191, 0.15)',
  purple: '#d176e8',
  purpleDim: 'rgba(209, 118, 232, 0.15)',
  action: '#34d399',
  danger: '#e06b9f',
  caution: '#a78bfa',
  grid: 'rgba(90, 100, 180, 0.08)',
  text: '#a09bb5',
  textMuted: '#605a75',
} as const;

/** Known collateral symbols by principal. */
export const COLLATERAL_SYMBOLS: Record<string, string> = {
  'ryjl3-tyaaa-aaaaa-aaaba-cai': 'ICP',
  'mxzaz-hqaaa-aaaar-qaada-cai': 'ckBTC',
  'ss2fx-dyaaa-aaaar-qacoq-cai': 'ckETH',
  'lkwrt-vyaaa-aaaar-qadkq-cai': 'ckXAUT',
  'buwm7-7yaaa-aaaar-qagva-cai': 'nICP',
  '7pail-xaaaa-aaaas-aabmq-cai': 'BOB',
  'rh2pm-ryaaa-aaaan-qeniq-cai': 'EXE',
};

/** Get symbol for a collateral principal. */
export function getCollateralSymbol(principal: string): string {
  return COLLATERAL_SYMBOLS[principal] ?? principal.slice(0, 5) + '...';
}

/** Collateral brand colors. */
export const COLLATERAL_COLORS: Record<string, string> = {
  ICP: '#29ABE2',
  ckBTC: '#F7931A',
  ckETH: '#627EEA',
  ckXAUT: '#C9A96E',
  nICP: '#5AC4BE',
  BOB: '#FF6B35',
  EXE: '#8B5CF6',
};

/** Time range presets for chart filters. */
export type TimeRange = '7d' | '30d' | '90d' | '1y' | 'all';

export const TIME_RANGES: { key: TimeRange; label: string; days: number }[] = [
  { key: '7d', label: '7D', days: 7 },
  { key: '30d', label: '30D', days: 30 },
  { key: '90d', label: '90D', days: 90 },
  { key: '1y', label: '1Y', days: 365 },
  { key: 'all', label: 'All', days: 0 },
];

/** Filter data points by time range. Returns items within the last N days. */
export function filterByTimeRange<T extends { timestamp_ns: bigint }>(
  data: T[],
  range: TimeRange
): T[] {
  if (range === 'all') return data;
  const preset = TIME_RANGES.find(r => r.key === range);
  if (!preset) return data;
  const cutoff = BigInt(Date.now() - preset.days * 86_400_000) * 1_000_000n;
  return data.filter(d => d.timestamp_ns >= cutoff);
}

/**
 * Compute Y-axis scale for an array of values.
 * Returns { min, max, ticks } with nice round numbers.
 */
export function computeYScale(values: number[]): { min: number; max: number; ticks: number[] } {
  if (values.length === 0) return { min: 0, max: 100, ticks: [0, 25, 50, 75, 100] };
  const rawMin = Math.min(...values);
  const rawMax = Math.max(...values);
  const range = rawMax - rawMin || rawMax * 0.1 || 1;
  const padding = range * 0.05;
  const min = Math.max(0, rawMin - padding);
  const max = rawMax + padding;
  const step = (max - min) / 4;
  const ticks = Array.from({ length: 5 }, (_, i) => min + step * i);
  return { min, max, ticks };
}
```

- [ ] **Step 2: Verify build**

```bash
cd src/vault_frontend && npm run build
```

- [ ] **Step 3: Commit**

```bash
git add src/vault_frontend/src/lib/utils/explorerChartHelpers.ts
git commit -m "feat(explorer): add chart helper utilities"
```

---

## Task 4: Create ModePill component

**Files:**
- Create: `src/vault_frontend/src/lib/components/explorer/ModePill.svelte`

- [ ] **Step 1: Create ModePill.svelte**

```svelte
<script lang="ts">
  interface Props {
    mode: string;
    frozen?: boolean;
  }
  let { mode, frozen = false }: Props = $props();

  const config = $derived.by(() => {
    if (frozen) return { label: 'Frozen', color: 'bg-pink-500/20 text-pink-300 border-pink-500/30' };
    switch (mode) {
      case 'ReadOnly':
        return { label: 'Read-Only', color: 'bg-amber-500/20 text-amber-300 border-amber-500/30' };
      case 'Recovery':
        return { label: 'Recovery', color: 'bg-violet-500/20 text-violet-300 border-violet-500/30' };
      default:
        return { label: 'Normal', color: 'bg-teal-500/15 text-teal-300 border-teal-500/25' };
    }
  });
</script>

<span class="inline-flex items-center gap-1.5 px-2.5 py-1 text-xs font-medium rounded-full border {config.color}">
  <span class="w-1.5 h-1.5 rounded-full {frozen ? 'bg-pink-400' : mode === 'Recovery' ? 'bg-violet-400' : mode === 'ReadOnly' ? 'bg-amber-400' : 'bg-teal-400'}"></span>
  {config.label}
</span>
```

- [ ] **Step 2: Verify build**

```bash
cd src/vault_frontend && npm run build
```

- [ ] **Step 3: Commit**

```bash
git add src/vault_frontend/src/lib/components/explorer/ModePill.svelte
git commit -m "feat(explorer): add ModePill component"
```

---

## Task 5: Create CeilingBar component

**Files:**
- Create: `src/vault_frontend/src/lib/components/explorer/CeilingBar.svelte`

- [ ] **Step 1: Create CeilingBar.svelte**

```svelte
<script lang="ts">
  interface Props {
    used: number;
    total: number;
    /** If true, total is unlimited (u64::MAX). Show "Unlimited" instead of bar. */
    unlimited?: boolean;
  }
  let { used, total, unlimited = false }: Props = $props();

  const pct = $derived(unlimited ? 0 : total > 0 ? Math.min((used / total) * 100, 100) : 0);
  const label = $derived(unlimited ? 'No limit' : `${pct.toFixed(0)}%`);
  const barColor = $derived(
    pct > 90 ? 'bg-pink-400' : pct > 70 ? 'bg-violet-400' : 'bg-teal-400'
  );
</script>

{#if unlimited}
  <span class="text-xs text-gray-500">Unlimited</span>
{:else}
  <div class="flex items-center gap-2">
    <div class="fill-bar flex-1 min-w-[3rem]">
      <div class="fill-bar-inner {barColor}" style="width: {pct}%"></div>
    </div>
    <span class="text-xs tabular-nums text-gray-400 w-8 text-right">{label}</span>
  </div>
{/if}
```

- [ ] **Step 2: Verify build**

```bash
cd src/vault_frontend && npm run build
```

- [ ] **Step 3: Commit**

```bash
git add src/vault_frontend/src/lib/components/explorer/CeilingBar.svelte
git commit -m "feat(explorer): add CeilingBar component"
```

---

## Task 6: Create ProtocolVitals component

**Files:**
- Create: `src/vault_frontend/src/lib/components/explorer/ProtocolVitals.svelte`

- [ ] **Step 1: Create ProtocolVitals.svelte**

This component displays the top vitals strip. It receives pre-fetched data as props (data fetching happens in the page).

```svelte
<script lang="ts">
  import ModePill from './ModePill.svelte';
  import { formatCompact, e8sToNumber, bpsToPercent } from '$utils/explorerChartHelpers';

  interface Props {
    summary: any | null;
    loading?: boolean;
  }
  let { summary, loading = false }: Props = $props();

  const tvl = $derived(summary ? e8sToNumber(summary.total_collateral_usd_e8s) : 0);
  const debt = $derived(summary ? e8sToNumber(summary.total_debt_e8s) : 0);
  const supply = $derived(summary ? e8sToNumber(summary.circulating_supply_icusd_e8s) : 0);
  const cr = $derived(summary ? Number(summary.system_cr_bps) : 0);
  const mode = $derived.by(() => {
    if (!summary) return 'Normal';
    // Derive mode from CR: <100% = ReadOnly, <recovery threshold = Recovery, else Normal
    if (cr < 10000) return 'ReadOnly';
    if (cr < 14100) return 'Recovery';
    return 'Normal';
  });
  const volume24h = $derived(summary ? e8sToNumber(summary.volume_24h_e8s) : 0);
  const swapCount = $derived(summary ? Number(summary.swap_count_24h) : 0);
</script>

<div class="explorer-card">
  {#if loading}
    <div class="flex items-center justify-center py-4">
      <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
    </div>
  {:else if summary}
    <div class="flex flex-wrap items-center gap-6 md:gap-8">
      <ModePill {mode} />

      <div class="vital-metric">
        <span class="vital-label">System CR</span>
        <span class="vital-value {cr < 14100 ? 'text-violet-400' : cr < 15000 ? 'text-amber-400' : ''}">{bpsToPercent(cr)}</span>
      </div>

      <div class="vital-metric">
        <span class="vital-label">TVL</span>
        <span class="vital-value">${formatCompact(tvl)}</span>
      </div>

      <div class="vital-metric">
        <span class="vital-label">Total Debt</span>
        <span class="vital-value">{formatCompact(debt)} icUSD</span>
      </div>

      <div class="vital-metric">
        <span class="vital-label">icUSD Supply</span>
        <span class="vital-value">{formatCompact(supply)}</span>
      </div>

      <div class="vital-metric">
        <span class="vital-label">24h Volume</span>
        <span class="vital-value">${formatCompact(volume24h)}</span>
      </div>

      <div class="vital-metric">
        <span class="vital-label">24h Swaps</span>
        <span class="vital-value">{swapCount.toLocaleString()}</span>
      </div>
    </div>
  {:else}
    <p class="text-sm text-gray-500">Unable to load protocol data.</p>
  {/if}
</div>
```

- [ ] **Step 2: Verify build**

```bash
cd src/vault_frontend && npm run build
```

- [ ] **Step 3: Commit**

```bash
git add src/vault_frontend/src/lib/components/explorer/ProtocolVitals.svelte
git commit -m "feat(explorer): add ProtocolVitals component"
```

---

## Task 7: Create TvlChart component

**Files:**
- Create: `src/vault_frontend/src/lib/components/explorer/TvlChart.svelte`

- [ ] **Step 1: Create TvlChart.svelte**

An SVG area chart with time-range selector. Takes TVL series data as a prop.

```svelte
<script lang="ts">
  import {
    e8sToNumber, formatCompact, formatDateShort, nsToDate,
    filterByTimeRange, computeYScale,
    CHART_COLORS, TIME_RANGES, type TimeRange
  } from '$utils/explorerChartHelpers';

  interface TvlRow {
    timestamp_ns: bigint;
    total_icp_collateral_e8s: bigint;
    total_icusd_supply_e8s: bigint;
    system_collateral_ratio_bps: number;
  }

  interface Props {
    data: TvlRow[];
    loading?: boolean;
  }
  let { data, loading = false }: Props = $props();

  let selectedRange: TimeRange = $state('90d');
  const WIDTH = 800;
  const HEIGHT = 240;
  const PADDING = { top: 20, right: 16, bottom: 32, left: 60 };
  const chartW = WIDTH - PADDING.left - PADDING.right;
  const chartH = HEIGHT - PADDING.top - PADDING.bottom;

  const filtered = $derived(filterByTimeRange(data, selectedRange));

  const points = $derived(
    filtered.map(row => ({
      x: Number(row.timestamp_ns),
      collateral: e8sToNumber(row.total_icp_collateral_e8s),
      debt: e8sToNumber(row.total_icusd_supply_e8s),
    }))
  );

  const yScale = $derived(computeYScale(points.map(p => p.collateral)));

  function xPos(ts: number): number {
    if (points.length < 2) return PADDING.left;
    const min = points[0].x;
    const max = points[points.length - 1].x;
    const range = max - min || 1;
    return PADDING.left + ((ts - min) / range) * chartW;
  }

  function yPos(val: number): number {
    const { min, max } = yScale;
    const range = max - min || 1;
    return PADDING.top + chartH - ((val - min) / range) * chartH;
  }

  const collateralPath = $derived.by(() => {
    if (points.length === 0) return '';
    return points.map((p, i) => `${i === 0 ? 'M' : 'L'}${xPos(p.x).toFixed(1)},${yPos(p.collateral).toFixed(1)}`).join(' ');
  });

  const collateralAreaPath = $derived.by(() => {
    if (points.length === 0) return '';
    const baseline = PADDING.top + chartH;
    return collateralPath + ` L${xPos(points[points.length - 1].x).toFixed(1)},${baseline} L${xPos(points[0].x).toFixed(1)},${baseline} Z`;
  });

  const debtPath = $derived.by(() => {
    if (points.length === 0) return '';
    return points.map((p, i) => `${i === 0 ? 'M' : 'L'}${xPos(p.x).toFixed(1)},${yPos(p.debt).toFixed(1)}`).join(' ');
  });

  // X-axis labels (show ~5 dates)
  const xLabels = $derived.by(() => {
    if (points.length < 2) return [];
    const step = Math.max(1, Math.floor(points.length / 5));
    const labels: { x: number; label: string }[] = [];
    for (let i = 0; i < points.length; i += step) {
      labels.push({ x: xPos(points[i].x), label: formatDateShort(nsToDate(BigInt(points[i].x))) });
    }
    return labels;
  });
</script>

<div class="explorer-card">
  <div class="flex items-center justify-between mb-4">
    <h3 class="text-sm font-medium text-gray-300">Total Value Locked</h3>
    <div class="flex gap-1">
      {#each TIME_RANGES as range}
        <button
          class="px-2.5 py-1 text-xs rounded-md transition-colors
            {selectedRange === range.key
            ? 'bg-teal-500/15 text-teal-300'
            : 'text-gray-500 hover:text-gray-300'}"
          onclick={() => selectedRange = range.key}
        >
          {range.label}
        </button>
      {/each}
    </div>
  </div>

  {#if loading}
    <div class="flex items-center justify-center" style="height: {HEIGHT}px">
      <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
    </div>
  {:else if points.length === 0}
    <div class="flex items-center justify-center text-gray-500 text-sm" style="height: {HEIGHT}px">
      No data available
    </div>
  {:else}
    <svg viewBox="0 0 {WIDTH} {HEIGHT}" class="w-full" preserveAspectRatio="xMidYMid meet">
      <!-- Grid lines -->
      {#each yScale.ticks as tick}
        <line x1={PADDING.left} x2={WIDTH - PADDING.right} y1={yPos(tick)} y2={yPos(tick)}
          stroke={CHART_COLORS.grid} stroke-width="1" />
        <text x={PADDING.left - 8} y={yPos(tick) + 4} text-anchor="end"
          fill={CHART_COLORS.textMuted} font-size="10" font-family="Inter">
          ${formatCompact(tick)}
        </text>
      {/each}

      <!-- Area fill -->
      <path d={collateralAreaPath} fill="url(#tvlGradient)" />

      <!-- Lines -->
      <path d={collateralPath} fill="none" stroke={CHART_COLORS.teal} stroke-width="2" />
      <path d={debtPath} fill="none" stroke={CHART_COLORS.purple} stroke-width="1.5" stroke-dasharray="4 3" />

      <!-- X-axis labels -->
      {#each xLabels as lbl}
        <text x={lbl.x} y={HEIGHT - 6} text-anchor="middle"
          fill={CHART_COLORS.textMuted} font-size="10" font-family="Inter">
          {lbl.label}
        </text>
      {/each}

      <!-- Gradient definition -->
      <defs>
        <linearGradient id="tvlGradient" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stop-color={CHART_COLORS.teal} stop-opacity="0.2" />
          <stop offset="100%" stop-color={CHART_COLORS.teal} stop-opacity="0.02" />
        </linearGradient>
      </defs>
    </svg>

    <!-- Legend -->
    <div class="flex items-center gap-4 mt-2 ml-[60px]">
      <div class="flex items-center gap-1.5">
        <div class="w-3 h-0.5 rounded" style="background: {CHART_COLORS.teal}"></div>
        <span class="text-xs text-gray-500">Collateral (USD)</span>
      </div>
      <div class="flex items-center gap-1.5">
        <div class="w-3 h-0.5 rounded border-b border-dashed" style="border-color: {CHART_COLORS.purple}"></div>
        <span class="text-xs text-gray-500">Debt (icUSD)</span>
      </div>
    </div>
  {/if}
</div>
```

- [ ] **Step 2: Verify build**

```bash
cd src/vault_frontend && npm run build
```

- [ ] **Step 3: Commit**

```bash
git add src/vault_frontend/src/lib/components/explorer/TvlChart.svelte
git commit -m "feat(explorer): add TvlChart SVG area chart component"
```

---

## Task 8: Create CollateralTable component

**Files:**
- Create: `src/vault_frontend/src/lib/components/explorer/CollateralTable.svelte`

- [ ] **Step 1: Create CollateralTable.svelte**

The signature table showing all collateral types with prices, stats, and ceiling bars.

```svelte
<script lang="ts">
  import CeilingBar from './CeilingBar.svelte';
  import { e8sToNumber, formatCompact, getCollateralSymbol, COLLATERAL_COLORS } from '$utils/explorerChartHelpers';

  interface CollateralRow {
    principal: string;
    symbol: string;
    price: number;
    vaultCount: number;
    totalCollateralUsd: number;
    totalDebt: number;
    debtCeiling: number;
    unlimited: boolean;
    medianCrBps: number;
    volatility: number | null;
  }

  interface Props {
    rows: CollateralRow[];
    loading?: boolean;
  }
  let { rows, loading = false }: Props = $props();

  function formatPrice(price: number): string {
    if (price >= 10000) return `$${price.toLocaleString(undefined, { maximumFractionDigits: 0 })}`;
    if (price >= 100) return `$${price.toFixed(1)}`;
    if (price >= 1) return `$${price.toFixed(2)}`;
    return `$${price.toFixed(4)}`;
  }
</script>

<div class="explorer-card overflow-x-auto">
  <h3 class="text-sm font-medium text-gray-300 mb-4">Collateral Overview</h3>

  {#if loading}
    <div class="flex items-center justify-center py-8">
      <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
    </div>
  {:else if rows.length === 0}
    <p class="text-sm text-gray-500 py-4">No collateral data.</p>
  {:else}
    <table class="w-full text-sm">
      <thead>
        <tr class="border-b border-white/5">
          <th class="text-left py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Asset</th>
          <th class="text-right py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Price</th>
          <th class="text-right py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider hidden md:table-cell">Vol</th>
          <th class="text-right py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Vaults</th>
          <th class="text-right py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider hidden sm:table-cell">Collateral</th>
          <th class="text-right py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Debt</th>
          <th class="py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider w-28 hidden lg:table-cell">Ceiling</th>
          <th class="text-right py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider hidden md:table-cell">Avg CR</th>
        </tr>
      </thead>
      <tbody>
        {#each rows as row}
          <tr class="border-b border-white/[0.03] hover:bg-white/[0.02] transition-colors">
            <td class="py-2.5 px-2">
              <a href="/explorer/token/{row.principal}" class="flex items-center gap-2 hover:text-teal-300 transition-colors">
                <span class="w-2 h-2 rounded-full flex-shrink-0" style="background: {COLLATERAL_COLORS[row.symbol] ?? '#666'}"></span>
                <span class="font-medium text-gray-200">{row.symbol}</span>
              </a>
            </td>
            <td class="py-2.5 px-2 text-right tabular-nums text-gray-300">{formatPrice(row.price)}</td>
            <td class="py-2.5 px-2 text-right tabular-nums text-gray-500 hidden md:table-cell">
              {row.volatility != null ? `${row.volatility.toFixed(1)}%` : '--'}
            </td>
            <td class="py-2.5 px-2 text-right tabular-nums text-gray-400">{row.vaultCount}</td>
            <td class="py-2.5 px-2 text-right tabular-nums text-gray-300 hidden sm:table-cell">${formatCompact(row.totalCollateralUsd)}</td>
            <td class="py-2.5 px-2 text-right tabular-nums text-gray-300">{formatCompact(row.totalDebt)}</td>
            <td class="py-2.5 px-2 hidden lg:table-cell">
              <CeilingBar used={row.totalDebt} total={row.debtCeiling} unlimited={row.unlimited} />
            </td>
            <td class="py-2.5 px-2 text-right tabular-nums hidden md:table-cell
              {row.medianCrBps < 15000 ? 'text-violet-400' : row.medianCrBps < 20000 ? 'text-gray-300' : 'text-teal-400'}">
              {row.medianCrBps > 0 ? `${(row.medianCrBps / 100).toFixed(0)}%` : '--'}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</div>
```

- [ ] **Step 2: Verify build**

```bash
cd src/vault_frontend && npm run build
```

- [ ] **Step 3: Commit**

```bash
git add src/vault_frontend/src/lib/components/explorer/CollateralTable.svelte src/vault_frontend/src/lib/components/explorer/CeilingBar.svelte
git commit -m "feat(explorer): add CollateralTable with CeilingBar"
```

---

## Task 9: Create PoolHealthStrip component

**Files:**
- Create: `src/vault_frontend/src/lib/components/explorer/PoolHealthStrip.svelte`

- [ ] **Step 1: Create PoolHealthStrip.svelte**

```svelte
<script lang="ts">
  interface Props {
    pegStatus: any | null;
    lpApy: number | null;
    spApy: number | null;
    loading?: boolean;
  }
  let { pegStatus, lpApy, spApy, loading = false }: Props = $props();

  const pegColor = $derived.by(() => {
    if (!pegStatus) return 'text-gray-500';
    const imb = pegStatus.max_imbalance_pct;
    if (imb < 2) return 'text-teal-400';
    if (imb < 5) return 'text-violet-400';
    return 'text-pink-400';
  });

  const pegLabel = $derived.by(() => {
    if (!pegStatus) return '--';
    const imb = pegStatus.max_imbalance_pct;
    if (imb < 2) return 'Stable';
    if (imb < 5) return 'Minor drift';
    return `${imb.toFixed(1)}% imbalance`;
  });
</script>

<div class="grid grid-cols-1 sm:grid-cols-3 gap-3">
  <!-- 3pool Peg -->
  <div class="explorer-card flex flex-col gap-1">
    <span class="vital-label">3pool Peg</span>
    {#if loading}
      <span class="text-sm text-gray-500">Loading...</span>
    {:else}
      <span class="text-sm font-medium {pegColor}">{pegLabel}</span>
    {/if}
  </div>

  <!-- LP APY -->
  <div class="explorer-card flex flex-col gap-1">
    <span class="vital-label">LP APY (7d)</span>
    {#if loading}
      <span class="text-sm text-gray-500">Loading...</span>
    {:else}
      <span class="text-sm font-medium text-gray-200 tabular-nums">
        {lpApy != null ? `${lpApy.toFixed(2)}%` : '--'}
      </span>
    {/if}
  </div>

  <!-- SP APY -->
  <div class="explorer-card flex flex-col gap-1">
    <span class="vital-label">Stability Pool APY (7d)</span>
    {#if loading}
      <span class="text-sm text-gray-500">Loading...</span>
    {:else}
      <span class="text-sm font-medium text-gray-200 tabular-nums">
        {spApy != null ? `${spApy.toFixed(2)}%` : '--'}
      </span>
    {/if}
  </div>
</div>
```

- [ ] **Step 2: Verify build**

```bash
cd src/vault_frontend && npm run build
```

- [ ] **Step 3: Commit**

```bash
git add src/vault_frontend/src/lib/components/explorer/PoolHealthStrip.svelte
git commit -m "feat(explorer): add PoolHealthStrip component"
```

---

## Task 10: Rewrite explorer navigation layout

**Files:**
- Modify: `src/vault_frontend/src/routes/explorer/+layout.svelte`

- [ ] **Step 1: Rewrite +layout.svelte**

Replace the entire file with the new navigation. Key changes:
- 7 nav sections: Dashboard, Markets, Pools, Revenue, Activity, Holders, Risk
- Responsive: top bar on desktop, slide-out on mobile
- Search bar preserved
- Existing child routes continue to work

The new layout uses a horizontal top navigation bar (keeping the familiar pattern) but with the expanded set of sections. The search functionality is preserved.

**Important:** The existing routes `/explorer/activity`, `/explorer/holders`, `/explorer/vault/[id]`, `/explorer/address/[principal]`, `/explorer/event/[index]`, `/explorer/canister/[id]`, `/explorer/dex/[source]/[id]`, `/explorer/token/[id]` must still work. The nav highlights the correct section based on current URL.

Read the current `+layout.svelte` to understand the search logic (handleSearch function, isVaultId, isEventIndex, etc.), then rewrite preserving that logic but with the new navigation structure. The search helpers and routing logic should be kept as-is.

**Navigation items:**

```typescript
const NAV_ITEMS = [
  { href: '/explorer', label: 'Dashboard', match: (p: string) => p === '/explorer' },
  { href: '/explorer/markets', label: 'Markets', match: (p: string) => p.startsWith('/explorer/markets') || p.startsWith('/explorer/token/') },
  { href: '/explorer/pools', label: 'Pools', match: (p: string) => p.startsWith('/explorer/pools') },
  { href: '/explorer/revenue', label: 'Revenue', match: (p: string) => p.startsWith('/explorer/revenue') },
  { href: '/explorer/activity', label: 'Activity', match: (p: string) => p.startsWith('/explorer/activity') || p.startsWith('/explorer/event/') || p.startsWith('/explorer/dex/') },
  { href: '/explorer/holders', label: 'Holders', match: (p: string) => p.startsWith('/explorer/holders') },
  { href: '/explorer/risk', label: 'Risk', match: (p: string) => p.startsWith('/explorer/risk') || p.startsWith('/explorer/liquidations') },
];
```

**Note:** Markets, Pools, Revenue, and Risk pages don't exist yet (Phases 2-4). The nav links will point to them now; they'll show 404 until built. That's fine for Phase 1 — the links establish the navigation skeleton. Alternatively, if you prefer, create placeholder `+page.svelte` files for each that show "Coming soon" with a redirect back to dashboard.

- [ ] **Step 2: Create placeholder pages for future sections**

Create minimal placeholder pages so nav links don't 404:

`src/vault_frontend/src/routes/explorer/markets/+page.svelte`:
```svelte
<svelte:head><title>Markets | Rumi Explorer</title></svelte:head>
<div class="max-w-[1100px] mx-auto px-4 py-12 text-center">
  <h1 class="text-xl font-medium text-gray-300 mb-2">Markets</h1>
  <p class="text-sm text-gray-500">Coming in Phase 2. <a href="/explorer" class="text-teal-400 hover:text-teal-300">Back to Dashboard</a></p>
</div>
```

Repeat for: `pools`, `revenue`, `risk` (same pattern, different title).

- [ ] **Step 3: Verify build and that all existing routes still work**

```bash
cd src/vault_frontend && npm run build
```

Check that the build output includes all existing routes (activity, holders, vault, etc.).

- [ ] **Step 4: Commit**

```bash
git add src/vault_frontend/src/routes/explorer/
git commit -m "feat(explorer): rewrite navigation with 7-section structure"
```

---

## Task 11: Rewrite dashboard page

**Files:**
- Modify: `src/vault_frontend/src/routes/explorer/+page.svelte`

- [ ] **Step 1: Rewrite +page.svelte**

This is the biggest task. Replace the 1742-line overview page with the new dashboard. The new page should be structured as:

```
1. ProtocolVitals strip (from get_protocol_summary)
2. TvlChart (from get_tvl_series via fetchTvlSeries)
3. CollateralTable (from fetchVaultSeries + fetchTwap + fetchCollateralConfigs + fetchVolatility per asset)
4. PoolHealthStrip (from fetchPegStatus + fetchApys)
5. Recent Activity (last 10 events, from fetchEvents — reuse EventRow component)
```

**Data fetching pattern:** Use `onMount` with parallel `Promise.allSettled` for independent sections. Each section has its own loading/error state.

```svelte
<script lang="ts">
  import { onMount } from 'svelte';
  import ProtocolVitals from '$components/explorer/ProtocolVitals.svelte';
  import TvlChart from '$components/explorer/TvlChart.svelte';
  import CollateralTable from '$components/explorer/CollateralTable.svelte';
  import PoolHealthStrip from '$components/explorer/PoolHealthStrip.svelte';
  import EventRow from '$components/explorer/EventRow.svelte';
  import {
    fetchProtocolSummary, fetchTvlSeries, fetchVaultSeries,
    fetchTwap, fetchPegStatus, fetchApys, fetchVolatility
  } from '$services/explorer/analyticsService';
  import { fetchEvents, fetchCollateralConfigs, fetchAllVaults } from '$services/explorer/explorerService';
  import { e8sToNumber, getCollateralSymbol, COLLATERAL_SYMBOLS } from '$utils/explorerChartHelpers';

  // State per section
  let summary: any = $state(null);
  let summaryLoading = $state(true);

  let tvlData: any[] = $state([]);
  let tvlLoading = $state(true);

  let collateralRows: any[] = $state([]);
  let collateralLoading = $state(true);

  let pegStatus: any = $state(null);
  let lpApy: number | null = $state(null);
  let spApy: number | null = $state(null);
  let poolsLoading = $state(true);

  let recentEvents: any[] = $state([]);
  let eventsLoading = $state(true);

  // Vault maps for EventRow
  let vaultCollateralMap: Map<number, string> = $state(new Map());
  let vaultOwnerMap: Map<number, string> = $state(new Map());

  onMount(async () => {
    // Fetch all sections in parallel
    const [summaryResult, tvlResult, vaultSeriesResult, twapResult, configsResult, pegResult, apyResult, eventsResult, vaultsResult] = await Promise.allSettled([
      fetchProtocolSummary(),
      fetchTvlSeries(365),
      fetchVaultSeries(1),  // Just latest for collateral table
      fetchTwap(),
      fetchCollateralConfigs(),
      fetchPegStatus(),
      fetchApys(),
      fetchEvents(0n, 10n),
      fetchAllVaults(),
    ]);

    // Protocol summary
    if (summaryResult.status === 'fulfilled' && summaryResult.value) {
      summary = summaryResult.value;
    }
    summaryLoading = false;

    // TVL chart
    if (tvlResult.status === 'fulfilled' && tvlResult.value?.rows) {
      tvlData = tvlResult.value.rows;
    }
    tvlLoading = false;

    // Pools
    if (pegResult.status === 'fulfilled') {
      pegStatus = pegResult.value?.[0] ?? null;  // opt type returns array
    }
    if (apyResult.status === 'fulfilled' && apyResult.value) {
      lpApy = apyResult.value.lp_apy_pct?.[0] ?? null;
      spApy = apyResult.value.sp_apy_pct?.[0] ?? null;
    }
    poolsLoading = false;

    // Recent events
    if (eventsResult.status === 'fulfilled' && eventsResult.value) {
      recentEvents = eventsResult.value.events ?? [];
    }
    eventsLoading = false;

    // Vault maps for EventRow
    if (vaultsResult.status === 'fulfilled' && vaultsResult.value) {
      const collMap = new Map<number, string>();
      const ownerMap = new Map<number, string>();
      for (const v of vaultsResult.value) {
        const id = Number(v.vault_id);
        collMap.set(id, v.collateral_type?.toText?.() ?? String(v.collateral_type ?? ''));
        ownerMap.set(id, v.owner?.toText?.() ?? '');
      }
      vaultCollateralMap = collMap;
      vaultOwnerMap = ownerMap;
    }

    // Build collateral table rows
    await buildCollateralRows(vaultSeriesResult, twapResult, configsResult);
  });

  async function buildCollateralRows(
    vaultSeriesResult: PromiseSettledResult<any>,
    twapResult: PromiseSettledResult<any>,
    configsResult: PromiseSettledResult<any>
  ) {
    try {
      const latestVaultSnapshot = vaultSeriesResult.status === 'fulfilled'
        ? vaultSeriesResult.value?.rows?.[vaultSeriesResult.value.rows.length - 1]
        : null;

      const twapEntries = twapResult.status === 'fulfilled'
        ? twapResult.value?.entries ?? []
        : [];

      const configs = configsResult.status === 'fulfilled'
        ? configsResult.value ?? []
        : [];

      // Build price map from TWAP
      const priceMap = new Map<string, number>();
      for (const entry of twapEntries) {
        const principal = entry.collateral?.toText?.() ?? String(entry.collateral);
        priceMap.set(principal, entry.twap_price);
      }

      // Build config map
      const configMap = new Map<string, any>();
      for (const cfg of configs) {
        const principal = cfg.ledger_canister_id?.toText?.() ?? String(cfg.ledger_canister_id);
        configMap.set(principal, cfg);
      }

      // Build collateral stats from vault series
      const collaterals = latestVaultSnapshot?.collaterals ?? [];

      // Fetch volatility for each collateral in parallel
      const volResults = await Promise.allSettled(
        Object.keys(COLLATERAL_SYMBOLS).map(async (principal) => {
          const { Principal } = await import('@dfinity/principal');
          const vol = await fetchVolatility(Principal.fromText(principal));
          return { principal, vol };
        })
      );
      const volMap = new Map<string, number>();
      for (const r of volResults) {
        if (r.status === 'fulfilled' && r.value.vol) {
          volMap.set(r.value.principal, r.value.vol.annualized_vol_pct);
        }
      }

      const rows = [];
      for (const [principal, symbol] of Object.entries(COLLATERAL_SYMBOLS)) {
        const stats = collaterals.find((c: any) => {
          const p = c.collateral_type?.toText?.() ?? String(c.collateral_type);
          return p === principal;
        });
        const cfg = configMap.get(principal);
        const price = priceMap.get(principal) ?? stats?.price_usd_e8s ? e8sToNumber(stats.price_usd_e8s) : 0;
        const debtCeiling = cfg ? Number(cfg.debt_ceiling) : 0;
        const isUnlimited = debtCeiling >= Number(18446744073709551615n);

        rows.push({
          principal,
          symbol,
          price,
          vaultCount: stats ? Number(stats.vault_count) : 0,
          totalCollateralUsd: stats ? e8sToNumber(stats.total_collateral_e8s) * price : 0,
          totalDebt: stats ? e8sToNumber(stats.total_debt_e8s) : 0,
          debtCeiling: e8sToNumber(debtCeiling),
          unlimited: isUnlimited,
          medianCrBps: stats ? Number(stats.median_cr_bps) : 0,
          volatility: volMap.get(principal) ?? null,
        });
      }

      // Sort by TVL descending
      rows.sort((a, b) => b.totalCollateralUsd - a.totalCollateralUsd);
      collateralRows = rows;
    } catch (err) {
      console.error('[dashboard] buildCollateralRows error:', err);
    } finally {
      collateralLoading = false;
    }
  }
</script>

<svelte:head>
  <title>Dashboard | Rumi Explorer</title>
</svelte:head>

<div class="max-w-[1100px] mx-auto px-4 py-6 space-y-4">
  <!-- Protocol Vitals -->
  <ProtocolVitals {summary} loading={summaryLoading} />

  <!-- TVL Chart -->
  <TvlChart data={tvlData} loading={tvlLoading} />

  <!-- Collateral Overview -->
  <CollateralTable rows={collateralRows} loading={collateralLoading} />

  <!-- Pool Health -->
  <PoolHealthStrip {pegStatus} {lpApy} {spApy} loading={poolsLoading} />

  <!-- Recent Activity -->
  <div class="explorer-card">
    <div class="flex items-center justify-between mb-3">
      <h3 class="text-sm font-medium text-gray-300">Recent Activity</h3>
      <a href="/explorer/activity" class="text-xs text-teal-400 hover:text-teal-300">View all &rarr;</a>
    </div>
    {#if eventsLoading}
      <div class="flex items-center justify-center py-6">
        <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
      </div>
    {:else if recentEvents.length === 0}
      <p class="text-sm text-gray-500 py-4">No recent events.</p>
    {:else}
      <div class="overflow-x-auto">
        <table class="w-full">
          <tbody>
            {#each recentEvents as [index, event]}
              <EventRow {event} index={Number(index)} {vaultCollateralMap} {vaultOwnerMap} />
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  </div>

  <!-- Link to docs for protocol parameters -->
  <div class="text-center py-2">
    <a href="/docs/parameters" class="text-xs text-gray-500 hover:text-gray-400 transition-colors">
      Protocol parameters are documented in the Docs &rarr;
    </a>
  </div>
</div>
```

- [ ] **Step 2: Verify build**

```bash
cd src/vault_frontend && npm run build
```

- [ ] **Step 3: Commit**

```bash
git add src/vault_frontend/src/routes/explorer/+page.svelte
git commit -m "feat(explorer): rewrite dashboard with analytics-powered sections"
```

---

## Task 12: Integration verification

- [ ] **Step 1: Full build verification**

```bash
cd src/vault_frontend && npm run build
```

All routes should compile. Check the build output for any TypeScript errors or missing imports.

- [ ] **Step 2: Deploy to IC mainnet**

```bash
dfx deploy vault_frontend --network ic
```

- [ ] **Step 3: Visual verification in browser**

Navigate to `https://app.rumiprotocol.com/explorer` and verify:
1. New navigation appears with 7 sections
2. Protocol Vitals strip loads with live data
3. TVL chart renders with time-range selector
4. Collateral table shows all 7 assets with prices, vaults, debt, ceiling bars
5. Pool health strip shows peg status and APYs
6. Recent activity shows last events
7. All existing explorer sub-pages still work (/activity, /holders, /vault/[id], etc.)
8. No console errors related to data fetching

- [ ] **Step 4: Fix any issues found during verification**

Address any bugs, layout issues, or data formatting problems.

- [ ] **Step 5: Final commit if fixes were needed**

```bash
git add -A
git commit -m "fix(explorer): Phase 1 integration fixes"
```

---

## Verification Checklist

After all tasks are complete, verify:

- [ ] `npm run build` succeeds with zero errors
- [ ] `lightweight-charts` is in package.json dependencies
- [ ] Analytics service has wrappers for: fetchOhlc, fetchVolatility, fetchPriceSeries, fetchThreePoolSeries
- [ ] Explorer CSS utilities exist in app.css (explorer-card, explorer-nav-link, fill-bar, vital-*)
- [ ] Chart helper utilities exist at `$utils/explorerChartHelpers.ts`
- [ ] New components exist: ModePill, CeilingBar, ProtocolVitals, TvlChart, CollateralTable, PoolHealthStrip
- [ ] Navigation layout has 7 sections with correct URL matching
- [ ] Placeholder pages exist for: markets, pools, revenue, risk
- [ ] Dashboard page uses analytics canister data (not just backend)
- [ ] Protocol Configuration card is REMOVED from overview
- [ ] Per-collateral config cards are REMOVED from overview
- [ ] Liquidation bot section is REMOVED from overview
- [ ] All existing child routes still work (/activity, /holders, /vault/[id], etc.)
- [ ] Deployed and visually verified on mainnet
