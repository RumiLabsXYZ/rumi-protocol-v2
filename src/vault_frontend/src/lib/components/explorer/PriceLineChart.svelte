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
  let resizeObserver: ResizeObserver | null = null;

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

    resizeObserver = new ResizeObserver(entries => {
      for (const entry of entries) {
        const { width } = entry.contentRect;
        if (chart && width > 0) chart.applyOptions({ width });
      }
    });
    resizeObserver.observe(chartContainer);
  }

  $effect(() => {
    if (series && formattedData.length > 0) {
      series.setData(formattedData);
      chart?.timeScale().fitContent();
    }
  });

  onMount(() => { initChart(); });

  onDestroy(() => {
    resizeObserver?.disconnect();
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
