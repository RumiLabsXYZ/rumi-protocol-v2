<script lang="ts">
  import ModePill from './ModePill.svelte';
  import { formatCompact, e8sToNumber, bpsToPercent } from '$utils/explorerChartHelpers';
  import type { ProtocolSummary } from '$declarations/rumi_analytics/rumi_analytics.did';

  interface Props {
    summary: ProtocolSummary | null;
    loading?: boolean;
  }
  let { summary, loading = false }: Props = $props();

  const tvl = $derived(summary ? e8sToNumber(summary.total_collateral_usd_e8s) : 0);
  const debt = $derived(summary ? e8sToNumber(summary.total_debt_e8s) : 0);
  const supply = $derived(
    summary?.circulating_supply_icusd_e8s?.length
      ? e8sToNumber(summary.circulating_supply_icusd_e8s[0])
      : 0
  );
  const cr = $derived(summary ? Number(summary.system_cr_bps) : 0);
  const mode = $derived.by(() => {
    if (!summary) return 'Normal';
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
