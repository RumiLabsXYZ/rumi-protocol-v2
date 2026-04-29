<script lang="ts">
  import { onMount } from 'svelte';
  import LensHealthStrip from '../LensHealthStrip.svelte';
  import LensActivityPanel from '../LensActivityPanel.svelte';
  import AdminBreakdownCard from '../AdminBreakdownCard.svelte';
  import CanisterInventoryCard from '../CanisterInventoryCard.svelte';
  import { fetchCollectorHealth, fetchAdminEventBreakdown } from '$services/explorer/analyticsService';
  import { fetchProtocolStatus } from '$services/explorer/explorerService';

  let collectorHealth: any = $state(null);
  let status: any = $state(null);
  let lastAdminTsNs: number = $state(0);
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

    // The backend's admin/setter Event variants do NOT carry a timestamp field
    // (most are empty structs like `SetBorrowingFee {}`), so deriving the
    // "Last admin action" relative time from a `fetchEvents` lookup yields no
    // usable timestamp. The analytics shadow log (evt_admin) stamps each entry
    // with the tail-time as an upper-bound timestamp, and the analytics
    // breakdown response exposes that as `last_at_ns` per label. Use a 24h
    // breakdown for the count, then a wider (default 30d) window to find the
    // most recent admin timestamp across all labels.
    try {
      const windowNs = BigInt(86_400) * 1_000_000_000n;
      const breakdown = await fetchAdminEventBreakdown(windowNs);
      adminCount24h = breakdown.labels.reduce((s, l) => s + Number(l.count), 0);
    } catch (err) {
      console.warn('[AdminLens] 24h admin count fetch failed:', err);
    }
    try {
      const breakdown30d = await fetchAdminEventBreakdown();
      let maxTs = 0;
      for (const l of breakdown30d.labels) {
        const t = l.last_at_ns?.[0];
        if (t != null && Number(t) > maxTs) maxTs = Number(t);
      }
      lastAdminTsNs = maxTs;
    } catch (err) {
      console.warn('[AdminLens] last admin timestamp fetch failed:', err);
    }
  });

  const mode = $derived.by(() => {
    if (!status?.mode) return 'Unknown';
    return Object.keys(status.mode)[0] ?? 'Unknown';
  });

  const lastAdminRel = $derived.by(() => {
    if (!lastAdminTsNs) return '--';
    const ms = lastAdminTsNs / 1_000_000;
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
        sub: 'failed tailer calls (see breakdown below)',
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
