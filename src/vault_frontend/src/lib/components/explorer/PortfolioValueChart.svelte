<script lang="ts">
  import { onMount } from 'svelte';
  import { Principal } from '@dfinity/principal';
  import { fetchAddressValueSeries } from '$services/explorer/analyticsService';
  import type {
    AddressValuePoint,
    AddressValueSeriesResponse,
  } from '$declarations/rumi_analytics/rumi_analytics.did';
  import {
    CHART_COLORS,
    formatCompact,
    formatDateShort,
    nsToDate,
  } from '$utils/explorerChartHelpers';
  import { formatUsdRaw } from '$utils/explorerHelpers';

  interface Props {
    principal: string;
  }
  let { principal }: Props = $props();

  // ── Window selector ────────────────────────────────────────────────────
  //
  // Matches the spec: 7d / 30d / 90d / 1y / all. Resolution shrinks for tight
  // windows so short periods still have enough points to see motion, and
  // grows for wide windows so /1y and /all don't blow past the backend's
  // 730-point cap.

  type RangeKey = '7d' | '30d' | '90d' | '1y' | 'all';

  interface RangeSpec {
    key: RangeKey;
    label: string;
    windowNs: bigint | undefined; // undefined → backend default (90d)
    resolutionNs: bigint | undefined;
  }

  const NANOS_PER_SEC = 1_000_000_000n;
  const DAY_NS = 86_400n * NANOS_PER_SEC;
  const HOUR_NS = 3_600n * NANOS_PER_SEC;

  const RANGES: readonly RangeSpec[] = [
    { key: '7d', label: '7D', windowNs: 7n * DAY_NS, resolutionNs: HOUR_NS },
    { key: '30d', label: '30D', windowNs: 30n * DAY_NS, resolutionNs: 4n * HOUR_NS },
    { key: '90d', label: '90D', windowNs: 90n * DAY_NS, resolutionNs: DAY_NS },
    { key: '1y', label: '1Y', windowNs: 365n * DAY_NS, resolutionNs: DAY_NS },
    // "all": request the max window the backend supports. Backend clamps
    // samples to MAX_POINTS (730), so we let it pick a resolution.
    { key: 'all', label: 'All', windowNs: (1n << 63n) - 1n, resolutionNs: undefined },
  ] as const;

  let selected: RangeKey = $state('90d');
  const currentRange = $derived(RANGES.find((r) => r.key === selected) ?? RANGES[2]);

  // ── Data load ──────────────────────────────────────────────────────────

  let response = $state<AddressValueSeriesResponse | null>(null);
  let loading = $state(true);
  let errorMessage = $state<string | null>(null);

  async function loadSeries(target: string, range: RangeSpec) {
    let principalObj: Principal;
    try {
      principalObj = Principal.fromText(target);
    } catch {
      errorMessage = 'Invalid principal';
      loading = false;
      return;
    }
    loading = true;
    errorMessage = null;
    try {
      const res = await fetchAddressValueSeries(principalObj, range.windowNs, range.resolutionNs);
      response = res;
    } catch (err) {
      console.error('[PortfolioValueChart] load failed:', err);
      errorMessage = 'Failed to load portfolio series.';
      response = null;
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    loadSeries(principal, currentRange);
  });

  // ── Layout constants ───────────────────────────────────────────────────

  const WIDTH = 800;
  const HEIGHT = 220;
  const PADDING = { top: 16, right: 12, bottom: 32, left: 54 };
  const chartW = WIDTH - PADDING.left - PADDING.right;
  const chartH = HEIGHT - PADDING.top - PADDING.bottom;

  // ── Source metadata ───────────────────────────────────────────────────
  //
  // Stable ordering across renders so the stacked-area colors don't reshuffle
  // as points mount/unmount.

  interface SourceStyle {
    key: string;
    label: string;
    color: string;
    fill: string;
  }

  const SOURCE_STYLES: readonly SourceStyle[] = [
    { key: 'vault_collateral', label: 'Vault collateral', color: '#d176e8', fill: 'rgba(209, 118, 232, 0.22)' },
    { key: 'three_pool_lp',    label: '3pool LP',         color: '#fbbf24', fill: 'rgba(251, 191, 36, 0.22)' },
    { key: 'sp_deposit',       label: 'Stability pool',   color: '#34d399', fill: 'rgba(52, 211, 153, 0.22)' },
    { key: 'icusd',            label: 'icUSD',            color: '#2DD4BF', fill: 'rgba(45, 212, 191, 0.22)' },
    { key: 'threeusd',         label: '3USD',             color: '#818cf8', fill: 'rgba(129, 140, 248, 0.22)' },
  ] as const;

  const APPROX_LABEL: Record<string, string> = {
    icusd: 'icUSD ledger balance shown as current value (approximation).',
    threeusd: '3USD ledger balance shown as current value (approximation).',
  };

  // ── Derived series ─────────────────────────────────────────────────────

  /** Active points with a non-zero total. Keep zero-valued leading points so
   * the chart shows "nothing → first activity" motion. */
  const points = $derived<AddressValuePoint[]>(response?.points ?? []);

  const hasAnyValue = $derived(points.some((p) => Number(p.value_usd_e8s) > 0));

  /** Value per source at each point, 8-decimal → USD float. */
  function sourceValueUsd(pt: AddressValuePoint, sourceKey: string): number {
    for (const b of pt.breakdown) {
      if (b.source === sourceKey) return Number(b.value_usd_e8s) / 1e8;
    }
    return 0;
  }

  const totalsUsd = $derived(points.map((p) => Number(p.value_usd_e8s) / 1e8));
  const maxTotalUsd = $derived(totalsUsd.reduce((a, b) => Math.max(a, b), 0));

  /**
   * Cumulative stacked bands in the order declared by SOURCE_STYLES. Each band
   * is `[lowerUsd, upperUsd]` per point so the polygon draws from lower up to
   * upper, and the next band's lower starts where this one's upper ended.
   */
  interface BandPoint {
    ts: number;
    lower: number;
    upper: number;
  }

  const stackedBands = $derived.by<Array<{ style: SourceStyle; points: BandPoint[] }>>(() => {
    if (points.length === 0) return [];
    const runningLower = new Array<number>(points.length).fill(0);
    const bands: Array<{ style: SourceStyle; points: BandPoint[] }> = [];
    for (const style of SOURCE_STYLES) {
      const bandPts: BandPoint[] = [];
      for (let i = 0; i < points.length; i += 1) {
        const ts = Number(points[i].ts_ns);
        const value = sourceValueUsd(points[i], style.key);
        const lower = runningLower[i];
        const upper = lower + value;
        bandPts.push({ ts, lower, upper });
        runningLower[i] = upper;
      }
      // Keep bands with any non-zero contribution so the legend stays pruned.
      if (bandPts.some((p) => p.upper - p.lower > 0)) {
        bands.push({ style, points: bandPts });
      }
    }
    return bands;
  });

  /** Per-source total at the most recent point. Drives the legend values. */
  const latestBreakdown = $derived.by<Array<{ style: SourceStyle; valueUsd: number }>>(() => {
    if (points.length === 0) return [];
    const last = points[points.length - 1];
    return SOURCE_STYLES.map((style) => ({
      style,
      valueUsd: sourceValueUsd(last, style.key),
    })).filter((row) => row.valueUsd > 0);
  });

  const latestTotalUsd = $derived(totalsUsd.length ? totalsUsd[totalsUsd.length - 1] : 0);

  // ── Geometry ───────────────────────────────────────────────────────────

  function xPos(ts: number): number {
    if (points.length < 2) return PADDING.left + chartW / 2;
    const first = Number(points[0].ts_ns);
    const last = Number(points[points.length - 1].ts_ns);
    const range = last - first || 1;
    return PADDING.left + ((ts - first) / range) * chartW;
  }

  function yPos(valueUsd: number): number {
    const max = Math.max(maxTotalUsd, 1);
    return PADDING.top + chartH - (valueUsd / max) * chartH;
  }

  /** SVG polygon path string for a stacked band. Walks the upper edge L→R,
   * then the lower edge R→L, then closes. */
  function bandPath(band: { points: BandPoint[] }): string {
    if (band.points.length === 0) return '';
    const upper = band.points
      .map((p, i) => `${i === 0 ? 'M' : 'L'}${xPos(p.ts).toFixed(1)},${yPos(p.upper).toFixed(1)}`)
      .join(' ');
    const lowerReversed = band.points
      .slice()
      .reverse()
      .map((p) => `L${xPos(p.ts).toFixed(1)},${yPos(p.lower).toFixed(1)}`)
      .join(' ');
    return `${upper} ${lowerReversed} Z`;
  }

  // ── Y-axis ticks (4 steps from 0 to max) ─────────────────────────────

  const yTicks = $derived.by<number[]>(() => {
    const max = Math.max(maxTotalUsd, 1);
    return [0, max * 0.25, max * 0.5, max * 0.75, max];
  });

  // ── X-axis labels (~5 evenly-spaced) ──────────────────────────────────

  const xLabels = $derived.by(() => {
    if (points.length < 2) return [] as Array<{ x: number; label: string }>;
    const step = Math.max(1, Math.floor(points.length / 5));
    const labels: Array<{ x: number; label: string }> = [];
    for (let i = 0; i < points.length; i += step) {
      const ts = Number(points[i].ts_ns);
      labels.push({ x: xPos(ts), label: formatDateShort(nsToDate(BigInt(ts))) });
    }
    return labels;
  });

  // ── Hover tooltip ─────────────────────────────────────────────────────

  let hoverIdx: number | null = $state(null);
  const hoverPoint = $derived(hoverIdx !== null ? points[hoverIdx] : null);
  const hoverX = $derived(hoverPoint ? xPos(Number(hoverPoint.ts_ns)) : null);

  function handleMouseMove(e: MouseEvent) {
    const svg = e.currentTarget as SVGSVGElement;
    const rect = svg.getBoundingClientRect();
    const relX = ((e.clientX - rect.left) / rect.width) * WIDTH;
    if (relX < PADDING.left || relX > WIDTH - PADDING.right || points.length === 0) {
      hoverIdx = null;
      return;
    }
    // Binary search for nearest point by x.
    let lo = 0;
    let hi = points.length - 1;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      const midX = xPos(Number(points[mid].ts_ns));
      if (midX < relX) lo = mid + 1;
      else hi = mid;
    }
    // lo might be off by one; pick the closer of lo / lo-1.
    if (lo > 0 && Math.abs(xPos(Number(points[lo].ts_ns)) - relX) > Math.abs(xPos(Number(points[lo - 1].ts_ns)) - relX)) {
      hoverIdx = lo - 1;
    } else {
      hoverIdx = lo;
    }
  }

  function handleMouseLeave() {
    hoverIdx = null;
  }

  const hoverBreakdown = $derived.by(() => {
    if (!hoverPoint) return [];
    return SOURCE_STYLES
      .map((style) => ({ style, valueUsd: sourceValueUsd(hoverPoint, style.key) }))
      .filter((row) => row.valueUsd > 0);
  });

  const hoverTotal = $derived(hoverPoint ? Number(hoverPoint.value_usd_e8s) / 1e8 : 0);
  const hoverDate = $derived(hoverPoint ? nsToDate(hoverPoint.ts_ns) : null);

  // ── Approximate-source note ───────────────────────────────────────────

  const approximateNote = $derived.by(() => {
    if (!response) return '';
    const active = response.approximate_sources
      .filter((src) => latestBreakdown.some((row) => row.style.key === src))
      .map((src) => APPROX_LABEL[src])
      .filter(Boolean);
    return active.join(' ');
  });
