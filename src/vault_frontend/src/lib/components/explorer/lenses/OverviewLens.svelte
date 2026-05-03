<script lang="ts">
  import { onMount } from 'svelte';
  import LensHealthStrip from '../LensHealthStrip.svelte';
  import LensActivityPanel from '../LensActivityPanel.svelte';
  import MiniAreaChart from '../MiniAreaChart.svelte';
  import ProtocolVitals from '../ProtocolVitals.svelte';
  import PoolHealthStrip from '../PoolHealthStrip.svelte';
  import TvlChart from '../TvlChart.svelte';
  import TokenFlowChord from '../TokenFlowChord.svelte';
  import {
    fetchProtocolSummary, fetchVaultSeries,
    fetchFeeSeries, fetchPegStatus, fetchApys, fetchTokenFlow,
  } from '$services/explorer/analyticsService';
  import { ProtocolService } from '$services/protocol';
  import { threePoolService } from '$services/threePoolService';
  import { stabilityPoolService } from '$services/stabilityPoolService';
  import { e8sToNumber, formatCompact, CHART_COLORS } from '$utils/explorerChartHelpers';
  import { liveSpApyPct, liveLpApyPct } from '$utils/liveApy';
  import type { ProtocolSummary, PegStatus, TokenFlowEdge } from '$declarations/rumi_analytics/rumi_analytics.did';

  let summary: ProtocolSummary | null = $state(null);
  let summaryLoading = $state(true);
  let vaultRows: any[] = $state([]);
  let feeRows: any[] = $state([]);
  let seriesLoading = $state(true);
  let pegStatus: PegStatus | null = $state(null);
  let analyticsLpApy: number | null = $state(null);
  let analyticsSpApy: number | null = $state(null);
  let liveLp: number | null = $state(null);
  let liveSp: number | null = $state(null);
  let poolsLoading = $state(true);

  type FlowWindowKey = '24h' | '7d' | '30d';
  const FLOW_WINDOW_NS: Record<FlowWindowKey, bigint> = {
    '24h': 86_400n * 1_000_000_000n,
    '7d': 7n * 86_400n * 1_000_000_000n,
    '30d': 30n * 86_400n * 1_000_000_000n,
  };
  let flowWindow = $state<FlowWindowKey>('7d');
  let flowEdges = $state<TokenFlowEdge[]>([]);
  let flowLoading = $state(true);

  async function reloadFlow() {
    flowLoading = true;
    try {
      const resp = await fetchTokenFlow(FLOW_WINDOW_NS[flowWindow], undefined, 12);
      flowEdges = resp.edges;
    } catch (err) {
      console.error('[OverviewLens] fetchTokenFlow failed:', err);
      flowEdges = [];
    } finally {
      flowLoading = false;
    }
  }

  $effect(() => {
    // React to window flips; the first run also bootstraps the initial fetch.
    void flowWindow;
    reloadFlow();
  });

  onMount(async () => {
    const [sumR, vaultR, feeR, pegR, apyR] = await Promise.allSettled([
      fetchProtocolSummary(),
      fetchVaultSeries(90),
      fetchFeeSeries(90),
      fetchPegStatus(),
      fetchApys(),
    ]);

    if (sumR.status === 'fulfilled' && sumR.value) summary = sumR.value;
    summaryLoading = false;

    if (vaultR.status === 'fulfilled') vaultRows = vaultR.value ?? [];
    if (feeR.status === 'fulfilled') feeRows = feeR.value ?? [];
    seriesLoading = false;

    if (pegR.status === 'fulfilled') pegStatus = pegR.value ?? null;
    if (apyR.status === 'fulfilled' && apyR.value) {
      const aLp = apyR.value.lp_apy_pct?.[0];
      const aSp = apyR.value.sp_apy_pct?.[0];
      if (typeof aLp === 'number') analyticsLpApy = aLp;
      if (typeof aSp === 'number') analyticsSpApy = aSp;
    }

    // Always compute the live values too. The analytics 7d numbers can sit at
    // zero when the rolling window has no realized fee activity, even though
    // LPs/SP depositors are still earning from interest_split. Live derives
    // from current protocol + pool state and reflects what a depositor would
    // earn right now. Falls back to analytics if any input is missing.
    try {
      const [psR, poolR, spR] = await Promise.allSettled([
        ProtocolService.getProtocolStatus(),
        threePoolService.getPoolStatus(),
        stabilityPoolService.getPoolStatus(),
      ]);
      const ps = psR.status === 'fulfilled' ? psR.value : null;
      const pool = poolR.status === 'fulfilled' ? poolR.value : null;
      const sp = spR.status === 'fulfilled' ? spR.value : null;
      liveLp = liveLpApyPct(ps, pool?.balances);
      liveSp = liveSpApyPct(ps, sp);
    } catch (err) {
      console.error('[OverviewLens] live APY compute error:', err);
    }
    poolsLoading = false;
  });

  const lpApy = $derived(liveLp ?? analyticsLpApy);
  const spApy = $derived(liveSp ?? analyticsSpApy);
  const lpApySub = $derived(liveLp != null ? 'live' : '7d');
  const spApySub = $derived(liveSp != null ? 'live' : '7d');

  const pegPct = $derived.by(() => {
    if (!pegStatus) return '--';
    const imb = pegStatus.max_imbalance_pct;
    return `${imb >= 0 ? '+' : ''}${imb.toFixed(2)}%`;
  });

  const pegTone = $derived.by(() => {
    if (!pegStatus) return 'muted' as const;
    const imb = pegStatus.max_imbalance_pct;
    if (imb < 2) return 'good' as const;
    if (imb < 5) return 'caution' as const;
    return 'danger' as const;
  });

  const healthMetrics = $derived.by(() => {
    if (!summary) return [];
    const tvl = e8sToNumber(summary.total_collateral_usd_e8s);
    const supply = summary.circulating_supply_icusd_e8s?.length
      ? e8sToNumber(summary.circulating_supply_icusd_e8s[0]) : 0;
    const volume24h = e8sToNumber(summary.volume_24h_e8s);
    return [
      { label: 'TVL', value: `$${formatCompact(tvl)}` },
      { label: 'icUSD Supply', value: `$${formatCompact(supply)}` },
      { label: '24h Volume', value: `$${formatCompact(volume24h)}` },
      { label: '24h Swaps', value: Number(summary.swap_count_24h).toLocaleString() },
      { label: 'Peg', value: pegPct, tone: pegTone },
      { label: 'LP APY', value: lpApy != null ? `${lpApy.toFixed(2)}%` : '--', sub: lpApySub },
      { label: 'SP APY', value: spApy != null ? `${spApy.toFixed(2)}%` : '--', sub: spApySub },
    ];
  });

  const vaultCountPoints = $derived(
    vaultRows.map((r: any) => ({
      t: Number(r.timestamp_ns) / 1_000_000,
      v: Number(r.total_vault_count ?? r.total_vaults ?? r.vault_count ?? 0),
    }))
  );
  const debtPoints = $derived(
    vaultRows.map((r: any) => ({ t: Number(r.timestamp_ns) / 1_000_000, v: e8sToNumber(r.total_debt_e8s ?? 0n) }))
  );
  const feePoints = $derived(
    feeRows.map((r: any) => {
      const b = e8sToNumber(r.borrowing_fees_e8s?.[0] ?? r.borrowing_fees_e8s ?? 0n);
      const rd = e8sToNumber(r.redemption_fees_e8s?.[0] ?? r.redemption_fees_e8s ?? 0n);
      const s = e8sToNumber(r.swap_fees_e8s ?? 0n);
      return { t: Number(r.timestamp_ns) / 1_000_000, v: b + rd + s };
    })
  );
