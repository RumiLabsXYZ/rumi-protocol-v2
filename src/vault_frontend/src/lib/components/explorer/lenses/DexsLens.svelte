<script lang="ts">
  import { onMount } from 'svelte';
  import LensHealthStrip from '../LensHealthStrip.svelte';
  import LensActivityPanel from '../LensActivityPanel.svelte';
  import MiniAreaChart from '../MiniAreaChart.svelte';
  import PoolHealthStrip from '../PoolHealthStrip.svelte';
  import {
    fetchSwapSeries, fetchThreePoolSeries, fetchPegStatus, fetchApys,
  } from '$services/explorer/analyticsService';
  import {
    fetchThreePoolState, fetchThreePoolStats, fetchThreePoolHealth,
    fetchThreePoolVolumeSeries, fetchThreePoolVirtualPriceSeries,
    fetchAmmPools,
  } from '$services/explorer/explorerService';
  import { POOL_TOKENS } from '$services/threePoolService';
  import { e8sToNumber, formatCompact, CHART_COLORS } from '$utils/explorerChartHelpers';
  import type { PegStatus } from '$declarations/rumi_analytics/rumi_analytics.did';

  let pegStatus: PegStatus | null = $state(null);
  let poolState: any = $state(null);
  let stats: any = $state(null);
  let health: any = $state(null);
  let swapSeries: any[] = $state([]);
  let volumeSeries: any[] = $state([]);
  let vpSeries: any[] = $state([]);
  let ammPools: any[] = $state([]);
  let lpApy: number | null = $state(null);
  let spApy: number | null = $state(null);
  let loading = $state(true);

  onMount(async () => {
    try {
      const [pegR, stR, statR, hlR, ssR, vsR, vpR, ammR, apyR] = await Promise.allSettled([
        fetchPegStatus(),
        fetchThreePoolState(),
        fetchThreePoolStats('Last24h'),
        fetchThreePoolHealth(),
        fetchSwapSeries(90),
        fetchThreePoolVolumeSeries('Last7d', 3600n),
        fetchThreePoolVirtualPriceSeries('Last30d', 86400n),
        fetchAmmPools(),
        fetchApys(),
      ]);
      if (pegR.status === 'fulfilled') pegStatus = pegR.value ?? null;
      if (stR.status === 'fulfilled') poolState = stR.value;
      if (statR.status === 'fulfilled') stats = statR.value;
      if (hlR.status === 'fulfilled') health = hlR.value;
      if (ssR.status === 'fulfilled') swapSeries = ssR.value ?? [];
      if (vsR.status === 'fulfilled') volumeSeries = vsR.value ?? [];
      if (vpR.status === 'fulfilled') vpSeries = vpR.value ?? [];
      if (ammR.status === 'fulfilled') ammPools = ammR.value ?? [];
      if (apyR.status === 'fulfilled' && apyR.value) {
        const aLp = apyR.value.lp_apy_pct?.[0];
        const aSp = apyR.value.sp_apy_pct?.[0];
        if (typeof aLp === 'number' && aLp > 0) lpApy = aLp;
        if (typeof aSp === 'number' && aSp > 0) spApy = aSp;
      }
    } catch (err) {
      console.error('[DexsLens] onMount error:', err);
    } finally {
      loading = false;
    }
  });

  // 3pool TVL in $USD (each balance is in token decimals; all 3 stables are ~$1)
  const threePoolTvl = $derived.by(() => {
    if (!poolState?.balances) return 0;
    let total = 0;
    for (let i = 0; i < poolState.balances.length; i++) {
      const tok = POOL_TOKENS[i];
      if (!tok) continue;
      total += Number(poolState.balances[i]) / Math.pow(10, tok.decimals);
    }
    return total;
  });

  const imbalancePct = $derived.by(() => {
    if (!pegStatus) return 0;
    return pegStatus.max_imbalance_pct;
  });

  const virtualPrice = $derived.by(() => {
    if (!poolState?.virtual_price) return 1;
    return Number(poolState.virtual_price) / 1e18;
  });

  const volume24h = $derived.by(() => {
    if (!stats?.swap_volume_per_token) return 0;
    let total = 0;
    for (let i = 0; i < stats.swap_volume_per_token.length; i++) {
      const tok = POOL_TOKENS[i];
      if (!tok) continue;
      total += Number(stats.swap_volume_per_token[i]) / Math.pow(10, tok.decimals);
    }
    return total;
  });

  const volumePoints = $derived(
    volumeSeries.map((p: any) => {
      let total = 0;
      for (let i = 0; i < p.volume_per_token.length; i++) {
        const tok = POOL_TOKENS[i];
        if (!tok) continue;
        total += Number(p.volume_per_token[i]) / Math.pow(10, tok.decimals);
      }
      return { t: Number(p.timestamp) * 1000, v: total };
    })
  );

  const vpPoints = $derived(
    vpSeries.map((p: any) => ({
      t: Number(p.timestamp) * 1000,
      v: Number(p.virtual_price) / 1e18,
    }))
  );

  const swapSeriesPoints = $derived(
    swapSeries.map((r: any) => ({
      t: Number(r.timestamp_ns) / 1_000_000,
      v: e8sToNumber(r.three_pool_volume_e8s ?? 0n) + e8sToNumber(r.amm_volume_e8s ?? 0n),
    }))
  );

  const healthMetrics = $derived.by(() => {
    const metrics = [
      { label: '3pool TVL', value: `$${formatCompact(threePoolTvl)}` },
      { label: '24h volume', value: `$${formatCompact(volume24h)}` },
      { label: 'Virtual price', value: virtualPrice.toFixed(6), sub: 'compounds with fees' },
      {
        label: 'Peg imbalance',
        value: pegStatus ? `${imbalancePct.toFixed(2)}%` : '--',
        tone: imbalancePct < 2 ? 'good' as const : imbalancePct < 5 ? 'caution' as const : 'danger' as const,
      },
    ];
    if (health) {
      const arb = Number(health.arb_opportunity_score);
      metrics.push({ label: 'Arb score', value: `${arb}/100`, tone: arb > 50 ? 'caution' as const : 'good' as const });
    }
    metrics.push({ label: 'LP APY', value: lpApy != null ? `${lpApy.toFixed(2)}%` : '--', sub: '7d' });
    return metrics;
  });

  // AMM pools: collapse list to simple summary
  const ammSummary = $derived.by(() => {
    return ammPools.map((p: any) => {
      const tokens: string[] = Array.isArray(p.tokens)
        ? p.tokens.map((t: any) => {
            const sym = t?.symbol ?? t?.name ?? '';
            return typeof sym === 'string' ? sym : String(sym);
          })
        : [];
      return {
        id: p.id ?? p.pool_id ?? p.canister_id ?? 'amm',
        name: tokens.length > 1 ? tokens.join(' / ') : 'AMM pool',
        feeBps: Number(p.fee_bps ?? p.fee ?? 0),
      };
    });
  });
