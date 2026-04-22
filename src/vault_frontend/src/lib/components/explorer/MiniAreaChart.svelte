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
    return { minT, maxT, yMin: min, yMax: max };
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
      </svg>
    {/if}
  </div>
</div>
