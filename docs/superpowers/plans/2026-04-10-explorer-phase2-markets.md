# Explorer Phase 2: Markets Page Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Markets section with an overview table of all collateral assets and individual market detail pages featuring TradingView OHLC candlestick charts, price history, volatility, and vault statistics.

**Architecture:** The Markets page (`/explorer/markets`) shows a summary table of all 7 collateral assets with live prices, 24h changes, volatility, and TVL. Clicking an asset navigates to `/explorer/markets/[id]` which shows a full-featured market detail page with OHLC candlestick chart (via lightweight-charts), price line chart, volatility gauge, vault breakdown, and collateral config. The existing `/explorer/token/[id]` page remains as-is (it's linked from the collateral table on the dashboard) but the nav-active Markets section uses `/explorer/markets/[id]`.

**Tech Stack:** SvelteKit (Svelte 5 runes), Tailwind CSS, lightweight-charts v5, TypeScript, analytics canister (fetchOhlc, fetchTwap, fetchVolatility, fetchPriceSeries), backend canister (fetchCollateralConfigs, fetchCollateralTotals, fetchAllVaults)

---

### Task 1: Markets Overview Page

**Files:**
- Rewrite: `src/vault_frontend/src/routes/explorer/markets/+page.svelte`
- Reference: `src/vault_frontend/src/lib/utils/explorerChartHelpers.ts` (COLLATERAL_SYMBOLS, COLLATERAL_COLORS)
- Reference: `src/vault_frontend/src/lib/services/explorer/analyticsService.ts` (fetchTwap, fetchVolatility)
- Reference: `src/vault_frontend/src/lib/services/explorer/explorerService.ts` (fetchCollateralConfigs, fetchCollateralTotals)

- [ ] **Step 1: Write the Markets overview page**

Replace the placeholder with a full markets table. The page fetches TWAP prices, volatility for each asset, collateral configs, and collateral totals in parallel. Displays a responsive table with columns: Asset (with color dot + symbol), Price, 24h TWAP, Volatility (annualized %), Vaults, TVL (collateral value in USD), Debt, Ceiling Util. Each row links to `/explorer/markets/{principal}`.

```svelte
<script lang="ts">
  import { onMount } from 'svelte';
  import { Principal } from '@dfinity/principal';
  import CeilingBar from '$components/explorer/CeilingBar.svelte';
  import {
    fetchTwap, fetchVolatility, fetchProtocolSummary
  } from '$services/explorer/analyticsService';
  import {
    fetchCollateralConfigs, fetchCollateralTotals
  } from '$services/explorer/explorerService';
  import {
    e8sToNumber, formatCompact, COLLATERAL_SYMBOLS, COLLATERAL_COLORS
  } from '$utils/explorerChartHelpers';

  interface MarketRow {
    principal: string;
    symbol: string;
    color: string;
    price: number;
    twapPrice: number;
    volatility: number | null;
    vaultCount: number;
    tvlUsd: number;
    totalDebt: number;
    debtCeiling: number;
    unlimited: boolean;
  }

  let rows: MarketRow[] = $state([]);
  let loading = $state(true);

  onMount(async () => {
    const [twapResult, configsResult, totalsResult, summaryResult, ...volResults] = await Promise.allSettled([
      fetchTwap(),
      fetchCollateralConfigs(),
      fetchCollateralTotals(),
      fetchProtocolSummary(),
      ...Object.keys(COLLATERAL_SYMBOLS).map(p =>
        fetchVolatility(Principal.fromText(p))
      ),
    ]);

    const twapData = twapResult.status === 'fulfilled' ? twapResult.value : null;
    const twapEntries = twapData?.entries ?? [];
    const configs = configsResult.status === 'fulfilled' ? configsResult.value ?? [] : [];
    const totals = totalsResult.status === 'fulfilled' ? totalsResult.value ?? [] : [];
    const summary = summaryResult.status === 'fulfilled' ? summaryResult.value : null;

    // Build lookup maps
    const twapMap = new Map<string, { price: number; twap: number }>();
    for (const e of twapEntries) {
      const pid = e.collateral?.toText?.() ?? String(e.collateral);
      twapMap.set(pid, { price: e.latest_price, twap: e.twap_price });
    }

    // Summary prices as fallback
    const summaryPriceMap = new Map<string, number>();
    if (summary?.prices) {
      for (const p of summary.prices) {
        const pid = p.collateral?.toText?.() ?? String(p.collateral);
        summaryPriceMap.set(pid, p.twap_price);
      }
    }

    const configMap = new Map<string, any>();
    for (const c of configs) {
      const pid = c.ledger_canister_id?.toText?.() ?? String(c.ledger_canister_id);
      configMap.set(pid, c);
    }

    const totalsMap = new Map<string, any>();
    for (const t of totals) {
      const pid = t.collateral_type?.toText?.() ?? String(t.collateral_type ?? '');
      if (pid) totalsMap.set(pid, t);
    }

    const principals = Object.keys(COLLATERAL_SYMBOLS);
    const volMap = new Map<string, number>();
    for (let i = 0; i < principals.length; i++) {
      const r = volResults[i];
      if (r.status === 'fulfilled' && r.value) {
        volMap.set(principals[i], r.value.annualized_vol_pct);
      }
    }

    const built: MarketRow[] = [];
    for (const [principal, symbol] of Object.entries(COLLATERAL_SYMBOLS)) {
      const twap = twapMap.get(principal);
      const cfg = configMap.get(principal);
      const tot = totalsMap.get(principal);

      const price = twap?.price ?? summaryPriceMap.get(principal) ?? 0;
      const twapPrice = twap?.twap ?? price;

      const totalCollateral = tot?.total_collateral_e8s != null
        ? e8sToNumber(tot.total_collateral_e8s) : 0;
      const totalDebt = tot?.total_debt_e8s != null
        ? e8sToNumber(tot.total_debt_e8s) : 0;
      const vaultCount = tot?.vault_count != null ? Number(tot.vault_count) : 0;

      const debtCeilingRaw = cfg?.debt_ceiling ?? 0n;
      const isUnlimited = typeof debtCeilingRaw === 'bigint'
        ? debtCeilingRaw >= 18446744073709551615n
        : Number(debtCeilingRaw) >= Number.MAX_SAFE_INTEGER;

      built.push({
        principal,
        symbol,
        color: COLLATERAL_COLORS[symbol as keyof typeof COLLATERAL_COLORS] ?? '#888',
        price,
        twapPrice,
        volatility: volMap.get(principal) ?? null,
        vaultCount,
        tvlUsd: totalCollateral * price,
        totalDebt,
        debtCeiling: e8sToNumber(Number(debtCeilingRaw)),
        unlimited: isUnlimited,
      });
    }

    built.sort((a, b) => b.tvlUsd - a.tvlUsd);
    rows = built;
    loading = false;
  });
</script>

<svelte:head>
  <title>Markets | Rumi Explorer</title>
</svelte:head>

<div class="space-y-4">
  <div class="explorer-card">
    <div class="flex items-center justify-between mb-4">
      <h2 class="text-sm font-medium text-gray-300">Collateral Markets</h2>
      <span class="text-xs text-gray-500">{rows.length} assets</span>
    </div>

    {#if loading}
      <div class="flex items-center justify-center py-12">
        <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
      </div>
    {:else}
      <div class="overflow-x-auto">
        <table class="w-full text-sm">
          <thead>
            <tr class="border-b text-xs text-gray-500 uppercase tracking-wider" style="border-color: var(--rumi-border);">
              <th class="px-3 py-2.5 text-left">Asset</th>
              <th class="px-3 py-2.5 text-right">Price</th>
              <th class="px-3 py-2.5 text-right hidden sm:table-cell">TWAP</th>
              <th class="px-3 py-2.5 text-right hidden md:table-cell">Volatility</th>
              <th class="px-3 py-2.5 text-right hidden lg:table-cell">Vaults</th>
              <th class="px-3 py-2.5 text-right">TVL</th>
              <th class="px-3 py-2.5 text-right hidden sm:table-cell">Debt</th>
              <th class="px-3 py-2.5 text-right hidden md:table-cell">Ceiling</th>
            </tr>
          </thead>
          <tbody>
            {#each rows as row}
              <tr class="border-b hover:bg-white/[0.02] transition-colors cursor-pointer" style="border-color: var(--rumi-border);">
                <td class="px-3 py-3">
                  <a href="/explorer/markets/{row.principal}" class="flex items-center gap-2 hover:text-teal-300 transition-colors">
                    <span class="w-2.5 h-2.5 rounded-full flex-shrink-0" style="background: {row.color};"></span>
                    <span class="font-medium text-gray-200">{row.symbol}</span>
                  </a>
                </td>
                <td class="px-3 py-3 text-right tabular-nums text-gray-200">
                  ${row.price >= 1 ? row.price.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 }) : row.price.toFixed(6)}
                </td>
                <td class="px-3 py-3 text-right tabular-nums text-gray-400 hidden sm:table-cell">
                  ${row.twapPrice >= 1 ? row.twapPrice.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 }) : row.twapPrice.toFixed(6)}
                </td>
                <td class="px-3 py-3 text-right tabular-nums hidden md:table-cell {row.volatility != null && row.volatility > 80 ? 'text-pink-400' : row.volatility != null && row.volatility > 40 ? 'text-violet-400' : 'text-gray-400'}">
                  {row.volatility != null ? `${row.volatility.toFixed(1)}%` : '--'}
                </td>
                <td class="px-3 py-3 text-right tabular-nums text-gray-400 hidden lg:table-cell">
                  {row.vaultCount}
                </td>
                <td class="px-3 py-3 text-right tabular-nums text-gray-200">
                  ${formatCompact(row.tvlUsd)}
                </td>
                <td class="px-3 py-3 text-right tabular-nums text-gray-400 hidden sm:table-cell">
                  {formatCompact(row.totalDebt)} icUSD
                </td>
                <td class="px-3 py-3 hidden md:table-cell">
                  <div class="flex justify-end">
                    <div class="w-20">
                      <CeilingBar used={row.totalDebt} ceiling={row.debtCeiling} unlimited={row.unlimited} />
                    </div>
                  </div>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  </div>
</div>
```

- [ ] **Step 2: Verify the page renders**

Run: `cd src/vault_frontend && npm run build`
Expected: Build succeeds with no errors.

- [ ] **Step 3: Commit**

```bash
git add src/vault_frontend/src/routes/explorer/markets/+page.svelte
git commit -m "feat(explorer): add markets overview page with collateral table"
```

---

### Task 2: OHLC Candlestick Chart Component

**Files:**
- Create: `src/vault_frontend/src/lib/components/explorer/OhlcChart.svelte`
- Reference: `src/vault_frontend/package.json` (lightweight-charts ^5.1.0 already installed)
- Reference: `src/declarations/rumi_analytics/rumi_analytics.did.d.ts` (OhlcCandle, OhlcResponse types)

- [ ] **Step 1: Create OhlcChart component**

This component wraps the TradingView lightweight-charts library. It accepts an array of OhlcCandle data and renders a candlestick chart with volume. Uses Svelte 5 runes and $effect for reactivity. Supports bucket size selection (1h, 4h, 1d).

```svelte
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import type { OhlcCandle } from '$declarations/rumi_analytics/rumi_analytics.did';

  interface Props {
    candles: OhlcCandle[];
    symbol?: string;
    loading?: boolean;
  }

  let { candles, symbol = '', loading = false }: Props = $props();

  let chartContainer: HTMLDivElement;
  let chart: any = null;
  let candleSeries: any = null;

  const formattedData = $derived(
    candles
      .map(c => ({
        time: Number(c.timestamp_ns) / 1_000_000_000 as any,
        open: c.open,
        high: c.high,
        low: c.low,
        close: c.close,
      }))
      .sort((a, b) => (a.time as number) - (b.time as number))
  );

  async function initChart() {
    if (!chartContainer || chart) return;
    const { createChart, CandlestickSeries, ColorType } = await import('lightweight-charts');

    chart = createChart(chartContainer, {
      layout: {
        background: { type: ColorType.Solid, color: 'transparent' },
        textColor: '#605a75',
        fontFamily: 'Inter, system-ui, sans-serif',
        fontSize: 11,
      },
      grid: {
        vertLines: { color: 'rgba(90, 100, 180, 0.06)' },
        horzLines: { color: 'rgba(90, 100, 180, 0.06)' },
      },
      crosshair: {
        vertLine: { color: 'rgba(45, 212, 191, 0.3)', width: 1, style: 2 },
        horzLine: { color: 'rgba(45, 212, 191, 0.3)', width: 1, style: 2 },
      },
      rightPriceScale: {
        borderColor: 'rgba(90, 100, 180, 0.12)',
      },
      timeScale: {
        borderColor: 'rgba(90, 100, 180, 0.12)',
        timeVisible: true,
      },
      handleScale: { axisPressedMouseMove: true },
      handleScroll: { vertTouchDrag: false },
    });

    candleSeries = chart.addSeries(CandlestickSeries, {
      upColor: '#2DD4BF',
      downColor: '#e06b9f',
      borderUpColor: '#2DD4BF',
      borderDownColor: '#e06b9f',
      wickUpColor: '#2DD4BF',
      wickDownColor: '#e06b9f',
    });

    if (formattedData.length > 0) {
      candleSeries.setData(formattedData);
      chart.timeScale().fitContent();
    }

    // Handle resize
    const observer = new ResizeObserver(entries => {
      for (const entry of entries) {
        const { width } = entry.contentRect;
        if (chart && width > 0) {
          chart.applyOptions({ width });
        }
      }
    });
    observer.observe(chartContainer);
  }

  $effect(() => {
    if (candleSeries && formattedData.length > 0) {
      candleSeries.setData(formattedData);
      chart?.timeScale().fitContent();
    }
  });

  onMount(() => {
    initChart();
  });

  onDestroy(() => {
    if (chart) {
      chart.remove();
      chart = null;
      candleSeries = null;
    }
  });
</script>

<div class="explorer-card">
  <div class="flex items-center justify-between mb-3">
    <h3 class="text-sm font-medium text-gray-300">
      {symbol ? `${symbol}/USD` : 'Price'} OHLC
    </h3>
  </div>

  {#if loading}
    <div class="flex items-center justify-center" style="height: 320px;">
      <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
    </div>
  {:else if formattedData.length === 0}
    <div class="flex items-center justify-center text-gray-500 text-sm" style="height: 320px;">
      No OHLC data available
    </div>
  {:else}
    <div bind:this={chartContainer} style="height: 320px; width: 100%;"></div>
  {/if}
</div>
```

- [ ] **Step 2: Verify build**

Run: `cd src/vault_frontend && npm run build`
Expected: Build succeeds. lightweight-charts is dynamically imported so it won't break SSR.

- [ ] **Step 3: Commit**

```bash
git add src/vault_frontend/src/lib/components/explorer/OhlcChart.svelte
git commit -m "feat(explorer): add OHLC candlestick chart component using lightweight-charts"
```

---

### Task 3: Price Line Chart Component

**Files:**
- Create: `src/vault_frontend/src/lib/components/explorer/PriceLineChart.svelte`

- [ ] **Step 1: Create PriceLineChart component**

A simpler line chart for price history using lightweight-charts AreaSeries. Shows price over time with gradient fill. Used as an alternative view on the market detail page.

```svelte
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';

  interface PricePoint {
    timestamp_ns: bigint;
    price: number;
  }

  interface Props {
    data: PricePoint[];
    symbol?: string;
    color?: string;
    loading?: boolean;
  }

  let { data, symbol = '', color = '#2DD4BF', loading = false }: Props = $props();

  let chartContainer: HTMLDivElement;
  let chart: any = null;
  let series: any = null;

  const formattedData = $derived(
    data
      .map(d => ({
        time: Number(d.timestamp_ns) / 1_000_000_000 as any,
        value: d.price,
      }))
      .sort((a, b) => (a.time as number) - (b.time as number))
  );

  async function initChart() {
    if (!chartContainer || chart) return;
    const { createChart, AreaSeries, ColorType } = await import('lightweight-charts');

    chart = createChart(chartContainer, {
      layout: {
        background: { type: ColorType.Solid, color: 'transparent' },
        textColor: '#605a75',
        fontFamily: 'Inter, system-ui, sans-serif',
        fontSize: 11,
      },
      grid: {
        vertLines: { color: 'rgba(90, 100, 180, 0.06)' },
        horzLines: { color: 'rgba(90, 100, 180, 0.06)' },
      },
      crosshair: {
        vertLine: { color: 'rgba(45, 212, 191, 0.3)', width: 1, style: 2 },
        horzLine: { color: 'rgba(45, 212, 191, 0.3)', width: 1, style: 2 },
      },
      rightPriceScale: {
        borderColor: 'rgba(90, 100, 180, 0.12)',
      },
      timeScale: {
        borderColor: 'rgba(90, 100, 180, 0.12)',
        timeVisible: true,
      },
      handleScroll: { vertTouchDrag: false },
    });

    series = chart.addSeries(AreaSeries, {
      lineColor: color,
      topColor: `${color}33`,
      bottomColor: `${color}05`,
      lineWidth: 2,
    });

    if (formattedData.length > 0) {
      series.setData(formattedData);
      chart.timeScale().fitContent();
    }

    const observer = new ResizeObserver(entries => {
      for (const entry of entries) {
        const { width } = entry.contentRect;
        if (chart && width > 0) chart.applyOptions({ width });
      }
    });
    observer.observe(chartContainer);
  }

  $effect(() => {
    if (series && formattedData.length > 0) {
      series.setData(formattedData);
      chart?.timeScale().fitContent();
    }
  });

  onMount(() => { initChart(); });

  onDestroy(() => {
    if (chart) { chart.remove(); chart = null; series = null; }
  });
</script>

<div class="explorer-card">
  <div class="flex items-center justify-between mb-3">
    <h3 class="text-sm font-medium text-gray-300">
      {symbol ? `${symbol}/USD` : 'Price'} History
    </h3>
  </div>

  {#if loading}
    <div class="flex items-center justify-center" style="height: 240px;">
      <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
    </div>
  {:else if formattedData.length === 0}
    <div class="flex items-center justify-center text-gray-500 text-sm" style="height: 240px;">
      No price data available
    </div>
  {:else}
    <div bind:this={chartContainer} style="height: 240px; width: 100%;"></div>
  {/if}
</div>
```

- [ ] **Step 2: Verify build**

Run: `cd src/vault_frontend && npm run build`
Expected: Build succeeds.

- [ ] **Step 3: Commit**

```bash
git add src/vault_frontend/src/lib/components/explorer/PriceLineChart.svelte
git commit -m "feat(explorer): add price line chart component using lightweight-charts"
```

---

### Task 4: Volatility Gauge Component

**Files:**
- Create: `src/vault_frontend/src/lib/components/explorer/VolatilityGauge.svelte`

- [ ] **Step 1: Create VolatilityGauge component**

A compact visual indicator showing annualized volatility percentage with a color-coded semicircular gauge. Low (<20%) = teal, Medium (20-50%) = violet, High (50-80%) = amber, Extreme (>80%) = pink.

```svelte
<script lang="ts">
  interface Props {
    value: number | null;
    label?: string;
  }

  let { value, label = 'Annualized Volatility' }: Props = $props();

  const displayValue = $derived(value != null ? `${value.toFixed(1)}%` : '--');
  const severity = $derived.by(() => {
    if (value == null) return 'unknown';
    if (value < 20) return 'low';
    if (value < 50) return 'medium';
    if (value < 80) return 'high';
    return 'extreme';
  });

  const colorMap = {
    low: 'text-teal-400',
    medium: 'text-violet-400',
    high: 'text-amber-400',
    extreme: 'text-pink-400',
    unknown: 'text-gray-500',
  };

  const bgMap = {
    low: 'bg-teal-400/10',
    medium: 'bg-violet-400/10',
    high: 'bg-amber-400/10',
    extreme: 'bg-pink-400/10',
    unknown: 'bg-gray-500/10',
  };

  const labelMap = {
    low: 'Low',
    medium: 'Medium',
    high: 'High',
    extreme: 'Extreme',
    unknown: '--',
  };
</script>

<div class="flex items-center gap-3 px-3 py-2 rounded-lg {bgMap[severity]}">
  <div class="text-right">
    <div class="text-lg font-semibold tabular-nums {colorMap[severity]}">{displayValue}</div>
    <div class="text-xs text-gray-500">{label}</div>
  </div>
  <div class="px-2 py-0.5 rounded text-xs font-medium {colorMap[severity]} {bgMap[severity]}">
    {labelMap[severity]}
  </div>
</div>
```

- [ ] **Step 2: Commit**

```bash
git add src/vault_frontend/src/lib/components/explorer/VolatilityGauge.svelte
git commit -m "feat(explorer): add volatility gauge component"
```

---

### Task 5: Market Detail Page

**Files:**
- Create: `src/vault_frontend/src/routes/explorer/markets/[id]/+page.svelte`
- Reference: All chart components from Tasks 2-4
- Reference: `src/vault_frontend/src/lib/services/explorer/analyticsService.ts`
- Reference: `src/vault_frontend/src/lib/services/explorer/explorerService.ts`

- [ ] **Step 1: Create the market detail page**

This is the main feature page. Shows: header with asset info + price, OHLC candlestick chart with bucket selector, price line chart, volatility gauge, vault stats (total collateral, debt, vault count, CR), config parameters, and links to related pages. Uses tab-based chart switching (Candlestick / Line).

```svelte
<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/stores';
  import { Principal } from '@dfinity/principal';
  import OhlcChart from '$components/explorer/OhlcChart.svelte';
  import PriceLineChart from '$components/explorer/PriceLineChart.svelte';
  import VolatilityGauge from '$components/explorer/VolatilityGauge.svelte';
  import CeilingBar from '$components/explorer/CeilingBar.svelte';
  import {
    fetchOhlc, fetchTwap, fetchVolatility, fetchPriceSeries
  } from '$services/explorer/analyticsService';
  import {
    fetchCollateralConfigs, fetchCollateralTotals, fetchAllVaults
  } from '$services/explorer/explorerService';
  import {
    e8sToNumber, formatCompact, COLLATERAL_SYMBOLS, COLLATERAL_COLORS,
    getCollateralSymbol, bpsToPercent
  } from '$utils/explorerChartHelpers';
  import { decodeRustDecimal } from '$utils/decimalUtils';
  import type { OhlcCandle } from '$declarations/rumi_analytics/rumi_analytics.did';

  const tokenId = $derived($page.params.id);
  const symbol = $derived(COLLATERAL_SYMBOLS[tokenId as keyof typeof COLLATERAL_SYMBOLS] ?? getCollateralSymbol(tokenId));
  const assetColor = $derived(COLLATERAL_COLORS[symbol as keyof typeof COLLATERAL_COLORS] ?? '#888');

  // Chart view state
  type ChartView = 'candle' | 'line';
  let chartView: ChartView = $state('candle');

  // Bucket size for OHLC
  type BucketSize = '1h' | '4h' | '1d';
  let bucketSize: BucketSize = $state('4h');
  const BUCKET_SECS: Record<BucketSize, bigint> = {
    '1h': 3600n,
    '4h': 14400n,
    '1d': 86400n,
  };

  // Data state
  let candles: OhlcCandle[] = $state([]);
  let candleLoading = $state(true);

  let priceData: { timestamp_ns: bigint; price: number }[] = $state([]);
  let priceLoading = $state(true);

  let volatility: number | null = $state(null);
  let currentPrice = $state(0);
  let twapPrice = $state(0);
  let config: any = $state(null);
  let totals: any = $state(null);
  let vaultCount = $state(0);
  let totalCollateral = $state(0);
  let totalDebt = $state(0);
  let infoLoading = $state(true);

  async function loadCandles() {
    candleLoading = true;
    try {
      const principal = Principal.fromText(tokenId);
      const result = await fetchOhlc(principal, BUCKET_SECS[bucketSize]);
      candles = result?.candles ?? [];
    } catch (e) {
      console.error('[markets] OHLC fetch error:', e);
      candles = [];
    } finally {
      candleLoading = false;
    }
  }

  onMount(async () => {
    const principal = Principal.fromText(tokenId);

    // Parallel fetch of all data
    const [twapResult, volResult, priceResult, configsResult, totalsResult, vaultsResult] =
      await Promise.allSettled([
        fetchTwap(),
        fetchVolatility(principal),
        fetchPriceSeries(500),
        fetchCollateralConfigs(),
        fetchCollateralTotals(),
        fetchAllVaults(),
      ]);

    // TWAP / current price
    if (twapResult.status === 'fulfilled' && twapResult.value) {
      const entry = twapResult.value.entries?.find((e: any) => {
        const pid = e.collateral?.toText?.() ?? String(e.collateral);
        return pid === tokenId;
      });
      if (entry) {
        currentPrice = entry.latest_price;
        twapPrice = entry.twap_price;
      }
    }

    // Volatility
    if (volResult.status === 'fulfilled' && volResult.value) {
      volatility = volResult.value.annualized_vol_pct;
    }

    // Price series for line chart - filter to this collateral
    if (priceResult.status === 'fulfilled' && priceResult.value) {
      priceData = priceResult.value
        .flatMap((snap: any) => {
          const match = snap.prices?.find((p: any) => {
            const pid = typeof p[0] === 'object' ? p[0]?.toText?.() ?? String(p[0]) : String(p[0]);
            return pid === tokenId;
          });
          if (match) {
            return [{ timestamp_ns: snap.timestamp_ns, price: match[1] }];
          }
          return [];
        });
    }
    priceLoading = false;

    // Config
    if (configsResult.status === 'fulfilled') {
      const configs = configsResult.value ?? [];
      config = configs.find((c: any) => {
        const pid = c.ledger_canister_id?.toText?.() ?? String(c.ledger_canister_id);
        return pid === tokenId;
      });
    }

    // Totals
    if (totalsResult.status === 'fulfilled') {
      const tots = totalsResult.value ?? [];
      const match = tots.find((t: any) => {
        const pid = t.collateral_type?.toText?.() ?? String(t.collateral_type ?? '');
        return pid === tokenId;
      });
      if (match) {
        totals = match;
        totalCollateral = match.total_collateral_e8s != null ? e8sToNumber(match.total_collateral_e8s) : 0;
        totalDebt = match.total_debt_e8s != null ? e8sToNumber(match.total_debt_e8s) : 0;
        vaultCount = match.vault_count != null ? Number(match.vault_count) : 0;
      }
    }

    // Vault count from full vault list as fallback
    if (vaultCount === 0 && vaultsResult.status === 'fulfilled') {
      const vaults = vaultsResult.value ?? [];
      const filtered = vaults.filter((v: any) => {
        const ct = v.collateral_type?.toText?.() ?? String(v.collateral_type ?? '');
        return ct === tokenId;
      });
      vaultCount = filtered.length;
    }

    infoLoading = false;

    // Load OHLC candles
    await loadCandles();
  });

  // Re-fetch candles when bucket size changes
  $effect(() => {
    const _ = bucketSize;
    loadCandles();
  });

  // Derived: config display values
  const liquidationRatio = $derived(config ? decodeRustDecimal(config.liquidation_ratio) : 0);
  const borrowThreshold = $derived(config ? decodeRustDecimal(config.borrow_threshold_ratio) : 0);
  const borrowingFee = $derived(config ? decodeRustDecimal(config.borrowing_fee) : 0);
  const liquidationBonus = $derived(config ? decodeRustDecimal(config.liquidation_bonus) : 0);
  const interestRateApr = $derived(config?.interest_rate_apr ? decodeRustDecimal(config.interest_rate_apr) : 0);
  const debtCeilingRaw = $derived(config?.debt_ceiling ?? 0n);
  const isUnlimited = $derived(
    typeof debtCeilingRaw === 'bigint'
      ? debtCeilingRaw >= 18446744073709551615n
      : Number(debtCeilingRaw) >= Number.MAX_SAFE_INTEGER
  );
  const debtCeiling = $derived(e8sToNumber(Number(debtCeilingRaw)));
  const collateralValueUsd = $derived(totalCollateral * currentPrice);

  function formatPct(val: number, decimals = 1): string {
    return `${(val * 100).toFixed(decimals)}%`;
  }
</script>

<svelte:head>
  <title>{symbol} Market | Rumi Explorer</title>
</svelte:head>

<div class="space-y-4">
  <!-- Back link -->
  <a href="/explorer/markets" class="inline-flex items-center gap-1 text-xs text-gray-500 hover:text-teal-400 transition-colors">
    <svg class="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2"><path stroke-linecap="round" stroke-linejoin="round" d="M15 19l-7-7 7-7"/></svg>
    All Markets
  </a>

  <!-- Header -->
  <div class="explorer-card">
    <div class="flex flex-wrap items-center gap-4">
      <div class="flex items-center gap-3">
        <span class="w-4 h-4 rounded-full" style="background: {assetColor};"></span>
        <h1 class="text-xl font-semibold text-gray-100">{symbol}</h1>
      </div>

      {#if infoLoading}
        <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
      {:else}
        <div class="flex items-baseline gap-2">
          <span class="text-2xl font-semibold tabular-nums text-gray-100">
            ${currentPrice >= 1 ? currentPrice.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 }) : currentPrice.toFixed(6)}
          </span>
          {#if twapPrice > 0 && twapPrice !== currentPrice}
            <span class="text-sm text-gray-500">
              TWAP: ${twapPrice >= 1 ? twapPrice.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 }) : twapPrice.toFixed(6)}
            </span>
          {/if}
        </div>
        <div class="ml-auto">
          <VolatilityGauge value={volatility} />
        </div>
      {/if}
    </div>
  </div>

  <!-- Chart Section -->
  <div>
    <!-- Chart view tabs + bucket selector -->
    <div class="flex items-center justify-between mb-2">
      <div class="flex gap-1">
        <button
          class="px-2.5 py-1 text-xs rounded-md transition-colors {chartView === 'candle' ? 'bg-teal-500/15 text-teal-300' : 'text-gray-500 hover:text-gray-300'}"
          onclick={() => chartView = 'candle'}
        >
          Candlestick
        </button>
        <button
          class="px-2.5 py-1 text-xs rounded-md transition-colors {chartView === 'line' ? 'bg-teal-500/15 text-teal-300' : 'text-gray-500 hover:text-gray-300'}"
          onclick={() => chartView = 'line'}
        >
          Line
        </button>
      </div>

      {#if chartView === 'candle'}
        <div class="flex gap-1">
          {#each (['1h', '4h', '1d'] as const) as size}
            <button
              class="px-2.5 py-1 text-xs rounded-md transition-colors {bucketSize === size ? 'bg-teal-500/15 text-teal-300' : 'text-gray-500 hover:text-gray-300'}"
              onclick={() => bucketSize = size}
            >
              {size}
            </button>
          {/each}
        </div>
      {/if}
    </div>

    {#if chartView === 'candle'}
      <OhlcChart {candles} {symbol} loading={candleLoading} />
    {:else}
      <PriceLineChart data={priceData} {symbol} color={assetColor} loading={priceLoading} />
    {/if}
  </div>

  <!-- Stats Grid -->
  <div class="grid grid-cols-2 md:grid-cols-4 gap-3">
    <div class="explorer-card">
      <div class="text-xs text-gray-500 mb-1">Collateral Locked</div>
      <div class="text-lg font-semibold tabular-nums text-gray-200">
        {infoLoading ? '--' : formatCompact(totalCollateral)}
        <span class="text-xs text-gray-500">{symbol}</span>
      </div>
      {#if !infoLoading && collateralValueUsd > 0}
        <div class="text-xs text-gray-500 mt-0.5">${formatCompact(collateralValueUsd)}</div>
      {/if}
    </div>
    <div class="explorer-card">
      <div class="text-xs text-gray-500 mb-1">Total Debt</div>
      <div class="text-lg font-semibold tabular-nums text-gray-200">
        {infoLoading ? '--' : formatCompact(totalDebt)}
        <span class="text-xs text-gray-500">icUSD</span>
      </div>
    </div>
    <div class="explorer-card">
      <div class="text-xs text-gray-500 mb-1">Vaults</div>
      <div class="text-lg font-semibold tabular-nums text-gray-200">
        {infoLoading ? '--' : vaultCount}
      </div>
    </div>
    <div class="explorer-card">
      <div class="text-xs text-gray-500 mb-1">Debt Ceiling</div>
      {#if infoLoading}
        <div class="text-lg font-semibold text-gray-200">--</div>
      {:else if isUnlimited}
        <div class="text-lg font-semibold text-gray-400">Unlimited</div>
      {:else}
        <div class="mb-1">
          <CeilingBar used={totalDebt} ceiling={debtCeiling} unlimited={false} />
        </div>
        <div class="text-xs text-gray-500 tabular-nums">
          {formatCompact(totalDebt)} / {formatCompact(debtCeiling)} icUSD
        </div>
      {/if}
    </div>
  </div>

  <!-- Config Parameters -->
  {#if config && !infoLoading}
    <div class="explorer-card">
      <h3 class="text-sm font-medium text-gray-300 mb-3">Collateral Parameters</h3>
      <div class="grid grid-cols-2 md:grid-cols-3 gap-x-6 gap-y-2 text-sm">
        <div class="flex justify-between py-1.5 border-b" style="border-color: var(--rumi-border);">
          <span class="text-gray-500">Liquidation Ratio</span>
          <span class="tabular-nums text-gray-300">{formatPct(liquidationRatio)}</span>
        </div>
        <div class="flex justify-between py-1.5 border-b" style="border-color: var(--rumi-border);">
          <span class="text-gray-500">Borrow Threshold</span>
          <span class="tabular-nums text-gray-300">{formatPct(borrowThreshold)}</span>
        </div>
        <div class="flex justify-between py-1.5 border-b" style="border-color: var(--rumi-border);">
          <span class="text-gray-500">Liquidation Bonus</span>
          <span class="tabular-nums text-gray-300">{formatPct(liquidationBonus)}</span>
        </div>
        <div class="flex justify-between py-1.5 border-b" style="border-color: var(--rumi-border);">
          <span class="text-gray-500">Borrowing Fee</span>
          <span class="tabular-nums text-gray-300">{borrowingFee > 0 ? formatPct(borrowingFee, 2) : '0%'}</span>
        </div>
        <div class="flex justify-between py-1.5 border-b" style="border-color: var(--rumi-border);">
          <span class="text-gray-500">Interest APR</span>
          <span class="tabular-nums text-gray-300">{interestRateApr > 0 ? formatPct(interestRateApr) : '0%'}</span>
        </div>
        <div class="flex justify-between py-1.5 border-b" style="border-color: var(--rumi-border);">
          <span class="text-gray-500">Min Vault Debt</span>
          <span class="tabular-nums text-gray-300">
            {config.min_vault_debt ? `${e8sToNumber(Number(config.min_vault_debt))} icUSD` : '--'}
          </span>
        </div>
      </div>
    </div>
  {/if}

  <!-- Links -->
  <div class="text-center py-2">
    <a href="/explorer/token/{tokenId}" class="text-xs text-gray-500 hover:text-gray-400 transition-colors">
      View full token detail page (vaults, events) &rarr;
    </a>
  </div>
</div>
```

- [ ] **Step 2: Verify build**

Run: `cd src/vault_frontend && npm run build`
Expected: Build succeeds.

- [ ] **Step 3: Commit**

```bash
git add src/vault_frontend/src/routes/explorer/markets/\[id\]/+page.svelte
git commit -m "feat(explorer): add market detail page with OHLC chart and asset stats"
```

---

### Task 6: Update Dashboard Collateral Table Links

**Files:**
- Modify: `src/vault_frontend/src/lib/components/explorer/CollateralTable.svelte`

- [ ] **Step 1: Update collateral table links to point to markets**

In CollateralTable.svelte, the asset name links should point to `/explorer/markets/{principal}` instead of `/explorer/token/{principal}` since the markets page now has the richer chart experience.

Find the `<a>` tag that wraps the asset symbol and update its href from `/explorer/token/{row.principal}` to `/explorer/markets/{row.principal}`.

- [ ] **Step 2: Verify build**

Run: `cd src/vault_frontend && npm run build`
Expected: Build succeeds.

- [ ] **Step 3: Commit**

```bash
git add src/vault_frontend/src/lib/components/explorer/CollateralTable.svelte
git commit -m "feat(explorer): link collateral table rows to markets detail pages"
```

---

### Task 7: Build, Deploy, and Verify

- [ ] **Step 1: Full build**

Run: `cd src/vault_frontend && npm run build`
Expected: Build succeeds with no errors.

- [ ] **Step 2: Deploy to mainnet**

Run: `dfx deploy vault_frontend --network ic`
Expected: Deployment succeeds.

- [ ] **Step 3: Visual verification**

Navigate to `https://tcfua-yaaaa-aaaap-qrd7q-cai.icp0.io/explorer/markets` and verify:
1. Markets overview table shows all 7 assets with prices, volatility, TVL
2. Click an asset (e.g., ICP) and verify the detail page loads
3. OHLC candlestick chart renders (or shows "No OHLC data available" if analytics hasn't accumulated candles yet)
4. Bucket size buttons (1h, 4h, 1d) work
5. Line chart tab works
6. Stats grid shows collateral, debt, vaults, ceiling
7. Config parameters display correctly
8. Back link to Markets overview works
9. Navigation bar correctly highlights "Markets" when on markets pages

- [ ] **Step 4: Commit any fixes from verification**