</script>

<LensHealthStrip title="DEXs" metrics={healthMetrics} loading={loading} />

<div class="grid grid-cols-1 lg:grid-cols-2 gap-3">
  <div class="explorer-card">
    <h3 class="text-sm font-medium text-gray-300 mb-3">3Pool balances</h3>
    {#if loading}
      <div class="flex items-center justify-center py-8">
        <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
      </div>
    {:else if !poolState}
      <p class="text-sm text-gray-500 py-4">3pool state unavailable.</p>
    {:else}
      {@const bal = poolState.balances ?? []}
      {@const normTotal = bal.reduce((s: number, b: any, i: number) => {
        const tok = POOL_TOKENS[i];
        return tok ? s + Number(b) / Math.pow(10, tok.decimals) : s;
      }, 0)}
      <div class="flex h-3 rounded overflow-hidden mb-3">
        {#each bal as b, i}
          {@const tok = POOL_TOKENS[i]}
          {#if tok}
            {@const amount = Number(b) / Math.pow(10, tok.decimals)}
            <div
              style="flex: {amount}; background: {tok.color};"
              title="{tok.symbol}: ${formatCompact(amount)}"
            ></div>
          {/if}
        {/each}
      </div>
      <div class="space-y-1.5">
        {#each bal as b, i}
          {@const tok = POOL_TOKENS[i]}
          {#if tok}
            {@const amount = Number(b) / Math.pow(10, tok.decimals)}
            {@const pct = normTotal > 0 ? (amount / normTotal) * 100 : 0}
            <div class="flex items-center gap-2 text-sm">
              <span class="w-2 h-2 rounded-full flex-shrink-0" style="background: {tok.color};"></span>
              <span class="w-20 font-medium text-gray-300">{tok.symbol}</span>
              <div class="flex-1 h-1.5 rounded-sm overflow-hidden" style="background: var(--rumi-bg-surface2);">
                <div class="h-full rounded-sm" style="width: {pct}%; background: {tok.color}; opacity: 0.6;"></div>
              </div>
              <span class="w-20 text-right tabular-nums text-gray-400">${formatCompact(amount)}</span>
              <span class="w-12 text-right tabular-nums text-gray-500">{pct.toFixed(1)}%</span>
            </div>
          {/if}
        {/each}
      </div>
    {/if}
  </div>

  <div class="explorer-card">
    <MiniAreaChart
      points={volumePoints}
      label="3Pool swap volume (7d / hourly)"
      color={CHART_COLORS.teal}
      fillColor={CHART_COLORS.tealDim}
      valueFormat={(v) => `$${formatCompact(v)}`}
      loading={loading}
    />
  </div>
</div>

<div class="grid grid-cols-1 lg:grid-cols-2 gap-3">
  <div class="explorer-card">
    <MiniAreaChart
      points={swapSeriesPoints}
      label="Daily swap volume (all DEXs, 90d)"
      color={CHART_COLORS.purple}
      fillColor={CHART_COLORS.purpleDim}
      valueFormat={(v) => `$${formatCompact(v)}`}
      loading={loading}
    />
  </div>
  <div class="explorer-card">
    <MiniAreaChart
      points={vpPoints}
      label="3Pool virtual price (30d / daily)"
      color={CHART_COLORS.action}
      fillColor="rgba(52, 211, 153, 0.15)"
      valueFormat={(v) => v.toFixed(6)}
      loading={loading}
    />
  </div>
</div>

{#if ammSummary.length > 0}
  <div class="explorer-card">
    <h3 class="text-sm font-medium text-gray-300 mb-3">AMM pools</h3>
    <div class="overflow-x-auto">
      <table class="w-full text-sm">
        <thead>
          <tr class="border-b border-white/5">
            <th class="text-left py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Pool</th>
            <th class="text-right py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Fee</th>
            <th class="text-left py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">ID</th>
          </tr>
        </thead>
        <tbody>
          {#each ammSummary as p}
            <tr class="border-b border-white/[0.03]">
              <td class="py-2 px-2 font-medium text-gray-200">{p.name}</td>
              <td class="py-2 px-2 text-right tabular-nums text-gray-400">{(p.feeBps / 100).toFixed(2)}%</td>
              <td class="py-2 px-2 text-gray-500 font-mono text-xs">{p.id}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  </div>
{/if}

<PoolHealthStrip {pegStatus} {lpApy} {spApy} loading={loading} />

<LensActivityPanel scope="dexs" title="DEX activity" />
