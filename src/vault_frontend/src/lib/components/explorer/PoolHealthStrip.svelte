<script lang="ts">
  import type { PegStatus } from '$declarations/rumi_analytics/rumi_analytics.did';

  interface Props {
    pegStatus: PegStatus | null;
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
