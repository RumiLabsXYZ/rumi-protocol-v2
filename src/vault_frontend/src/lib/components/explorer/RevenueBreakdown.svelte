<script lang="ts">
  import type { InterestSplitEntryDTO } from '$lib/services/types';

  interface Props {
    interestSplit: InterestSplitEntryDTO[];
    weightedAvgRate: number;
    loading?: boolean;
  }

  let { interestSplit, weightedAvgRate, loading = false }: Props = $props();

  const destinationLabels: Record<string, string> = {
    stability_pool: 'Stability Pool',
    treasury: 'Treasury',
    three_pool: '3Pool',
  };

  const destinationColors: Record<string, string> = {
    stability_pool: '#34d399',
    treasury: '#818cf8',
    three_pool: '#f59e0b',
  };

  function getLabel(dest: string): string {
    return destinationLabels[dest] ?? dest;
  }

  function getColor(dest: string): string {
    return destinationColors[dest] ?? '#94a3b8';
  }

  const totalBps = $derived(interestSplit.reduce((sum, e) => sum + e.bps, 0));
</script>

<div class="bg-gray-800/50 border border-gray-700/50 rounded-xl overflow-hidden">
  <div class="px-5 py-4 border-b border-gray-700/50">
    <h3 class="text-sm font-semibold text-gray-200">Treasury & Revenue</h3>
  </div>

  {#if loading}
    <div class="px-5 py-8 text-center text-gray-500 text-sm">Loading...</div>
  {:else}
    <div class="p-5 space-y-4">
      <!-- Weighted average interest rate -->
      <div class="flex items-center justify-between">
        <span class="text-sm text-gray-400">Weighted Avg Interest Rate</span>
        <span class="text-sm font-semibold text-gray-200 tabular-nums">
          {(weightedAvgRate * 100).toFixed(2)}%
        </span>
      </div>

      <!-- Interest split breakdown -->
      {#if interestSplit.length > 0}
        <div class="space-y-3">
          <p class="text-xs text-gray-500 uppercase tracking-wider font-medium">Interest Split</p>

          <!-- Visual bar -->
          <div class="flex h-2.5 rounded-full overflow-hidden bg-gray-700">
            {#each interestSplit as entry}
              {@const pct = totalBps > 0 ? (entry.bps / totalBps) * 100 : 0}
              <div
                class="h-full transition-all duration-300"
                style="width: {pct}%; background: {getColor(entry.destination)};"
                title="{getLabel(entry.destination)}: {(entry.bps / 100).toFixed(1)}%"
              ></div>
            {/each}
          </div>

          <!-- Legend -->
          <div class="flex flex-wrap gap-x-4 gap-y-1.5">
            {#each interestSplit as entry}
              <div class="flex items-center gap-1.5">
                <div class="w-2.5 h-2.5 rounded-full" style="background: {getColor(entry.destination)};"></div>
                <span class="text-xs text-gray-400">{getLabel(entry.destination)}</span>
                <span class="text-xs font-medium text-gray-300 tabular-nums">{(entry.bps / 100).toFixed(1)}%</span>
              </div>
            {/each}
          </div>
        </div>
      {:else}
        <div class="text-sm text-gray-500">No interest split data available</div>
      {/if}
    </div>
  {/if}
</div>
