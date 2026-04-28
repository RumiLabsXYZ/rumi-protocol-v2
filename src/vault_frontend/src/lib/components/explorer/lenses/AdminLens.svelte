<script lang="ts">
  import { onMount } from 'svelte';
  import LensHealthStrip from '../LensHealthStrip.svelte';
  import LensActivityPanel from '../LensActivityPanel.svelte';
  import AdminBreakdownCard from '../AdminBreakdownCard.svelte';
  import { fetchCollectorHealth } from '$services/explorer/analyticsService';
  import { fetchProtocolStatus } from '$services/explorer/explorerService';

  let collectorHealth: any = $state(null);
  let status: any = $state(null);
  let loading = $state(true);

  onMount(async () => {
    try {
      const [chR, stR] = await Promise.allSettled([
        fetchCollectorHealth(),
        fetchProtocolStatus(),
      ]);
      if (chR.status === 'fulfilled') collectorHealth = chR.value;
      if (stR.status === 'fulfilled') status = stR.value;
    } catch (err) {
      console.error('[AdminLens] onMount error:', err);
    } finally {
      loading = false;
    }
  });

  const mode = $derived.by(() => {
    if (!status?.mode) return 'Unknown';
    return Object.keys(status.mode)[0] ?? 'Unknown';
  });

  const healthMetrics = $derived.by(() => {
    const metrics: any[] = [
      { label: 'Mode', value: mode },
    ];
    if (collectorHealth) {
      const errs = Object.values(collectorHealth?.errors ?? {}).reduce((s: number, v: any) => s + Number(v ?? 0), 0);
      metrics.push({
        label: 'Collector errors',
        value: errs.toLocaleString(),
        sub: 'analytics tailing',
        tone: errs > 0 ? 'caution' as const : 'good' as const,
      });
    }
    return metrics;
  });
</script>

<LensHealthStrip title="Admin" metrics={healthMetrics} loading={loading} />

<AdminBreakdownCard />

{#if collectorHealth}
  <div class="explorer-card">
    <h3 class="text-sm font-medium text-gray-300 mb-3">Analytics tailing health</h3>
    <p class="text-xs text-gray-500 mb-3">
      Per-source counters for the analytics canister's tailers. Non-zero error counts mean some
      events may have been missed and rollups could be stale until the tailer catches up.
    </p>
    <div class="grid grid-cols-2 md:grid-cols-4 gap-4 text-sm">
      <div>
        <div class="text-xs text-gray-500">Last collect</div>
        <div class="text-gray-200 tabular-nums mt-0.5">
          {#if collectorHealth.last_collect_ns && Number(collectorHealth.last_collect_ns) > 0}
            {new Date(Number(collectorHealth.last_collect_ns) / 1_000_000).toLocaleString()}
          {:else}
            --
          {/if}
        </div>
      </div>
      {#if collectorHealth.errors}
        {#each Object.entries(collectorHealth.errors) as [src, count]}
          <div>
            <div class="text-xs text-gray-500">{src} errors</div>
            <div class="text-gray-200 tabular-nums mt-0.5">{Number(count).toLocaleString()}</div>
          </div>
        {/each}
      {/if}
    </div>
  </div>
{/if}

<LensActivityPanel
  scope="admin"
  title="Admin actions"
  viewAllHref="/explorer/activity?type=admin"
/>
