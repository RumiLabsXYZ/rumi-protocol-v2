<script lang="ts">
  import { onMount } from 'svelte';
  import LensHealthStrip from '../LensHealthStrip.svelte';
  import LensActivityPanel from '../LensActivityPanel.svelte';
  import MiniAreaChart from '../MiniAreaChart.svelte';
  import TvlChart from '../TvlChart.svelte';
  import TokenFlowBars from '../TokenFlowBars.svelte';
  import LiquidationsOverviewCard from '../LiquidationsOverviewCard.svelte';
  import {
    fetchProtocolSummary, fetchVaultSeries,
    fetchFeeSeries, fetchPegStatus, fetchApys, fetchTokenFlow,
  } from '$services/explorer/analyticsService';
  import { ProtocolService } from '$services/protocol';
  import { fetchProtocolStatus } from '$services/explorer/explorerService';
  import { threePoolService } from '$services/threePoolService';
  import { stabilityPoolService } from '$services/stabilityPoolService';
  import { e8sToNumber, formatCompact, bpsToPercent, CHART_COLORS } from '$utils/explorerChartHelpers';
  import { liveSpApyPct, liveLpApyPct } from '$utils/liveApy';
  import { getThreePoolApy } from '$services/threePoolApyService';
  import { getAmm1Apy, combinedBestLpApyPct } from '$services/amm1ApyService';
  import type { ProtocolSummary, PegStatus, TokenFlowEdge } from '$declarations/rumi_analytics/rumi_analytics.did';

  let summary: ProtocolSummary | null = $state(null);
  let summaryLoading = $state(true);
  let vaultRows: any[] = $state([]);
  let feeRows: any[] = $state([]);
  let seriesLoading = $state(true);
  let pegStatus: PegStatus | null = $state(null);
  // Live backend status — true mode, oracle freshness, bad-debt accounting.
  let protocolStatus: any = $state(null);
  let analyticsLpApy: number | null = $state(null);
  let analyticsAmmApy: number | null = $state(null);
  let analyticsSpApy: number | null = $state(null);
  let liveLp: number | null = $state(null);
  let liveSp: number | null = $state(null);
  // Live 3pool/AMM1 APY inputs for the combined "best LP APY" headline vital.
  let liveThreePoolApy: number | null = $state(null);
  let liveAmm1Apy: number | null = $state(null);

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
      const aAmm = apyR.value.amm_apy_pct?.[0];
      const aSp = apyR.value.sp_apy_pct?.[0];
      if (typeof aLp === 'number') analyticsLpApy = aLp;
      if (typeof aAmm === 'number') analyticsAmmApy = aAmm;
      if (typeof aSp === 'number') analyticsSpApy = aSp;
    }

    // Always compute the live values too. The analytics 7d numbers can sit at
    // zero when the rolling window has no realized fee activity, even though
    // LPs/SP depositors are still earning from interest_split. Live derives
    // from current protocol + pool state and reflects what a depositor would
    // earn right now. Falls back to analytics if any input is missing.
    try {
      const [psR, poolR, spR, rawStatusR] = await Promise.allSettled([
        ProtocolService.getProtocolStatus(),
        threePoolService.getPoolStatus(),
        stabilityPoolService.getPoolStatus(),
        // Raw (snake_case) backend status — the ProtocolService DTO drops the
        // deficit/breaker/oracle-timestamp fields the vitals strip needs.
        fetchProtocolStatus(),
      ]);
      const ps = psR.status === 'fulfilled' ? psR.value : null;
      const pool = poolR.status === 'fulfilled' ? poolR.value : null;
      const sp = spR.status === 'fulfilled' ? spR.value : null;
      protocolStatus = rawStatusR.status === 'fulfilled' ? rawStatusR.value : null;
      liveLp = liveLpApyPct(ps, pool?.balances);
      liveSp = liveSpApyPct(ps, sp);
    } catch (err) {
      console.error('[OverviewLens] live APY compute error:', err);
    }

    // Combined "best LP APY" inputs for the headline vital. These reuse the
    // exact same cached services as the Swap "Earn up to" banner, so the
    // Explorer and Swap surfaces never disagree. Graceful: each stays null on
    // failure and the vital falls back to the 3pool-only number.
    try {
      const [tpR, ammR] = await Promise.allSettled([
        getThreePoolApy(),
        getAmm1Apy(),
      ]);
      if (tpR.status === 'fulfilled') liveThreePoolApy = tpR.value.total_apy_pct;
      if (ammR.status === 'fulfilled') liveAmm1Apy = ammR.value.total_apy_pct;
    } catch (err) {
      console.error('[OverviewLens] combined LP APY compute error:', err);
    }
  });

  const lpApy: number | null = $derived(liveLp ?? analyticsLpApy);
  const spApy: number | null = $derived(liveSp ?? analyticsSpApy);
  const lpApySub = $derived(liveLp != null ? 'live' : '7d');
  const spApySub = $derived(liveSp != null ? 'live' : '7d');

  // Headline LP APY = the best per-dollar return across 3pool and AMM1 (AMM1
  // stacks 3pool yield on its 3USD half). Matches the Swap "Earn up to" banner.
  // Falls back to the 3pool-only number when the live inputs are unavailable.
  const combinedLpApy = $derived.by(() => {
    // Live primary: best per-dollar of (3pool-only) vs (AMM1 + half 3pool).
    if (liveThreePoolApy != null && liveAmm1Apy != null) {
      return combinedBestLpApyPct(liveThreePoolApy, liveAmm1Apy);
    }
    // Analytics fallback: the canister now tracks a faithful AMM1 LP APY (trading
    // fees + icUSD rewards), so combine it the same way when both values exist;
    // otherwise fall back to the 3pool-only number (lpApy) as before.
    if (analyticsLpApy != null && analyticsAmmApy != null) {
      return combinedBestLpApyPct(analyticsLpApy, analyticsAmmApy);
    }
    return lpApy;
  });
  const combinedLpApySub = $derived(
    liveThreePoolApy != null && liveAmm1Apy != null ? 'best · live' : lpApySub,
  );

  // 3pool balance skew — % deviation from the 33/33/33 target weighting.
  // This was previously labeled "Peg", which overstated it: it measures pool
  // composition imbalance, not a market price deviation from $1.
  const skewPct = $derived.by(() => {
    if (!pegStatus) return '--';
    const imb = pegStatus.max_imbalance_pct;
    return `${imb >= 0 ? '+' : ''}${imb.toFixed(2)}%`;
  });

  const skewTone = $derived.by(() => {
    if (!pegStatus) return 'muted' as const;
    const imb = pegStatus.max_imbalance_pct;
    if (imb < 2) return 'good' as const;
    if (imb < 5) return 'caution' as const;
    return 'danger' as const;
  });

  // True protocol mode from the backend (candid variant), falling back to
  // the CR heuristic until the status call resolves.
  const protocolMode = $derived.by(() => {
    const m = protocolStatus?.mode;
    if (m) {
      const key = Object.keys(m)[0] ?? '';
      if (key === 'ReadOnly') return 'ReadOnly';
      if (key === 'Recovery') return 'Recovery';
      return 'Normal';
    }
    if (!summary) return null;
    const cr = Number(summary.system_cr_bps);
    if (cr < 10000) return 'ReadOnly';
    if (cr < 14100) return 'Recovery';
    return 'Normal';
  });

  // Bad debt (Wave-8e deficit accounting), live from the backend.
  const badDebtIcusd = $derived(
    protocolStatus ? Number(protocolStatus.protocol_deficit_icusd ?? 0) / 1e8 : null
  );

  // Oracle freshness — operations reject on prices older than 10 minutes,
  // so staleness beyond that is a danger signal, not trivia.
  const oracleAgeMin = $derived.by(() => {
    const ts = protocolStatus?.last_icp_timestamp;
    if (!ts) return null;
    const ageMs = Date.now() - Number(ts) / 1_000_000;
    return ageMs / 60_000;
  });

  const healthMetrics = $derived.by(() => {
    if (!summary) return [];
    const tvl = e8sToNumber(summary.total_collateral_usd_e8s);
    const supply = summary.circulating_supply_icusd_e8s?.length
      ? e8sToNumber(summary.circulating_supply_icusd_e8s[0]) : 0;
    const volume24h = e8sToNumber(summary.volume_24h_e8s);
    const debt = e8sToNumber(summary.total_debt_e8s);
    const cr = Number(summary.system_cr_bps);
    const metrics: { label: string; value: string; sub?: string; tone?: 'normal' | 'good' | 'caution' | 'danger' | 'muted' }[] = [
      {
        label: 'System CR',
        value: bpsToPercent(cr),
        tone: cr < 14100 ? 'danger' : cr < 15000 ? 'caution' : 'normal',
      },
      { label: 'TVL', value: `$${formatCompact(tvl)}` },
      { label: 'Total Debt', value: `${formatCompact(debt)} icUSD` },
      { label: 'icUSD Supply', value: `$${formatCompact(supply)}` },
      { label: '24h Volume', value: `$${formatCompact(volume24h)}` },
      { label: '24h Swaps', value: Number(summary.swap_count_24h).toLocaleString() },
      { label: 'LP APY', value: combinedLpApy != null ? `${combinedLpApy.toFixed(2)}%` : '--', sub: combinedLpApySub },
      { label: 'SP APY', value: spApy != null ? `${Number(spApy).toFixed(2)}%` : '--', sub: spApySub },
      { label: '3Pool Skew', value: skewPct, sub: 'vs 33/33/33', tone: skewTone },
    ];
    if (badDebtIcusd != null) {
      metrics.push({
        label: 'Bad Debt',
        value: badDebtIcusd === 0 ? '$0' : `$${formatCompact(badDebtIcusd)}`,
        sub: badDebtIcusd === 0 ? 'none accrued' : 'repaying via fees',
        tone: badDebtIcusd === 0 ? 'good' : 'danger',
      });
    }
    if (oracleAgeMin != null) {
      metrics.push({
        label: 'Oracle',
        value: oracleAgeMin < 1 ? '<1m ago' : `${Math.round(oracleAgeMin)}m ago`,
        sub: 'last price',
        tone: oracleAgeMin < 10 ? 'good' : oracleAgeMin < 30 ? 'caution' : 'danger',
      });
    }
    if (protocolStatus?.frozen) {
      metrics.push({ label: 'Liquidations', value: 'Frozen', tone: 'danger' });
    }
    if (protocolStatus?.liquidation_breaker_tripped) {
      metrics.push({ label: 'Liq. Breaker', value: 'Tripped', sub: 'auto-routing paused', tone: 'danger' });
    }
    return metrics;
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
  mode={protocolMode}
  loading={summaryLoading}
/>

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
      label="Vaults with debt (90d)"
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
    valueFormat={(v) => `$${formatCompact(v)}`}
    headlineValue={feePoints.reduce((s, p) => s + p.v, 0)}
    height={160}
    kind="bar"
    loading={seriesLoading}
  />
</div>

<LiquidationsOverviewCard />

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
  <TokenFlowBars edges={flowEdges} loading={flowLoading} timePreset={flowWindow} />
</div>

<LensActivityPanel scope="all" title="Recent activity" viewAllHref="/explorer/activity" />
