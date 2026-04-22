<script lang="ts">
  import { onMount } from 'svelte';
  import LensHealthStrip from '../LensHealthStrip.svelte';
  import LensActivityPanel from '../LensActivityPanel.svelte';
  import MiniAreaChart from '../MiniAreaChart.svelte';
  import ProtocolVitals from '../ProtocolVitals.svelte';
  import PoolHealthStrip from '../PoolHealthStrip.svelte';
  import TvlChart from '../TvlChart.svelte';
  import {
    fetchProtocolSummary, fetchTvlSeries, fetchVaultSeries,
    fetchFeeSeries, fetchPegStatus, fetchApys,
  } from '$services/explorer/analyticsService';
  import { ProtocolService } from '$services/protocol';
  import { threePoolService, POOL_TOKENS, calculateTheoreticalApy } from '$services/threePoolService';
  import { stabilityPoolService } from '$services/stabilityPoolService';
  import { e8sToNumber, formatCompact, CHART_COLORS } from '$utils/explorerChartHelpers';
  import type { ProtocolSummary, DailyTvlRow, PegStatus } from '$declarations/rumi_analytics/rumi_analytics.did';

  let summary: ProtocolSummary | null = $state(null);
  let summaryLoading = $state(true);
  let tvlData: DailyTvlRow[] = $state([]);
  let tvlLoading = $state(true);
  let vaultRows: any[] = $state([]);
  let feeRows: any[] = $state([]);
  let seriesLoading = $state(true);
  let pegStatus: PegStatus | null = $state(null);
  let lpApy: number | null = $state(null);
  let spApy: number | null = $state(null);
  let poolsLoading = $state(true);

  onMount(async () => {
    const [sumR, tvlR, vaultR, feeR, pegR, apyR] = await Promise.allSettled([
      fetchProtocolSummary(),
      fetchTvlSeries(365),
      fetchVaultSeries(90),
      fetchFeeSeries(90),
      fetchPegStatus(),
      fetchApys(),
    ]);

    if (sumR.status === 'fulfilled' && sumR.value) summary = sumR.value;
    summaryLoading = false;

    if (tvlR.status === 'fulfilled') tvlData = tvlR.value ?? [];
    tvlLoading = false;

    if (vaultR.status === 'fulfilled') vaultRows = vaultR.value ?? [];
    if (feeR.status === 'fulfilled') feeRows = feeR.value ?? [];
    seriesLoading = false;

    if (pegR.status === 'fulfilled') pegStatus = pegR.value ?? null;
    if (apyR.status === 'fulfilled' && apyR.value) {
      const aLp = apyR.value.lp_apy_pct?.[0];
      const aSp = apyR.value.sp_apy_pct?.[0];
      if (typeof aLp === 'number' && aLp > 0) lpApy = aLp;
      if (typeof aSp === 'number' && aSp > 0) spApy = aSp;
    }

    // Fallback APY compute when analytics is empty.
    if (!lpApy || !spApy) {
      try {
        const [psR, poolR, spR] = await Promise.allSettled([
          ProtocolService.getProtocolStatus(),
          threePoolService.getPoolStatus(),
          stabilityPoolService.getPoolStatus(),
        ]);
        const ps = psR.status === 'fulfilled' ? psR.value : null;
        const pool = poolR.status === 'fulfilled' ? poolR.value : null;
        const sp = spR.status === 'fulfilled' ? spR.value : null;

        if (!lpApy && ps && pool) {
          let poolTvlE8s = 0;
          for (let i = 0; i < pool.balances.length; i++) {
            const token = POOL_TOKENS[i];
            if (token) {
              poolTvlE8s += token.decimals === 8 ? Number(pool.balances[i]) : Number(pool.balances[i]) * 100;
            }
          }
          const threePoolBps = ps.interestSplit?.find((e: any) => e.destination === 'three_pool')?.bps ?? 5000;
          const computed = calculateTheoreticalApy(threePoolBps, ps.perCollateralInterest, poolTvlE8s / 1e8);
          if (computed != null) lpApy = computed * 100;
        }
        if (!spApy && ps && sp) {
          const poolShare = (ps.interestSplit?.find((e: any) => e.destination === 'stability_pool')?.bps ?? 0) / 10000;
          const perC = ps.perCollateralInterest;
          if (poolShare > 0 && perC?.length > 0) {
            const eligibleMap = new Map<string, number>(
              (sp.eligible_icusd_per_collateral ?? []).map(([p, v]: [any, bigint]) => [
                typeof p === 'object' && typeof p.toText === 'function' ? p.toText() : String(p),
                Number(v) / 1e8,
              ])
            );
            let totalApr = 0;
            for (const info of perC) {
              const eligible = eligibleMap.get(info.collateralType) ?? 0;
              if (eligible === 0 || info.totalDebtE8s === 0 || info.weightedInterestRate === 0) continue;
              totalApr += (info.weightedInterestRate * poolShare * info.totalDebtE8s) / eligible;
            }
            if (totalApr > 0) spApy = (Math.pow(1 + totalApr / 365, 365) - 1) * 100;
          }
        }
      } catch (err) {
        console.error('[OverviewLens] apy fallback error:', err);
      }
    }
    poolsLoading = false;
  });

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
      { label: 'LP APY', value: lpApy != null ? `${lpApy.toFixed(2)}%` : '--', sub: '7d' },
      { label: 'SP APY', value: spApy != null ? `${spApy.toFixed(2)}%` : '--', sub: '7d' },
    ];
  });

  const vaultCountPoints = $derived(
    vaultRows.map((r: any) => ({ t: Number(r.timestamp_ns) / 1_000_000, v: Number(r.total_vaults ?? r.vault_count ?? 0) }))
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
  <TvlChart data={tvlData} loading={tvlLoading} />
</div>

<div class="grid grid-cols-1 md:grid-cols-2 gap-3">
  <div class="explorer-card">
    <MiniAreaChart
      points={vaultCountPoints}
      label="Vaults open (90d)"
      color={CHART_COLORS.teal}
      fillColor={CHART_COLORS.tealDim}
      valueFormat={(v) => v.toLocaleString(undefined, { maximumFractionDigits: 0 })}
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
    height={160}
    loading={seriesLoading}
  />
</div>

<PoolHealthStrip {pegStatus} {lpApy} {spApy} loading={poolsLoading} />

<LensActivityPanel scope="all" title="Recent activity" />
