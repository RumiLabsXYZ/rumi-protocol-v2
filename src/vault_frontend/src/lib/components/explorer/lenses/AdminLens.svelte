<script lang="ts">
  import { onMount } from 'svelte';
  import LensHealthStrip from '../LensHealthStrip.svelte';
  import LensActivityPanel from '../LensActivityPanel.svelte';
  import AdminBreakdownCard from '../AdminBreakdownCard.svelte';
  import CanisterInventoryCard from '../CanisterInventoryCard.svelte';
  import { fetchCollectorHealth, fetchAdminEventBreakdown } from '$services/explorer/analyticsService';
  import { fetchProtocolStatus, fetchEvents } from '$services/explorer/explorerService';
  import { extractEventTimestamp } from '$utils/displayEvent';

  let collectorHealth: any = $state(null);
  let status: any = $state(null);
  let lastAdminEvent: any = $state(null);
  let adminCount24h = $state(0);
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

    // Latest admin event (page=0 returns most recent first)
    try {
      const result = await fetchEvents(0n, 1n, { types: ['Admin'] });
      lastAdminEvent = result.events?.[0]?.[1] ?? null;
    } catch (err) {
      console.warn('[AdminLens] latest admin fetch failed:', err);
    }

    // 24h admin count: ask analytics for a 24h windowed breakdown and sum labels
    try {
      const windowNs = BigInt(86_400) * 1_000_000_000n;
      const breakdown = await fetchAdminEventBreakdown(windowNs);
      adminCount24h = breakdown.labels.reduce((s, l) => s + Number(l.count), 0);
    } catch (err) {
      console.warn('[AdminLens] 24h admin count fetch failed:', err);
    }
  });

  const mode = $derived.by(() => {
    if (!status?.mode) return 'Unknown';
    return Object.keys(status.mode)[0] ?? 'Unknown';
  });

  const lastAdminRel = $derived.by(() => {
    if (!lastAdminEvent) return '--';
    const tsNs = extractEventTimestamp(lastAdminEvent);
    if (!tsNs) return '--';
    const ms = tsNs / 1_000_000;
    const ago = Date.now() - ms;
    if (ago < 60_000) return 'just now';
    if (ago < 3_600_000) return `${Math.floor(ago / 60_000)}m ago`;
    if (ago < 86_400_000) return `${Math.floor(ago / 3_600_000)}h ago`;
    return `${Math.floor(ago / 86_400_000)}d ago`;
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
        sub: 'failed inter-canister calls from analytics tailers (unexpected response shape, decode failure). Per-source breakdown below.',
        tone: errs > 0 ? 'caution' as const : 'good' as const,
      });
    }
    metrics.push({ label: 'Last admin action', value: lastAdminRel });
    metrics.push({ label: 'Admin actions 24h', value: adminCount24h.toLocaleString() });
    return metrics;
  });
</script>

<LensHealthStrip title="Admin" metrics={healthMetrics} loading={loading} />

<AdminBreakdownCard />

<CanisterInventoryCard />

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
