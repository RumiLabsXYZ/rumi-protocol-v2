<script lang="ts">
  import { onMount } from 'svelte';
  import LensHealthStrip from '../LensHealthStrip.svelte';
  import LensActivityPanel from '../LensActivityPanel.svelte';
  import MiniAreaChart from '../MiniAreaChart.svelte';
  import {
    fetchCycleSeries, fetchCollectorHealth,
  } from '$services/explorer/analyticsService';
  import { fetchProtocolConfig, fetchProtocolStatus } from '$services/explorer/explorerService';
  import { formatCompact, CHART_COLORS } from '$utils/explorerChartHelpers';

  interface CyclePoint { canister: string; points: { t: number; v: number }[]; latest: number }

  let cycleSeries: any[] = $state([]);
  let collectorHealth: any = $state(null);
  let config: any = $state(null);
  let status: any = $state(null);
  let loading = $state(true);

  onMount(async () => {
    try {
      const [cR, chR, cfR, stR] = await Promise.allSettled([
        fetchCycleSeries(500),
        fetchCollectorHealth(),
        fetchProtocolConfig(),
        fetchProtocolStatus(),
      ]);
      if (cR.status === 'fulfilled') cycleSeries = cR.value ?? [];
      if (chR.status === 'fulfilled') collectorHealth = chR.value;
      if (cfR.status === 'fulfilled') config = cfR.value;
      if (stR.status === 'fulfilled') status = stR.value;
    } catch (err) {
      console.error('[AdminLens] onMount error:', err);
    } finally {
      loading = false;
    }
  });

  // cycleSeries rows are HourlyCycleSnapshot { timestamp_ns, cycle_balance }
  // For v1, show the aggregate series (single-canister analytics canister only).
  const aggregatePoints = $derived(
    cycleSeries.map((r: any) => ({
      t: Number(r.timestamp_ns) / 1_000_000,
      v: Number(r.cycle_balance) / 1e12, // TC
    }))
  );
  const latestCycles = $derived(
    aggregatePoints.length ? aggregatePoints[aggregatePoints.length - 1].v : 0
  );

  const mode = $derived.by(() => {
    if (!status?.mode) return 'Unknown';
    return Object.keys(status.mode)[0] ?? 'Unknown';
  });

  const configRows = $derived.by(() => {
    if (!config) return [] as { label: string; value: string }[];
    const rows: { label: string; value: string }[] = [];
    const pct = (v: any) => v == null ? '--' : `${(Number(v) * 100).toFixed(2)}%`;
    const cr = (v: any) => v == null ? '--' : `${(Number(v) * 100).toFixed(0)}%`;
    rows.push({ label: 'Mode', value: mode });
    rows.push({ label: 'Frozen', value: config.frozen ? 'yes' : 'no' });
    rows.push({ label: 'Borrowing fee', value: pct(config.borrowing_fee) });
    rows.push({ label: 'Redemption fee floor', value: pct(config.redemption_fee_floor) });
    rows.push({ label: 'Redemption fee ceiling', value: pct(config.redemption_fee_ceiling) });
    rows.push({ label: 'Liquidation bonus', value: pct(config.liquidation_bonus) });
    rows.push({ label: 'Liquidation protocol share', value: pct(config.liquidation_protocol_share) });
    rows.push({ label: 'RMR floor', value: pct(config.rmr_floor) });
    rows.push({ label: 'RMR ceiling', value: pct(config.rmr_ceiling) });
    rows.push({ label: 'RMR floor CR', value: cr(config.rmr_floor_cr) });
    rows.push({ label: 'RMR ceiling CR', value: cr(config.rmr_ceiling_cr) });
    rows.push({ label: 'Recovery CR threshold', value: cr(config.recovery_mode_threshold) });
    rows.push({ label: 'Recovery CR multiplier', value: Number(config.recovery_cr_multiplier ?? 0).toFixed(2) });
    rows.push({ label: 'Max partial liq ratio', value: pct(config.max_partial_liquidation_ratio) });
    rows.push({ label: 'Min icUSD mint', value: `${(Number(config.min_icusd_amount ?? 0n) / 1e8).toFixed(2)} icUSD` });
    rows.push({ label: 'Global mint cap', value: `${formatCompact(Number(config.global_icusd_mint_cap ?? 0n) / 1e8)} icUSD` });
    rows.push({ label: 'Reserve redemptions enabled', value: config.reserve_redemptions_enabled ? 'yes' : 'no' });
    return rows;
  });

  const healthMetrics = $derived.by(() => {
    const metrics: any[] = [
      { label: 'Mode', value: mode },
      {
        label: 'Analytics cycles',
        value: `${formatCompact(latestCycles)} TC`,
        tone: latestCycles < 1 ? 'danger' as const : latestCycles < 3 ? 'caution' as const : 'good' as const,
      },
    ];
    if (collectorHealth) {
      const errs = Object.values(collectorHealth?.errors ?? {}).reduce((s: number, v: any) => s + Number(v ?? 0), 0);
      metrics.push({ label: 'Collector errors', value: errs.toLocaleString(), tone: errs > 0 ? 'caution' as const : 'good' as const });
    }
    metrics.push({ label: 'Config params', value: String(configRows.length), sub: 'tracked' });
    return metrics;
  });

  const splitRows = $derived.by(() => {
    if (!config?.interest_split) return [];
    return (config.interest_split as any[]).map(e => ({
      destination: e.destination,
      bps: Number(e.bps ?? 0),
    }));
  });
</script>

<LensHealthStrip title="Admin" metrics={healthMetrics} loading={loading} />

<div class="explorer-card">
  <MiniAreaChart
    points={aggregatePoints}
    label="Analytics canister cycles (TC)"
    color={CHART_COLORS.caution}
    fillColor="rgba(167, 139, 250, 0.15)"
    valueFormat={(v) => `${v.toFixed(2)} TC`}
    height={160}
    loading={loading}
  />
</div>

<div class="explorer-card">
  <h3 class="text-sm font-medium text-gray-300 mb-3">Protocol config</h3>
  {#if loading}
    <div class="flex items-center justify-center py-6">
      <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
    </div>
  {:else if !config}
    <p class="text-sm text-gray-500 py-4">Config unavailable.</p>
  {:else}
    <div class="grid grid-cols-1 md:grid-cols-2 gap-x-8 gap-y-2">
      {#each configRows as row}
        <div class="flex items-center justify-between text-sm py-1 border-b border-white/[0.03]">
          <span class="text-gray-400">{row.label}</span>
          <span class="tabular-nums text-gray-200 font-medium">{row.value}</span>
        </div>
      {/each}
    </div>
    {#if splitRows.length > 0}
      <div class="mt-4 pt-4 border-t border-white/5">
        <div class="text-xs font-medium text-gray-400 mb-2">Interest split</div>
        <div class="flex flex-wrap gap-4">
          {#each splitRows as r}
            <div class="flex items-baseline gap-2 text-sm">
              <span class="text-gray-500">{r.destination}</span>
              <span class="tabular-nums text-gray-200 font-medium">{(r.bps / 100).toFixed(1)}%</span>
            </div>
          {/each}
        </div>
      </div>
    {/if}
  {/if}
</div>

{#if collectorHealth}
  <div class="explorer-card">
    <h3 class="text-sm font-medium text-gray-300 mb-3">Collector health</h3>
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

<LensActivityPanel scope="admin" title="Admin events (protocol + 3Pool + AMM)" viewAllHref="/explorer/activity?type=admin,system" />