</script>

<div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-4 space-y-3">
  <div class="flex items-start justify-between gap-3 flex-wrap">
    <div>
      <div class="text-[10px] uppercase tracking-wider text-gray-500">Portfolio value</div>
      {#if !loading && response}
        <div class="text-2xl font-semibold text-white font-mono tabular-nums mt-0.5">
          {formatUsdRaw(latestTotalUsd)}
        </div>
      {/if}
    </div>
    <div class="inline-flex rounded-lg border border-gray-700/70 overflow-hidden text-[11px]">
      {#each RANGES as r (r.key)}
        <button
          type="button"
          class="px-2.5 py-1 border-r border-gray-700/70 last:border-r-0 transition-colors"
          class:bg-blue-500={selected === r.key}
          class:text-white={selected === r.key}
          class:text-gray-400={selected !== r.key}
          class:hover:text-gray-200={selected !== r.key}
          onclick={() => (selected = r.key)}
        >
          {r.label}
        </button>
      {/each}
    </div>
  </div>

  <div class="relative" style="height: {HEIGHT}px;">
    {#if loading && !response}
      <div class="absolute inset-0 flex items-center justify-center">
        <div class="w-5 h-5 border-2 border-gray-600 border-t-blue-400 rounded-full animate-spin"></div>
      </div>
    {:else if errorMessage}
      <div class="absolute inset-0 flex items-center justify-center text-xs text-rose-400">
        {errorMessage}
      </div>
    {:else if !hasAnyValue}
      <div class="absolute inset-0 flex items-center justify-center text-center">
        <p class="text-xs text-gray-500 max-w-[280px] leading-relaxed">
          No on-chain activity for this principal in the selected window.
        </p>
      </div>
    {:else}
      <svg
        viewBox="0 0 {WIDTH} {HEIGHT}"
        class="w-full h-full"
        preserveAspectRatio="xMidYMid meet"
        onmousemove={handleMouseMove}
        onmouseleave={handleMouseLeave}
        role="img"
        aria-label="Portfolio value over time, stacked by source"
      >
        <!-- Grid + y-axis labels -->
        {#each yTicks as tick (tick)}
          <line
            x1={PADDING.left}
            x2={WIDTH - PADDING.right}
            y1={yPos(tick)}
            y2={yPos(tick)}
            stroke={CHART_COLORS.grid}
            stroke-width="1"
          />
          <text
            x={PADDING.left - 8}
            y={yPos(tick) + 4}
            text-anchor="end"
            fill={CHART_COLORS.textMuted}
            font-size="10"
            font-family="Inter, sans-serif"
          >
            ${formatCompact(tick)}
          </text>
        {/each}

        <!-- Stacked bands -->
        {#each stackedBands as band (band.style.key)}
          <path d={bandPath(band)} fill={band.style.fill} stroke="none" />
          <!-- Top-edge line to give each band a crisp top -->
          <path
            d={band.points
              .map((p, i) => `${i === 0 ? 'M' : 'L'}${xPos(p.ts).toFixed(1)},${yPos(p.upper).toFixed(1)}`)
              .join(' ')}
            fill="none"
            stroke={band.style.color}
            stroke-width="1.25"
          />
        {/each}

        <!-- X-axis labels -->
        {#each xLabels as lbl (lbl.x)}
          <text
            x={lbl.x}
            y={HEIGHT - 8}
            text-anchor="middle"
            fill={CHART_COLORS.textMuted}
            font-size="10"
            font-family="Inter, sans-serif"
          >
            {lbl.label}
          </text>
        {/each}

        <!-- Hover crosshair -->
        {#if hoverX !== null}
          <line
            x1={hoverX}
            x2={hoverX}
            y1={PADDING.top}
            y2={HEIGHT - PADDING.bottom}
            stroke="rgba(255,255,255,0.25)"
            stroke-width="1"
            stroke-dasharray="3 3"
            pointer-events="none"
          />
        {/if}
      </svg>

      <!-- Tooltip -->
      {#if hoverPoint && hoverDate}
        <div
          class="absolute top-2 pointer-events-none bg-gray-900/95 border border-gray-700/70 rounded-lg px-3 py-2 text-xs shadow-xl max-w-[240px]"
          style="left: {Math.min(Math.max((hoverX ?? 0) / WIDTH * 100, 10), 70)}%;"
        >
          <div class="text-[10px] uppercase tracking-wider text-gray-500 mb-1">
            {hoverDate.toLocaleDateString('en-US', { month: 'short', day: 'numeric', year: 'numeric' })}
          </div>
          <div class="font-mono text-sm text-white mb-1.5">{formatUsdRaw(hoverTotal)}</div>
          {#if hoverBreakdown.length > 0}
            <ul class="space-y-0.5">
              {#each hoverBreakdown as row (row.style.key)}
                <li class="flex items-center justify-between gap-3">
                  <div class="flex items-center gap-1.5">
                    <span class="inline-block w-2 h-2 rounded-sm" style="background-color: {row.style.color}"></span>
                    <span class="text-gray-300">{row.style.label}</span>
                  </div>
                  <span class="font-mono text-gray-400 tabular-nums">{formatUsdRaw(row.valueUsd)}</span>
                </li>
              {/each}
            </ul>
          {/if}
        </div>
      {/if}
    {/if}
  </div>

  <!-- Legend / latest breakdown -->
  {#if latestBreakdown.length > 0}
    <div class="flex flex-wrap gap-x-4 gap-y-1.5 pt-1 border-t border-gray-700/40">
      {#each latestBreakdown as row (row.style.key)}
        <div class="flex items-center gap-2 text-xs">
          <span class="inline-block w-2.5 h-2.5 rounded-sm" style="background-color: {row.style.color}"></span>
          <span class="text-gray-400">{row.style.label}</span>
          <span class="text-gray-200 font-mono tabular-nums">{formatUsdRaw(row.valueUsd)}</span>
        </div>
      {/each}
    </div>
  {/if}

  {#if approximateNote}
    <p class="text-[10px] text-gray-500 leading-snug">{approximateNote}</p>
  {/if}
</div>
