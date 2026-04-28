<script lang="ts">
  import { CHART_COLORS, computeYScale } from '$utils/explorerChartHelpers';

  interface Props {
    points: { t: number; v: number }[];
    width?: number;
    height?: number;
    color?: string;
    fillColor?: string;
    label?: string;
    valueFormat?: (v: number) => string;
    loading?: boolean;
  }
  let {
    points,
    width = 600,
    height = 140,
    color = CHART_COLORS.teal,
    fillColor = CHART_COLORS.tealDim,
    label,
    valueFormat,
    loading = false,
  }: Props = $props();

  const padX = 8;
  const padY = 8;

  const scale = $derived.by(() => {
    if (!points.length) return { minT: 0, maxT: 0, yMin: 0, yMax: 1 };
    const minT = points[0].t;
    const maxT = points[points.length - 1].t;
    const { min, max } = computeYScale(points.map(p => p.v));
    // Anchor the y-axis at 0 when all values are non-negative — otherwise a
    // small spike on top of zeros (typical for low-volume swap charts) renders
    // as a flat line near the baseline because computeYScale tightens around
    // the data. Anchoring keeps the spike visually distinct from baseline.
    const allNonNeg = points.every((p) => p.v >= 0);
    const yMin = allNonNeg ? 0 : min;
    return { minT, maxT, yMin, yMax: max };
  });

  // Highlight dots for non-zero points when the series is sparse — without
  // them, an hourly chart with mostly-zero buckets looks empty even when
  // there's real activity in a couple of buckets.
  const dotPoints = $derived.by(() => {
    if (!points.length) return [];
    const nonZero = points.filter((p) => p.v > 0);
    if (nonZero.length === 0) return [];
    // Only annotate when the series is sparse enough that the line alone
    // wouldn't be obvious. Above this threshold a normal line is plenty.
    if (nonZero.length > 20) return [];
    return nonZero;
  });

  function x(t: number) {
    const { minT, maxT } = scale;
    if (maxT === minT) return padX;
    return padX + ((t - minT) / (maxT - minT)) * (width - padX * 2);
  }

  function y(v: number) {
    const { yMin, yMax } = scale;
    if (yMax === yMin) return height / 2;
    return padY + (1 - (v - yMin) / (yMax - yMin)) * (height - padY * 2);
  }

  const pathD = $derived.by(() => {
    if (!points.length) return '';
    return points.map((p, i) => `${i === 0 ? 'M' : 'L'} ${x(p.t).toFixed(2)} ${y(p.v).toFixed(2)}`).join(' ');
  });

  const fillD = $derived.by(() => {
    if (!points.length) return '';
    const base = height - padY;
    const top = points.map((p, i) => `${i === 0 ? 'M' : 'L'} ${x(p.t).toFixed(2)} ${y(p.v).toFixed(2)}`).join(' ');
    return `${top} L ${x(points[points.length - 1].t).toFixed(2)} ${base} L ${x(points[0].t).toFixed(2)} ${base} Z`;
  });

  const latest = $derived(points.length ? points[points.length - 1].v : 0);
</script>

<div class="space-y-2">
  {#if label}
    <div class="flex items-center justify-between">
      <span class="text-xs font-medium text-gray-400">{label}</span>
      {#if !loading && points.length}
        <span class="text-sm font-semibold tabular-nums" style="color: {color};">
          {valueFormat ? valueFormat(latest) : latest.toLocaleString()}
        </span>
      {/if}
    </div>
  {/if}
  <div class="relative" style="height: {height}px;">
    {#if loading}
      <div class="absolute inset-0 flex items-center justify-center">
        <div class="w-4 h-4 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
      </div>
    {:else if !points.length}
      <div class="absolute inset-0 flex items-center justify-center text-xs text-gray-500">
        No data
      </div>
    {:else}
      <svg viewBox="0 0 {width} {height}" class="w-full h-full" preserveAspectRatio="none">
        <path d={fillD} fill={fillColor} stroke="none" />
        <path d={pathD} fill="none" stroke={color} stroke-width="1.5" stroke-linejoin="round" />
        {#each dotPoints as p (p.t)}
          <circle cx={x(p.t).toFixed(2)} cy={y(p.v).toFixed(2)} r="3" fill={color} />
        {/each}
      </svg>
    {/if}
  </div>
</div>
