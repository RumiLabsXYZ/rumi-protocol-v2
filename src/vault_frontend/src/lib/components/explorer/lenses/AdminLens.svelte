<script lang="ts">
  import { onMount } from 'svelte';
  import LensHealthStrip from '../LensHealthStrip.svelte';
  import LensActivityPanel from '../LensActivityPanel.svelte';
  import AdminBreakdownCard from '../AdminBreakdownCard.svelte';
  import CanisterInventoryCard from '../CanisterInventoryCard.svelte';
  import MiniAreaChart from '../MiniAreaChart.svelte';
  import { fetchCollectorHealth, fetchAdminEventBreakdown, fetchCycleSeries } from '$services/explorer/analyticsService';
  import { fetchProtocolStatus } from '$services/explorer/explorerService';
  import { CHART_COLORS } from '$utils/explorerChartHelpers';

  let collectorHealth: any = $state(null);
  let status: any = $state(null);
  let lastAdminTsNs: number = $state(0);
  let adminCount24h = $state(0);
  let cycleRows: any[] = $state([]);
  let cyclesLoading = $state(true);
  let loading = $state(true);

  onMount(async () => {
    try {
      const [chR, stR, cyR] = await Promise.allSettled([
        fetchCollectorHealth(),
        fetchProtocolStatus(),
        fetchCycleSeries(720), // ~30d of hourly snapshots
      ]);
      if (chR.status === 'fulfilled') collectorHealth = chR.value;
      if (stR.status === 'fulfilled') status = stR.value;
      if (cyR.status === 'fulfilled') cycleRows = cyR.value ?? [];
    } catch (err) {
      console.error('[AdminLens] onMount error:', err);
    } finally {
      loading = false;
      cyclesLoading = false;
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

  // CollectorHealth wire shape (see rumi_analytics.did): `error_counters`
  // is a record of per-source counters, `cursors` carries per-tailer
  // positions + last_success_ns, `last_pull_cycle_ns` stamps the last
  // pull cycle. (An earlier version of this card read `errors` /
  // `last_collect_ns`, which don't exist — it rendered "--" forever while
  // real error counters went unseen.)
  const collectorErrorEntries = $derived.by(() => {
    const ec = collectorHealth?.error_counters;
    if (!ec) return [] as [string, number][];
    return Object.entries(ec).map(([k, v]) => [k, Number(v ?? 0)] as [string, number]);
  });
  const collectorErrorTotal = $derived(
    collectorErrorEntries.reduce((s, [, v]) => s + v, 0)
  );

  const oracleAgeMin = $derived.by(() => {
    const ts = status?.last_icp_timestamp;
    if (!ts) return null;
    return (Date.now() - Number(ts) / 1_000_000) / 60_000;
  });

  const badDebt = $derived(
    status ? Number(status.protocol_deficit_icusd ?? 0) / 1e8 : null
  );

  const healthMetrics = $derived.by(() => {
    const metrics: any[] = [];
    if (oracleAgeMin != null) {
      metrics.push({
        label: 'Oracle',
        value: oracleAgeMin < 1 ? '<1m ago' : `${Math.round(oracleAgeMin)}m ago`,
        sub: 'last price update',
        tone: oracleAgeMin < 10 ? 'good' : oracleAgeMin < 30 ? 'caution' : 'danger',
      });
    }
    if (badDebt != null) {
      metrics.push({
        label: 'Bad debt',
        value: badDebt === 0 ? '$0' : `${badDebt.toLocaleString(undefined, { maximumFractionDigits: 2 })} icUSD`,
        tone: badDebt === 0 ? 'good' : 'danger',
      });
    }
    if (status) {
      metrics.push({
        label: 'Surge breaker',
        value: status.liquidation_breaker_tripped ? 'Tripped' : 'Normal',
        tone: status.liquidation_breaker_tripped ? 'danger' : 'good',
      });
    }
    metrics.push({
      label: 'Collector errors',
      value: collectorHealth ? collectorErrorTotal.toLocaleString() : '--',
      sub: 'lifetime failed tailer calls',
      tone: collectorErrorTotal > 0 ? 'caution' : 'good',
    });
    metrics.push({ label: 'Last admin action', value: lastAdminRel });
    metrics.push({ label: 'Admin actions 24h', value: adminCount24h.toLocaleString() });
    return metrics;
  });

  const cyclePoints = $derived(
    cycleRows.map((r: any) => ({
      t: Number(r.timestamp_ns) / 1_000_000,
      v: Number(r.cycle_balance) / 1e12, // T cycles
    }))
  );

  function fmtRelNs(ns: any): string {
    if (!ns) return '--';
    const ago = Date.now() - Number(ns) / 1_000_000;
    if (ago < 60_000) return 'just now';
    if (ago < 3_600_000) return `${Math.floor(ago / 60_000)}m ago`;
    if (ago < 86_400_000) return `${Math.floor(ago / 3_600_000)}h ago`;
    return `${Math.floor(ago / 86_400_000)}d ago`;
  }
</script>

<LensHealthStrip title="System health" metrics={healthMetrics} mode={mode === 'GeneralAvailability' ? 'Normal' : mode} loading={loading} />

<LensActivityPanel
  scope="health"
  title="Protocol incidents"
  viewAllHref="/explorer/activity?type=health"
/>

<AdminBreakdownCard />

<CanisterInventoryCard />

<div class="grid grid-cols-1 lg:grid-cols-2 gap-3">
  {#if collectorHealth}
    <div class="explorer-card">
      <h3 class="text-sm font-medium text-gray-300 mb-3">Analytics tailing health</h3>
      <p class="text-xs text-gray-500 mb-3">
        Per-source tailer cursors for the analytics canister. A stale "last success" or a growing
        error counter means rollups may lag until the tailer catches up.
      </p>
      <div class="grid grid-cols-2 md:grid-cols-3 gap-4 text-sm mb-4">
        <div>
          <div class="text-xs text-gray-500">Last pull cycle</div>
          <div class="text-gray-200 tabular-nums mt-0.5">{fmtRelNs(collectorHealth.last_pull_cycle_ns)}</div>
        </div>
        {#each collectorErrorEntries as [src, count] (src)}
          <div>
            <div class="text-xs text-gray-500">{src} errors</div>
            <div class="tabular-nums mt-0.5 {count > 0 ? 'text-amber-300' : 'text-gray-200'}">{count.toLocaleString()}</div>
          </div>
        {/each}
      </div>
      {#if collectorHealth.cursors?.length}
        <div class="space-y-1.5">
          {#each collectorHealth.cursors as c (c.name)}
            <div class="flex items-baseline justify-between text-xs border-b border-white/[0.03] py-1">
              <span class="font-mono text-gray-400">{c.name}</span>
              <span class="tabular-nums text-gray-300">
                {Number(c.cursor_position).toLocaleString()} read
                <span class="text-gray-500">· {fmtRelNs(c.last_success_ns)}</span>
                {#if c.last_error?.length}
                  <span class="text-amber-300" title={String(c.last_error[0])}> · error</span>
                {/if}
              </span>
            </div>
          {/each}
        </div>
      {/if}
    </div>
  {/if}

  <div class="explorer-card">
    <MiniAreaChart
      points={cyclePoints}
      label="Analytics canister cycles (30d)"
      color={CHART_COLORS.caution}
      fillColor="rgba(167, 139, 250, 0.12)"
      valueFormat={(v) => `${v.toFixed(2)}T`}
      yAxisMode="data-fit"
      height={170}
      loading={cyclesLoading}
    />
    <p class="text-xs text-gray-500 mt-2">
      Hourly cycle balance of the analytics canister — the explorer's indexing infrastructure.
      A steady downward slope is normal burn; cliffs mean heavy query traffic, climbs are top-ups.
    </p>
  </div>
</div>

<LensActivityPanel
  scope="admin"
  title="Admin actions"
  viewAllHref="/explorer/activity?type=admin"
/>
