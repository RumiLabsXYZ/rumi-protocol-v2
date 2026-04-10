<script lang="ts">
  import {
    e8sToNumber, formatCompact, formatDateShort, nsToDate,
    filterByTimeRange, computeYScale,
    CHART_COLORS, TIME_RANGES, type TimeRange
  } from '$utils/explorerChartHelpers';
  import type { DailyTvlRow } from '$declarations/rumi_analytics/rumi_analytics.did';

  interface Props {
    data: DailyTvlRow[];
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

  // Use collateral values for primary scale, but also include debt for proper range
  const allValues = $derived([...points.map(p => p.collateral), ...points.map(p => p.debt)]);
  const yScale = $derived(computeYScale(allValues));

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

      <!-- Gradient definition -->
      <defs>
        <linearGradient id="tvlGradient" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stop-color={CHART_COLORS.teal} stop-opacity="0.2" />
          <stop offset="100%" stop-color={CHART_COLORS.teal} stop-opacity="0.02" />
        </linearGradient>
      </defs>

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
    </svg>

    <!-- Legend -->
    <div class="flex items-center gap-4 mt-2 ml-[60px]">
      <div class="flex items-center gap-1.5">
        <div class="w-3 h-0.5 rounded" style="background: {CHART_COLORS.teal}"></div>
        <span class="text-xs text-gray-500">Collateral (ICP)</span>
      </div>
      <div class="flex items-center gap-1.5">
        <div class="w-3 h-0.5 rounded border-b border-dashed" style="border-color: {CHART_COLORS.purple}"></div>
        <span class="text-xs text-gray-500">Debt (icUSD)</span>
      </div>
    </div>
  {/if}
</div>