</script>

<LensHealthStrip
  title="Protocol health"
  metrics={healthMetrics}
  loading={summaryLoading}
/>

<ProtocolVitals {summary} loading={summaryLoading} />

<div class="explorer-card">
  <div class="flex items-center justify-between mb-3">
    <h3 class="text-sm font-medium text-gray-300">Total Value Locked</h3>
  </div>
  <TvlChart data={vaultRows} loading={seriesLoading} />
</div>

<div class="grid grid-cols-1 md:grid-cols-2 gap-3">
  <div class="explorer-card">
    <MiniAreaChart
      points={vaultCountPoints}
      label="Vaults open (90d)"
      color={CHART_COLORS.teal}
      fillColor={CHART_COLORS.tealDim}
      valueFormat={(v) => v.toLocaleString(undefined, { maximumFractionDigits: 0 })}
      yAxisMode="data-fit"
      loading={seriesLoading}
    />
  </div>
  <div class="explorer-card">
    <MiniAreaChart
      points={debtPoints}
      label="Outstanding debt (90d)"
      color={CHART_COLORS.purple}
      fillColor={CHART_COLORS.purpleDim}
      valueFormat={(v) => `$${formatCompact(v)}`}
      yAxisMode="data-fit"
      loading={seriesLoading}
    />
  </div>
</div>

<div class="explorer-card">
  <MiniAreaChart
    points={feePoints}
    label="Daily protocol fees (90d)"
    color={CHART_COLORS.action}
    fillColor="rgba(52, 211, 153, 0.15)"
    valueFormat={(v) => `$${formatCompact(v)}`}
    headlineValue={feePoints.reduce((s, p) => s + p.v, 0)}
    height={160}
    loading={seriesLoading}
  />
</div>

<PoolHealthStrip {pegStatus} {lpApy} {spApy} loading={poolsLoading} />

<div class="explorer-card">
  <div class="flex items-center justify-between mb-3">
    <h3 class="text-sm font-medium text-gray-300">Token flow</h3>
    <div class="flex items-center gap-1 text-xs">
      {#each ['24h', '7d', '30d'] as preset (preset)}
        <button
          type="button"
          class="px-2 py-0.5 rounded transition-colors
                 {flowWindow === preset
                   ? 'bg-white/10 text-gray-100'
                   : 'text-gray-400 hover:text-gray-200'}"
          onclick={() => (flowWindow = preset as FlowWindowKey)}
        >{preset}</button>
      {/each}
    </div>
  </div>
  <TokenFlowChord edges={flowEdges} loading={flowLoading} timePreset={flowWindow} />
</div>

<LensActivityPanel scope="all" title="Recent activity" viewAllHref="/explorer/activity" />
